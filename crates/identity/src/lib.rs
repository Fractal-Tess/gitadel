//! Account authentication, credential validation, and session persistence.
//! HTTP extraction, cookies, response DTOs, and application state belong to callers.

use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{Duration as ChronoDuration, Utc};
use gitadel_db::entity::{api_token, oauth_access_token, session, ssh_key, user};
use rand::RngExt as _;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait,
    PaginatorTrait, QueryFilter, Set, TransactionTrait,
};
use sha2::{Digest, Sha256};
use std::sync::LazyLock;
use tokio::sync::Semaphore;
use uuid::Uuid;

mod error;
pub use error::IdentityError;
pub mod email;
pub mod permissions;

pub mod accounts;
pub mod auth;
pub mod avatar;
pub mod commit_emails;
pub mod images;
pub mod mirror_identities;
pub mod notifications;
pub mod oauth;
pub mod principal;
pub mod resources;
pub mod runtime;
#[cfg(test)]
mod service_tests;
pub mod sso;
#[cfg(test)]
mod tests;
pub mod two_factor;
pub mod users;
pub use principal::{CredentialSource, Principal};

/// Account workflows with explicit persistence and audit dependencies.
pub struct AccountService<'a> {
    database: &'a DatabaseConnection,
    audit: gitadel_core::audit::AuditContext,
}

impl<'a> AccountService<'a> {
    pub fn new(database: &'a DatabaseConnection, audit: gitadel_core::audit::AuditContext) -> Self {
        Self { database, audit }
    }

    pub fn database(&self) -> &DatabaseConnection {
        self.database
    }

    /// Reload authority on the writer; callers cannot manufacture administrator access.
    pub async fn require_actor(
        &self,
        actor: &Principal,
        admin: bool,
        required_scope: i32,
    ) -> Result<AuthenticatedUser, IdentityError> {
        let actor = actor.authorize(self.database, required_scope).await?;
        if admin && !actor.user.is_admin {
            return Err(IdentityError::forbidden(
                "Administrator access is required.",
            ));
        }
        Ok(actor)
    }

    pub async fn audit_on<C: ConnectionTrait>(
        &self,
        connection: &C,
        actor: Option<Uuid>,
        action: impl Into<String>,
        target: Option<String>,
    ) -> Result<(), IdentityError> {
        gitadel_db::audit::record(connection, actor, action, target, self.audit).await?;
        Ok(())
    }
}

/// General API read access.
pub const SCOPE_READ: i32 = 1;
/// General API write access.
pub const SCOPE_WRITE: i32 = 1 << 1;
/// SSH key management access.
pub const SCOPE_SSH_KEYS: i32 = 1 << 2;
/// OAuth repository read access; personal API tokens use general read access.
pub const SCOPE_REPOSITORY_READ: i32 = 1 << 3;

const PASSWORD_WORK_CONCURRENCY: usize = 4;
static PASSWORD_WORK: LazyLock<Semaphore> =
    LazyLock::new(|| Semaphore::new(PASSWORD_WORK_CONCURRENCY));

/// An enabled account authenticated with a session or scoped credential.
#[derive(Clone)]
pub struct AuthenticatedUser {
    pub user: user::Model,
    pub via_api_token: bool,
}

/// Credential operations use the writer so revocations are immediately visible.
pub struct IdentityService<'a> {
    database: &'a DatabaseConnection,
}

