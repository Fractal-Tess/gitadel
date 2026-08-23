use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(IssueAttachment::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(IssueAttachment::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(IssueAttachment::RepositoryId)
                            .uuid()
                            .not_null(),
                    )
                    .col(ColumnDef::new(IssueAttachment::IssueId).uuid())
                    .col(
                        ColumnDef::new(IssueAttachment::UploaderUserId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(IssueAttachment::Name)
                            .string_len(255)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(IssueAttachment::ContentType)
                            .string_len(255)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(IssueAttachment::SizeBytes)
                            .big_integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(IssueAttachment::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-issue-attachment-repository")
                            .from(IssueAttachment::Table, IssueAttachment::RepositoryId)
                            .to(Repository::Table, Repository::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-issue-attachment-issue")
                            .from(IssueAttachment::Table, IssueAttachment::IssueId)
                            .to(RepositoryIssue::Table, RepositoryIssue::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-issue-attachment-uploader")
                            .from(IssueAttachment::Table, IssueAttachment::UploaderUserId)
                            .to(User::Table, User::Id),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx-issue-attachment-repository")
                    .table(IssueAttachment::Table)
                    .col(IssueAttachment::RepositoryId)
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx-issue-attachment-issue")
                    .table(IssueAttachment::Table)
                    .col(IssueAttachment::IssueId)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(IssueAttachment::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum IssueAttachment {
    #[sea_orm(iden = "issue_attachments")]
    Table,
    Id,
    RepositoryId,
    IssueId,
    UploaderUserId,
    Name,
    ContentType,
    SizeBytes,
    CreatedAt,
}

#[derive(DeriveIden)]
enum Repository {
    #[sea_orm(iden = "repositories")]
    Table,
    Id,
}

#[derive(DeriveIden)]
enum RepositoryIssue {
    #[sea_orm(iden = "repository_issues")]
    Table,
    Id,
}

#[derive(DeriveIden)]
enum User {
    #[sea_orm(iden = "users")]
    Table,
    Id,
}
