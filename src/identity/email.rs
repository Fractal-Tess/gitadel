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
use chrono::{Duration, Utc};
use lettre::message::Mailbox;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, Set,
    TransactionTrait, sea_query::Expr,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{
    ApiError, IdentityState, SCOPE_READ, SCOPE_WRITE, hash_password, hash_secret, random_secret,
    require_admin, validate_password, validate_slug, verify_password,
};
use crate::{
    entity::{email_token, session, user, user_email},
    mail::{Email, SmtpSummary, parse_address},
};

const PURPOSE_VERIFY: &str = "verify";
const PURPOSE_RESET: &str = "reset";
const VERIFY_LIFETIME: Duration = Duration::hours(24);
const RESET_LIFETIME: Duration = Duration::hours(1);
const RESET_ACCEPTED: &str =
    "If that account has a verified email address, a reset link is on its way.";

fn email_unavailable() -> ApiError {
    ApiError::conflict("Email is not configured on this Gitadel instance.")
}

fn require_mail(state: &IdentityState) -> Result<(), ApiError> {
    if state.mailer().is_enabled() {
        Ok(())
    } else {
        Err(email_unavailable())
    }
}

// ---------------------------------------------------------------------------
// Administration

#[derive(Serialize)]
pub struct SmtpStatusResponse {
    configured: bool,
    #[serde(flatten)]
    summary: Option<SmtpSummary>,
}