impl<'a> IdentityService<'a> {
    /// Borrow the writer connection without owning application state.
    pub fn new(database: &'a DatabaseConnection) -> Self {
        Self { database }
    }
    pub async fn authenticate_session_principal(
        &self,
        raw: &str,
    ) -> Result<Principal, IdentityError> {
        let actor = self.authenticate_session(raw).await?;
        Ok(Principal {
            actor,
            credential: principal::Credential::Browser {
                hash: hash_secret(raw),
            },
        })
    }
    /// Authenticate a personal API token or OAuth access token with required scopes.
    pub async fn authenticate_token(
        &self,
        token: &str,
        required_scope: i32,
    ) -> Result<AuthenticatedUser, IdentityError> {
        Ok(self
            .authenticate_token_principal(token, required_scope)
            .await?
            .actor)
    }
    pub async fn authenticate_token_principal(
        &self,
        token: &str,
        required_scope: i32,
    ) -> Result<Principal, IdentityError> {
        let token_hash = hash_secret(token);
        let now = Utc::now();
        if let Some(stored) = api_token::Entity::find()
            .filter(api_token::Column::TokenHash.eq(&token_hash))
            .filter(api_token::Column::RevokedAt.is_null())
            .one(self.database)
            .await?
            .filter(|token| token.expires_at.is_none_or(|expires_at| expires_at > now))
        {
            let required_api_scope = if required_scope & SCOPE_REPOSITORY_READ != 0 {
                (required_scope & !SCOPE_REPOSITORY_READ) | SCOPE_READ
            } else {
                required_scope
            };
            if stored.scopes & required_api_scope != required_api_scope {
                return Err(IdentityError::forbidden(
                    "The API token does not have the required scope.",
                ));
            }
            let account = self
                .enabled_user(stored.user_id)
                .await?
                .ok_or_else(IdentityError::unauthorized)?;
            let credential = principal::Credential::Personal {
                id: stored.id,
                scopes: stored.scopes,
            };
            if stored
                .last_used_at
                .is_none_or(|last_used_at| now - last_used_at > ChronoDuration::minutes(5))
            {
                let mut active: api_token::ActiveModel = stored.into();
                active.last_used_at = Set(Some(now));
                active.update(self.database).await?;
            }
            return Ok(Principal {
                actor: AuthenticatedUser {
                    user: account,
                    via_api_token: true,
                },
                credential,
            });
        }

        let stored = oauth_access_token::Entity::find_by_id(token_hash)
            .filter(oauth_access_token::Column::RevokedAt.is_null())
            .one(self.database)
            .await?
            .filter(|token| token.expires_at > now)
            .ok_or_else(IdentityError::unauthorized)?;
        if stored.scopes & required_scope != required_scope {
            return Err(IdentityError::forbidden(
                "The OAuth token does not have the required scope.",
            ));
        }
        let account = self
            .enabled_user(stored.user_id)
            .await?
            .ok_or_else(IdentityError::unauthorized)?;
        let credential = principal::Credential::OAuth {
            hash: stored.token_hash.clone(),
            scopes: stored.scopes,
        };
        if stored
            .last_used_at
            .is_none_or(|last_used_at| now - last_used_at > ChronoDuration::minutes(5))
        {
            let mut active: oauth_access_token::ActiveModel = stored.into();
            active.last_used_at = Set(Some(now));
            active.update(self.database).await?;
        }
        Ok(Principal {
            actor: AuthenticatedUser {
                user: account,
                via_api_token: true,
            },
            credential,
        })
    }

    /// Resolve an SSH fingerprint to an enabled account and refresh key activity.
    pub async fn authenticate_ssh_key(
        &self,
        fingerprint: &str,
    ) -> Result<Option<user::Model>, IdentityError> {
        let Some(stored) = ssh_key::Entity::find()
            .filter(ssh_key::Column::Fingerprint.eq(fingerprint))
            .one(self.database)
            .await?
        else {
            return Ok(None);
        };
        let Some(account) = self.enabled_user(stored.user_id).await? else {
            return Ok(None);
        };
        let now = Utc::now();
        if stored
            .last_used_at
            .is_none_or(|last_used_at| now - last_used_at > ChronoDuration::minutes(5))
        {
            let mut active: ssh_key::ActiveModel = stored.into();
            active.last_used_at = Set(Some(now));
            active.update(self.database).await?;
        }
        Ok(Some(account))
    }

