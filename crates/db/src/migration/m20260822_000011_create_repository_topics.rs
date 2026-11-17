use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Topic::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(Topic::Id).uuid().not_null().primary_key())
                    .col(
                        ColumnDef::new(Topic::Name)
                            .string_len(64)
                            .not_null()
                            .unique_key(),
                    )
                    .col(
                        ColumnDef::new(Topic::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_table(
                Table::create()
                    .table(RepositoryTopic::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(RepositoryTopic::RepositoryId)
                            .uuid()
                            .not_null(),
                    )
                    .col(ColumnDef::new(RepositoryTopic::TopicId).uuid().not_null())
                    .col(
                        ColumnDef::new(RepositoryTopic::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .primary_key(
                        Index::create()
                            .col(RepositoryTopic::RepositoryId)
                            .col(RepositoryTopic::TopicId),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-repository-topic-repository")
                            .from(RepositoryTopic::Table, RepositoryTopic::RepositoryId)
                            .to(Repository::Table, Repository::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-repository-topic-topic")
                            .from(RepositoryTopic::Table, RepositoryTopic::TopicId)
                            .to(Topic::Table, Topic::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx-repository-topic-topic")
                    .table(RepositoryTopic::Table)
                    .col(RepositoryTopic::TopicId)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(RepositoryTopic::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(Topic::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum Topic {
    #[sea_orm(iden = "topics")]
    Table,
    Id,
    Name,
    CreatedAt,
}

#[derive(DeriveIden)]
enum RepositoryTopic {
    #[sea_orm(iden = "repository_topics")]
    Table,
    RepositoryId,
    TopicId,
    CreatedAt,
}

#[derive(DeriveIden)]
enum Repository {
    #[sea_orm(iden = "repositories")]
    Table,
    Id,
}
