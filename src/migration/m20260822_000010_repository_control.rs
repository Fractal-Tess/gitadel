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
                    .add_column(ColumnDef::new(Repository::DeletedAt).timestamp_with_time_zone())
                    .to_owned(),
            )
            .await?;
        manager
            .create_table(
                Table::create()
                    .table(RepositoryAlias::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(RepositoryAlias::RepositoryId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(RepositoryAlias::Namespace)
                            .string_len(39)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(RepositoryAlias::Name)
                            .string_len(100)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(RepositoryAlias::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .primary_key(
                        Index::create()
                            .col(RepositoryAlias::Namespace)
                            .col(RepositoryAlias::Name),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-repository-alias-repository")
                            .from(RepositoryAlias::Table, RepositoryAlias::RepositoryId)
                            .to(Repository::Table, Repository::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(RepositoryAlias::Table).to_owned())
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(Repository::Table)
                    .drop_column(Repository::DeletedAt)
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
    DeletedAt,
}

#[derive(DeriveIden)]
enum RepositoryAlias {
    #[sea_orm(iden = "repository_aliases")]
    Table,
    RepositoryId,
    Namespace,
    Name,
    CreatedAt,
}
