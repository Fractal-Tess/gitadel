use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

/// Optional account email addresses and single-use email tokens.
///
/// Addresses live beside `users` rather than in it: they are optional,
/// verified separately, and only matter when SMTP is configured.
#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(UserEmail::Table)
                    .col(
                        ColumnDef::new(UserEmail::UserId)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(UserEmail::Email).string_len(254).not_null())
                    .col(ColumnDef::new(UserEmail::VerifiedAt).timestamp_with_time_zone())
                    .col(
                        ColumnDef::new(UserEmail::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(UserEmail::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-user-email-user")
                            .from(UserEmail::Table, UserEmail::UserId)
                            .to(User::Table, User::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx-user-email-email")
                    .table(UserEmail::Table)
                    .col(UserEmail::Email)
                    .unique()
                    .to_owned(),
            )
            .await?;
        manager
            .create_table(
                Table::create()
                    .table(EmailToken::Table)
                    .col(
                        ColumnDef::new(EmailToken::TokenHash)
                            .string_len(64)
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(EmailToken::UserId).uuid().not_null())
                    .col(
                        ColumnDef::new(EmailToken::Purpose)
                            .string_len(16)
                            .not_null(),
                    )
                    .col(ColumnDef::new(EmailToken::Email).string_len(254))
                    .col(
                        ColumnDef::new(EmailToken::ExpiresAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(ColumnDef::new(EmailToken::UsedAt).timestamp_with_time_zone())
                    .col(
                        ColumnDef::new(EmailToken::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-email-token-user")
                            .from(EmailToken::Table, EmailToken::UserId)
                            .to(User::Table, User::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx-email-token-user-purpose")
                    .table(EmailToken::Table)
                    .col(EmailToken::UserId)
                    .col(EmailToken::Purpose)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(EmailToken::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(UserEmail::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum User {
    #[sea_orm(iden = "users")]
    Table,
    Id,
}

#[derive(DeriveIden)]
enum UserEmail {
    #[sea_orm(iden = "user_emails")]
    Table,
    UserId,
    Email,
    VerifiedAt,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum EmailToken {
    #[sea_orm(iden = "email_tokens")]
    Table,
    TokenHash,
    UserId,
    Purpose,
    Email,
    ExpiresAt,
    UsedAt,
    CreatedAt,
}
