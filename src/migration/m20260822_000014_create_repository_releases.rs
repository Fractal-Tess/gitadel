use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(RepositoryRelease::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(RepositoryRelease::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(RepositoryRelease::RepositoryId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(RepositoryRelease::AuthorUserId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(RepositoryRelease::TargetRevision)
                            .string_len(255)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(RepositoryRelease::TargetOid)
                            .string_len(64)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(RepositoryRelease::Title)
                            .string_len(255)
                            .not_null(),
                    )
                    .col(ColumnDef::new(RepositoryRelease::Body).text().not_null())
                    .col(
                        ColumnDef::new(RepositoryRelease::Prerelease)
                            .boolean()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(RepositoryRelease::PublishedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(RepositoryRelease::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(RepositoryRelease::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-repository-release-repository")
                            .from(RepositoryRelease::Table, RepositoryRelease::RepositoryId)
                            .to(Repository::Table, Repository::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-repository-release-author")
                            .from(RepositoryRelease::Table, RepositoryRelease::AuthorUserId)
                            .to(User::Table, User::Id),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx-repository-release-repository-published")
                    .table(RepositoryRelease::Table)
                    .col(RepositoryRelease::RepositoryId)
                    .col(RepositoryRelease::PublishedAt)
                    .to_owned(),
            )
            .await?;
        manager
            .create_table(
                Table::create()
                    .table(ReleaseAsset::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(ReleaseAsset::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(ReleaseAsset::ReleaseId).uuid().not_null())
                    .col(
                        ColumnDef::new(ReleaseAsset::Name)
                            .string_len(255)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ReleaseAsset::ContentType)
                            .string_len(255)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ReleaseAsset::SizeBytes)
                            .big_integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ReleaseAsset::DownloadCount)
                            .big_integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ReleaseAsset::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-release-asset-release")
                            .from(ReleaseAsset::Table, ReleaseAsset::ReleaseId)
                            .to(RepositoryRelease::Table, RepositoryRelease::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx-release-asset-release-name")
                    .table(ReleaseAsset::Table)
                    .col(ReleaseAsset::ReleaseId)
                    .col(ReleaseAsset::Name)
                    .unique()
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(ReleaseAsset::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(RepositoryRelease::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum RepositoryRelease {
    #[sea_orm(iden = "repository_releases")]
    Table,
    Id,
    RepositoryId,
    AuthorUserId,
    TargetRevision,
    TargetOid,
    Title,
    Body,
    Prerelease,
    PublishedAt,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum ReleaseAsset {
    #[sea_orm(iden = "release_assets")]
    Table,
    Id,
    ReleaseId,
    Name,
    ContentType,
    SizeBytes,
    DownloadCount,
    CreatedAt,
}

#[derive(DeriveIden)]
enum Repository {
    #[sea_orm(iden = "repositories")]
    Table,
    Id,
}

#[derive(DeriveIden)]
enum User {
    #[sea_orm(iden = "users")]
    Table,
    Id,
}
