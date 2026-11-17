use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(CommitStatus::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(CommitStatus::Id)
                            .big_integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(CommitStatus::RepositoryId).uuid().not_null())
                    .col(ColumnDef::new(CommitStatus::Sha).string_len(64).not_null())
                    .col(
                        ColumnDef::new(CommitStatus::Context)
                            .string_len(255)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(CommitStatus::State)
                            .string_len(16)
                            .not_null(),
                    )
                    .col(ColumnDef::new(CommitStatus::Description).text())
                    .col(ColumnDef::new(CommitStatus::TargetUrl).string_len(2048))
                    .col(ColumnDef::new(CommitStatus::CreatorId).uuid())
                    .col(
                        ColumnDef::new(CommitStatus::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(CommitStatus::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-repository-commit-status-repository")
                            .from(CommitStatus::Table, CommitStatus::RepositoryId)
                            .to(Repository::Table, Repository::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-repository-commit-status-creator")
                            .from(CommitStatus::Table, CommitStatus::CreatorId)
                            .to(User::Table, User::Id)
                            .on_delete(ForeignKeyAction::SetNull),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx-repository-commit-status-context")
                    .table(CommitStatus::Table)
                    .col(CommitStatus::RepositoryId)
                    .col(CommitStatus::Sha)
                    .col(CommitStatus::Context)
                    .unique()
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(CommitStatus::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum CommitStatus {
    #[sea_orm(iden = "repository_commit_statuses")]
    Table,
    Id,
    RepositoryId,
    Sha,
    Context,
    State,
    Description,
    TargetUrl,
    CreatorId,
    CreatedAt,
    UpdatedAt,
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
