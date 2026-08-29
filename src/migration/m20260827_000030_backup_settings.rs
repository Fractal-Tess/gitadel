use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(BackupSetting::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(BackupSetting::Id)
                            .integer()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(BackupSetting::Endpoint).string().not_null())
                    .col(ColumnDef::new(BackupSetting::Bucket).string().not_null())
                    .col(ColumnDef::new(BackupSetting::AccessKey).string().not_null())
                    .col(ColumnDef::new(BackupSetting::SecretKey).string().not_null())
                    .col(ColumnDef::new(BackupSetting::Region).string().not_null())
                    .col(ColumnDef::new(BackupSetting::Prefix).string().not_null())
                    .col(
                        ColumnDef::new(BackupSetting::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(BackupSetting::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum BackupSetting {
    #[sea_orm(iden = "backup_settings")]
    Table,
    Id,
    Endpoint,
    Bucket,
    AccessKey,
    SecretKey,
    Region,
    Prefix,
    UpdatedAt,
}
