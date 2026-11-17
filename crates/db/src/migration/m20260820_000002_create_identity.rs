use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(User::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(User::Id).uuid().not_null().primary_key())
                    .col(
                        ColumnDef::new(User::Username)
                            .string_len(64)
                            .not_null()
                            .unique_key(),
                    )
                    .col(ColumnDef::new(User::PasswordHash).text().not_null())
                    .col(
                        ColumnDef::new(User::IsAdmin)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .col(ColumnDef::new(User::DisabledAt).timestamp_with_time_zone())
                    .col(
                        ColumnDef::new(User::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(User::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(Session::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Session::TokenHash)
                            .string_len(64)
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Session::UserId).uuid().not_null())
                    .col(
                        ColumnDef::new(Session::ExpiresAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Session::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Session::LastSeenAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-session-user")
                            .from(Session::Table, Session::UserId)
                            .to(User::Table, User::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(Invitation::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Invitation::TokenHash)
                            .string_len(64)
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Invitation::CreatedBy).uuid().not_null())
                    .col(
                        ColumnDef::new(Invitation::ExpiresAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(ColumnDef::new(Invitation::UsedAt).timestamp_with_time_zone())
                    .col(
                        ColumnDef::new(Invitation::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-invitation-creator")
                            .from(Invitation::Table, Invitation::CreatedBy)
                            .to(User::Table, User::Id)
                            .on_delete(ForeignKeyAction::Restrict),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(Passkey::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(Passkey::Id).uuid().not_null().primary_key())
                    .col(ColumnDef::new(Passkey::UserId).uuid().not_null())
                    .col(ColumnDef::new(Passkey::Name).string_len(128).not_null())
                    .col(
                        ColumnDef::new(Passkey::CredentialId)
                            .text()
                            .not_null()
                            .unique_key(),
                    )
                    .col(ColumnDef::new(Passkey::Credential).text().not_null())
                    .col(
                        ColumnDef::new(Passkey::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(ColumnDef::new(Passkey::LastUsedAt).timestamp_with_time_zone())
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-passkey-user")
                            .from(Passkey::Table, Passkey::UserId)
                            .to(User::Table, User::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(SshKey::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(SshKey::Id).uuid().not_null().primary_key())
                    .col(ColumnDef::new(SshKey::UserId).uuid().not_null())
                    .col(ColumnDef::new(SshKey::Name).string_len(128).not_null())
                    .col(
                        ColumnDef::new(SshKey::Fingerprint)
                            .string_len(128)
                            .not_null()
                            .unique_key(),
                    )
                    .col(ColumnDef::new(SshKey::PublicKey).text().not_null())
                    .col(
                        ColumnDef::new(SshKey::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(ColumnDef::new(SshKey::LastUsedAt).timestamp_with_time_zone())
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-ssh-key-user")
                            .from(SshKey::Table, SshKey::UserId)
                            .to(User::Table, User::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(ApiToken::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(ApiToken::Id).uuid().not_null().primary_key())
                    .col(ColumnDef::new(ApiToken::UserId).uuid().not_null())
                    .col(ColumnDef::new(ApiToken::Name).string_len(128).not_null())
                    .col(
                        ColumnDef::new(ApiToken::TokenHash)
                            .string_len(64)
                            .not_null()
                            .unique_key(),
                    )
                    .col(ColumnDef::new(ApiToken::Scopes).integer().not_null())
                    .col(ColumnDef::new(ApiToken::ExpiresAt).timestamp_with_time_zone())
                    .col(
                        ColumnDef::new(ApiToken::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(ColumnDef::new(ApiToken::LastUsedAt).timestamp_with_time_zone())
                    .col(ColumnDef::new(ApiToken::RevokedAt).timestamp_with_time_zone())
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-api-token-user")
                            .from(ApiToken::Table, ApiToken::UserId)
                            .to(User::Table, User::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(Organization::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Organization::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(Organization::Slug)
                            .string_len(64)
                            .not_null()
                            .unique_key(),
                    )
                    .col(
                        ColumnDef::new(Organization::DisplayName)
                            .string_len(128)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Organization::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Organization::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(OrganizationMember::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(OrganizationMember::OrganizationId)
                            .uuid()
                            .not_null(),
                    )
                    .col(ColumnDef::new(OrganizationMember::UserId).uuid().not_null())
                    .col(
                        ColumnDef::new(OrganizationMember::Role)
                            .string_len(16)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(OrganizationMember::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .primary_key(
                        Index::create()
                            .col(OrganizationMember::OrganizationId)
                            .col(OrganizationMember::UserId),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-organization-member-organization")
                            .from(
                                OrganizationMember::Table,
                                OrganizationMember::OrganizationId,
                            )
                            .to(Organization::Table, Organization::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-organization-member-user")
                            .from(OrganizationMember::Table, OrganizationMember::UserId)
                            .to(User::Table, User::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(AuditEvent::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(AuditEvent::Id)
                            .big_integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(AuditEvent::ActorUserId).uuid())
                    .col(ColumnDef::new(AuditEvent::Action).string_len(96).not_null())
                    .col(ColumnDef::new(AuditEvent::Target).text())
                    .col(ColumnDef::new(AuditEvent::RemoteAddress).string_len(128))
                    .col(
                        ColumnDef::new(AuditEvent::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-audit-event-actor")
                            .from(AuditEvent::Table, AuditEvent::ActorUserId)
                            .to(User::Table, User::Id)
                            .on_delete(ForeignKeyAction::SetNull),
                    )
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(AuditEvent::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(OrganizationMember::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(Organization::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(ApiToken::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(SshKey::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(Passkey::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(Invitation::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(Session::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(User::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum User {
    #[sea_orm(iden = "users")]
    Table,
    Id,
    Username,
    PasswordHash,
    IsAdmin,
    DisabledAt,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum Session {
    #[sea_orm(iden = "sessions")]
    Table,
    TokenHash,
    UserId,
    ExpiresAt,
    CreatedAt,
    LastSeenAt,
}

#[derive(DeriveIden)]
enum Invitation {
    #[sea_orm(iden = "invitations")]
    Table,
    TokenHash,
    CreatedBy,
    ExpiresAt,
    UsedAt,
    CreatedAt,
}

#[derive(DeriveIden)]
enum Passkey {
    #[sea_orm(iden = "passkeys")]
    Table,
    Id,
    UserId,
    Name,
    CredentialId,
    Credential,
    CreatedAt,
    LastUsedAt,
}

#[derive(DeriveIden)]
enum SshKey {
    #[sea_orm(iden = "ssh_keys")]
    Table,
    Id,
    UserId,
    Name,
    Fingerprint,
    PublicKey,
    CreatedAt,
    LastUsedAt,
}

#[derive(DeriveIden)]
enum ApiToken {
    #[sea_orm(iden = "api_tokens")]
    Table,
    Id,
    UserId,
    Name,
    TokenHash,
    Scopes,
    ExpiresAt,
    CreatedAt,
    LastUsedAt,
    RevokedAt,
}

#[derive(DeriveIden)]
enum Organization {
    #[sea_orm(iden = "organizations")]
    Table,
    Id,
    Slug,
    DisplayName,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum OrganizationMember {
    #[sea_orm(iden = "organization_members")]
    Table,
    OrganizationId,
    UserId,
    Role,
    CreatedAt,
}

#[derive(DeriveIden)]
enum AuditEvent {
    #[sea_orm(iden = "audit_events")]
    Table,
    Id,
    ActorUserId,
    Action,
    Target,
    RemoteAddress,
    CreatedAt,
}
