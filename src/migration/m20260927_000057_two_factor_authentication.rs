use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(UserTotp::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(UserTotp::UserId)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(UserTotp::Secret).text().not_null())
                    .col(ColumnDef::new(UserTotp::ConfirmedAt).timestamp_with_time_zone())
                    .col(ColumnDef::new(UserTotp::LastUsedStep).big_integer())
                    .col(
                        ColumnDef::new(UserTotp::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-user-totp-user")
                            .from(UserTotp::Table, UserTotp::UserId)
                            .to(User::Table, User::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_table(
                Table::create()
                    .table(UserRecoveryCode::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(UserRecoveryCode::CodeHash)
                            .string_len(64)
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(UserRecoveryCode::UserId).uuid().not_null())
                    .col(ColumnDef::new(UserRecoveryCode::UsedAt).timestamp_with_time_zone())
                    .col(
                        ColumnDef::new(UserRecoveryCode::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-user-recovery-code-user")
                            .from(UserRecoveryCode::Table, UserRecoveryCode::UserId)
                            .to(User::Table, User::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx-user-recovery-code-user")
                    .table(UserRecoveryCode::Table)
                    .col(UserRecoveryCode::UserId)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(UserRecoveryCode::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(UserTotp::Table).to_owned())
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
enum UserTotp {
    #[sea_orm(iden = "user_totp")]
    Table,
    UserId,
    Secret,
    ConfirmedAt,
    LastUsedStep,
    CreatedAt,
}

#[derive(DeriveIden)]
enum UserRecoveryCode {
    #[sea_orm(iden = "user_recovery_codes")]
    Table,
    CodeHash,
    UserId,
    UsedAt,
    CreatedAt,
}
