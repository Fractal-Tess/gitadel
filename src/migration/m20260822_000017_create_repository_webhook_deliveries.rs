use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(WebhookDelivery::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(WebhookDelivery::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(WebhookDelivery::WebhookId).uuid().not_null())
                    .col(ColumnDef::new(WebhookDelivery::Event).text().not_null())
                    .col(ColumnDef::new(WebhookDelivery::Payload).text().not_null())
                    .col(ColumnDef::new(WebhookDelivery::ResponseStatus).integer())
                    .col(ColumnDef::new(WebhookDelivery::ResponseBody).text())
                    .col(
                        ColumnDef::new(WebhookDelivery::DurationMs)
                            .integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(WebhookDelivery::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-repository-webhook-delivery-webhook")
                            .from(WebhookDelivery::Table, WebhookDelivery::WebhookId)
                            .to(RepositoryWebhook::Table, RepositoryWebhook::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx-repository-webhook-delivery-webhook-created")
                    .table(WebhookDelivery::Table)
                    .col(WebhookDelivery::WebhookId)
                    .col(WebhookDelivery::CreatedAt)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(WebhookDelivery::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum WebhookDelivery {
    #[sea_orm(iden = "repository_webhook_deliveries")]
    Table,
    Id,
    WebhookId,
    Event,
    Payload,
    ResponseStatus,
    ResponseBody,
    DurationMs,
    CreatedAt,
}

#[derive(DeriveIden)]
enum RepositoryWebhook {
    #[sea_orm(iden = "repository_webhooks")]
    Table,
    Id,
}
