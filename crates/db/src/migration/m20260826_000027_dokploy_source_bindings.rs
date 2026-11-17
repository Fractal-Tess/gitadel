use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(DokploySourceBinding::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(DokploySourceBinding::IntegrationId)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(DokploySourceBinding::GiteaId)
                            .string_len(255)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(DokploySourceBinding::GitProviderId)
                            .string_len(255)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(DokploySourceBinding::OauthApplicationId)
                            .uuid()
                            .unique_key(),
                    )
                    .col(
                        ColumnDef::new(DokploySourceBinding::Managed)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .col(
                        ColumnDef::new(DokploySourceBinding::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(DokploySourceBinding::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-dokploy-source-integration")
                            .from(
                                DokploySourceBinding::Table,
                                DokploySourceBinding::IntegrationId,
                            )
                            .to(NamespaceIntegration::Table, NamespaceIntegration::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-dokploy-source-oauth-application")
                            .from(
                                DokploySourceBinding::Table,
                                DokploySourceBinding::OauthApplicationId,
                            )
                            .to(OauthApplication::Table, OauthApplication::Id)
                            .on_delete(ForeignKeyAction::Restrict),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(DokploySourceBinding::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum DokploySourceBinding {
    #[sea_orm(iden = "dokploy_source_bindings")]
    Table,
    IntegrationId,
    GiteaId,
    GitProviderId,
    OauthApplicationId,
    Managed,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum NamespaceIntegration {
    #[sea_orm(iden = "namespace_integrations")]
    Table,
    Id,
}

#[derive(DeriveIden)]
enum OauthApplication {
    #[sea_orm(iden = "oauth_applications")]
    Table,
    Id,
}