    /// Recheck the account on the writer so disabling it takes effect immediately.
    pub async fn enabled_user(&self, id: Uuid) -> Result<Option<user::Model>, IdentityError> {
        Ok(user::Entity::find_by_id(id)
            .filter(user::Column::DisabledAt.is_null())
            .one(self.database)
            .await?)
    }

    /// Authenticate a raw browser session and refresh its activity timestamp.
    pub async fn authenticate_session(
        &self,
        raw_token: &str,
    ) -> Result<AuthenticatedUser, IdentityError> {
        let token_hash = hash_secret(raw_token);
        let now = Utc::now();
        let stored = session::Entity::find_by_id(token_hash)
            .one(self.database)
            .await?
            .filter(|session| session.expires_at > now)
            .ok_or_else(IdentityError::unauthorized)?;
        let account = self
            .enabled_user(stored.user_id)
            .await?
            .ok_or_else(IdentityError::unauthorized)?;

        if now - stored.last_seen_at > ChronoDuration::minutes(5) {
            let mut active: session::ActiveModel = stored.into();
            active.last_seen_at = Set(now);
            active.update(self.database).await?;
        }

        Ok(AuthenticatedUser {
            user: account,
            via_api_token: false,
        })
    }
    /// Find an enabled account for an unexpired session without refreshing activity.
    pub async fn session_user(
        &self,
        raw_token: &str,
    ) -> Result<Option<user::Model>, IdentityError> {
        let token_hash = hash_secret(raw_token);
        let now = Utc::now();
        let Some(stored) = session::Entity::find_by_id(token_hash)
            .one(self.database)
            .await?
            .filter(|session| session.expires_at > now)
        else {
            return Ok(None);
        };
        self.enabled_user(stored.user_id).await
    }
}

/// Persist a new session on the caller's connection or transaction.
pub async fn create_session_on<C: ConnectionTrait>(
    connection: &C,
    user_id: Uuid,
    lifetime: ChronoDuration,
) -> Result<String, IdentityError> {
    let raw_token = random_secret(32);
    let token_hash = hash_secret(&raw_token);
    let now = Utc::now();
    let expires_at = now + lifetime;
    session::ActiveModel {
        token_hash: Set(token_hash),
        user_id: Set(user_id),
        expires_at: Set(expires_at),
        created_at: Set(now),
        last_seen_at: Set(now),
    }
    .insert(connection)
    .await?;

    Ok(raw_token)
}

/// Revoke a raw session token.
pub async fn delete_session(
    connection: &DatabaseConnection,
    raw_token: &str,
) -> Result<(), IdentityError> {
    session::Entity::delete_by_id(hash_secret(raw_token))
        .exec(connection)
        .await?;
    Ok(())
}

/// Verify a password before changing account credentials.
pub async fn verify_current_password(
    account: &user::Model,
    current_password: String,
) -> Result<(), IdentityError> {
    if !verify_password(current_password, account.password_hash.clone()).await? {
        return Err(IdentityError::bad_request(
            "The current password is incorrect.",
        ));
    }
    Ok(())
}

/// Replace an already validated password hash and revoke all other sessions.
/// Use the caller's transaction so auditing commits with the password change.
pub async fn replace_password_on<C: ConnectionTrait>(
    connection: &C,
    account: &user::Model,
    password_hash: String,
    current_session_hash: &str,
) -> Result<(), IdentityError> {
    let mut active: user::ActiveModel = account.clone().into();
    active.password_hash = Set(password_hash);
    active.updated_at = Set(Utc::now());
    active.update(connection).await?;
    session::Entity::delete_many()
        .filter(session::Column::UserId.eq(account.id))
        .filter(session::Column::TokenHash.ne(current_session_hash))
        .exec(connection)
        .await?;
    Ok(())
}

