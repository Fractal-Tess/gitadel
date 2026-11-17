use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

/// Records one-time registry data conversions that run at startup, so a
/// completed conversion is not rescanned on every start.
#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                r#"
CREATE TABLE registry_upgrades (
    step text NOT NULL PRIMARY KEY,
    completed_at timestamptz NOT NULL
);
"#,
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE registry_upgrades;")
            .await?;
        Ok(())
    }
}
