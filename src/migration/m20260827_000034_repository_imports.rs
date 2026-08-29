use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(RepositoryImport::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(RepositoryImport::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(RepositoryImport::CreatedBy)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(RepositoryImport::TargetNamespace)
                            .string_len(100)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(RepositoryImport::Provider)
                            .string_len(32)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(RepositoryImport::InstanceUrl)
                            .string_len(2048)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(RepositoryImport::State)
                            .string_len(32)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(RepositoryImport::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(RepositoryImport::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-repository-import-user")
                            .from(RepositoryImport::Table, RepositoryImport::CreatedBy)
                            .to(User::Table, User::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_table(
                Table::create()
                    .table(RepositoryImportItem::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(RepositoryImportItem::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(RepositoryImportItem::ImportId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(RepositoryImportItem::SourceId)
                            .string_len(255)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(RepositoryImportItem::SourceFullName)
                            .string_len(512)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(RepositoryImportItem::SourceWebUrl)
                            .string_len(2048)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(RepositoryImportItem::SourceCloneUrl)
                            .string_len(2048)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(RepositoryImportItem::TargetName)
                            .string_len(100)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(RepositoryImportItem::TargetVisibility)
                            .string_len(16)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(RepositoryImportItem::State)
                            .string_len(32)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(RepositoryImportItem::Attempts)
                            .integer()
                            .not_null()
                            .default(0),
                    )
                    .col(ColumnDef::new(RepositoryImportItem::RepositoryId).uuid())
                    .col(ColumnDef::new(RepositoryImportItem::LastError).text())
                    .col(
                        ColumnDef::new(RepositoryImportItem::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(RepositoryImportItem::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-repository-import-item-import")
                            .from(RepositoryImportItem::Table, RepositoryImportItem::ImportId)
                            .to(RepositoryImport::Table, RepositoryImport::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-repository-import-item-repository")
                            .from(
                                RepositoryImportItem::Table,
                                RepositoryImportItem::RepositoryId,
                            )
                            .to(Repository::Table, Repository::Id)
                            .on_delete(ForeignKeyAction::SetNull),
                    )
                    .index(
                        Index::create()
                            .name("uq-repository-import-item-source")
                            .col(RepositoryImportItem::ImportId)
                            .col(RepositoryImportItem::SourceId)
                            .unique(),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx-repository-import-item-state")
                    .table(RepositoryImportItem::Table)
                    .col(RepositoryImportItem::ImportId)
                    .col(RepositoryImportItem::State)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(RepositoryImportItem::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(RepositoryImport::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum RepositoryImport {
    #[sea_orm(iden = "repository_imports")]
    Table,
    Id,
    CreatedBy,
    TargetNamespace,
    Provider,
    InstanceUrl,
    State,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum RepositoryImportItem {
    #[sea_orm(iden = "repository_import_items")]
    Table,
    Id,
    ImportId,
    SourceId,
    SourceFullName,
    SourceWebUrl,
    SourceCloneUrl,
    TargetName,
    TargetVisibility,
    State,
    Attempts,
    RepositoryId,
    LastError,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum User {
    #[sea_orm(iden = "users")]
    Table,
    Id,
}

#[derive(DeriveIden)]
enum Repository {
    #[sea_orm(iden = "repositories")]
    Table,
    Id,
}
