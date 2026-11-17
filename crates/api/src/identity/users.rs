//! Administrator management of accounts and pending invitations.
//!
//! Disabling is the reversible way to remove someone's access and keeps every
//! reference to the account intact. Deleting is permanent and is refused while
//! the account still anchors data that other people rely on: repositories in
//! its namespace, organizations it alone owns, or issues, comments, releases,
//! and attachments it authored. Repositories it created in other namespaces are
//! reattributed to the deleting administrator.

use axum::{
    Json,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
};
use axum_extra::extract::cookie::CookieJar;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use super::{ApiError, IdentityState, SCOPE_READ, SCOPE_WRITE, admin::require_admin_principal};

#[derive(Serialize, utoipa::ToSchema)]
pub struct AdminUserResponse {
    id: Uuid,
    username: String,
    is_admin: bool,
    disabled_at: Option<chrono::DateTime<Utc>>,
    avatar_updated_at: Option<chrono::DateTime<Utc>>,
    two_factor_enabled: bool,
    created_at: chrono::DateTime<Utc>,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct AdminUserPage {
    users: Vec<AdminUserResponse>,
    total: u64,
}

#[derive(Deserialize, utoipa::ToSchema, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct UserListQuery {
    #[serde(default)]
    q: String,
    limit: Option<u64>,
    offset: Option<u64>,
}

#[utoipa::path(
    get, path = "/admin/users",
    operation_id = "get_admin_users",
    summary = "List users",
    tag = "Administration",
    params(
        UserListQuery,
    ),
    responses(
        (status = 200, description = "Success", body = AdminUserPage, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn list_users(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Query(query): Query<UserListQuery>,
) -> Result<Json<AdminUserPage>, ApiError> {
    let actor = require_admin_principal(&state, &headers, &jar, SCOPE_READ).await?;
    Ok(Json(
        domain::list_users(
            &state.account_service(),
            &actor,
            UserListQueryDomain {
                q: query.q,
                limit: query.limit,
                offset: query.offset,
            },
        )
        .await?
        .into(),
    ))
}

#[utoipa::path(
    post, path = "/admin/users/{username}/disable",
    operation_id = "post_admin_users_username_disable",
    summary = "Disable user",
    tag = "Administration",
    params(
        ("username" = String, Path),
    ),
    responses(
        (status = 200, description = "Success", body = AdminUserResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn disable_user(
    State(state): State<IdentityState>,
    Path(username): Path<String>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<AdminUserResponse>, ApiError> {
    let actor = require_admin_principal(&state, &headers, &jar, SCOPE_WRITE).await?;
    Ok(Json(
        domain::disable_user(&state.account_service(), &actor, username)
            .await?
            .into(),
    ))
}

#[utoipa::path(
    post, path = "/admin/users/{username}/enable",
    operation_id = "post_admin_users_username_enable",
    summary = "Enable user",
    tag = "Administration",
    params(
        ("username" = String, Path),
    ),
    responses(
        (status = 200, description = "Success", body = AdminUserResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn enable_user(
    State(state): State<IdentityState>,
    Path(username): Path<String>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<AdminUserResponse>, ApiError> {
    let actor = require_admin_principal(&state, &headers, &jar, SCOPE_WRITE).await?;
    Ok(Json(
        domain::enable_user(&state.account_service(), &actor, username)
            .await?
            .into(),
    ))
}

#[utoipa::path(
    delete, path = "/admin/users/{username}",
    operation_id = "delete_admin_users_username",
    summary = "Delete user",
    tag = "Administration",
    params(
        ("username" = String, Path),
    ),
    responses(
        (status = 204, description = "Success"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn delete_user(
    State(state): State<IdentityState>,
    Path(username): Path<String>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<StatusCode, ApiError> {
    let actor = require_admin_principal(&state, &headers, &jar, SCOPE_WRITE).await?;
    domain::delete_user(&state.account_service(), &actor, username).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Lets an administrator recover an account whose authenticator and recovery
/// codes are both lost. The user can enroll again after signing in.
#[utoipa::path(
    delete, path = "/admin/users/{username}/two-factor",
    operation_id = "delete_admin_users_username_two_factor",
    summary = "Reset two factor",
    tag = "Administration",
    params(
        ("username" = String, Path),
    ),
    responses(
        (status = 204, description = "Success"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn reset_two_factor(
    State(state): State<IdentityState>,
    Path(username): Path<String>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<StatusCode, ApiError> {
    let actor = require_admin_principal(&state, &headers, &jar, SCOPE_WRITE).await?;
    domain::reset_two_factor(&state.account_service(), &actor, username).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize, ToSchema)]
pub struct PendingInvitationResponse {
    /// The stored token hash. It identifies the invitation without revealing
    /// the token that redeems it.
    id: String,
    created_by: Option<String>,
    created_at: chrono::DateTime<Utc>,
    expires_at: chrono::DateTime<Utc>,
}

#[utoipa::path(
    get, path = "/invitations",
    operation_id = "get_invitations",
    summary = "List pending invitations",
    tag = "Account",
    responses(
        (status = 200, description = "Success", body = Vec<PendingInvitationResponse>, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn list_invitations(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<Vec<PendingInvitationResponse>>, ApiError> {
    let actor = require_admin_principal(&state, &headers, &jar, SCOPE_READ).await?;
    Ok(Json(
        domain::list_invitations(&state.account_service(), &actor)
            .await?
            .into_iter()
            .map(Into::into)
            .collect(),
    ))
}

#[utoipa::path(
    delete, path = "/invitations/{id}",
    operation_id = "delete_invitations_id",
    summary = "Revoke an invitation",
    tag = "Account",
    params(
        ("id" = String, Path),
    ),
    responses(
        (status = 204, description = "Success"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn revoke_invitation(
    State(state): State<IdentityState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<StatusCode, ApiError> {
    let actor = require_admin_principal(&state, &headers, &jar, SCOPE_WRITE).await?;
    domain::revoke_invitation(&state.account_service(), &actor, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Deletes an organization that no longer owns repositories. Its namespace,
/// memberships, avatar, integrations, mirror credentials, and runners cascade.
use gitadel_identity::users as domain;
impl From<domain::AdminUserResponse> for AdminUserResponse {
    fn from(value: domain::AdminUserResponse) -> Self {
        Self {
            id: value.id,
            username: value.username,
            is_admin: value.is_admin,
            disabled_at: value.disabled_at,
            avatar_updated_at: value.avatar_updated_at,
            two_factor_enabled: value.two_factor_enabled,
            created_at: value.created_at,
        }
    }
}
impl From<domain::AdminUserPage> for AdminUserPage {
    fn from(value: domain::AdminUserPage) -> Self {
        Self {
            users: value.users.into_iter().map(Into::into).collect(),
            total: value.total,
        }
    }
}
use domain::UserListQuery as UserListQueryDomain;

impl From<domain::PendingInvitationResponse> for PendingInvitationResponse {
    fn from(value: domain::PendingInvitationResponse) -> Self {
        Self {
            id: value.id,
            created_by: value.created_by,
            created_at: value.created_at,
            expires_at: value.expires_at,
        }
    }
}
