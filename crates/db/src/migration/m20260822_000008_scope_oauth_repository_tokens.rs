use sea_orm::ConnectionTrait;
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "UPDATE oauth_access_tokens \
                 SET scopes = scopes | 8 \
                 WHERE (' ' || scope || ' ') LIKE '% read:repository %' \
                    OR (' ' || scope || ' ') LIKE '% repo %'",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("UPDATE oauth_access_tokens SET scopes = scopes & ~8")
            .await?;
        Ok(())
    }
}
