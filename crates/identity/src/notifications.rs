//! Notification preference persistence and account policy.
use crate::{AccountService, IdentityError, Principal};
use chrono::Utc;
use gitadel_db::entity::user_notification_preferences;
use sea_orm::{ConnectionTrait, EntityTrait, Set, sea_query::OnConflict};
use uuid::Uuid;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NotificationKind {
    Issues,
    IssueComments,
    ActionFailures,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NotificationPreferences {
    pub issues: bool,
    pub issue_comments: bool,
    pub action_failures: bool,
}
impl Default for NotificationPreferences {
    fn default() -> Self {
        Self {
            issues: true,
            issue_comments: true,
            action_failures: true,
        }
    }
}
impl NotificationPreferences {
    pub const fn allows(self, kind: NotificationKind) -> bool {
        match kind {
            NotificationKind::Issues => self.issues,
            NotificationKind::IssueComments => self.issue_comments,
            NotificationKind::ActionFailures => self.action_failures,
        }
    }
}
pub async fn preferences<C: ConnectionTrait>(
    connection: &C,
    user_id: Uuid,
) -> Result<NotificationPreferences, IdentityError> {
    Ok(user_notification_preferences::Entity::find_by_id(user_id)
        .one(connection)
        .await?
        .map_or_else(NotificationPreferences::default, |row| {
            NotificationPreferences {
                issues: row.issues,
                issue_comments: row.issue_comments,
                action_failures: row.action_failures,
            }
        }))
}
pub async fn update(
    state: &AccountService<'_>,
    actor: &Principal,
    request: NotificationPreferences,
) -> Result<NotificationPreferences, IdentityError> {
    let actor = state
        .require_actor(actor, false, crate::SCOPE_WRITE)
        .await?;
    user_notification_preferences::Entity::insert(user_notification_preferences::ActiveModel {
        user_id: Set(actor.user.id),
        issues: Set(request.issues),
        issue_comments: Set(request.issue_comments),
        action_failures: Set(request.action_failures),
        updated_at: Set(Utc::now()),
    })
    .on_conflict(
        OnConflict::column(user_notification_preferences::Column::UserId)
            .update_columns([
                user_notification_preferences::Column::Issues,
                user_notification_preferences::Column::IssueComments,
                user_notification_preferences::Column::ActionFailures,
                user_notification_preferences::Column::UpdatedAt,
            ])
            .to_owned(),
    )
    .exec(state.database())
    .await?;
    preferences(state.database(), actor.user.id).await
}
