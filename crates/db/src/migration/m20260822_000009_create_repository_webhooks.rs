use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(RepositoryWebhook::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(RepositoryWebhook::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(RepositoryWebhook::RepositoryId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(RepositoryWebhook::Url)
                            .string_len(2048)
                            .not_null(),
                    )
                    .col(ColumnDef::new(RepositoryWebhook::Secret).text())
                    .col(
                        ColumnDef::new(RepositoryWebhook::Active)
                            .boolean()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(RepositoryWebhook::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(RepositoryWebhook::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(RepositoryWebhook::LastDeliveryAt)
                            .timestamp_with_time_zone(),
                    )
                    .col(ColumnDef::new(RepositoryWebhook::LastResponseStatus).integer())
                    .col(ColumnDef::new(RepositoryWebhook::LastResponseMessage).text())
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-repository-webhook-repository")
                            .from(RepositoryWebhook::Table, RepositoryWebhook::RepositoryId)
                            .to(Repository::Table, Repository::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx-repository-webhook-repository-created")
                    .table(RepositoryWebhook::Table)
                    .col(RepositoryWebhook::RepositoryId)
                    .col(RepositoryWebhook::CreatedAt)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(RepositoryWebhook::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum RepositoryWebhook {
    #[sea_orm(iden = "repository_webhooks")]
    Table,
    Id,
    RepositoryId,
    Url,
    Secret,
    Active,
    CreatedAt,
    UpdatedAt,
    LastDeliveryAt,
    LastResponseStatus,
    LastResponseMessage,
}

#[derive(DeriveIden)]
enum Repository {
    #[sea_orm(iden = "repositories")]
    Table,
    Id,
}
