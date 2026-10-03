use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result};
use sea_orm::{ConnectOptions, Database, DatabaseConnection};
use sea_orm_migration::MigratorTrait;

use crate::{
    config::DatabaseSettings,
    filesystem::{create_private_directory, open_private_file, protect_file},
    migration::Migrator,
};

const BUSY_TIMEOUT: Duration = Duration::from_secs(5);
/// Readers the read-only pool opens. WAL lets them run beside the single
/// writer, so page loads no longer queue behind background writes.
const READ_POOL_CONNECTIONS: u32 = 4;

pub async fn connect_and_migrate(settings: &DatabaseSettings) -> Result<DatabaseConnection> {
    let database_path = sqlite_file_path(&settings.url);
    if let Some(path) = database_path.as_deref() {
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        if !parent.as_os_str().is_empty() && !parent.exists() {
            create_private_directory(parent)
                .with_context(|| format!("could not create {}", parent.display()))?;
        }
        drop(
            open_private_file(path, false)
                .with_context(|| format!("could not protect database {}", path.display()))?,
        );
    }

    let connection = Database::connect(connect_options(&settings.url, PoolRole::Writer))
        .await
        .with_context(|| format!("could not connect to database {}", settings.url))?;
    if let Some(path) = database_path.as_deref() {
        protect_file(path)
            .await
            .with_context(|| format!("could not protect database {}", path.display()))?;
    }

    Migrator::up(&connection, None)
        .await
        .context("could not apply database migrations")?;

    Ok(connection)
}

