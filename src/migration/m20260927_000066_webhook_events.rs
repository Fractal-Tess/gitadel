use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

/// Adds per-hook event selection. Existing hooks keep receiving push events.
#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Webhook::Table)
                    .add_column(
                        ColumnDef::new(Webhook::Events)
                            .string_len(512)
                            .not_null()
                            .default("push"),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Webhook::Table)
                    .drop_column(Webhook::Events)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum Webhook {
    #[sea_orm(iden = "repository_webhooks")]
    Table,
    Events,
}
