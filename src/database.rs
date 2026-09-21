use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use sea_orm::{ConnectOptions, ConnectionTrait, Database, DatabaseConnection};
use sea_orm_migration::MigratorTrait;

use crate::{
    config::DatabaseSettings,
    filesystem::{create_private_directory, open_private_file, protect_file},
    migration::Migrator,
};

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

    let mut options = ConnectOptions::new(&settings.url);
    options.sqlx_logging(false);
    // A single SQLite writer connection makes transaction boundaries deterministic and
    // ensures every operation inherits these connection-local safety pragmas.
    options.max_connections(1);

    let connection = Database::connect(options)
        .await
        .with_context(|| format!("could not connect to database {}", settings.url))?;
    if let Some(path) = database_path.as_deref() {
        protect_file(path)
            .await
            .with_context(|| format!("could not protect database {}", path.display()))?;
    }
    connection
        .execute_unprepared(
            "PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL; PRAGMA busy_timeout = 5000;",
        )
        .await
        .context("could not configure SQLite concurrency guarantees")?;

    Migrator::up(&connection, None)
        .await
        .context("could not apply database migrations")?;

    Ok(connection)
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

    use uuid::Uuid;

    use super::*;

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
