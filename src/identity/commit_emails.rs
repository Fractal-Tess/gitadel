//! Commit identities: the addresses a user writes commits under, which
//! attribute those commits to them on their profile, and the name and address
//! commits made in the browser are authored with.
//!
//! A commit counts as a user's when its author address is one of their
//! verified commit addresses or their account address, or when it carries a
//! valid SSH signature from one of their keys.
//!
//! An address is verified by opening an emailed link. Until then it is a
//! claim: it attributes nothing, cannot author browser commits, and does not
//! stop the real owner from verifying the same address, which removes the
//! unverified claim. When the instance cannot send mail, addresses are
//! verified on creation, as they were before verification existed.

use std::collections::HashSet;

use super::{
    ApiError, IdentityState, SCOPE_READ, SCOPE_WRITE,
    email::{PURPOSE_COMMIT_VERIFY, VERIFY_LIFETIME, consume_token, issue_token, link},
};
use crate::{
    entity::{email_token, ssh_key, user, user_commit_email, user_commit_profile, user_email},
    mail::{Email, parse_address},
};
use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
};
use axum_extra::extract::cookie::CookieJar;
use chrono::Utc;
use lettre::message::Mailbox;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, QueryOrder, Set,
    TransactionTrait, sea_query::Expr,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

const MAX_EMAIL_BYTES: usize = 254;
const MAX_NAME_CHARS: usize = 100;
const MAX_COMMIT_EMAILS: usize = 20;

/// Everything that marks a commit as one user's.
#[derive(Debug, Default)]
pub struct CommitIdentities {
    /// Lowercased author addresses.
    pub emails: HashSet<String>,
    /// SHA256 fingerprints of the user's SSH keys, as signatures report them.
    pub fingerprints: HashSet<String>,
}

impl CommitIdentities {
    pub fn matches(&self, email: &str, signer: Option<&str>) -> bool {
        self.emails.contains(email) || signer.is_some_and(|key| self.fingerprints.contains(key))
    }
}

/// The address commits made on this instance fall back to when a user has
/// named none: it matches what the in-browser editor used before commit
/// identities existed, so those commits still count.
fn placeholder_email(username: &str) -> String {
    format!("{}@gitadel.local", username.to_ascii_lowercase())
}

/// The user's verified identities. Unverified claims are left out, so a
/// claimed address attributes nothing until its owner proves it.
pub async fn commit_identities<C: ConnectionTrait>(
    database: &C,
    account: &user::Model,
) -> Result<CommitIdentities, ApiError> {
    let mut emails = user_commit_email::Entity::find()
        .filter(user_commit_email::Column::UserId.eq(account.id))
        .filter(user_commit_email::Column::VerifiedAt.is_not_null())
        .all(database)
        .await?
        .into_iter()
        .map(|row| row.email)
        .collect::<HashSet<_>>();
    if let Some(row) = user_email::Entity::find_by_id(account.id)
        .one(database)
        .await?
    {
        emails.insert(row.email.to_ascii_lowercase());
    }
    emails.insert(placeholder_email(&account.username));
    let fingerprints = ssh_key::Entity::find()
        .filter(ssh_key::Column::UserId.eq(account.id))
        .all(database)
        .await?
        .into_iter()
        .map(|key| key.fingerprint)
        .collect();
    Ok(CommitIdentities {
        emails,
        fingerprints,
    })
}

