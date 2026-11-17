use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for column in [
            ColumnDef::new(IssueLabel::ExternalSource)
                .string_len(32)
                .to_owned(),
            ColumnDef::new(IssueLabel::ExternalInstanceUrl)
                .string_len(2048)
                .to_owned(),
            ColumnDef::new(IssueLabel::ExternalId)
                .string_len(255)
                .to_owned(),
            ColumnDef::new(IssueLabel::ExternalUrl)
                .string_len(2048)
                .to_owned(),
            ColumnDef::new(IssueLabel::ExternalUpdatedAt)
                .timestamp_with_time_zone()
                .to_owned(),
        ] {
            manager
                .alter_table(
                    Table::alter()
                        .table(IssueLabel::Table)
                        .add_column(column)
                        .to_owned(),
                )
                .await?;
        }
        for column in [
            ColumnDef::new(RepositoryRelease::ExternalSource)
                .string_len(32)
                .to_owned(),
            ColumnDef::new(RepositoryRelease::ExternalInstanceUrl)
                .string_len(2048)
                .to_owned(),
            ColumnDef::new(RepositoryRelease::ExternalId)
                .string_len(255)
                .to_owned(),
            ColumnDef::new(RepositoryRelease::ExternalUrl)
                .string_len(2048)
                .to_owned(),
            ColumnDef::new(RepositoryRelease::ExternalAuthor)
                .string_len(255)
                .to_owned(),
            ColumnDef::new(RepositoryRelease::ExternalAuthorUrl)
                .string_len(2048)
                .to_owned(),
            ColumnDef::new(RepositoryRelease::ExternalUpdatedAt)
                .timestamp_with_time_zone()
                .to_owned(),
        ] {
            manager
                .alter_table(
                    Table::alter()
                        .table(RepositoryRelease::Table)
                        .add_column(column)
                        .to_owned(),
                )
                .await?;
        }
        for column in [
            ColumnDef::new(ReleaseAsset::ExternalSource)
                .string_len(32)
                .to_owned(),
            ColumnDef::new(ReleaseAsset::ExternalInstanceUrl)
                .string_len(2048)
                .to_owned(),
            ColumnDef::new(ReleaseAsset::ExternalId)
                .string_len(255)
                .to_owned(),
            ColumnDef::new(ReleaseAsset::ExternalUrl)
                .string_len(2048)
                .to_owned(),
            ColumnDef::new(ReleaseAsset::ExternalUpdatedAt)
                .timestamp_with_time_zone()
                .to_owned(),
        ] {
            manager
                .alter_table(
                    Table::alter()
                        .table(ReleaseAsset::Table)
                        .add_column(column)
                        .to_owned(),
                )
                .await?;
        }

        manager
            .create_index(
                Index::create()
                    .name("uq-issue-label-imported-source")
                    .table(IssueLabel::Table)
                    .col(IssueLabel::RepositoryId)
                    .col(IssueLabel::ExternalSource)
                    .col(IssueLabel::ExternalInstanceUrl)
                    .col(IssueLabel::ExternalId)
                    .unique()
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("uq-repository-release-imported-source")
                    .table(RepositoryRelease::Table)
                    .col(RepositoryRelease::RepositoryId)
                    .col(RepositoryRelease::ExternalSource)
                    .col(RepositoryRelease::ExternalInstanceUrl)
                    .col(RepositoryRelease::ExternalId)
                    .unique()
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("uq-release-asset-imported-source")
                    .table(ReleaseAsset::Table)
                    .col(ReleaseAsset::ReleaseId)
                    .col(ReleaseAsset::ExternalSource)
                    .col(ReleaseAsset::ExternalInstanceUrl)
                    .col(ReleaseAsset::ExternalId)
                    .unique()
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for name in [
            "uq-release-asset-imported-source",
            "uq-repository-release-imported-source",
            "uq-issue-label-imported-source",
        ] {
            manager
                .drop_index(Index::drop().name(name).to_owned())
                .await?;
        }
        for column in [
            ReleaseAsset::ExternalUpdatedAt,
            ReleaseAsset::ExternalUrl,
            ReleaseAsset::ExternalId,
            ReleaseAsset::ExternalInstanceUrl,
            ReleaseAsset::ExternalSource,
        ] {
            manager
                .alter_table(
                    Table::alter()
                        .table(ReleaseAsset::Table)
                        .drop_column(column)
                        .to_owned(),
                )
                .await?;
        }
        for column in [
            RepositoryRelease::ExternalUpdatedAt,
            RepositoryRelease::ExternalAuthorUrl,
            RepositoryRelease::ExternalAuthor,
            RepositoryRelease::ExternalUrl,
            RepositoryRelease::ExternalId,
            RepositoryRelease::ExternalInstanceUrl,
            RepositoryRelease::ExternalSource,
        ] {
            manager
                .alter_table(
                    Table::alter()
                        .table(RepositoryRelease::Table)
                        .drop_column(column)
                        .to_owned(),
                )
                .await?;
        }
        for column in [
            IssueLabel::ExternalUpdatedAt,
            IssueLabel::ExternalUrl,
            IssueLabel::ExternalId,
            IssueLabel::ExternalInstanceUrl,
            IssueLabel::ExternalSource,
        ] {
            manager
                .alter_table(
                    Table::alter()
                        .table(IssueLabel::Table)
                        .drop_column(column)
                        .to_owned(),
                )
                .await?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, DeriveIden)]
enum IssueLabel {
    #[sea_orm(iden = "issue_labels")]
    Table,
    RepositoryId,
    ExternalSource,
    ExternalInstanceUrl,
    ExternalId,
    ExternalUrl,
    ExternalUpdatedAt,
}

#[derive(Clone, Copy, DeriveIden)]
enum RepositoryRelease {
    #[sea_orm(iden = "repository_releases")]
    Table,
    RepositoryId,
    ExternalSource,
    ExternalInstanceUrl,
    ExternalId,
    ExternalUrl,
    ExternalAuthor,
    ExternalAuthorUrl,
    ExternalUpdatedAt,
}

#[derive(Clone, Copy, DeriveIden)]
enum ReleaseAsset {
    #[sea_orm(iden = "release_assets")]
    Table,
    ReleaseId,
    ExternalSource,
    ExternalInstanceUrl,
    ExternalId,
    ExternalUrl,
    ExternalUpdatedAt,
}
