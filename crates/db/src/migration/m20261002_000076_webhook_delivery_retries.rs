use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

/// Lets a failed delivery wait in the outbox for its next attempt.
///
/// A pending row without a time is due immediately, which is what every row
/// queued before this migration means.
#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(WebhookDelivery::Table)
                    .add_column(
                        ColumnDef::new(WebhookDelivery::NextAttemptAt).timestamp_with_time_zone(),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(WebhookDelivery::Table)
                    .drop_column(WebhookDelivery::NextAttemptAt)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum WebhookDelivery {
    #[sea_orm(iden = "repository_webhook_deliveries")]
    Table,
    NextAttemptAt,
}
