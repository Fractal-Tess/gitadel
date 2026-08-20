use sea_orm::ConnectionTrait;
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
                        ColumnDef::new(Instance::SiteName)
                            .string()
                            .not_null()
                            .default("Gitadel"),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(Instance::Table)
                    .add_column(ColumnDef::new(Instance::SiteDescription).string().null())
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(Instance::Table)
                    .add_column(
                        ColumnDef::new(Instance::DefaultRepositoryVisibility)
                            .string()
                            .not_null()
                            .default("private"),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(Instance::Table)
                    .add_column(
                        ColumnDef::new(Instance::UpdatedAt)
                            .timestamp()
                            .not_null()
                            .default("1970-01-01 00:00:00"),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .get_connection()
            .execute_unprepared("UPDATE instance SET updated_at = CURRENT_TIMESTAMP")
            .await
            .map(|_| ())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for column in [
            Instance::UpdatedAt,
            Instance::DefaultRepositoryVisibility,
            Instance::SiteDescription,
            Instance::SiteName,
        ] {
            manager
                .alter_table(
                    Table::alter()
                        .table(Instance::Table)
                        .drop_column(column)
                        .to_owned(),
                )
                .await?;
        }
        Ok(())
    }
}

#[derive(DeriveIden)]
enum Instance {
    Table,
    SiteName,
    SiteDescription,
    DefaultRepositoryVisibility,
    UpdatedAt,
}
