//! Audit persistence usable within a caller's transaction.

use chrono::Utc;
use gitadel_core::audit::AuditContext;
use sea_orm::{ActiveModelTrait, ActiveValue::NotSet, ConnectionTrait, DbErr, Set};
use uuid::Uuid;

use crate::entity::audit_event;

/// Append an event without changing the caller's transaction boundary.
pub async fn record<C: ConnectionTrait>(
    connection: &C,
    actor_user_id: Option<Uuid>,
    action: impl Into<String>,
    target: Option<String>,
    context: AuditContext,
) -> Result<(), DbErr> {
    audit_event::ActiveModel {
        id: NotSet,
        actor_user_id: Set(actor_user_id),
        action: Set(action.into()),
        target: Set(target),
        remote_address: Set(context.remote_address.map(|address| address.to_string())),
        created_at: Set(Utc::now()),
    }
    .insert(connection)
    .await?;
    Ok(())
}
