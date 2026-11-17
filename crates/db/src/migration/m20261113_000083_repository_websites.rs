use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(RepositoryWebsite::Table)
                    .col(
                        ColumnDef::new(RepositoryWebsite::RepositoryId)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(RepositoryWebsite::Links).text().not_null())
                    .foreign_key(
                        ForeignKey::create()
                            .from(RepositoryWebsite::Table, RepositoryWebsite::RepositoryId)
                            .to(Repository::Table, Repository::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await
    }
    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(RepositoryWebsite::Table).to_owned())
            .await
    }
}
#[derive(DeriveIden)]
enum RepositoryWebsite {
    #[sea_orm(iden = "repository_websites")]
    Table,
    RepositoryId,
    Links,
}
#[derive(DeriveIden)]
enum Repository {
    #[sea_orm(iden = "repositories")]
    Table,
    Id,
}
