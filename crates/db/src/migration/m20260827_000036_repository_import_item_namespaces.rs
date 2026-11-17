use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(RepositoryImportItem::Table)
                    .add_column(
                        ColumnDef::new(RepositoryImportItem::TargetNamespace)
                            .string_len(100)
                            .not_null()
                            .default(""),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .get_connection()
            .execute_unprepared(
                "UPDATE repository_import_items
                 SET target_namespace = (
                     SELECT target_namespace
                     FROM repository_imports
                     WHERE repository_imports.id = repository_import_items.import_id
                 )",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(RepositoryImportItem::Table)
                    .drop_column(RepositoryImportItem::TargetNamespace)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum RepositoryImportItem {
    #[sea_orm(iden = "repository_import_items")]
    Table,
    TargetNamespace,
}
