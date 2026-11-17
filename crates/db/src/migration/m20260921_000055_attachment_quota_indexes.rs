use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_index(
                Index::create()
                    .name("idx-issue-attachment-uploader")
                    .table(IssueAttachment::Table)
                    .col(IssueAttachment::UploaderUserId)
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx-issue-attachment-unbound-created")
                    .table(IssueAttachment::Table)
                    .col(IssueAttachment::IssueId)
                    .col(IssueAttachment::CreatedAt)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_index(
                Index::drop()
                    .name("idx-issue-attachment-unbound-created")
                    .table(IssueAttachment::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_index(
                Index::drop()
                    .name("idx-issue-attachment-uploader")
                    .table(IssueAttachment::Table)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum IssueAttachment {
    #[sea_orm(iden = "issue_attachments")]
    Table,
    IssueId,
    UploaderUserId,
    CreatedAt,
}
