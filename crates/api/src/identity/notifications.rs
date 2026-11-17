//! Per-user email notification preferences.

use axum::{Json, extract::State, http::HeaderMap};
use axum_extra::extract::cookie::CookieJar;
use sea_orm::ConnectionTrait;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use super::{ApiError, IdentityState, SCOPE_READ, SCOPE_WRITE};

/// Which notification a message belongs to.
pub use gitadel_identity::notifications::NotificationKind;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
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
    Ok(domain::preferences(connection, user_id).await?.into())
}

#[derive(Serialize, ToSchema)]
pub struct NotificationSettingsResponse {
    email_enabled: bool,
    #[serde(flatten)]
    preferences: NotificationPreferences,
}

#[utoipa::path(
    get, path = "/me/notifications",
    operation_id = "get_me_notifications",
    summary = "Get email notification preferences",
    tag = "Account",
    responses(
        (status = 200, description = "Success", body = NotificationSettingsResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn get_notifications(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<NotificationSettingsResponse>, ApiError> {
    let actor = state
        .authenticate_principal(&headers, &jar, SCOPE_READ)
        .await?;
    Ok(Json(NotificationSettingsResponse {
        email_enabled: state.mailer().is_enabled(),
        preferences: preferences(state.database(), actor.user.id).await?,
    }))
}

#[utoipa::path(
    put, path = "/me/notifications",
    operation_id = "put_me_notifications",
    summary = "Update email notification preferences",
    tag = "Account",

    responses(
        (status = 200, description = "Success", body = NotificationSettingsResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn update_notifications(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<NotificationPreferences>,
) -> Result<Json<NotificationSettingsResponse>, ApiError> {
    let actor = state
        .authenticate_principal(&headers, &jar, SCOPE_WRITE)
        .await?;
    let preferences = domain::update(&state.account_service(), &actor, request.into()).await?;
    Ok(Json(NotificationSettingsResponse {
        email_enabled: state.mailer().is_enabled(),
        preferences: preferences.into(),
    }))
}

use gitadel_identity::notifications as domain;
impl From<domain::NotificationPreferences> for NotificationPreferences {
    fn from(v: domain::NotificationPreferences) -> Self {
        Self {
            issues: v.issues,
            issue_comments: v.issue_comments,
            action_failures: v.action_failures,
        }
    }
}
impl From<NotificationPreferences> for domain::NotificationPreferences {
    fn from(v: NotificationPreferences) -> Self {
        Self {
            issues: v.issues,
            issue_comments: v.issue_comments,
            action_failures: v.action_failures,
        }
    }
}
