use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(BackupSchedule::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(BackupSchedule::Id)
                            .integer()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(BackupSchedule::Schedule)
                            .string_len(255)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(BackupSchedule::NextBackupAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(BackupSchedule::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(BackupSchedule::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum BackupSchedule {
    #[sea_orm(iden = "backup_schedules")]
    Table,
    Id,
    Schedule,
    NextBackupAt,
    UpdatedAt,
}