/// A read-only pool beside `writer` for hot read paths. Every connection is
/// `query_only`, so a write routed here fails instead of racing the writer.
///
/// WAL guarantees a reader sees every transaction committed before its own
/// statement starts, so a read here right after a write on `writer` observes
/// that write. In-memory databases cannot be shared between pools and fall
/// back to the writer itself.
pub async fn connect_reader(
    settings: &DatabaseSettings,
    writer: &DatabaseConnection,
) -> Result<DatabaseConnection> {
    if sqlite_file_path(&settings.url).is_none() || is_memory_url(&settings.url) {
        return Ok(writer.clone());
    }
    Database::connect(connect_options(&settings.url, PoolRole::Reader))
        .await
        .with_context(|| format!("could not open read-only pool for {}", settings.url))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PoolRole {
    Writer,
    Reader,
}

fn connect_options(url: &str, role: PoolRole) -> ConnectOptions {
    let mut options = ConnectOptions::new(url);
    options.sqlx_logging(false);
    options.max_connections(match role {
        // A single SQLite writer connection makes transaction boundaries deterministic.
        PoolRole::Writer => 1,
        PoolRole::Reader => READ_POOL_CONNECTIONS,
    });
    // `foreign_keys` and `busy_timeout` are connection-local, so they are part
    // of the connect options: the pool replaces its connection when it ages
    // out, and a pragma issued once after startup would not follow it.
    options.map_sqlx_sqlite_opts(move |sqlite| {
        let sqlite = sqlite
            .foreign_keys(true)
            .pragma("journal_mode", "WAL")
            .busy_timeout(BUSY_TIMEOUT);
        match role {
            PoolRole::Writer => sqlite,
            PoolRole::Reader => sqlite.pragma("query_only", "1"),
        }
    });
    options
}

fn sqlite_file_path(url: &str) -> Option<PathBuf> {
    let raw = url
        .strip_prefix("sqlite://")
        .or_else(|| url.strip_prefix("sqlite:"))?
        .split('?')
        .next()?;
    (!raw.is_empty() && raw != ":memory:").then(|| PathBuf::from(raw))
}

fn is_memory_url(url: &str) -> bool {
    url.contains(":memory:") || url.contains("mode=memory")
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt as _;

    use sea_orm::{ConnectionTrait as _, Statement};
    use uuid::Uuid;

    use super::*;

    async fn pragma(connection: &DatabaseConnection, name: &str, column: &str) -> String {
        let row = connection
            .query_one_raw(Statement::from_string(
                connection.get_database_backend(),
                format!("SELECT CAST({column} AS TEXT) AS value FROM pragma_{name}"),
            ))
            .await
            .unwrap()
            .unwrap();
        row.try_get::<String>("", "value").unwrap()
    }

    #[tokio::test]
    async fn every_pooled_connection_is_opened_with_the_safety_pragmas() -> Result<()> {
        let root = std::env::temp_dir().join(format!("gitadel-database-pragma-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root)?;
        let url = format!("sqlite://{}?mode=rwc", root.join("gitadel.db").display());
        // The pool's connection is retired after every use, so each statement
        // below runs on a connection no earlier statement configured.
        let mut options = connect_options(&url, PoolRole::Writer);
        options.map_sqlx_sqlite_pool_opts(|pool| pool.max_lifetime(Duration::ZERO));
        let connection = Database::connect(options).await?;

        for _ in 0..2 {
            assert_eq!(
                pragma(&connection, "foreign_keys", "foreign_keys").await,
                "1"
            );
            assert_eq!(pragma(&connection, "busy_timeout", "timeout").await, "5000");
            assert_eq!(
                pragma(&connection, "journal_mode", "journal_mode").await,
                "wal"
            );
        }
        connection.close().await?;
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    #[tokio::test]
    async fn database_creation_uses_owner_only_modes() -> Result<()> {
        let root =
            std::env::temp_dir().join(format!("gitadel-database-mode-test-{}", Uuid::new_v4()));
        let database_path = root.join("data/gitadel.db");
        let settings = DatabaseSettings {
            url: format!("sqlite://{}?mode=rwc", database_path.display()),
        };

        let connection = connect_and_migrate(&settings).await?;
        connection.close().await?;

        let directory_mode = std::fs::metadata(root.join("data"))?.permissions().mode() & 0o777;
        let database_mode = std::fs::metadata(&database_path)?.permissions().mode() & 0o777;
        std::fs::remove_dir_all(root)?;

        assert_eq!((directory_mode, database_mode), (0o700, 0o600));
        Ok(())
    }

    #[tokio::test]
    async fn read_pool_sees_committed_writes_and_refuses_its_own() -> Result<()> {
        let root = std::env::temp_dir().join(format!("gitadel-database-reader-{}", Uuid::new_v4()));
        let settings = DatabaseSettings {
            url: format!("sqlite://{}?mode=rwc", root.join("gitadel.db").display()),
        };
        let writer = connect_and_migrate(&settings).await?;
        let reader = connect_reader(&settings, &writer).await?;
        let backend = writer.get_database_backend();
        let statement = |sql: &str| Statement::from_string(backend, sql.to_owned());

        writer
            .execute_raw(statement("CREATE TABLE reader_probe (value TEXT NOT NULL)"))
            .await?;
        // Each pooled reader sees a write the moment the writer commits it.
        for round in 0..(READ_POOL_CONNECTIONS + 1) {
            writer
                .execute_raw(statement(&format!(
                    "INSERT INTO reader_probe (value) VALUES ('{round}')"
                )))
                .await?;
            let count = reader
                .query_one_raw(statement(
                    "SELECT CAST(COUNT(*) AS TEXT) AS value FROM reader_probe",
                ))
                .await?
                .unwrap()
                .try_get::<String>("", "value")?;
            assert_eq!(count, (round + 1).to_string());
        }
        assert_eq!(pragma(&reader, "query_only", "query_only").await, "1");
        assert!(
            reader
                .execute_raw(statement("DELETE FROM reader_probe"))
                .await
                .is_err(),
            "the read pool must not accept writes"
        );

        reader.close().await?;
        writer.close().await?;
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    #[tokio::test]
    async fn in_memory_databases_share_the_writer_as_their_reader() -> Result<()> {
        assert!(is_memory_url("sqlite::memory:"));
        assert!(is_memory_url("sqlite://probe.db?mode=memory&cache=shared"));
        assert!(!is_memory_url("sqlite://probe.db?mode=rwc"));

        let url = "sqlite::memory:";
        let settings = DatabaseSettings {
            url: url.to_owned(),
        };
        let writer = Database::connect(connect_options(url, PoolRole::Writer)).await?;
        let reader = connect_reader(&settings, &writer).await?;
        // A separate pool could not see this table; the shared writer can
        // even write through the "reader".
        writer
            .execute_raw(Statement::from_string(
                writer.get_database_backend(),
                "CREATE TABLE memory_probe (value TEXT NOT NULL)",
            ))
            .await?;
        reader
            .execute_raw(Statement::from_string(
                reader.get_database_backend(),
                "INSERT INTO memory_probe (value) VALUES ('shared')",
            ))
            .await
            .context("an in-memory URL should fall back to the writer")?;
        Ok(())
    }
}
