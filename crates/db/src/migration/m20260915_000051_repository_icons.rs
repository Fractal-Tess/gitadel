use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Repository::Table)
                    .add_column(
                        ColumnDef::new(Repository::IconUpdatedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .to_owned(),
            )
            .await?;
        // Detected icons are replaced whenever the default branch changes,
        // while uploaded ones are owned by the maintainer, so the row has to
        // record which of the two it is holding.
        manager
            .alter_table(
                Table::alter()
                    .table(Repository::Table)
                    .add_column(ColumnDef::new(Repository::IconSource).string_len(16).null())
                    .to_owned(),
            )
            .await?;
        manager
            .create_table(
                Table::create()
                    .table(RepositoryIcon::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(RepositoryIcon::RepositoryId)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(RepositoryIcon::Content).binary().not_null())
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-repository-icon-repository")
                            .from(RepositoryIcon::Table, RepositoryIcon::RepositoryId)
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
                    .table(RepositoryIcon::Table)
                    .if_exists()
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(Repository::Table)
                    .drop_column(Repository::IconSource)
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(Repository::Table)
                    .drop_column(Repository::IconUpdatedAt)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum Repository {
    #[sea_orm(iden = "repositories")]
    Table,
    Id,
    IconUpdatedAt,
    IconSource,
}

#[derive(DeriveIden)]
enum RepositoryIcon {
    #[sea_orm(iden = "repository_icons")]
    Table,
    RepositoryId,
    Content,
}
