//! Verified address lookup and one-time email credential persistence.
use crate::{IdentityError, hash_secret, random_secret};
use chrono::{Duration, Utc};
use gitadel_db::entity::{email_token, user_email};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, Set, sea_query::Expr,
};
use uuid::Uuid;

pub async fn verified_email<C: ConnectionTrait>(
    connection: &C,
    user_id: Uuid,
) -> Result<Option<String>, IdentityError> {
    Ok(user_email::Entity::find_by_id(user_id)
        .filter(user_email::Column::VerifiedAt.is_not_null())
        .one(connection)
        .await?
        .map(|row| row.email))
}

pub async fn issue_token<C: ConnectionTrait>(
    connection: &C,
    user_id: Uuid,
    purpose: &str,
    email: Option<&str>,
    lifetime: Duration,
) -> Result<String, IdentityError> {
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
pub async fn consume_token<C: ConnectionTrait>(
    connection: &C,
    token_hash: &str,
    purpose: &str,
) -> Result<Option<email_token::Model>, IdentityError> {
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

use crate::{
    Principal, hash_password, runtime::AuthService, validate_password, validate_slug,
    verify_password,
};
use gitadel_db::entity::{session, user};
use gitadel_mail::{Email, parse_address};
use lettre::message::Mailbox;
use sea_orm::TransactionTrait;
const PURPOSE_VERIFY: &str = "verify";
const PURPOSE_RESET: &str = "reset";
/// Verifies a commit address; see [`super::commit_emails`].
pub const PURPOSE_COMMIT_VERIFY: &str = "verify-commit";
pub const VERIFY_LIFETIME: Duration = Duration::hours(24);
const RESET_LIFETIME: Duration = Duration::hours(1);
const RESET_ACCEPTED: &str =
    "If that account has a verified email address, a reset link is on its way.";

pub struct SmtpStatusResponse {
    pub configured: bool,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub tls: Option<String>,
    pub from: Option<String>,
    pub username: Option<Option<String>>,
}

pub struct TestEmailRequest {
    pub to: Option<String>,
}

pub struct TestEmailResponse {
    pub delivered_to: String,
}

pub struct EmailStatusResponse {
    /// Whether the instance can send mail at all.
    pub email_enabled: bool,
    pub email: Option<String>,
    pub verified: bool,
}

pub struct SetEmailRequest {
    pub email: String,
    /// Required when the request uses an API token, because a verified
    /// address can reset the password.
    pub current_password: Option<String>,
}

pub struct TokenRequest {
    pub token: String,
}

pub struct PasswordResetRequest {
    /// Username or verified email address.
    pub login: String,
}

pub struct PasswordResetAccepted {
    pub message: &'static str,
}

pub struct PasswordResetConfirmRequest {
    pub token: String,
    pub password: String,
}

pub fn email_unavailable() -> IdentityError {
    IdentityError::conflict("Email is not configured on this Gitadel instance.")
}
pub fn require_mail(state: &AuthService<'_>) -> Result<(), IdentityError> {
    if state.mailer().is_enabled() {
        Ok(())
    } else {
        Err(email_unavailable())
    }
}
pub async fn verified_mailbox<C: ConnectionTrait>(
    connection: &C,
    user_id: Uuid,
) -> Result<Option<(user::Model, Mailbox)>, IdentityError> {
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
pub async fn email_status(
    state: &AuthService<'_>,
    user_id: Uuid,
) -> Result<EmailStatusResponse, IdentityError> {
    let row = user_email::Entity::find_by_id(user_id)
        .one(state.database())
        .await?;
    Ok(EmailStatusResponse {
        email_enabled: state.mailer().is_enabled(),
        verified: row.as_ref().is_some_and(|row| row.verified_at.is_some()),
        email: row.map(|row| row.email),
    })
}
pub async fn require_reset_available(state: &AuthService<'_>) -> Result<(), IdentityError> {
    if !state.mailer().is_enabled() {
        return Err(IdentityError::conflict(
            "Password reset by email is not available on this instance.",
        ));
    }
    if !crate::sso::public_configuration(state.database())
        .await?
        .password_enabled
    {
        return Err(IdentityError::forbidden("Password login is disabled."));
    }
    Ok(())
}
pub async fn reset_recipient(
    state: &AuthService<'_>,
    login: &str,
) -> Result<Option<(user::Model, Mailbox)>, IdentityError> {
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
pub fn link(state: &AuthService<'_>, path: &str, token: &str) -> String {
    let mut url = state.public_url().clone();
    url.set_path(path);
    url.query_pairs_mut().clear().append_pair("token", token);
    url.to_string()
}
pub fn send_verification(
    state: &AuthService<'_>,
    account: &user::Model,
    address: &str,
    token: &str,
) {
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
pub async fn smtp_status(
    state: &AuthService<'_>,
    actor: &Principal,
) -> Result<SmtpStatusResponse, IdentityError> {
    state.require_actor(actor, true, crate::SCOPE_READ).await?;
    let summary = state.mailer().summary();
    Ok(SmtpStatusResponse {
        configured: state.mailer().is_enabled(),
        host: summary.map(|s| s.host.clone()),
        port: summary.map(|s| s.port),
        tls: summary.map(|s| s.tls.to_owned()),
        from: summary.map(|s| s.from.clone()),
        username: summary.map(|s| s.username.clone()),
    })
}
pub async fn send_test_email(
    state: &AuthService<'_>,
    actor: &Principal,
    request: TestEmailRequest,
) -> Result<TestEmailResponse, IdentityError> {
    let actor = state.require_actor(actor, true, crate::SCOPE_WRITE).await?;
    if !state.mailer().is_enabled() {
        return Err(IdentityError::conflict(
            "SMTP is not configured. Add an [smtp] section to the server configuration.",
        ));
    }
    state.enforce_auth_rate_limit("smtp-test", 5).await?;
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
                IdentityError::bad_request(
                    "Enter a recipient, or verify an email address on your account.",
                )
            })?,
    };
    let address = parse_address(&recipient)
        .ok_or_else(|| IdentityError::bad_request("Enter a valid email address."))?;
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
        .map_err(|error| IdentityError::bad_request(format!("The test email failed: {error}")))?;
    Ok(TestEmailResponse {
        delivered_to: address.to_string(),
    })
}
pub async fn get_email(
    state: &AuthService<'_>,
    actor: &Principal,
) -> Result<EmailStatusResponse, IdentityError> {
    let actor = state.require_actor(actor, false, crate::SCOPE_READ).await?;
    email_status(state, actor.user.id).await
}
pub async fn set_email(
    state: &AuthService<'_>,
    actor: &Principal,
    request: SetEmailRequest,
) -> Result<EmailStatusResponse, IdentityError> {
    let actor = state
        .require_actor(actor, false, crate::SCOPE_WRITE)
        .await?;
    require_mail(state)?;
    if actor.via_api_token {
        let password = request.current_password.ok_or_else(|| {
            IdentityError::bad_request(
                "Changing the email address with a token needs your password.",
            )
        })?;
        state
            .enforce_account_auth_rate_limit(&actor.user.username, 8)
            .await?;
        if !verify_password(password, actor.user.password_hash.clone()).await? {
            return Err(IdentityError::bad_request(
                "The current password is incorrect.",
            ));
        }
    }
    state
        .enforce_rate_limit(format!("email-change:{}", actor.user.id), 5)
        .await?;
    let address = parse_address(&request.email)
        .ok_or_else(|| IdentityError::bad_request("Enter a valid email address."))?
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
            return email_status(state, actor.user.id).await;
        }
        Some(row) if row.user_id != actor.user.id => {
            if row.verified_at.is_some() {
                return Err(IdentityError::conflict(
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
    send_verification(state, &actor.user, &address, &token);
    email_status(state, actor.user.id).await
}
pub async fn resend_verification(
    state: &AuthService<'_>,
    actor: &Principal,
) -> Result<(), IdentityError> {
    let actor = state
        .require_actor(actor, false, crate::SCOPE_WRITE)
        .await?;
    require_mail(state)?;
    state
        .enforce_rate_limit(format!("email-change:{}", actor.user.id), 5)
        .await?;
    let row = user_email::Entity::find_by_id(actor.user.id)
        .one(state.database())
        .await?
        .ok_or_else(|| IdentityError::bad_request("Add an email address first."))?;
    if row.verified_at.is_some() {
        return Err(IdentityError::conflict(
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
    send_verification(state, &actor.user, &row.email, &token);
    Ok(())
}
pub async fn delete_email(state: &AuthService<'_>, actor: &Principal) -> Result<(), IdentityError> {
    let actor = state
        .require_actor(actor, false, crate::SCOPE_WRITE)
        .await?;
    if actor.via_api_token {
        return Err(IdentityError::forbidden(
            "Remove the email address from a browser session.",
        ));
    }
    let transaction = state.database().begin().await?;
    user_email::Entity::delete_by_id(actor.user.id)
        .exec(&transaction)
        .await?;
    // Commit-email links are unrelated to the account address and stay.
    email_token::Entity::delete_many()
        .filter(email_token::Column::UserId.eq(actor.user.id))
        .filter(email_token::Column::Purpose.ne(PURPOSE_COMMIT_VERIFY))
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
    Ok(())
}
pub async fn verify_email(
    state: &AuthService<'_>,
    request: TokenRequest,
) -> Result<(), IdentityError> {
    state.enforce_auth_rate_limit("email-verify", 20).await?;
    let invalid = || IdentityError::bad_request("The verification link is invalid or has expired.");
    let token_hash = hash_secret(request.token.trim());
    let now = Utc::now();
    let transaction = state.database().begin().await?;
    let Some(token) = consume_token(&transaction, &token_hash, PURPOSE_VERIFY).await? else {
        if crate::commit_emails::verify_with_token(state, &transaction, &token_hash).await? {
            transaction.commit().await?;
            return Ok(());
        }
        return Err(invalid());
    };
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
    Ok(())
}
pub async fn request_password_reset(
    state: &AuthService<'_>,
    request: PasswordResetRequest,
) -> Result<PasswordResetAccepted, IdentityError> {
    require_reset_available(state).await?;
    state.enforce_auth_rate_limit("password-reset", 5).await?;
    let login = request.login.trim().to_ascii_lowercase();
    if login.is_empty() || login.len() > 254 {
        return Err(IdentityError::bad_request(
            "Enter a username or email address.",
        ));
    }
    state
        .enforce_rate_limit(format!("password-reset:{login}"), 3)
        .await?;
    if let Some((account, mailbox)) = reset_recipient(state, &login).await? {
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
        let link = link(state, "/reset-password", &token);
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
    Ok(PasswordResetAccepted {
        message: RESET_ACCEPTED,
    })
}
pub async fn confirm_password_reset(
    state: &AuthService<'_>,
    request: PasswordResetConfirmRequest,
) -> Result<(), IdentityError> {
    require_reset_available(state).await?;
    state
        .enforce_auth_rate_limit("password-reset-confirm", 10)
        .await?;
    let invalid = || IdentityError::bad_request("The reset link is invalid or has expired.");
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
    Ok(())
}
