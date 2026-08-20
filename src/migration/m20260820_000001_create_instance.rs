use sea_orm::ConnectionTrait;

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Instance::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Instance::Id)
                            .integer()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Instance::CreatedAt).timestamp().not_null())
                    .to_owned(),
            )
            .await?;

        let insert = Query::insert()
            .into_table(Instance::Table)
            .columns([Instance::Id, Instance::CreatedAt])
            .values([1.into(), Expr::current_timestamp()])
            .map_err(|error| DbErr::Custom(error.to_string()))?
            .on_conflict(OnConflict::column(Instance::Id).do_nothing().to_owned())
            .to_owned();

        manager.get_connection().execute(&insert).await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Instance::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum Instance {
    Table,
    Id,
    CreatedAt,
}
