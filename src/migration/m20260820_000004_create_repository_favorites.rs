use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(RepositoryFavorite::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(RepositoryFavorite::UserId).uuid().not_null())
                    .col(
                        ColumnDef::new(RepositoryFavorite::RepositoryId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(RepositoryFavorite::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .primary_key(
                        Index::create()
                            .col(RepositoryFavorite::UserId)
                            .col(RepositoryFavorite::RepositoryId),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-repository-favorite-user")
                            .from(RepositoryFavorite::Table, RepositoryFavorite::UserId)
                            .to(User::Table, User::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-repository-favorite-repository")
                            .from(RepositoryFavorite::Table, RepositoryFavorite::RepositoryId)
                            .to(Repository::Table, Repository::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx-repository-favorite-repository")
                    .table(RepositoryFavorite::Table)
                    .col(RepositoryFavorite::RepositoryId)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(RepositoryFavorite::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum RepositoryFavorite {
    #[sea_orm(iden = "repository_favorites")]
    Table,
    UserId,
    RepositoryId,
    CreatedAt,
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
