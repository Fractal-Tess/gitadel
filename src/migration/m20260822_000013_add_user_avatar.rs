use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    fn use_transaction(&self) -> Option<bool> {
        Some(true)
    }

    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(User::Table)
                    .add_column(
                        ColumnDef::new(User::AvatarUpdatedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(UserAvatar::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(UserAvatar::UserId)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(UserAvatar::Content).blob().not_null())
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-user-avatar-user")
                            .from(UserAvatar::Table, UserAvatar::UserId)
                            .to(User::Table, User::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(UserAvatar::Table).to_owned())
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(User::Table)
                    .drop_column(User::AvatarUpdatedAt)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum User {
    #[sea_orm(iden = "users")]
    Table,
    Id,
    AvatarUpdatedAt,
}

#[derive(DeriveIden)]
enum UserAvatar {
    #[sea_orm(iden = "user_avatars")]
    Table,
    UserId,
    Content,
}
