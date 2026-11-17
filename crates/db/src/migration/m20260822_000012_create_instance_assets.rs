use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(InstanceAsset::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(InstanceAsset::Name)
                            .string_len(64)
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(InstanceAsset::ContentType)
                            .string_len(128)
                            .not_null(),
                    )
                    .col(ColumnDef::new(InstanceAsset::Content).blob().not_null())
                    .col(
                        ColumnDef::new(InstanceAsset::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(InstanceAsset::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum InstanceAsset {
    #[sea_orm(iden = "instance_assets")]
    Table,
    Name,
    ContentType,
    Content,
    UpdatedAt,
}
