use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(LfsLock::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(LfsLock::Id).uuid().not_null().primary_key())
                    .col(ColumnDef::new(LfsLock::RepositoryId).uuid().not_null())
                    .col(ColumnDef::new(LfsLock::UserId).uuid().not_null())
                    .col(ColumnDef::new(LfsLock::Path).string().not_null())
                    .col(
                        ColumnDef::new(LfsLock::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-lfs-lock-repository")
                            .from(LfsLock::Table, LfsLock::RepositoryId)
                            .to(Repository::Table, Repository::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-lfs-lock-user")
                            .from(LfsLock::Table, LfsLock::UserId)
                            .to(User::Table, User::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx-lfs-lock-repository-path")
                    .table(LfsLock::Table)
                    .col(LfsLock::RepositoryId)
                    .col(LfsLock::Path)
                    .unique()
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx-lfs-lock-repository-created")
                    .table(LfsLock::Table)
                    .col(LfsLock::RepositoryId)
                    .col(LfsLock::CreatedAt)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(LfsLock::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum LfsLock {
    #[sea_orm(iden = "lfs_locks")]
    Table,
    Id,
    RepositoryId,
    UserId,
    Path,
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
