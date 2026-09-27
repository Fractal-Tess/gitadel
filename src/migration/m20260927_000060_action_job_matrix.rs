use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // JSON with the combination's values and the strategy's fail-fast and
        // max-parallel settings; NULL for jobs without a matrix.
        manager
            .get_connection()
            .execute_unprepared("ALTER TABLE action_jobs ADD COLUMN matrix_json TEXT")
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("ALTER TABLE action_jobs DROP COLUMN matrix_json")
            .await?;
        Ok(())
    }
}
