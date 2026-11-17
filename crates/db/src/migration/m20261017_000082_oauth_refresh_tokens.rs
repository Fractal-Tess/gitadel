use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

/// Refresh tokens let OAuth clients replace an expired access token without
/// sending the user through authorization again. Each is used once.
#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(OauthRefreshToken::Table)
                    .col(
                        ColumnDef::new(OauthRefreshToken::TokenHash)
                            .string_len(64)
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(OauthRefreshToken::ApplicationId)
                            .uuid()
                            .not_null(),
                    )
                    .col(ColumnDef::new(OauthRefreshToken::UserId).uuid().not_null())
                    .col(ColumnDef::new(OauthRefreshToken::Scope).text().not_null())
                    .col(
                        ColumnDef::new(OauthRefreshToken::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(OauthRefreshToken::ExpiresAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-oauth-refresh-token-application")
                            .from(OauthRefreshToken::Table, OauthRefreshToken::ApplicationId)
                            .to(OauthApplication::Table, OauthApplication::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-oauth-refresh-token-user")
                            .from(OauthRefreshToken::Table, OauthRefreshToken::UserId)
                            .to(User::Table, User::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(OauthRefreshToken::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum OauthRefreshToken {
    #[sea_orm(iden = "oauth_refresh_tokens")]
    Table,
    TokenHash,
    ApplicationId,
    UserId,
    Scope,
    CreatedAt,
    ExpiresAt,
}

#[derive(DeriveIden)]
enum OauthApplication {
    #[sea_orm(iden = "oauth_applications")]
    Table,
    Id,
}

#[derive(DeriveIden)]
enum User {
    #[sea_orm(iden = "users")]
    Table,
    Id,
}
