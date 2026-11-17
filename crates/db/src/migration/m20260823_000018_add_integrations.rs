use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Credentials are keyed by namespace so one row covers a user or an
        // organization: an integration belongs to whoever owns the repository
        // being pushed. The provider slug is part of the key so a namespace can
        // connect several deployment platforms at once.
        manager
            .create_table(
                Table::create()
                    .table(NamespaceIntegration::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(NamespaceIntegration::Namespace)
                            .text()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(NamespaceIntegration::Provider)
                            .text()
                            .not_null(),
                    )
                    .col(ColumnDef::new(NamespaceIntegration::Url).text().not_null())
                    .col(
                        ColumnDef::new(NamespaceIntegration::ApiKey)
                            .text()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(NamespaceIntegration::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(NamespaceIntegration::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .primary_key(
                        Index::create()
                            .col(NamespaceIntegration::Namespace)
                            .col(NamespaceIntegration::Provider),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-namespace-integration-namespace")
                            .from(NamespaceIntegration::Table, NamespaceIntegration::Namespace)
                            .to(Namespace::Table, Namespace::Slug)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        // Absence means "inherit the owner's integration", so opting out is the
        // row and the common case stores nothing.
        manager
            .create_table(
                Table::create()
                    .table(RepositoryIntegrationOptOut::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(RepositoryIntegrationOptOut::RepositoryId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(RepositoryIntegrationOptOut::Provider)
                            .text()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(RepositoryIntegrationOptOut::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .primary_key(
                        Index::create()
                            .col(RepositoryIntegrationOptOut::RepositoryId)
                            .col(RepositoryIntegrationOptOut::Provider),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-repository-integration-opt-out-repository")
                            .from(
                                RepositoryIntegrationOptOut::Table,
                                RepositoryIntegrationOptOut::RepositoryId,
                            )
                            .to(Repository::Table, Repository::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(RepositoryIntegrationOptOut::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(Table::drop().table(NamespaceIntegration::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum NamespaceIntegration {
    #[sea_orm(iden = "namespace_integrations")]
    Table,
    Namespace,
    Provider,
    Url,
    ApiKey,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum RepositoryIntegrationOptOut {
    #[sea_orm(iden = "repository_integration_opt_outs")]
    Table,
    RepositoryId,
    Provider,
    CreatedAt,
}

#[derive(DeriveIden)]
enum Namespace {
    #[sea_orm(iden = "namespaces")]
    Table,
    Slug,
}

#[derive(DeriveIden)]
enum Repository {
    #[sea_orm(iden = "repositories")]
    Table,
    Id,
}
