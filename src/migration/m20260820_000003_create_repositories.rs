use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Namespace::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Namespace::Slug)
                            .string_len(39)
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Namespace::Kind).string_len(16).not_null())
                    .col(ColumnDef::new(Namespace::UserId).uuid().unique_key())
                    .col(
                        ColumnDef::new(Namespace::OrganizationId)
                            .uuid()
                            .unique_key(),
                    )
                    .col(
                        ColumnDef::new(Namespace::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-namespace-user")
                            .from(Namespace::Table, Namespace::UserId)
                            .to(User::Table, User::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-namespace-organization")
                            .from(Namespace::Table, Namespace::OrganizationId)
                            .to(Organization::Table, Organization::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .get_connection()
            .execute_unprepared(
                "INSERT INTO namespaces (slug, kind, user_id, organization_id, created_at) \
                 SELECT username, 'user', id, NULL, created_at FROM users",
            )
            .await?;
        manager
            .get_connection()
            .execute_unprepared(
                "INSERT INTO namespaces (slug, kind, user_id, organization_id, created_at) \
                 SELECT slug, 'organization', NULL, id, created_at FROM organizations",
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(Repository::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Repository::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(Repository::Namespace)
                            .string_len(39)
                            .not_null(),
                    )
                    .col(ColumnDef::new(Repository::Name).string_len(100).not_null())
                    .col(ColumnDef::new(Repository::Description).string_len(512))
                    .col(
                        ColumnDef::new(Repository::Visibility)
                            .string_len(16)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Repository::ObjectFormat)
                            .string_len(16)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Repository::DefaultBranch)
                            .string_len(255)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Repository::StorageKey)
                            .uuid()
                            .not_null()
                            .unique_key(),
                    )
                    .col(ColumnDef::new(Repository::CreatedBy).uuid().not_null())
                    .col(ColumnDef::new(Repository::ArchivedAt).timestamp_with_time_zone())
                    .col(
                        ColumnDef::new(Repository::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Repository::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-repository-namespace")
                            .from(Repository::Table, Repository::Namespace)
                            .to(Namespace::Table, Namespace::Slug)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-repository-creator")
                            .from(Repository::Table, Repository::CreatedBy)
                            .to(User::Table, User::Id)
                            .on_delete(ForeignKeyAction::Restrict),
                    )
                    .index(
                        Index::create()
                            .name("uq-repository-namespace-name")
                            .unique()
                            .col(Repository::Namespace)
                            .col(Repository::Name),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(RepositoryCollaborator::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(RepositoryCollaborator::RepositoryId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(RepositoryCollaborator::UserId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(RepositoryCollaborator::Role)
                            .string_len(16)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(RepositoryCollaborator::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .primary_key(
                        Index::create()
                            .col(RepositoryCollaborator::RepositoryId)
                            .col(RepositoryCollaborator::UserId),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-repository-collaborator-repository")
                            .from(
                                RepositoryCollaborator::Table,
                                RepositoryCollaborator::RepositoryId,
                            )
                            .to(Repository::Table, Repository::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-repository-collaborator-user")
                            .from(
                                RepositoryCollaborator::Table,
                                RepositoryCollaborator::UserId,
                            )
                            .to(User::Table, User::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(RepositoryCollaborator::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(Table::drop().table(Repository::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(Namespace::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum Namespace {
    #[sea_orm(iden = "namespaces")]
    Table,
    Slug,
    Kind,
    UserId,
    OrganizationId,
    CreatedAt,
}

#[derive(DeriveIden)]
enum Repository {
    #[sea_orm(iden = "repositories")]
    Table,
    Id,
    Namespace,
    Name,
    Description,
    Visibility,
    ObjectFormat,
    DefaultBranch,
    StorageKey,
    CreatedBy,
    ArchivedAt,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum RepositoryCollaborator {
    #[sea_orm(iden = "repository_collaborators")]
    Table,
    RepositoryId,
    UserId,
    Role,
    CreatedAt,
}

#[derive(DeriveIden)]
enum User {
    #[sea_orm(iden = "users")]
    Table,
    Id,
}

#[derive(DeriveIden)]
enum Organization {
    #[sea_orm(iden = "organizations")]
    Table,
    Id,
}
