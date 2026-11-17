use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(NamespaceIntegration::Table)
                    .add_column(
                        ColumnDef::new(NamespaceIntegration::DokployInternalUrl)
                            .string_len(2048)
                            .null(),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(NamespaceIntegration::Table)
                    .drop_column(NamespaceIntegration::DokployInternalUrl)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum NamespaceIntegration {
    #[sea_orm(iden = "namespace_integrations")]
    Table,
    DokployInternalUrl,
}
