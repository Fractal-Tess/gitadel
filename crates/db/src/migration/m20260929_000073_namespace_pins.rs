use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

/// Repositories a namespace features at the top of its profile, in order.
/// Rows follow the repository: deleting it drops its pin.
#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(NamespacePin::Table)
                    .col(ColumnDef::new(NamespacePin::Namespace).string().not_null())
                    .col(ColumnDef::new(NamespacePin::RepositoryId).uuid().not_null())
                    .col(ColumnDef::new(NamespacePin::Position).integer().not_null())
                    .col(
                        ColumnDef::new(NamespacePin::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .primary_key(
                        Index::create()
                            .col(NamespacePin::Namespace)
                            .col(NamespacePin::RepositoryId),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-namespace-pin-repository")
                            .from(NamespacePin::Table, NamespacePin::RepositoryId)
                            .to(Repository::Table, Repository::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx-namespace-pin-repository")
                    .table(NamespacePin::Table)
                    .col(NamespacePin::RepositoryId)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(NamespacePin::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum NamespacePin {
    #[sea_orm(iden = "namespace_pins")]
    Table,
    Namespace,
    RepositoryId,
    Position,
    CreatedAt,
}

#[derive(DeriveIden)]
enum Repository {
    #[sea_orm(iden = "repositories")]
    Table,
    Id,
}
