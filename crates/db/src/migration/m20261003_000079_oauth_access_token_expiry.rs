use sea_orm::{ConnectionTrait, DbBackend, Statement};
use sea_orm_migration::prelude::*;

/// How long tokens issued before expiry existed keep working, so clients
/// re-authorise within a month rather than breaking at the upgrade.
const GRACE_DAYS: i64 = 30;

#[derive(DeriveMigrationName)]
pub struct Migration;

/// OAuth access tokens expire. Tokens that already exist are given the
/// default lifetime counted from the upgrade.
#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let connection = manager.get_connection();
        connection
            .execute_unprepared("ALTER TABLE oauth_access_tokens ADD COLUMN expires_at TEXT")
            .await?;
        let expires_at = chrono::Utc::now() + chrono::Duration::days(GRACE_DAYS);
        connection
            .execute_raw(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                "UPDATE oauth_access_tokens SET expires_at = ? WHERE expires_at IS NULL",
                [expires_at.into()],
            ))
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("ALTER TABLE oauth_access_tokens DROP COLUMN expires_at")
            .await?;
        Ok(())
    }
}
