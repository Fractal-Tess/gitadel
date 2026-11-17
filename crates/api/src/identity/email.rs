//! Email endpoints: SMTP status and test delivery, account addresses with
//! emailed verification, and password reset by email.
//!
//! Every endpoint here degrades to "not available" when SMTP is not
//! configured. Messages are queued; only the administrator's test waits for
//! the relay.

use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
};
use axum_extra::extract::cookie::CookieJar;
#[cfg(test)]
use chrono::Duration;
use lettre::message::Mailbox;
use sea_orm::ConnectionTrait;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use super::{ApiError, IdentityState, SCOPE_READ, SCOPE_WRITE, require_admin_principal};
use crate::entity::user;

#[cfg(test)]
const PURPOSE_VERIFY: &str = "verify";
#[cfg(test)]
const PURPOSE_RESET: &str = "reset";
/// Verifies a commit address; see [`super::commit_emails`].
#[cfg(test)]
pub(super) const PURPOSE_COMMIT_VERIFY: &str = "verify-commit";
#[cfg(test)]
pub(super) const VERIFY_LIFETIME: Duration = Duration::hours(24);
#[cfg(test)]
const RESET_LIFETIME: Duration = Duration::hours(1);

// ---------------------------------------------------------------------------
// Administration

#[derive(Serialize, utoipa::ToSchema)]
pub struct SmtpStatusResponse {
    configured: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    host: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    tls: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    username: Option<Option<String>>,
}

