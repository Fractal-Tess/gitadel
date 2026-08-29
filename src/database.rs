use anyhow::{Context, Result};
use sea_orm::{ConnectOptions, ConnectionTrait, Database, DatabaseConnection};
use sea_orm_migration::MigratorTrait;

use crate::{config::DatabaseSettings, migration::Migrator};

pub async fn connect_and_migrate(settings: &DatabaseSettings) -> Result<DatabaseConnection> {
    let mut options = ConnectOptions::new(&settings.url);
    options.sqlx_logging(false);
    // A single SQLite writer connection makes transaction boundaries deterministic and
    // ensures every operation inherits these connection-local safety pragmas.
    options.max_connections(1);

    let connection = Database::connect(options)
        .await
        .with_context(|| format!("could not connect to database {}", settings.url))?;
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
