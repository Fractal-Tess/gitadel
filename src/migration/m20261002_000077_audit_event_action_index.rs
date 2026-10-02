use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

/// Finding the latest event of one action (`WHERE action = ? ORDER BY id DESC
/// LIMIT 1`) walked the whole table, which only ever grows.
#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_index(
                Index::create()
                    .name("idx-audit-event-action-id")
                    .table(AuditEvent::Table)
                    .col(AuditEvent::Action)
                    .col(AuditEvent::Id)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_index(
                Index::drop()
                    .name("idx-audit-event-action-id")
                    .table(AuditEvent::Table)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum AuditEvent {
    #[sea_orm(iden = "audit_events")]
    Table,
    Id,
    Action,
}
