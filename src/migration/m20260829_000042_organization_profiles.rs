use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Organization::Table)
                    .add_column(
                        ColumnDef::new(Organization::AvatarUpdatedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_table(
                Table::create()
                    .table(OrganizationAvatar::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(OrganizationAvatar::OrganizationId)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(OrganizationAvatar::Content)
                            .binary()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-organization-avatar-organization")
                            .from(
                                OrganizationAvatar::Table,
                                OrganizationAvatar::OrganizationId,
                            )
                            .to(Organization::Table, Organization::Id)
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
                    .table(OrganizationAvatar::Table)
                    .if_exists()
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(Organization::Table)
                    .drop_column(Organization::AvatarUpdatedAt)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum Organization {
    #[sea_orm(iden = "organizations")]
    Table,
    Id,
    AvatarUpdatedAt,
}

#[derive(DeriveIden)]
enum OrganizationAvatar {
    #[sea_orm(iden = "organization_avatars")]
    Table,
    OrganizationId,
    Content,
}
