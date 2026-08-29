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
                        ColumnDef::new(Repository::Mirrored)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_table(
                Table::create()
                    .table(RepositoryMirror::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(RepositoryMirror::RepositoryId)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(RepositoryMirror::RemoteUrl)
                            .string_len(2048)
                            .not_null(),
                    )
                    .col(ColumnDef::new(RepositoryMirror::Username).string_len(255))
                    .col(ColumnDef::new(RepositoryMirror::Secret).text())
                    .col(ColumnDef::new(RepositoryMirror::Schedule).string_len(255))
                    .col(
                        ColumnDef::new(RepositoryMirror::LastAttemptedAt)
                            .timestamp_with_time_zone(),
                    )
                    .col(ColumnDef::new(RepositoryMirror::LastSyncedAt).timestamp_with_time_zone())
                    .col(ColumnDef::new(RepositoryMirror::LastError).text())
                    .col(ColumnDef::new(RepositoryMirror::NextSyncAt).timestamp_with_time_zone())
                    .col(
                        ColumnDef::new(RepositoryMirror::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(RepositoryMirror::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-repository-mirror-repository")
                            .from(RepositoryMirror::Table, RepositoryMirror::RepositoryId)
                            .to(Repository::Table, Repository::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx-repository-mirror-due")
                    .table(RepositoryMirror::Table)
                    .col(RepositoryMirror::NextSyncAt)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(RepositoryMirror::Table).to_owned())
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(Repository::Table)
                    .drop_column(Repository::Mirrored)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum Repository {
    #[sea_orm(iden = "repositories")]
    Table,
    Mirrored,
    Id,
}

#[derive(DeriveIden)]
enum RepositoryMirror {
    #[sea_orm(iden = "repository_mirrors")]
    Table,
    RepositoryId,
    RemoteUrl,
    Username,
    Secret,
    Schedule,
    LastAttemptedAt,
    LastSyncedAt,
    LastError,
    NextSyncAt,
    CreatedAt,
    UpdatedAt,
}
