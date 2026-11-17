use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(OauthApplication::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(OauthApplication::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(OauthApplication::UserId).uuid().not_null())
                    .col(
                        ColumnDef::new(OauthApplication::Name)
                            .string_len(128)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(OauthApplication::ClientId)
                            .string_len(64)
                            .not_null()
                            .unique_key(),
                    )
                    .col(
                        ColumnDef::new(OauthApplication::ClientSecretHash)
                            .string_len(64)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(OauthApplication::RedirectUri)
                            .text()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(OauthApplication::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-oauth-application-user")
                            .from(OauthApplication::Table, OauthApplication::UserId)
                            .to(User::Table, User::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(OauthAuthorizationCode::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(OauthAuthorizationCode::CodeHash)
                            .string_len(64)
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(OauthAuthorizationCode::ApplicationId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(OauthAuthorizationCode::UserId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(OauthAuthorizationCode::RedirectUri)
                            .text()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(OauthAuthorizationCode::Scope)
                            .text()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(OauthAuthorizationCode::ExpiresAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(OauthAuthorizationCode::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-oauth-code-application")
                            .from(
                                OauthAuthorizationCode::Table,
                                OauthAuthorizationCode::ApplicationId,
                            )
                            .to(OauthApplication::Table, OauthApplication::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-oauth-code-user")
                            .from(
                                OauthAuthorizationCode::Table,
                                OauthAuthorizationCode::UserId,
                            )
                            .to(User::Table, User::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(OauthAccessToken::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(OauthAccessToken::TokenHash)
                            .string_len(64)
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(OauthAccessToken::ApplicationId)
                            .uuid()
                            .not_null(),
                    )
                    .col(ColumnDef::new(OauthAccessToken::UserId).uuid().not_null())
                    .col(
                        ColumnDef::new(OauthAccessToken::Scopes)
                            .integer()
                            .not_null(),
                    )
                    .col(ColumnDef::new(OauthAccessToken::Scope).text().not_null())
                    .col(
                        ColumnDef::new(OauthAccessToken::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(ColumnDef::new(OauthAccessToken::LastUsedAt).timestamp_with_time_zone())
                    .col(ColumnDef::new(OauthAccessToken::RevokedAt).timestamp_with_time_zone())
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-oauth-token-application")
                            .from(OauthAccessToken::Table, OauthAccessToken::ApplicationId)
                            .to(OauthApplication::Table, OauthApplication::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-oauth-token-user")
                            .from(OauthAccessToken::Table, OauthAccessToken::UserId)
                            .to(User::Table, User::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(OauthAccessToken::Table).to_owned())
            .await?;
        manager
            .drop_table(
                Table::drop()
                    .table(OauthAuthorizationCode::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(Table::drop().table(OauthApplication::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum OauthApplication {
    #[sea_orm(iden = "oauth_applications")]
    Table,
    Id,
    UserId,
    Name,
    ClientId,
    ClientSecretHash,
    RedirectUri,
    CreatedAt,
}

#[derive(DeriveIden)]
enum OauthAuthorizationCode {
    #[sea_orm(iden = "oauth_authorization_codes")]
    Table,
    CodeHash,
    ApplicationId,
    UserId,
    RedirectUri,
    Scope,
    ExpiresAt,
    CreatedAt,
}

#[derive(DeriveIden)]
enum OauthAccessToken {
    #[sea_orm(iden = "oauth_access_tokens")]
    Table,
    TokenHash,
    ApplicationId,
    UserId,
    Scopes,
    Scope,
    CreatedAt,
    LastUsedAt,
    RevokedAt,
}

#[derive(DeriveIden)]
enum User {
    #[sea_orm(iden = "users")]
    Table,
    Id,
}
