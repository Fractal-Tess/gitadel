use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Instance::Table)
                    .add_column(
                        ColumnDef::new(Instance::IntegrityChecksEnabled)
                            .boolean()
                            .not_null()
                            .default(true),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(Instance::Table)
                    .add_column(
                        ColumnDef::new(Instance::IntegrityCheckSchedule)
                            .string()
                            .not_null()
                            .default("0 3 * * *"),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Instance::Table)
                    .drop_column(Instance::IntegrityCheckSchedule)
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(Instance::Table)
                    .drop_column(Instance::IntegrityChecksEnabled)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum Instance {
    Table,
    IntegrityChecksEnabled,
    IntegrityCheckSchedule,
}