/// The name and address a commit made in the browser is authored with: the
/// chosen ones, else the account address, else the instance placeholder.
///
/// A chosen address that is no longer verified, because its real owner
/// verified it elsewhere, falls back rather than impersonating them.
pub async fn commit_author<C: ConnectionTrait>(
    database: &C,
    account: &user::Model,
) -> Result<(String, String), ApiError> {
    let profile = user_commit_profile::Entity::find_by_id(account.id)
        .one(database)
        .await?;
    let name = profile
        .as_ref()
        .and_then(|profile| profile.name.clone())
        .unwrap_or_else(|| account.username.clone());
    let chosen = match profile.and_then(|profile| profile.primary_email) {
        Some(email)
            if commit_identities(database, account)
                .await?
                .emails
                .contains(&email) =>
        {
            Some(email)
        }
        _ => None,
    };
    let email = match chosen {
        Some(email) => email,
        None => user_email::Entity::find_by_id(account.id)
            .one(database)
            .await?
            .map(|row| row.email)
            .unwrap_or_else(|| placeholder_email(&account.username)),
    };
    Ok((name, email))
}

fn normalize_email(raw: &str) -> Result<String, ApiError> {
    let email = raw.trim().to_ascii_lowercase();
    let valid = email.len() <= MAX_EMAIL_BYTES
        && email.split_once('@').is_some_and(|(local, domain)| {
            !local.is_empty() && !domain.is_empty() && !domain.contains('@')
        })
        && !email.chars().any(|character| {
            character.is_whitespace() || character.is_control() || "<>,".contains(character)
        });
    if valid {
        Ok(email)
    } else {
        Err(ApiError::bad_request("Enter a valid email address."))
    }
}

fn normalize_name(raw: &str) -> Result<Option<String>, ApiError> {
    let name = raw.trim();
    if name.is_empty() {
        return Ok(None);
    }
    if name.chars().count() > MAX_NAME_CHARS
        || name
            .chars()
            .any(|character| character.is_control() || "<>".contains(character))
    {
        return Err(ApiError::bad_request(
            "The commit name must be at most 100 characters, without < or >.",
        ));
    }
    Ok(Some(name.to_owned()))
}

/// How an address came to count, or why it does not yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Verification {
    /// Proved by opening the emailed link.
    Verified,
    /// Waiting for the emailed link; attributes nothing yet.
    Pending,
    /// Accepted as entered because this instance cannot send mail.
    NotRequired,
}

#[derive(Serialize)]
pub struct CommitEmailResponse {
    email: String,
    verified_at: Option<chrono::DateTime<Utc>>,
    verification: Verification,
}

impl CommitEmailResponse {
    fn new(row: user_commit_email::Model, mail_enabled: bool) -> Self {
        let verification = match (row.verified_at, mail_enabled) {
            (None, _) => Verification::Pending,
            (Some(_), true) => Verification::Verified,
            (Some(_), false) => Verification::NotRequired,
        };
        Self {
            email: row.email,
            verified_at: row.verified_at,
            verification,
        }
    }
}

#[derive(Serialize)]
pub struct CommitIdentityResponse {
    /// The chosen name, if any.
    name: Option<String>,
    /// The chosen address for browser commits, if any.
    primary_email: Option<String>,
    /// The name and address browser commits actually use.
    author_name: String,
    author_email: String,
    emails: Vec<CommitEmailResponse>,
    /// The account address, which counts without being listed.
    account_email: Option<String>,
    /// Whether new addresses must be confirmed by email before they count.
    /// False when the instance cannot send mail.
    verification_required: bool,
}

async fn identity_response<C: ConnectionTrait>(
    state: &IdentityState,
    database: &C,
    account: &user::Model,
) -> Result<CommitIdentityResponse, ApiError> {
    let mail_enabled = state.mailer().is_enabled();
    let emails = user_commit_email::Entity::find()
        .filter(user_commit_email::Column::UserId.eq(account.id))
        .order_by_asc(user_commit_email::Column::CreatedAt)
        .all(database)
        .await?
        .into_iter()
        .map(|row| CommitEmailResponse::new(row, mail_enabled))
        .collect();
    let profile = user_commit_profile::Entity::find_by_id(account.id)
        .one(database)
        .await?;
    let account_email = user_email::Entity::find_by_id(account.id)
        .one(database)
        .await?
        .map(|row| row.email);
    let (author_name, author_email) = commit_author(database, account).await?;
    Ok(CommitIdentityResponse {
        name: profile.as_ref().and_then(|profile| profile.name.clone()),
        primary_email: profile.and_then(|profile| profile.primary_email),
        author_name,
        author_email,
        emails,
        account_email,
        verification_required: mail_enabled,
    })
}

