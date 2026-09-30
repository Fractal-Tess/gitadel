//! Commit identities: the addresses a user writes commits under, which
//! attribute those commits to them on their profile, and the name and address
//! commits made in the browser are authored with.
//!
//! A commit counts as a user's when its author address is one of their commit
//! addresses or their account address, or when it carries a valid SSH
//! signature from one of their keys.

use std::collections::HashSet;

use super::{ApiError, IdentityState, SCOPE_READ, SCOPE_WRITE};
use crate::entity::{ssh_key, user, user_commit_email, user_commit_profile, user_email};
use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
};
use axum_extra::extract::cookie::CookieJar;
use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, QueryOrder, Set,
    TransactionTrait,
};
use serde::{Deserialize, Serialize};

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

pub async fn commit_identities<C: ConnectionTrait>(
    database: &C,
    account: &user::Model,
) -> Result<CommitIdentities, ApiError> {
    let mut emails = user_commit_email::Entity::find()
        .filter(user_commit_email::Column::UserId.eq(account.id))
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
    let email = match profile.and_then(|profile| profile.primary_email) {
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

#[derive(Serialize)]
pub struct CommitIdentityResponse {
    /// The chosen name, if any.
    name: Option<String>,
    /// The chosen address for browser commits, if any.
    primary_email: Option<String>,
    /// The name and address browser commits actually use.
    author_name: String,
    author_email: String,
    emails: Vec<String>,
    /// The account address, which counts without being listed.
    account_email: Option<String>,
}

async fn identity_response<C: ConnectionTrait>(
    database: &C,
    account: &user::Model,
) -> Result<CommitIdentityResponse, ApiError> {
    let emails = user_commit_email::Entity::find()
        .filter(user_commit_email::Column::UserId.eq(account.id))
        .order_by_asc(user_commit_email::Column::CreatedAt)
        .all(database)
        .await?
        .into_iter()
        .map(|row| row.email)
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
    })
}

pub async fn get_identity(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<CommitIdentityResponse>, ApiError> {
    let actor = state.authenticate(&headers, &jar, SCOPE_READ).await?;
    Ok(Json(
        identity_response(state.database(), &actor.user).await?,
    ))
}

#[derive(Deserialize)]
pub struct UpdateIdentityRequest {
    name: Option<String>,
    primary_email: Option<String>,
}

/// Sets the name and address browser commits are authored with. The address
/// must be one of the user's commit addresses or their account address.
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
                "Add the address to your commit emails before choosing it.",
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
        identity_response(state.database(), &actor.user).await?,
    ))
}

#[derive(Deserialize)]
pub struct AddEmailRequest {
    email: String,
}

/// Claims an address for the user's commits. An address another account has
/// claimed, or uses as its account address, is refused, so nobody can count
/// someone else's commits as their own.
pub async fn add_email(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<AddEmailRequest>,
) -> Result<Json<CommitIdentityResponse>, ApiError> {
    let actor = state.authenticate(&headers, &jar, SCOPE_WRITE).await?;
    let email = normalize_email(&request.email)?;
    let database = state.database();
    if email.ends_with("@gitadel.local") {
        return Err(ApiError::bad_request(
            "Addresses at gitadel.local are reserved for this instance.",
        ));
    }
    if let Some(existing) = user_commit_email::Entity::find()
        .filter(user_commit_email::Column::Email.eq(&email))
        .one(database)
        .await?
    {
        return Err(if existing.user_id == actor.user.id {
            ApiError::conflict("That address is already on your list.")
        } else {
            ApiError::conflict("That address belongs to another account.")
        });
    }
    let account_owner = user_email::Entity::find()
        .all(database)
        .await?
        .into_iter()
        .find(|row| row.email.eq_ignore_ascii_case(&email))
        .map(|row| row.user_id);
    match account_owner {
        Some(owner) if owner == actor.user.id => {
            return Err(ApiError::conflict(
                "Your account email already counts; there is no need to add it.",
            ));
        }
        Some(_) => {
            return Err(ApiError::conflict(
                "That address belongs to another account.",
            ));
        }
        None => {}
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
    let transaction = database.begin().await?;
    user_commit_email::ActiveModel {
        user_id: Set(actor.user.id),
        email: Set(email.clone()),
        created_at: Set(Utc::now()),
    }
    .insert(&transaction)
    .await?;
    state
        .audit_on(
            &transaction,
            Some(actor.user.id),
            "account.commit_email.add",
            Some(email),
        )
        .await?;
    transaction.commit().await?;
    Ok(Json(identity_response(database, &actor.user).await?))
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
    let removed = user_commit_email::Entity::delete_many()
        .filter(user_commit_email::Column::UserId.eq(actor.user.id))
        .filter(user_commit_email::Column::Email.eq(&email))
        .exec(&transaction)
        .await?;
    if removed.rows_affected == 0 {
        return Err(ApiError::not_found());
    }
    // Browser commits stop using an address once it is off the list.
    if let Some(profile) = user_commit_profile::Entity::find_by_id(actor.user.id)
        .one(&transaction)
        .await?
        && profile.primary_email.as_deref() == Some(email.as_str())
    {
        let mut profile: user_commit_profile::ActiveModel = profile.into();
        profile.primary_email = Set(None);
        profile.updated_at = Set(Utc::now());
        profile.update(&transaction).await?;
    }
    state
        .audit_on(
            &transaction,
            Some(actor.user.id),
            "account.commit_email.remove",
            Some(email),
        )
        .await?;
    transaction.commit().await?;
    Ok(Json(identity_response(database, &actor.user).await?))
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
}
