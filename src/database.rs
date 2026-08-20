use anyhow::{Context, Result};
use sea_orm::{ConnectOptions, Database, DatabaseConnection};
use sea_orm_migration::MigratorTrait;

use crate::{config::DatabaseSettings, migration::Migrator};

pub async fn connect_and_migrate(settings: &DatabaseSettings) -> Result<DatabaseConnection> {
    let mut options = ConnectOptions::new(&settings.url);
    options.sqlx_logging(false);

    let connection = Database::connect(options)
        .await
        .with_context(|| format!("could not connect to database {}", settings.url))?;

    Migrator::up(&connection, None)
        .await
        .context("could not apply database migrations")?;

    Ok(connection)
}