pub async fn get_identity(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<CommitIdentityResponse>, ApiError> {
    let actor = state.authenticate(&headers, &jar, SCOPE_READ).await?;
    Ok(Json(
        identity_response(&state, state.database(), &actor.user).await?,
    ))
}

#[derive(Deserialize)]
pub struct UpdateIdentityRequest {
    name: Option<String>,
    primary_email: Option<String>,
}

/// Sets the name and address browser commits are authored with. The address
/// must be one of the user's verified commit addresses or their account
/// address.
pub async fn update_identity(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<UpdateIdentityRequest>,
) -> Result<Json<CommitIdentityResponse>, ApiError> {
    let actor = state.authenticate(&headers, &jar, SCOPE_WRITE).await?;
    let name = request
        .name
        .as_deref()
        .map(normalize_name)
        .transpose()?
        .flatten();
    let primary_email = request
        .primary_email
        .as_deref()
        .filter(|email| !email.trim().is_empty())
        .map(normalize_email)
        .transpose()?;
    if let Some(email) = &primary_email {
        let identities = commit_identities(state.database(), &actor.user).await?;
        if *email == placeholder_email(&actor.user.username) || !identities.emails.contains(email) {
            return Err(ApiError::bad_request(
                "Add and verify the address in your commit emails before choosing it.",
            ));
        }
    }
    let transaction = state.database().begin().await?;
    user_commit_profile::Entity::delete_by_id(actor.user.id)
        .exec(&transaction)
        .await?;
    user_commit_profile::ActiveModel {
        user_id: Set(actor.user.id),
        name: Set(name),
        primary_email: Set(primary_email),
        updated_at: Set(Utc::now()),
    }
    .insert(&transaction)
    .await?;
    state
        .audit_on(
            &transaction,
            Some(actor.user.id),
            "account.commit_identity.update",
            None,
        )
        .await?;
    transaction.commit().await?;
    Ok(Json(
        identity_response(&state, state.database(), &actor.user).await?,
    ))
}

#[derive(Deserialize)]
pub struct AddEmailRequest {
    email: String,
}

/// Whether another account has verified `email`, as a commit address or as
/// its account address. Unverified claims by others do not count: they must
/// not reserve an address its real owner can prove.
async fn verified_by_another<C: ConnectionTrait>(
    database: &C,
    user_id: Uuid,
    email: &str,
) -> Result<bool, ApiError> {
    let commit_claim = user_commit_email::Entity::find()
        .filter(user_commit_email::Column::Email.eq(email))
        .filter(user_commit_email::Column::UserId.ne(user_id))
        .filter(user_commit_email::Column::VerifiedAt.is_not_null())
        .one(database)
        .await?
        .is_some();
    if commit_claim {
        return Ok(true);
    }
    Ok(user_email::Entity::find()
        .filter(user_email::Column::Email.eq(email))
        .filter(user_email::Column::UserId.ne(user_id))
        .filter(user_email::Column::VerifiedAt.is_not_null())
        .one(database)
        .await?
        .is_some())
}

/// Claims an address for the user's commits. When the instance sends mail the
/// claim stays pending until the emailed link is opened; otherwise it counts
/// at once. An address another account has verified is refused, so nobody
/// can count someone else's commits as their own.
pub async fn add_email(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<AddEmailRequest>,
) -> Result<(StatusCode, Json<CommitIdentityResponse>), ApiError> {
    let actor = state.authenticate(&headers, &jar, SCOPE_WRITE).await?;
    let email = normalize_email(&request.email)?;
    let database = state.database();
    if email.ends_with("@gitadel.local") {
        return Err(ApiError::bad_request(
            "Addresses at gitadel.local are reserved for this instance.",
        ));
    }
    if let Some(existing) = user_commit_email::Entity::find_by_id((actor.user.id, email.clone()))
        .one(database)
        .await?
    {
        return Err(ApiError::conflict(if existing.verified_at.is_some() {
            "That address is already on your list."
        } else {
            "That address is already waiting for verification. Resend the email if it did not arrive."
        }));
    }
    if user_email::Entity::find_by_id(actor.user.id)
        .one(database)
        .await?
        .is_some_and(|row| row.email.eq_ignore_ascii_case(&email))
    {
        return Err(ApiError::conflict(
            "Your account email already counts; there is no need to add it.",
        ));
    }
    if verified_by_another(database, actor.user.id, &email).await? {
        return Err(ApiError::conflict(
            "That address belongs to another account.",
        ));
    }
    let count = user_commit_email::Entity::find()
        .filter(user_commit_email::Column::UserId.eq(actor.user.id))
        .all(database)
        .await?
        .len();
    if count >= MAX_COMMIT_EMAILS {
        return Err(ApiError::bad_request(format!(
            "Add at most {MAX_COMMIT_EMAILS} commit emails."
        )));
    }
    let mail_enabled = state.mailer().is_enabled();
    if mail_enabled {
        state
            .enforce_rate_limit(format!("commit-email-verify:{}", actor.user.id), 5)
            .await?;
    }
    let now = Utc::now();
    let transaction = database.begin().await?;
    user_commit_email::ActiveModel {
        user_id: Set(actor.user.id),
        email: Set(email.clone()),
        verified_at: Set((!mail_enabled).then_some(now)),
        created_at: Set(now),
    }
    .insert(&transaction)
    .await?;
    let token = if mail_enabled {
        Some(
            issue_token(
                &transaction,
                actor.user.id,
                PURPOSE_COMMIT_VERIFY,
                Some(&email),
                VERIFY_LIFETIME,
            )
            .await?,
        )
    } else {
        None
    };
    state
        .audit_on(
            &transaction,
            Some(actor.user.id),
            "account.commit_email.add",
            Some(email.clone()),
        )
        .await?;
    transaction.commit().await?;
    if let Some(token) = token {
        send_verification(&state, &actor.user, &email, &token);
    }
    Ok((
        StatusCode::CREATED,
        Json(identity_response(&state, database, &actor.user).await?),
    ))
}

/// Sends a fresh verification link for a pending address.
pub async fn resend_verification(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(email): Path<String>,
) -> Result<StatusCode, ApiError> {
    let actor = state.authenticate(&headers, &jar, SCOPE_WRITE).await?;
    let email = normalize_email(&email)?;
    if !state.mailer().is_enabled() {
        return Err(ApiError::conflict(
            "Email is not configured on this Gitadel instance.",
        ));
    }
    let row = user_commit_email::Entity::find_by_id((actor.user.id, email.clone()))
        .one(state.database())
        .await?
        .ok_or_else(ApiError::not_found)?;
    if row.verified_at.is_some() {
        return Err(ApiError::conflict("This address is already verified."));
    }
    state
        .enforce_rate_limit(format!("commit-email-verify:{}", actor.user.id), 5)
        .await?;
    let transaction = state.database().begin().await?;
    let token = issue_token(
        &transaction,
        actor.user.id,
        PURPOSE_COMMIT_VERIFY,
        Some(&email),
        VERIFY_LIFETIME,
    )
    .await?;
    transaction.commit().await?;
    send_verification(&state, &actor.user, &email, &token);
    Ok(StatusCode::ACCEPTED)
}

/// Confirms a pending address from its emailed token, inside the caller's
/// transaction. Returns `Ok(false)` when the token does not match a pending
/// claim, so the caller can answer as it does for any bad link.
///
/// Verifying resolves the address in the owner's favour: other accounts'
/// unverified claims on it, as commit or account addresses, are removed.
pub(super) async fn verify_with_token<C: ConnectionTrait>(
    state: &IdentityState,
    connection: &C,
    token_hash: &str,
) -> Result<bool, ApiError> {
    let Some(token) = consume_token(connection, token_hash, PURPOSE_COMMIT_VERIFY).await? else {
        return Ok(false);
    };
    let Some(email) = token.email else {
        return Ok(false);
    };
    let Some(row) = user_commit_email::Entity::find_by_id((token.user_id, email.clone()))
        .one(connection)
        .await?
    else {
        return Ok(false);
    };
    if row.verified_at.is_some() {
        return Ok(true);
    }
    if verified_by_another(connection, token.user_id, &email).await? {
        return Err(ApiError::conflict(
            "That address was verified by another account in the meantime.",
        ));
    }
    let now = Utc::now();
    let losing_claims = user_commit_email::Entity::find()
        .filter(user_commit_email::Column::Email.eq(&email))
        .filter(user_commit_email::Column::UserId.ne(token.user_id))
        .all(connection)
        .await?;
    for claim in losing_claims {
        remove_claim(connection, &claim).await?;
    }
    user_email::Entity::delete_many()
        .filter(user_email::Column::Email.eq(&email))
        .filter(user_email::Column::UserId.ne(token.user_id))
        .filter(user_email::Column::VerifiedAt.is_null())
        .exec(connection)
        .await?;
    user_commit_email::Entity::update_many()
        .col_expr(
            user_commit_email::Column::VerifiedAt,
            Expr::value(Some(now)),
        )
        .filter(user_commit_email::Column::UserId.eq(token.user_id))
        .filter(user_commit_email::Column::Email.eq(&email))
        .exec(connection)
        .await?;
    state
        .audit_on(
            connection,
            Some(token.user_id),
            "account.commit_email.verify",
            Some(email),
        )
        .await?;
    Ok(true)
}

/// Deletes one claim with its pending tokens, and stops browser commits from
/// using the address once it is off the list.
async fn remove_claim<C: ConnectionTrait>(
    connection: &C,
    claim: &user_commit_email::Model,
) -> Result<(), ApiError> {
    user_commit_email::Entity::delete_by_id((claim.user_id, claim.email.clone()))
        .exec(connection)
        .await?;
    email_token::Entity::delete_many()
        .filter(email_token::Column::UserId.eq(claim.user_id))
        .filter(email_token::Column::Purpose.eq(PURPOSE_COMMIT_VERIFY))
        .filter(email_token::Column::Email.eq(&claim.email))
        .exec(connection)
        .await?;
    user_commit_profile::Entity::update_many()
        .col_expr(
            user_commit_profile::Column::PrimaryEmail,
            Expr::value(Option::<String>::None),
        )
        .col_expr(
            user_commit_profile::Column::UpdatedAt,
            Expr::value(Utc::now()),
        )
        .filter(user_commit_profile::Column::UserId.eq(claim.user_id))
        .filter(user_commit_profile::Column::PrimaryEmail.eq(&claim.email))
        .exec(connection)
        .await?;
    Ok(())
}

pub async fn remove_email(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(email): Path<String>,
) -> Result<Json<CommitIdentityResponse>, ApiError> {
    let actor = state.authenticate(&headers, &jar, SCOPE_WRITE).await?;
    let email = normalize_email(&email)?;
    let database = state.database();
    let transaction = database.begin().await?;
    let claim = user_commit_email::Entity::find_by_id((actor.user.id, email.clone()))
        .one(&transaction)
        .await?
        .ok_or_else(ApiError::not_found)?;
    remove_claim(&transaction, &claim).await?;
    state
        .audit_on(
            &transaction,
            Some(actor.user.id),
            "account.commit_email.remove",
            Some(email),
        )
        .await?;
    transaction.commit().await?;
    Ok(Json(
        identity_response(&state, database, &actor.user).await?,
    ))
}

fn send_verification(state: &IdentityState, account: &user::Model, address: &str, token: &str) {
    let Some(mailbox) = parse_address(address) else {
        return;
    };
    let link = link(state, "/verify-email", token);
    state.mailer().enqueue(Email {
        to: Mailbox::new(Some(account.username.clone()), mailbox),
        subject: "Verify your Gitadel commit email".to_owned(),
        body: format!(
            "Confirm that commits written as {address} belong to the Gitadel \
             account \"{}\" at {} by opening this link within 24 hours:\n\n{link}\n\n\
             Until then the address counts for nothing. If you did not add it, \
             ignore this email.\n",
            account.username,
            state.public_url(),
        ),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_are_normalized_and_validated() {
        assert_eq!(
            normalize_email("  Me@Example.COM ").unwrap(),
            "me@example.com"
        );
        for invalid in [
            "",
            "no-at",
            "@example.com",
            "me@",
            "a@b@c",
            "me @x.com",
            "<me@x.com>",
        ] {
            assert!(normalize_email(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn names_are_optional_and_cannot_break_the_author_line() {
        assert_eq!(normalize_name("  ").unwrap(), None);
        assert_eq!(
            normalize_name(" Fractal Tess ").unwrap().as_deref(),
            Some("Fractal Tess")
        );
        assert!(normalize_name("Evil <x@y>").is_err());
        assert!(normalize_name("line\nbreak").is_err());
    }

    #[test]
    fn a_commit_matches_by_address_or_signing_key() {
        let identities = CommitIdentities {
            emails: HashSet::from(["me@example.com".to_owned()]),
            fingerprints: HashSet::from(["SHA256:key".to_owned()]),
        };
        assert!(identities.matches("me@example.com", None));
        assert!(identities.matches("other@example.com", Some("SHA256:key")));
        assert!(!identities.matches("other@example.com", Some("SHA256:else")));
        assert!(!identities.matches("other@example.com", None));
    }

    mod handlers {
        use axum::http::{Method, StatusCode};
        use sea_orm::{ActiveModelTrait, EntityTrait, Set};
        use serde_json::{Value, json};

        use super::super::*;
        use crate::{
            identity::test_support::{Harness, mailed_token},
            mail::{fake_smtp_server, test_mailer},
        };

        fn emails(identity: &Value) -> Vec<(String, String)> {
            identity["emails"]
                .as_array()
                .unwrap()
                .iter()
                .map(|entry| {
                    (
                        entry["email"].as_str().unwrap().to_owned(),
                        entry["verification"].as_str().unwrap().to_owned(),
                    )
                })
                .collect()
        }

        async fn verified_identities(harness: &Harness, account: &user::Model) -> HashSet<String> {
            commit_identities(&harness.database, account)
                .await
                .unwrap()
                .emails
        }

        #[tokio::test]
        async fn without_mail_addresses_count_at_once_and_say_so() {
            let harness = Harness::new().await;
            let alice = harness.user("alice").await;
            let cookie = harness.login(&alice).await;

            let (status, body) = harness
                .request(
                    Method::POST,
                    "/api/v1/me/commit-emails",
                    Some(&cookie),
                    Some(json!({ "email": "Alice@Example.com" })),
                )
                .await;
            assert_eq!(status, StatusCode::CREATED, "{body}");
            assert_eq!(body["verification_required"], Value::Bool(false));
            assert_eq!(
                emails(&body),
                vec![("alice@example.com".to_owned(), "not_required".to_owned())]
            );
            assert!(body["emails"][0]["verified_at"].is_string());
            assert!(
                verified_identities(&harness, &alice)
                    .await
                    .contains("alice@example.com")
            );
        }

        #[tokio::test]
        async fn with_mail_addresses_are_pending_until_the_link_is_opened() {
            let (port, server) = fake_smtp_server().await;
            let harness = Harness::new().await.with_mailer(test_mailer(port));
            let alice = harness.user("alice").await;
            let cookie = harness.login(&alice).await;

            let (status, body) = harness
                .request(
                    Method::POST,
                    "/api/v1/me/commit-emails",
                    Some(&cookie),
                    Some(json!({ "email": "alice@example.com" })),
                )
                .await;
            assert_eq!(status, StatusCode::CREATED, "{body}");
            assert_eq!(body["verification_required"], Value::Bool(true));
            assert_eq!(
                emails(&body),
                vec![("alice@example.com".to_owned(), "pending".to_owned())]
            );
            assert!(body["emails"][0]["verified_at"].is_null());
            assert!(
                !verified_identities(&harness, &alice)
                    .await
                    .contains("alice@example.com"),
                "a pending address attributes nothing"
            );

            // A pending address cannot author browser commits either.
            let (status, body) = harness
                .request(
                    Method::PUT,
                    "/api/v1/me/commit-identity",
                    Some(&cookie),
                    Some(json!({ "primary_email": "alice@example.com" })),
                )
                .await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

            let mail = tokio::time::timeout(std::time::Duration::from_secs(10), server)
                .await
                .unwrap()
                .unwrap();
            assert!(mail.contains("Subject: Verify your Gitadel commit email"));
            let token = mailed_token(&mail);

            let (status, body) = harness
                .request(
                    Method::POST,
                    "/api/v1/auth/email/verify",
                    None,
                    Some(json!({ "token": token })),
                )
                .await;
            assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
            assert!(
                verified_identities(&harness, &alice)
                    .await
                    .contains("alice@example.com")
            );
            let (_, body) = harness
                .request(
                    Method::GET,
                    "/api/v1/me/commit-identity",
                    Some(&cookie),
                    None,
                )
                .await;
            assert_eq!(
                emails(&body),
                vec![("alice@example.com".to_owned(), "verified".to_owned())]
            );
            let (status, body) = harness
                .request(
                    Method::PUT,
                    "/api/v1/me/commit-identity",
                    Some(&cookie),
                    Some(json!({ "primary_email": "alice@example.com" })),
                )
                .await;
            assert_eq!(status, StatusCode::OK, "{body}");
            assert_eq!(body["author_email"], "alice@example.com");

            // The link is single use.
            let (status, _) = harness
                .request(
                    Method::POST,
                    "/api/v1/auth/email/verify",
                    None,
                    Some(json!({ "token": token })),
                )
                .await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
        }

        #[tokio::test]
        async fn an_unverified_claim_cannot_block_the_owner_and_is_removed_when_they_verify() {
            let harness = Harness::new().await.with_mailer(test_mailer(1));
            let squatter = harness.user("squatter").await;
            let owner = harness.user("owner").await;
            let now = Utc::now();
            user_commit_email::ActiveModel {
                user_id: Set(squatter.id),
                email: Set("owner@example.com".to_owned()),
                verified_at: Set(None),
                created_at: Set(now),
            }
            .insert(&harness.database)
            .await
            .unwrap();
            user_commit_profile::ActiveModel {
                user_id: Set(squatter.id),
                name: Set(None),
                primary_email: Set(Some("owner@example.com".to_owned())),
                updated_at: Set(now),
            }
            .insert(&harness.database)
            .await
            .unwrap();

            // The owner can still claim the address.
            let owner_cookie = harness.login(&owner).await;
            let (status, body) = harness
                .request(
                    Method::POST,
                    "/api/v1/me/commit-emails",
                    Some(&owner_cookie),
                    Some(json!({ "email": "owner@example.com" })),
                )
                .await;
            assert_eq!(status, StatusCode::CREATED, "{body}");

            // ... and verifying it removes the squatter's claim.
            let token = issue_token(
                &harness.database,
                owner.id,
                PURPOSE_COMMIT_VERIFY,
                Some("owner@example.com"),
                VERIFY_LIFETIME,
            )
            .await
            .unwrap();
            let (status, body) = harness
                .request(
                    Method::POST,
                    "/api/v1/auth/email/verify",
                    None,
                    Some(json!({ "token": token })),
                )
                .await;
            assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
            assert!(
                verified_identities(&harness, &owner)
                    .await
                    .contains("owner@example.com")
            );
            assert!(
                user_commit_email::Entity::find_by_id((
                    squatter.id,
                    "owner@example.com".to_owned()
                ))
                .one(&harness.database)
                .await
                .unwrap()
                .is_none()
            );
            let squatter_profile = user_commit_profile::Entity::find_by_id(squatter.id)
                .one(&harness.database)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(squatter_profile.primary_email, None);

            // Once verified, the address is taken.
            let squatter_cookie = harness.login(&squatter).await;
            let (status, body) = harness
                .request(
                    Method::POST,
                    "/api/v1/me/commit-emails",
                    Some(&squatter_cookie),
                    Some(json!({ "email": "owner@example.com" })),
                )
                .await;
            assert_eq!(status, StatusCode::CONFLICT, "{body}");
        }

        #[tokio::test]
        async fn resending_needs_a_pending_address_and_mail() {
            let harness = Harness::new().await;
            let alice = harness.user("alice").await;
            let cookie = harness.login(&alice).await;
            harness
                .request(
                    Method::POST,
                    "/api/v1/me/commit-emails",
                    Some(&cookie),
                    Some(json!({ "email": "alice@example.com" })),
                )
                .await;
            let (status, _) = harness
                .request(
                    Method::POST,
                    "/api/v1/me/commit-emails/alice%40example.com/verification",
                    Some(&cookie),
                    None,
                )
                .await;
            assert_eq!(status, StatusCode::CONFLICT, "mail is not configured");

            let (port, server) = fake_smtp_server().await;
            let harness = Harness::new().await.with_mailer(test_mailer(port));
            let alice = harness.user("alice").await;
            let cookie = harness.login(&alice).await;
            user_commit_email::ActiveModel {
                user_id: Set(alice.id),
                email: Set("alice@example.com".to_owned()),
                verified_at: Set(None),
                created_at: Set(Utc::now()),
            }
            .insert(&harness.database)
            .await
            .unwrap();
            let (status, _) = harness
                .request(
                    Method::POST,
                    "/api/v1/me/commit-emails/alice%40example.com/verification",
                    Some(&cookie),
                    None,
                )
                .await;
            assert_eq!(status, StatusCode::ACCEPTED);
            let mail = tokio::time::timeout(std::time::Duration::from_secs(10), server)
                .await
                .unwrap()
                .unwrap();
            assert!(mail.contains("/verify-email?token="));
            let (status, _) = harness
                .request(
                    Method::POST,
                    "/api/v1/me/commit-emails/nobody%40example.com/verification",
                    Some(&cookie),
                    None,
                )
                .await;
            assert_eq!(status, StatusCode::NOT_FOUND);
        }

        #[tokio::test]
        async fn removing_a_pending_address_discards_its_tokens() {
            let harness = Harness::new().await.with_mailer(test_mailer(1));
            let alice = harness.user("alice").await;
            let cookie = harness.login(&alice).await;
            harness
                .request(
                    Method::POST,
                    "/api/v1/me/commit-emails",
                    Some(&cookie),
                    Some(json!({ "email": "alice@example.com" })),
                )
                .await;
            assert_eq!(
                email_token::Entity::find()
                    .all(&harness.database)
                    .await
                    .unwrap()
                    .len(),
                1
            );
            let (status, body) = harness
                .request(
                    Method::DELETE,
                    "/api/v1/me/commit-emails/alice%40example.com",
                    Some(&cookie),
                    None,
                )
                .await;
            assert_eq!(status, StatusCode::OK, "{body}");
            assert!(emails(&body).is_empty());
            assert!(
                email_token::Entity::find()
                    .all(&harness.database)
                    .await
                    .unwrap()
                    .is_empty()
            );
        }
    }
}
