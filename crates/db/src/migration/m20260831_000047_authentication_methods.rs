use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Instance::Table)
                    .add_column(
                        ColumnDef::new(Instance::PasswordLoginEnabled)
                            .boolean()
                            .not_null()
                            .default(true),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(Instance::Table)
                    .add_column(
                        ColumnDef::new(Instance::PasskeyLoginEnabled)
                            .boolean()
                            .not_null()
                            .default(true),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(OidcProvider::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(OidcProvider::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(OidcProvider::Name).string().not_null())
                    .col(ColumnDef::new(OidcProvider::IssuerUrl).string().not_null())
                    .col(ColumnDef::new(OidcProvider::ClientId).string().not_null())
                    .col(
                        ColumnDef::new(OidcProvider::ClientSecret)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(OidcProvider::Enabled)
                            .boolean()
                            .not_null()
                            .default(true),
                    )
                    .col(
                        ColumnDef::new(OidcProvider::AutoProvision)
                            .boolean()
                            .not_null()
                            .default(true),
                    )
                    .col(
                        ColumnDef::new(OidcProvider::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(OidcProvider::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(OidcIdentity::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(OidcIdentity::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(OidcIdentity::ProviderId).uuid().not_null())
                    .col(ColumnDef::new(OidcIdentity::Subject).string().not_null())
                    .col(ColumnDef::new(OidcIdentity::UserId).uuid().not_null())
                    .col(
                        ColumnDef::new(OidcIdentity::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_oidc_identity_provider")
                            .from(OidcIdentity::Table, OidcIdentity::ProviderId)
                            .to(OidcProvider::Table, OidcProvider::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_oidc_identity_user")
                            .from(OidcIdentity::Table, OidcIdentity::UserId)
                            .to(User::Table, User::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_oidc_identity_provider_subject")
                    .table(OidcIdentity::Table)
                    .col(OidcIdentity::ProviderId)
                    .col(OidcIdentity::Subject)
                    .unique()
                    .to_owned(),
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(OidcIdentity::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(OidcProvider::Table).to_owned())
            .await?;
        Ok(())
    }
}

#[derive(DeriveIden)]
enum Instance {
    Table,
    PasswordLoginEnabled,
    PasskeyLoginEnabled,
}

#[derive(DeriveIden)]
enum OidcProvider {
    #[sea_orm(iden = "oidc_providers")]
    Table,
    Id,
    Name,
    IssuerUrl,
    ClientId,
    ClientSecret,
    Enabled,
    AutoProvision,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum OidcIdentity {
    #[sea_orm(iden = "oidc_identities")]
    Table,
    Id,
    ProviderId,
    Subject,
    UserId,
    CreatedAt,
}

#[derive(DeriveIden)]
enum User {
    #[sea_orm(iden = "users")]
    Table,
    Id,
}
