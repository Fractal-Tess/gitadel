//! Per-user email notification preferences.

use axum::{Json, extract::State, http::HeaderMap};
use axum_extra::extract::cookie::CookieJar;
use chrono::Utc;
use sea_orm::{ConnectionTrait, EntityTrait, Set, sea_query::OnConflict};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{ApiError, IdentityState, SCOPE_READ, SCOPE_WRITE};
use crate::entity::user_notification_preferences;

/// Which notification a message belongs to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum NotificationKind {
    Issues,
    IssueComments,
    ActionFailures,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
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
    pub(crate) const fn allows(self, kind: NotificationKind) -> bool {
        match kind {
            NotificationKind::Issues => self.issues,
            NotificationKind::IssueComments => self.issue_comments,
            NotificationKind::ActionFailures => self.action_failures,
        }
    }
}

pub(crate) async fn preferences<C: ConnectionTrait>(
    connection: &C,
    user_id: Uuid,
) -> Result<NotificationPreferences, ApiError> {
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

#[derive(Serialize)]
pub struct NotificationSettingsResponse {
    email_enabled: bool,
    #[serde(flatten)]
    preferences: NotificationPreferences,
}

pub async fn get_notifications(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<NotificationSettingsResponse>, ApiError> {
    let actor = state.authenticate(&headers, &jar, SCOPE_READ).await?;
    Ok(Json(NotificationSettingsResponse {
        email_enabled: state.mailer().is_enabled(),
        preferences: preferences(state.database(), actor.user.id).await?,
    }))
}

pub async fn update_notifications(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<NotificationPreferences>,
) -> Result<Json<NotificationSettingsResponse>, ApiError> {
    let actor = state.authenticate(&headers, &jar, SCOPE_WRITE).await?;
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
    Ok(Json(NotificationSettingsResponse {
        email_enabled: state.mailer().is_enabled(),
        preferences: preferences(state.database(), actor.user.id).await?,
    }))
}