#[utoipa::path(
    get, path = "/admin/smtp",
    operation_id = "get_admin_smtp",
    summary = "Smtp status",
    tag = "Administration",
    responses(
        (status = 200, description = "Success", body = SmtpStatusResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn smtp_status(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<SmtpStatusResponse>, ApiError> {
    let actor = require_admin_principal(&state, &headers, &jar, SCOPE_READ).await?;
    Ok(Json(
        domain::smtp_status(&state.auth_service(), &actor)
            .await?
            .into(),
    ))
}

#[derive(Default, Deserialize, utoipa::ToSchema)]
pub struct TestEmailRequest {
    to: Option<String>,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct TestEmailResponse {
    delivered_to: String,
}

/// Send one message synchronously so the administrator sees the relay's
/// answer. This is the only email path that waits for delivery.
#[utoipa::path(
    post, path = "/admin/smtp/test",
    operation_id = "post_admin_smtp_test",
    summary = "Send test email",
    tag = "Administration",
    request_body = Option<TestEmailRequest>,
    responses(
        (status = 200, description = "Success", body = TestEmailResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn send_test_email(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    request: Option<Json<TestEmailRequest>>,
) -> Result<Json<TestEmailResponse>, ApiError> {
    let actor = require_admin_principal(&state, &headers, &jar, SCOPE_WRITE).await?;
    let Json(request) = request.unwrap_or_default();
    Ok(Json(
        domain::send_test_email(
            &state.auth_service(),
            &actor,
            domain::TestEmailRequest { to: request.to },
        )
        .await?
        .into(),
    ))
}

// ---------------------------------------------------------------------------
// Account address

/// Mailbox for an enabled user with a verified address, for notifications.
pub(crate) async fn verified_mailbox<C: ConnectionTrait>(
    connection: &C,
    user_id: Uuid,
) -> Result<Option<(user::Model, Mailbox)>, ApiError> {
    Ok(domain::verified_mailbox(connection, user_id).await?)
}

#[derive(Serialize, ToSchema)]
pub struct EmailStatusResponse {
    /// Whether the instance can send mail at all.
    email_enabled: bool,
    email: Option<String>,
    verified: bool,
}

#[utoipa::path(
    get, path = "/me/email",
    operation_id = "get_me_email",
    summary = "Get the account email address",
    tag = "Account",
    responses(
        (status = 200, description = "Success", body = EmailStatusResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn get_email(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<EmailStatusResponse>, ApiError> {
    let actor = state
        .authenticate_principal(&headers, &jar, SCOPE_READ)
        .await?;
    Ok(Json(
        domain::get_email(&state.auth_service(), &actor)
            .await?
            .into(),
    ))
}

#[derive(Deserialize, ToSchema)]
pub struct SetEmailRequest {
    email: String,
    /// Required when the request uses an API token, because a verified
    /// address can reset the password.
    current_password: Option<String>,
}

#[utoipa::path(
    put, path = "/me/email",
    operation_id = "put_me_email",
    summary = "Set the account email address",
    tag = "Account",

    responses(
        (status = 200, description = "Success", body = EmailStatusResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn set_email(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<SetEmailRequest>,
) -> Result<Json<EmailStatusResponse>, ApiError> {
    let actor = state
        .authenticate_principal(&headers, &jar, SCOPE_WRITE)
        .await?;
    Ok(Json(
        domain::set_email(
            &state.auth_service(),
            &actor,
            domain::SetEmailRequest {
                email: request.email,
                current_password: request.current_password,
            },
        )
        .await?
        .into(),
    ))
}

#[utoipa::path(
    post, path = "/me/email/verification",
    operation_id = "post_me_email_verification",
    summary = "Resend the account email verification",
    tag = "Account",
    responses(
        (status = 202, description = "Success"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn resend_verification(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<StatusCode, ApiError> {
    let actor = state
        .authenticate_principal(&headers, &jar, SCOPE_WRITE)
        .await?;
    domain::resend_verification(&state.auth_service(), &actor).await?;
    Ok(StatusCode::ACCEPTED)
}

#[utoipa::path(
    delete, path = "/me/email",
    operation_id = "delete_me_email",
    summary = "Remove the account email address",
    tag = "Account",
    responses(
        (status = 204, description = "Success"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn delete_email(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<StatusCode, ApiError> {
    let actor = state
        .authenticate_principal(&headers, &jar, SCOPE_WRITE)
        .await?;
    domain::delete_email(&state.auth_service(), &actor).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct TokenRequest {
    token: String,
}

/// Confirm an address from the emailed link. The token alone authorizes it.
///
/// Account addresses and commit addresses share the link format and this
/// endpoint; the token's purpose tells them apart.
#[utoipa::path(
    post, path = "/auth/email/verify",
    operation_id = "post_auth_email_verify",
    summary = "Verify email",
    tag = "Authentication",

    security(),
    responses(
        (status = 204, description = "Success"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn verify_email(
    State(state): State<IdentityState>,
    Json(request): Json<TokenRequest>,
) -> Result<StatusCode, ApiError> {
    domain::verify_email(
        &state.auth_service(),
        domain::TokenRequest {
            token: request.token,
        },
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// Password reset

#[derive(Deserialize, utoipa::ToSchema)]
pub struct PasswordResetRequest {
    /// Username or verified email address.
    login: String,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct PasswordResetAccepted {
    message: &'static str,
}

/// Start a reset. The response never reveals whether an account matched.
#[utoipa::path(
    post, path = "/auth/password-reset",
    operation_id = "post_auth_password_reset",
    summary = "Request password reset",
    tag = "Authentication",

    security(),
    responses(
        (status = 202, description = "Success", body = PasswordResetAccepted, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn request_password_reset(
    State(state): State<IdentityState>,
    Json(request): Json<PasswordResetRequest>,
) -> Result<(StatusCode, Json<PasswordResetAccepted>), ApiError> {
    Ok((
        StatusCode::ACCEPTED,
        Json(
            domain::request_password_reset(
                &state.auth_service(),
                domain::PasswordResetRequest {
                    login: request.login,
                },
            )
            .await?
            .into(),
        ),
    ))
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct PasswordResetConfirmRequest {
    token: String,
    password: String,
}

#[utoipa::path(
    post, path = "/auth/password-reset/confirm",
    operation_id = "post_auth_password_reset_confirm",
    summary = "Confirm password reset",
    tag = "Authentication",

    security(),
    responses(
        (status = 204, description = "Success"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn confirm_password_reset(
    State(state): State<IdentityState>,
    Json(request): Json<PasswordResetConfirmRequest>,
) -> Result<StatusCode, ApiError> {
    domain::confirm_password_reset(
        &state.auth_service(),
        domain::PasswordResetConfirmRequest {
            token: request.token,
            password: request.password,
        },
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// Tokens

#[cfg(test)]
pub(super) async fn issue_token<C: ConnectionTrait>(
    connection: &C,
    user_id: Uuid,
    purpose: &str,
    email: Option<&str>,
    lifetime: Duration,
) -> Result<String, ApiError> {
    Ok(gitadel_identity::email::issue_token(connection, user_id, purpose, email, lifetime).await?)
}

/// Atomically mark an unexpired, unused token as used and return it.
#[cfg(test)]
pub(super) async fn consume_token<C: ConnectionTrait>(
    connection: &C,
    token_hash: &str,
    purpose: &str,
) -> Result<Option<email_token::Model>, ApiError> {
    Ok(gitadel_identity::email::consume_token(connection, token_hash, purpose).await?)
}

use gitadel_identity::email as domain;

impl From<domain::SmtpStatusResponse> for SmtpStatusResponse {
    fn from(value: domain::SmtpStatusResponse) -> Self {
        Self {
            configured: value.configured,
            host: value.host,
            port: value.port,
            tls: value.tls,
            from: value.from,
            username: value.username,
        }
    }
}
impl From<domain::TestEmailResponse> for TestEmailResponse {
    fn from(value: domain::TestEmailResponse) -> Self {
        Self {
            delivered_to: value.delivered_to,
        }
    }
}
impl From<domain::EmailStatusResponse> for EmailStatusResponse {
    fn from(value: domain::EmailStatusResponse) -> Self {
        Self {
            email_enabled: value.email_enabled,
            email: value.email,
            verified: value.verified,
        }
    }
}
impl From<domain::PasswordResetAccepted> for PasswordResetAccepted {
    fn from(value: domain::PasswordResetAccepted) -> Self {
        Self {
            message: value.message,
        }
    }
}

#[cfg(test)]
use super::hash_secret;
#[cfg(test)]
use crate::entity::email_token;
#[cfg(test)]
use crate::entity::user_email;
#[cfg(test)]
use chrono::Utc;
#[cfg(test)]
use sea_orm::ActiveModelTrait;
#[cfg(test)]
use sea_orm::EntityTrait;
#[cfg(test)]
use sea_orm::Set;

#[cfg(test)]
mod tests {

    use super::*;

    use gitadel_db::test_support::database;

    #[tokio::test]
    async fn tokens_are_single_use_and_purpose_bound() {
        let database = database().await;
        let owner = crate::identity::bootstrap_admin(
            &database,
            "token-owner",
            "token-password!".to_owned(),
        )
        .await
        .unwrap();
        let token = issue_token(&database, owner.id, PURPOSE_RESET, None, RESET_LIFETIME)
            .await
            .unwrap();
        let hash = hash_secret(&token);

        assert!(
            consume_token(&database, &hash, PURPOSE_VERIFY)
                .await
                .unwrap()
                .is_none(),
            "a reset token must not verify an address"
        );
        let consumed = consume_token(&database, &hash, PURPOSE_RESET)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(consumed.user_id, owner.id);
        assert!(
            consume_token(&database, &hash, PURPOSE_RESET)
                .await
                .unwrap()
                .is_none(),
            "tokens are single use"
        );
        let stored = email_token::Entity::find_by_id(&hash)
            .one(&database)
            .await
            .unwrap()
            .unwrap();
        assert_ne!(stored.token_hash, token, "only the hash is stored");
    }

    #[tokio::test]
    async fn expired_tokens_are_rejected() {
        let database = database().await;
        let owner = crate::identity::bootstrap_admin(
            &database,
            "expired-owner",
            "token-password!".to_owned(),
        )
        .await
        .unwrap();
        let token = issue_token(
            &database,
            owner.id,
            PURPOSE_RESET,
            None,
            Duration::seconds(-1),
        )
        .await
        .unwrap();
        assert!(
            consume_token(&database, &hash_secret(&token), PURPOSE_RESET)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn only_verified_addresses_receive_mail() {
        let database = database().await;
        let owner =
            crate::identity::bootstrap_admin(&database, "mail-owner", "token-password!".to_owned())
                .await
                .unwrap();
        let now = Utc::now();
        user_email::ActiveModel {
            user_id: Set(owner.id),
            email: Set("owner@example.com".to_owned()),
            verified_at: Set(None),
            created_at: Set(now),
            updated_at: Set(now),
        }
        .insert(&database)
        .await
        .unwrap();
        assert!(
            verified_mailbox(&database, owner.id)
                .await
                .unwrap()
                .is_none()
        );

        let mut row: user_email::ActiveModel = user_email::Entity::find_by_id(owner.id)
            .one(&database)
            .await
            .unwrap()
            .unwrap()
            .into();
        row.verified_at = Set(Some(now));
        row.update(&database).await.unwrap();
        let (_, mailbox) = verified_mailbox(&database, owner.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(mailbox.email.to_string(), "owner@example.com");
        assert_eq!(mailbox.name.as_deref(), Some("mail-owner"));
    }
}
