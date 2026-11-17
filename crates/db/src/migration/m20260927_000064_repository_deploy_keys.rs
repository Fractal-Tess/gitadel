use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(DeployKey::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(DeployKey::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(DeployKey::RepositoryId).uuid().not_null())
                    .col(ColumnDef::new(DeployKey::Title).string_len(128).not_null())
                    .col(
                        ColumnDef::new(DeployKey::Fingerprint)
                            .string_len(128)
                            .not_null()
                            .unique_key(),
                    )
                    .col(ColumnDef::new(DeployKey::PublicKey).text().not_null())
                    .col(ColumnDef::new(DeployKey::ReadOnly).boolean().not_null())
                    .col(ColumnDef::new(DeployKey::CreatedBy).uuid())
                    .col(
                        ColumnDef::new(DeployKey::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(ColumnDef::new(DeployKey::LastUsedAt).timestamp_with_time_zone())
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-repository-deploy-key-repository")
                            .from(DeployKey::Table, DeployKey::RepositoryId)
                            .to(Repository::Table, Repository::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-repository-deploy-key-creator")
                            .from(DeployKey::Table, DeployKey::CreatedBy)
                            .to(User::Table, User::Id)
                            .on_delete(ForeignKeyAction::SetNull),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx-repository-deploy-key-repository-created")
                    .table(DeployKey::Table)
                    .col(DeployKey::RepositoryId)
                    .col(DeployKey::CreatedAt)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(DeployKey::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum DeployKey {
    #[sea_orm(iden = "repository_deploy_keys")]
    Table,
    Id,
    RepositoryId,
    Title,
    Fingerprint,
    PublicKey,
    ReadOnly,
    CreatedBy,
    CreatedAt,
    LastUsedAt,
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
