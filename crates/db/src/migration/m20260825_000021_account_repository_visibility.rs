use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let database = manager.get_connection();
        database
            .execute_unprepared(
                "ALTER TABLE users ADD COLUMN default_repository_visibility TEXT NOT NULL DEFAULT 'private'",
            )
            .await?;
        database
            .execute_unprepared(
                "UPDATE users
                 SET default_repository_visibility = COALESCE(
                     (SELECT default_repository_visibility FROM instance WHERE id = 1),
                     'private'
                 )",
            )
            .await?;
        database
            .execute_unprepared("ALTER TABLE instance DROP COLUMN default_repository_visibility")
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let database = manager.get_connection();
        database
            .execute_unprepared(
                "ALTER TABLE instance ADD COLUMN default_repository_visibility TEXT NOT NULL DEFAULT 'private'",
            )
            .await?;
        database
            .execute_unprepared(
                "UPDATE instance
                 SET default_repository_visibility = COALESCE(
                     (SELECT default_repository_visibility
                      FROM users
                      WHERE is_admin = TRUE
                      ORDER BY created_at
                      LIMIT 1),
                     'private'
                 )
                 WHERE id = 1",
            )
            .await?;
        database
            .execute_unprepared("ALTER TABLE users DROP COLUMN default_repository_visibility")
            .await?;
        Ok(())
    }
}