/// Create the first administrator, personal namespace, and audit record atomically.
pub async fn bootstrap_admin(
    database: &DatabaseConnection,
    username: &str,
    password: String,
) -> Result<user::Model, IdentityError> {
    let username = validate_slug(username, "Username")?;
    if user::Entity::find().count(database).await? != 0 {
        return Err(IdentityError::conflict(
            "The first administrator already exists.",
        ));
    }
    let password_hash = hash_password(password).await?;
    let transaction = database.begin().await?;
    if user::Entity::find().count(&transaction).await? != 0 {
        return Err(IdentityError::conflict(
            "The first administrator already exists.",
        ));
    }
    let now = Utc::now();
    let account =
        crate::accounts::insert_account_on(&transaction, &username, password_hash, true, now)
            .await?;
    gitadel_db::audit::record(
        &transaction,
        Some(account.id),
        "admin.bootstrap",
        Some(username),
        gitadel_core::audit::AuditContext::default(),
    )
    .await?;
    transaction.commit().await?;
    Ok(account)
}

/// Generate an unpadded URL-safe random credential with the requested entropy.
pub fn random_secret(bytes: usize) -> String {
    let mut data = vec![0_u8; bytes];
    rand::rng().fill(&mut data);
    URL_SAFE_NO_PAD.encode(data)
}

/// Hash a high-entropy credential in the persisted SHA-256/base64 format.
pub fn hash_secret(secret: &str) -> String {
    let digest = Sha256::digest(secret.as_bytes());
    URL_SAFE_NO_PAD.encode(digest)
}

/// Validate and hash a password with Argon2 using bounded blocking work.
pub async fn hash_password(password: String) -> Result<String, IdentityError> {
    let _permit = PASSWORD_WORK
        .acquire()
        .await
        .map_err(IdentityError::internal)?;
    validate_password(&password)?;
    tokio::task::spawn_blocking(move || {
        Argon2::default()
            .hash_password(password.as_bytes())
            .map(|hash| hash.to_string())
            .map_err(IdentityError::internal)
    })
    .await
    .map_err(IdentityError::internal)?
}

/// Verify a persisted Argon2 password with the shared work limit.
pub async fn verify_password(password: String, encoded: String) -> Result<bool, IdentityError> {
    let _permit = PASSWORD_WORK
        .acquire()
        .await
        .map_err(IdentityError::internal)?;
    tokio::task::spawn_blocking(move || {
        let hash = PasswordHash::new(&encoded).map_err(IdentityError::internal)?;
        Ok(Argon2::default()
            .verify_password(password.as_bytes(), &hash)
            .is_ok())
    })
    .await
    .map_err(IdentityError::internal)?
}

/// Enforce the existing password byte-length bounds.
pub fn validate_password(password: &str) -> Result<(), IdentityError> {
    if password.len() < 12 {
        return Err(IdentityError::bad_request(
            "Use at least 12 characters for the password.",
        ));
    }
    if password.len() > 1024 {
        return Err(IdentityError::bad_request("The password is too long."));
    }
    Ok(())
}

/// Normalize and validate a username or namespace slug.
pub fn validate_slug(value: &str, label: &str) -> Result<String, IdentityError> {
    let normalized = value.trim().to_ascii_lowercase();
    let valid_length = (1..=39).contains(&normalized.len());
    let valid_edges = !normalized.starts_with('-') && !normalized.ends_with('-');
    let valid_chars = normalized
        .bytes()
        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
    if !valid_length || !valid_edges || !valid_chars {
        return Err(IdentityError::bad_request(format!(
            "{label} must use 1 to 39 lowercase letters, numbers, or single hyphens."
        )));
    }
    Ok(normalized)
}

/// Trim a display name and validate its byte-length bounds.
pub fn validate_name(value: &str, label: &str) -> Result<String, IdentityError> {
    let value = value.trim();
    if value.is_empty() || value.len() > 128 {
        return Err(IdentityError::bad_request(format!(
            "{label} must contain 1 to 128 characters."
        )));
    }
    Ok(value.to_owned())
}
