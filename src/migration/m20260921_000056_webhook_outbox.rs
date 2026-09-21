use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(WebhookDelivery::Table)
                    .add_column(
                        ColumnDef::new(WebhookDelivery::Status)
                            .string_len(32)
                            .not_null()
                            .default("completed"),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(WebhookDelivery::Table)
                    .add_column(
                        ColumnDef::new(WebhookDelivery::AttemptCount)
                            .integer()
                            .not_null()
                            .default(0),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(WebhookDelivery::Table)
                    .add_column(
                        ColumnDef::new(WebhookDelivery::AttemptedAt).timestamp_with_time_zone(),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx-repository-webhook-delivery-status-created")
                    .table(WebhookDelivery::Table)
                    .col(WebhookDelivery::Status)
                    .col(WebhookDelivery::CreatedAt)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_index(
                Index::drop()
                    .name("idx-repository-webhook-delivery-status-created")
                    .table(WebhookDelivery::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(WebhookDelivery::Table)
                    .drop_column(WebhookDelivery::AttemptedAt)
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(WebhookDelivery::Table)
                    .drop_column(WebhookDelivery::AttemptCount)
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(WebhookDelivery::Table)
                    .drop_column(WebhookDelivery::Status)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum WebhookDelivery {
    #[sea_orm(iden = "repository_webhook_deliveries")]
    Table,
    Status,
    AttemptCount,
    AttemptedAt,
    CreatedAt,
}
