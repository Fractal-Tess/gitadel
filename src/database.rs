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

    let connection = Database::connect(connect_options(&settings.url))
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

fn connect_options(url: &str) -> ConnectOptions {
    let mut options = ConnectOptions::new(url);
    options.sqlx_logging(false);
    // A single SQLite writer connection makes transaction boundaries deterministic.
    options.max_connections(1);
    // `foreign_keys` and `busy_timeout` are connection-local, so they are part
    // of the connect options: the pool replaces its connection when it ages
    // out, and a pragma issued once after startup would not follow it.
    options.map_sqlx_sqlite_opts(|sqlite| {
        sqlite
            .foreign_keys(true)
            .pragma("journal_mode", "WAL")
            .busy_timeout(BUSY_TIMEOUT)
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
        let mut options = connect_options(&url);
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
}
