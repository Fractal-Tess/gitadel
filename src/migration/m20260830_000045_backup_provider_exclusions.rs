use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                r#"
CREATE TABLE backup_provider_exclusions (
    provider_id uuid NOT NULL PRIMARY KEY,
    created_at timestamptz NOT NULL
);
"#,
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(BackupProviderExclusion::Table)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum BackupProviderExclusion {
    Table,
}
