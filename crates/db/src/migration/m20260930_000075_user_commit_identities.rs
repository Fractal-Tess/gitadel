use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

/// The addresses a user writes commits under, which attribute those commits to
/// them on their profile, and the name and address commits made in the browser
/// are authored with. An address belongs to one account at most.
#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(UserCommitEmail::Table)
                    .col(ColumnDef::new(UserCommitEmail::UserId).uuid().not_null())
                    .col(
                        ColumnDef::new(UserCommitEmail::Email)
                            .string()
                            .not_null()
                            .unique_key(),
                    )
                    .col(
                        ColumnDef::new(UserCommitEmail::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .primary_key(
                        Index::create()
                            .col(UserCommitEmail::UserId)
                            .col(UserCommitEmail::Email),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-user-commit-email-user")
                            .from(UserCommitEmail::Table, UserCommitEmail::UserId)
                            .to(User::Table, User::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_table(
                Table::create()
                    .table(UserCommitProfile::Table)
                    .col(
                        ColumnDef::new(UserCommitProfile::UserId)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(UserCommitProfile::Name).string().null())
                    .col(
                        ColumnDef::new(UserCommitProfile::PrimaryEmail)
                            .string()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(UserCommitProfile::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-user-commit-profile-user")
                            .from(UserCommitProfile::Table, UserCommitProfile::UserId)
                            .to(User::Table, User::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(UserCommitProfile::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(UserCommitEmail::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum UserCommitEmail {
    #[sea_orm(iden = "user_commit_emails")]
    Table,
    UserId,
    Email,
    CreatedAt,
}

#[derive(DeriveIden)]
enum UserCommitProfile {
    #[sea_orm(iden = "user_commit_profiles")]
    Table,
    UserId,
    Name,
    PrimaryEmail,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum User {
    #[sea_orm(iden = "users")]
    Table,
    Id,
}