pub async fn smtp_status(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<SmtpStatusResponse>, ApiError> {
    require_admin(&state, &headers, &jar, SCOPE_READ).await?;
    Ok(Json(SmtpStatusResponse {
        configured: state.mailer().is_enabled(),
        summary: state.mailer().summary().cloned(),
    }))
}

#[derive(Default, Deserialize)]
pub struct TestEmailRequest {
    to: Option<String>,
}

#[derive(Serialize)]
pub struct TestEmailResponse {
    delivered_to: String,
}

/// Send one message synchronously so the administrator sees the relay's
/// answer. This is the only email path that waits for delivery.
pub async fn send_test_email(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    request: Option<Json<TestEmailRequest>>,
) -> Result<Json<TestEmailResponse>, ApiError> {
    let actor = require_admin(&state, &headers, &jar, SCOPE_WRITE).await?;
    if !state.mailer().is_enabled() {
        return Err(ApiError::conflict(
            "SMTP is not configured. Add an [smtp] section to the server configuration.",
        ));
    }
    state.enforce_auth_rate_limit("smtp-test", 5).await?;
    let Json(request) = request.unwrap_or_default();
    let requested = request
        .to
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    let recipient = match requested {
        Some(value) => value,
        None => verified_email(state.database(), actor.user.id)
            .await?
            .ok_or_else(|| {
                ApiError::bad_request(
                    "Enter a recipient, or verify an email address on your account.",
                )
            })?,
    };
    let address = parse_address(&recipient)
        .ok_or_else(|| ApiError::bad_request("Enter a valid email address."))?;
    state
        .audit(
            Some(actor.user.id),
            "admin.smtp.test",
            Some(address.to_string()),
        )
        .await?;
    state
        .mailer()
        .send_now(Email {
            to: address.clone().into(),
            subject: "Gitadel test email".to_owned(),
            body: format!(
                "This is a test message from the Gitadel instance at {}.\n\n\
                 Outgoing email is working.\n",
                state.public_url()
            ),
        })
        .await
        .map_err(|error| ApiError::bad_request(format!("The test email failed: {error}")))?;
    Ok(Json(TestEmailResponse {
        delivered_to: address.to_string(),
    }))
}

// ---------------------------------------------------------------------------
// Account address

/// The user's verified address, if any.
pub(crate) async fn verified_email<C: ConnectionTrait>(
    connection: &C,
    user_id: Uuid,
) -> Result<Option<String>, ApiError> {
    Ok(user_email::Entity::find_by_id(user_id)
        .filter(user_email::Column::VerifiedAt.is_not_null())
        .one(connection)
        .await?
        .map(|row| row.email))
}

/// Mailbox for an enabled user with a verified address, for notifications.
pub(crate) async fn verified_mailbox<C: ConnectionTrait>(
    connection: &C,
    user_id: Uuid,
) -> Result<Option<(user::Model, Mailbox)>, ApiError> {
    let Some(account) = user::Entity::find_by_id(user_id)
        .filter(user::Column::DisabledAt.is_null())
        .one(connection)
        .await?
    else {
        return Ok(None);
    };
    let Some(address) = verified_email(connection, user_id)
        .await?
        .and_then(|email| parse_address(&email))
    else {
        return Ok(None);
    };
    let mailbox = Mailbox::new(Some(account.username.clone()), address);
    Ok(Some((account, mailbox)))
}

#[derive(Serialize)]
pub struct EmailStatusResponse {
    /// Whether the instance can send mail at all.
    email_enabled: bool,
    email: Option<String>,
    verified: bool,
}

async fn email_status(
    state: &IdentityState,
    user_id: Uuid,
) -> Result<EmailStatusResponse, ApiError> {
    let row = user_email::Entity::find_by_id(user_id)
        .one(state.database())
        .await?;
    Ok(EmailStatusResponse {
        email_enabled: state.mailer().is_enabled(),
        verified: row.as_ref().is_some_and(|row| row.verified_at.is_some()),
        email: row.map(|row| row.email),
    })
}

pub async fn get_email(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<EmailStatusResponse>, ApiError> {
    let actor = state.authenticate(&headers, &jar, SCOPE_READ).await?;
    Ok(Json(email_status(&state, actor.user.id).await?))
}

#[derive(Deserialize)]
pub struct SetEmailRequest {
    email: String,
    /// Required when the request uses an API token, because a verified
    /// address can reset the password.
    current_password: Option<String>,
}

pub async fn set_email(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<SetEmailRequest>,
) -> Result<Json<EmailStatusResponse>, ApiError> {
    let actor = state.authenticate(&headers, &jar, SCOPE_WRITE).await?;
    require_mail(&state)?;
    if actor.via_api_token {
        let password = request.current_password.ok_or_else(|| {
            ApiError::bad_request("Changing the email address with a token needs your password.")
        })?;
        state
            .enforce_account_auth_rate_limit(&actor.user.username, 8)
            .await?;
        if !verify_password(password, actor.user.password_hash.clone()).await? {
            return Err(ApiError::bad_request("The current password is incorrect."));
        }
    }
    state
        .enforce_rate_limit(format!("email-change:{}", actor.user.id), 5)
        .await?;
    let address = parse_address(&request.email)
        .ok_or_else(|| ApiError::bad_request("Enter a valid email address."))?
        .to_string();

    let now = Utc::now();
    let transaction = state.database().begin().await?;
    let existing = user_email::Entity::find()
        .filter(user_email::Column::Email.eq(&address))
        .one(&transaction)
        .await?;
    match existing {
        Some(row) if row.user_id == actor.user.id && row.verified_at.is_some() => {
            transaction.commit().await?;
            return Ok(Json(email_status(&state, actor.user.id).await?));
        }
        Some(row) if row.user_id != actor.user.id => {
            if row.verified_at.is_some() {
                return Err(ApiError::conflict(
                    "That email address belongs to another account.",
                ));
            }
            // An unverified claim does not reserve an address.
            user_email::Entity::delete_by_id(row.user_id)
                .exec(&transaction)
                .await?;
        }
        _ => {}
    }
    user_email::Entity::delete_by_id(actor.user.id)
        .exec(&transaction)
        .await?;
    user_email::ActiveModel {
        user_id: Set(actor.user.id),
        email: Set(address.clone()),
        verified_at: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(&transaction)
    .await?;
    let token = issue_token(
        &transaction,
        actor.user.id,
        PURPOSE_VERIFY,
        Some(&address),
        VERIFY_LIFETIME,
    )
    .await?;
    state
        .audit_on(
            &transaction,
            Some(actor.user.id),
            "account.email.update",
            Some(address.clone()),
        )
        .await?;
    transaction.commit().await?;
    send_verification(&state, &actor.user, &address, &token);
    Ok(Json(email_status(&state, actor.user.id).await?))
}

pub async fn resend_verification(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<StatusCode, ApiError> {
    let actor = state.authenticate(&headers, &jar, SCOPE_WRITE).await?;
    require_mail(&state)?;
    state
        .enforce_rate_limit(format!("email-change:{}", actor.user.id), 5)
        .await?;
    let row = user_email::Entity::find_by_id(actor.user.id)
        .one(state.database())
        .await?
        .ok_or_else(|| ApiError::bad_request("Add an email address first."))?;
    if row.verified_at.is_some() {
        return Err(ApiError::conflict(
            "This email address is already verified.",
        ));
    }
    let transaction = state.database().begin().await?;
    let token = issue_token(
        &transaction,
        actor.user.id,
        PURPOSE_VERIFY,
        Some(&row.email),
        VERIFY_LIFETIME,
    )
    .await?;
    transaction.commit().await?;
    send_verification(&state, &actor.user, &row.email, &token);
    Ok(StatusCode::ACCEPTED)
}

pub async fn delete_email(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<StatusCode, ApiError> {
    let actor = state.authenticate(&headers, &jar, SCOPE_WRITE).await?;
    if actor.via_api_token {
        return Err(ApiError::forbidden(
            "Remove the email address from a browser session.",
        ));
    }
    let transaction = state.database().begin().await?;
    user_email::Entity::delete_by_id(actor.user.id)
        .exec(&transaction)
        .await?;
    email_token::Entity::delete_many()
        .filter(email_token::Column::UserId.eq(actor.user.id))
        .exec(&transaction)
        .await?;
    state
        .audit_on(
            &transaction,
            Some(actor.user.id),
            "account.email.delete",
            None,
        )
        .await?;
    transaction.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub struct TokenRequest {
    token: String,
}

/// Confirm an address from the emailed link. The token alone authorizes it.
pub async fn verify_email(
    State(state): State<IdentityState>,
    Json(request): Json<TokenRequest>,
) -> Result<StatusCode, ApiError> {
    state.enforce_auth_rate_limit("email-verify", 20).await?;
    let invalid = || ApiError::bad_request("The verification link is invalid or has expired.");
    let token_hash = hash_secret(request.token.trim());
    let now = Utc::now();
    let transaction = state.database().begin().await?;
    let token = consume_token(&transaction, &token_hash, PURPOSE_VERIFY)
        .await?
        .ok_or_else(invalid)?;
    let row = user_email::Entity::find_by_id(token.user_id)
        .one(&transaction)
        .await?
        .filter(|row| Some(&row.email) == token.email.as_ref())
        .ok_or_else(invalid)?;
    let email = row.email.clone();
    let mut active: user_email::ActiveModel = row.into();
    active.verified_at = Set(Some(now));
    active.updated_at = Set(now);
    active.update(&transaction).await?;
    state
        .audit_on(
            &transaction,
            Some(token.user_id),
            "account.email.verify",
            Some(email),
        )
        .await?;
    transaction.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// Password reset

#[derive(Deserialize)]
pub struct PasswordResetRequest {
    /// Username or verified email address.
    login: String,
}

#[derive(Serialize)]
pub struct PasswordResetAccepted {
    message: &'static str,
}

/// Start a reset. The response never reveals whether an account matched.
pub async fn request_password_reset(
    State(state): State<IdentityState>,
    Json(request): Json<PasswordResetRequest>,
) -> Result<(StatusCode, Json<PasswordResetAccepted>), ApiError> {
    require_reset_available(&state).await?;
    state.enforce_auth_rate_limit("password-reset", 5).await?;
    let login = request.login.trim().to_ascii_lowercase();
    if login.is_empty() || login.len() > 254 {
        return Err(ApiError::bad_request("Enter a username or email address."));
    }
    state
        .enforce_rate_limit(format!("password-reset:{login}"), 3)
        .await?;
    if let Some((account, mailbox)) = reset_recipient(&state, &login).await? {
        let transaction = state.database().begin().await?;
        email_token::Entity::delete_many()
            .filter(email_token::Column::UserId.eq(account.id))
            .filter(email_token::Column::Purpose.eq(PURPOSE_RESET))
            .exec(&transaction)
            .await?;
        let token = issue_token(
            &transaction,
            account.id,
            PURPOSE_RESET,
            None,
            RESET_LIFETIME,
        )
        .await?;
        state
            .audit_on(
                &transaction,
                Some(account.id),
                "auth.password_reset.request",
                None,
            )
            .await?;
        transaction.commit().await?;
        let link = link(&state, "/reset-password", &token);
        state.mailer().enqueue(Email {
            to: mailbox,
            subject: "Reset your Gitadel password".to_owned(),
            body: format!(
                "Someone asked to reset the password for the Gitadel account \
                 \"{username}\" at {origin}.\n\n\
                 Choose a new password here within the next hour:\n\n{link}\n\n\
                 If you did not ask for this, ignore this email; your password \
                 stays the same.\n",
                username = account.username,
                origin = state.public_url(),
            ),
        });
    }
    Ok((
        StatusCode::ACCEPTED,
        Json(PasswordResetAccepted {
            message: RESET_ACCEPTED,
        }),
    ))
}

#[derive(Deserialize)]
pub struct PasswordResetConfirmRequest {
    token: String,
    password: String,
}

pub async fn confirm_password_reset(
    State(state): State<IdentityState>,
    Json(request): Json<PasswordResetConfirmRequest>,
) -> Result<StatusCode, ApiError> {
    require_reset_available(&state).await?;
    state
        .enforce_auth_rate_limit("password-reset-confirm", 10)
        .await?;
    let invalid = || ApiError::bad_request("The reset link is invalid or has expired.");
    validate_password(&request.password)?;
    let token_hash = hash_secret(request.token.trim());
    // Check before the expensive hash; the transaction re-checks atomically.
    let pending = email_token::Entity::find_by_id(&token_hash)
        .filter(email_token::Column::Purpose.eq(PURPOSE_RESET))
        .filter(email_token::Column::UsedAt.is_null())
        .filter(email_token::Column::ExpiresAt.gt(Utc::now()))
        .one(state.database())
        .await?
        .ok_or_else(invalid)?;
    let password_hash = hash_password(request.password).await?;

    let transaction = state.database().begin().await?;
    let token = consume_token(&transaction, &token_hash, PURPOSE_RESET)
        .await?
        .filter(|token| token.user_id == pending.user_id)
        .ok_or_else(invalid)?;
    let account = user::Entity::find_by_id(token.user_id)
        .filter(user::Column::DisabledAt.is_null())
        .one(&transaction)
        .await?
        .ok_or_else(invalid)?;
    let mut active: user::ActiveModel = account.clone().into();
    active.password_hash = Set(password_hash);
    active.updated_at = Set(Utc::now());
    active.update(&transaction).await?;
    session::Entity::delete_many()
        .filter(session::Column::UserId.eq(account.id))
        .exec(&transaction)
        .await?;
    email_token::Entity::delete_many()
        .filter(email_token::Column::UserId.eq(account.id))
        .filter(email_token::Column::Purpose.eq(PURPOSE_RESET))
        .exec(&transaction)
        .await?;
    state
        .audit_on(
            &transaction,
            Some(account.id),
            "auth.password_reset.complete",
            None,
        )
        .await?;
    transaction.commit().await?;
    if let Some((_, mailbox)) = verified_mailbox(state.database(), account.id).await? {
        state.mailer().enqueue(Email {
            to: mailbox,
            subject: "Your Gitadel password was changed".to_owned(),
            body: format!(
                "The password for the Gitadel account \"{}\" at {} was reset \
                 and all sessions were signed out.\n\n\
                 If you did not do this, contact your administrator.\n",
                account.username,
                state.public_url(),
            ),
        });
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn require_reset_available(state: &IdentityState) -> Result<(), ApiError> {
    if !state.mailer().is_enabled() {
        return Err(ApiError::conflict(
            "Password reset by email is not available on this instance.",
        ));
    }
    if !super::sso::public_configuration(state.database())
        .await?
        .password_enabled
    {
        return Err(ApiError::forbidden("Password login is disabled."));
    }
    Ok(())
}

async fn reset_recipient(
    state: &IdentityState,
    login: &str,
) -> Result<Option<(user::Model, Mailbox)>, ApiError> {
    let user_id = if login.contains('@') {
        let Some(address) = parse_address(login) else {
            return Ok(None);
        };
        user_email::Entity::find()
            .filter(user_email::Column::Email.eq(address.to_string()))
            .one(state.database())
            .await?
            .map(|row| row.user_id)
    } else {
        let Ok(username) = validate_slug(login, "Username") else {
            return Ok(None);
        };
        user::Entity::find()
            .filter(user::Column::Username.eq(username))
            .one(state.database())
            .await?
            .map(|account| account.id)
    };
    match user_id {
        Some(id) => verified_mailbox(state.database(), id).await,
        None => Ok(None),
    }
}

// ---------------------------------------------------------------------------
// Tokens

async fn issue_token<C: ConnectionTrait>(
    connection: &C,
    user_id: Uuid,
    purpose: &str,
    email: Option<&str>,
    lifetime: Duration,
) -> Result<String, ApiError> {
    let now = Utc::now();
    email_token::Entity::delete_many()
        .filter(email_token::Column::UserId.eq(user_id))
        .filter(email_token::Column::Purpose.eq(purpose))
        .filter(email_token::Column::ExpiresAt.lte(now))
        .exec(connection)
        .await?;
    let token = random_secret(32);
    email_token::ActiveModel {
        token_hash: Set(hash_secret(&token)),
        user_id: Set(user_id),
        purpose: Set(purpose.to_owned()),
        email: Set(email.map(str::to_owned)),
        expires_at: Set(now + lifetime),
        used_at: Set(None),
        created_at: Set(now),
    }
    .insert(connection)
    .await?;
    Ok(token)
}

/// Atomically mark an unexpired, unused token as used and return it.
async fn consume_token<C: ConnectionTrait>(
    connection: &C,
    token_hash: &str,
    purpose: &str,
) -> Result<Option<email_token::Model>, ApiError> {
    let now = Utc::now();
    let consumed = email_token::Entity::update_many()
        .col_expr(email_token::Column::UsedAt, Expr::value(Some(now)))
        .filter(email_token::Column::TokenHash.eq(token_hash))
        .filter(email_token::Column::Purpose.eq(purpose))
        .filter(email_token::Column::UsedAt.is_null())
        .filter(email_token::Column::ExpiresAt.gt(now))
        .exec(connection)
        .await?;
    if consumed.rows_affected != 1 {
        return Ok(None);
    }
    Ok(email_token::Entity::find_by_id(token_hash)
        .one(connection)
        .await?)
}

fn link(state: &IdentityState, path: &str, token: &str) -> String {
    let mut url = state.public_url().clone();
    url.set_path(path);
    url.query_pairs_mut().clear().append_pair("token", token);
    url.to_string()
}

fn send_verification(state: &IdentityState, account: &user::Model, address: &str, token: &str) {
    let Some(address) = parse_address(address) else {
        return;
    };
    let link = link(state, "/verify-email", token);
    state.mailer().enqueue(Email {
        to: Mailbox::new(Some(account.username.clone()), address),
        subject: "Verify your Gitadel email address".to_owned(),
        body: format!(
            "Confirm this address for the Gitadel account \"{}\" at {} by \
             opening this link within 24 hours:\n\n{link}\n\n\
             If you did not add this address, ignore this email.\n",
            account.username,
            state.public_url(),
        ),
    });
}

#[cfg(test)]
mod tests {
    use sea_orm::{ConnectOptions, Database, DatabaseConnection};
    use sea_orm_migration::MigratorTrait;

    use super::*;
    use crate::migration::Migrator;

    async fn database() -> DatabaseConnection {
        let mut options = ConnectOptions::new("sqlite::memory:");
        options.max_connections(1).sqlx_logging(false);
        let database = Database::connect(options).await.unwrap();
        Migrator::up(&database, None).await.unwrap();
        database
    }

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
