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
                        ColumnDef::new(Repository::IssueCounter)
                            .big_integer()
                            .not_null()
                            .default(0),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_table(
                Table::create()
                    .table(RepositoryIssue::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(RepositoryIssue::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(RepositoryIssue::RepositoryId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(RepositoryIssue::Number)
                            .big_integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(RepositoryIssue::AuthorUserId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(RepositoryIssue::Title)
                            .string_len(255)
                            .not_null(),
                    )
                    .col(ColumnDef::new(RepositoryIssue::Body).text().not_null())
                    .col(
                        ColumnDef::new(RepositoryIssue::State)
                            .string_len(16)
                            .not_null(),
                    )
                    .col(ColumnDef::new(RepositoryIssue::AssigneeUserId).uuid())
                    .col(
                        ColumnDef::new(RepositoryIssue::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(RepositoryIssue::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(ColumnDef::new(RepositoryIssue::ClosedAt).timestamp_with_time_zone())
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-repository-issue-repository")
                            .from(RepositoryIssue::Table, RepositoryIssue::RepositoryId)
                            .to(Repository::Table, Repository::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-repository-issue-author")
                            .from(RepositoryIssue::Table, RepositoryIssue::AuthorUserId)
                            .to(User::Table, User::Id),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-repository-issue-assignee")
                            .from(RepositoryIssue::Table, RepositoryIssue::AssigneeUserId)
                            .to(User::Table, User::Id),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx-repository-issue-number")
                    .table(RepositoryIssue::Table)
                    .col(RepositoryIssue::RepositoryId)
                    .col(RepositoryIssue::Number)
                    .unique()
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx-repository-issue-state-updated")
                    .table(RepositoryIssue::Table)
                    .col(RepositoryIssue::RepositoryId)
                    .col(RepositoryIssue::State)
                    .col(RepositoryIssue::UpdatedAt)
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(IssueComment::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(IssueComment::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(IssueComment::IssueId).uuid().not_null())
                    .col(ColumnDef::new(IssueComment::AuthorUserId).uuid().not_null())
                    .col(ColumnDef::new(IssueComment::Body).text().not_null())
                    .col(
                        ColumnDef::new(IssueComment::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(IssueComment::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-issue-comment-issue")
                            .from(IssueComment::Table, IssueComment::IssueId)
                            .to(RepositoryIssue::Table, RepositoryIssue::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-issue-comment-author")
                            .from(IssueComment::Table, IssueComment::AuthorUserId)
                            .to(User::Table, User::Id),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx-issue-comment-issue-created")
                    .table(IssueComment::Table)
                    .col(IssueComment::IssueId)
                    .col(IssueComment::CreatedAt)
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(IssueLabel::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(IssueLabel::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(IssueLabel::RepositoryId).uuid().not_null())
                    .col(ColumnDef::new(IssueLabel::Name).string_len(64).not_null())
                    .col(ColumnDef::new(IssueLabel::Color).string_len(6).not_null())
                    .col(
                        ColumnDef::new(IssueLabel::Description)
                            .string_len(255)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(IssueLabel::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-issue-label-repository")
                            .from(IssueLabel::Table, IssueLabel::RepositoryId)
                            .to(Repository::Table, Repository::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx-issue-label-repository-name")
                    .table(IssueLabel::Table)
                    .col(IssueLabel::RepositoryId)
                    .col(IssueLabel::Name)
                    .unique()
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(IssueLabelAssignment::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(IssueLabelAssignment::IssueId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(IssueLabelAssignment::LabelId)
                            .uuid()
                            .not_null(),
                    )
                    .primary_key(
                        Index::create()
                            .col(IssueLabelAssignment::IssueId)
                            .col(IssueLabelAssignment::LabelId),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-issue-label-assignment-issue")
                            .from(IssueLabelAssignment::Table, IssueLabelAssignment::IssueId)
                            .to(RepositoryIssue::Table, RepositoryIssue::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-issue-label-assignment-label")
                            .from(IssueLabelAssignment::Table, IssueLabelAssignment::LabelId)
                            .to(IssueLabel::Table, IssueLabel::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(IssueLabelAssignment::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(IssueLabel::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(IssueComment::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(RepositoryIssue::Table).to_owned())
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(Repository::Table)
                    .drop_column(Repository::IssueCounter)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum RepositoryIssue {
    #[sea_orm(iden = "repository_issues")]
    Table,
    Id,
    RepositoryId,
    Number,
    AuthorUserId,
    Title,
    Body,
    State,
    AssigneeUserId,
    CreatedAt,
    UpdatedAt,
    ClosedAt,
}

#[derive(DeriveIden)]
enum IssueComment {
    #[sea_orm(iden = "issue_comments")]
    Table,
    Id,
    IssueId,
    AuthorUserId,
    Body,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum IssueLabel {
    #[sea_orm(iden = "issue_labels")]
    Table,
    Id,
    RepositoryId,
    Name,
    Color,
    Description,
    CreatedAt,
}

#[derive(DeriveIden)]
enum IssueLabelAssignment {
    #[sea_orm(iden = "issue_label_assignments")]
    Table,
    IssueId,
    LabelId,
}

#[derive(DeriveIden)]
enum Repository {
    #[sea_orm(iden = "repositories")]
    Table,
    Id,
    IssueCounter,
}

#[derive(DeriveIden)]
enum User {
    #[sea_orm(iden = "users")]
    Table,
    Id,
}
