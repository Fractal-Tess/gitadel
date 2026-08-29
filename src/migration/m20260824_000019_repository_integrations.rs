use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Per-repository integration state replaces the opt-out model: a row
        // opts a repository in, absence keeps it out. The JSON columns are
        // opaque to Gitadel's core - only the provider's dispatcher reads them
        // - so adding a provider never needs another migration.
        manager
            .create_table(
                Table::create()
                    .table(RepositoryIntegration::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(RepositoryIntegration::RepositoryId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(RepositoryIntegration::Provider)
                            .text()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(RepositoryIntegration::Enabled)
                            .boolean()
                            .not_null(),
                    )
                    .col(ColumnDef::new(RepositoryIntegration::Resource).text())
                    .col(ColumnDef::new(RepositoryIntegration::Config).text())
                    .col(
                        ColumnDef::new(RepositoryIntegration::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .primary_key(
                        Index::create()
                            .col(RepositoryIntegration::RepositoryId)
                            .col(RepositoryIntegration::Provider),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-repository-integration-repository")
                            .from(
                                RepositoryIntegration::Table,
                                RepositoryIntegration::RepositoryId,
                            )
                            .to(Repository::Table, Repository::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        // Preserve the behaviour the opt-out table described. Every repository
        // inherited each credential its owning namespace held unless it had an
        // opt-out row, so that effective state becomes explicit here: opted-in
        // repositories keep deploying, excluded ones become disabled rows.
        // Repositories created after this point start unconfigured and opt in
        // by hand.
        let database = manager.get_connection();
        database
            .execute_unprepared(
                "INSERT INTO repository_integrations \
                 (repository_id, provider, enabled, updated_at) \
                 SELECT r.id, ni.provider, \
                        NOT EXISTS (SELECT 1 FROM repository_integration_opt_outs o \
                                    WHERE o.repository_id = r.id AND o.provider = ni.provider), \
                 CURRENT_TIMESTAMP \
                 FROM repositories r \
                 JOIN namespace_integrations ni ON ni.namespace = r.namespace",
            )
            .await?;
        database
            .execute_unprepared("DROP TABLE repository_integration_opt_outs")
            .await
            .map(|_| ())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let database = manager.get_connection();
        // Only disabled rows were expressible before; enabled rows drop to the
        // inherited default on the way back.
        database
            .execute_unprepared(
                "CREATE TABLE repository_integration_opt_outs \
                 (repository_id uuid NOT NULL, provider text NOT NULL, \
                  created_at timestamptz NOT NULL DEFAULT CURRENT_TIMESTAMP, \
                  PRIMARY KEY (repository_id, provider))",
            )
            .await?;
        database
            .execute_unprepared(
                "INSERT INTO repository_integration_opt_outs (repository_id, provider) \
                 SELECT repository_id, provider FROM repository_integrations \
                 WHERE enabled = FALSE",
            )
            .await?;
        manager
            .drop_table(Table::drop().table(RepositoryIntegration::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum RepositoryIntegration {
    #[sea_orm(iden = "repository_integrations")]
    Table,
    RepositoryId,
    Provider,
    Enabled,
    Resource,
    Config,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum Repository {
    #[sea_orm(iden = "repositories")]
    Table,
    Id,
}
