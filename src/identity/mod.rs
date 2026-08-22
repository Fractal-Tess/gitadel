mod admin;
mod auth;
mod avatar;
mod oauth;
mod resources;

use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier, password_hash::SaltString};
use axum::{
    Json, Router,
    extract::DefaultBodyLimit,
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{delete, get, post, put},
};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{Duration as ChronoDuration, Utc};
use rand::RngCore;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait,
    PaginatorTrait, QueryFilter, Set, TransactionTrait,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use time::Duration as TimeDuration;
use tokio::sync::Mutex;
use url::Url;
use uuid::Uuid;
use webauthn_rs::{
    Webauthn, WebauthnBuilder,
    prelude::{PasskeyAuthentication, PasskeyRegistration},
};

use crate::{
    config::AuthSettings,
    entity::{api_token, audit_event, namespace, oauth_access_token, session, ssh_key, user},
};

const SESSION_COOKIE: &str = "gitadel_session";
pub const SCOPE_READ: i32 = 1;
pub const SCOPE_WRITE: i32 = 1 << 1;
pub const SCOPE_SSH_KEYS: i32 = 1 << 2;
pub const SCOPE_REPOSITORY_READ: i32 = 1 << 3;
const CHALLENGE_LIFETIME: Duration = Duration::from_secs(5 * 60);

#[derive(Clone)]
pub struct IdentityState {
    database: DatabaseConnection,
    settings: AuthSettings,
    public_url: Url,
    webauthn: Webauthn,
    registration_challenges: Arc<Mutex<HashMap<String, RegistrationChallenge>>>,
    authentication_challenges: Arc<Mutex<HashMap<String, AuthenticationChallenge>>>,
    authorization_requests: Arc<Mutex<HashMap<String, oauth::AuthorizationRequest>>>,
}

pub struct RegistrationChallenge {
    pub user_id: Uuid,
    pub name: String,
    pub state: PasskeyRegistration,
    pub created_at: Instant,
}

pub struct AuthenticationChallenge {
    pub user_id: Uuid,
    pub state: PasskeyAuthentication,
    pub created_at: Instant,
}

#[derive(Clone)]
pub struct AuthenticatedUser {
    pub user: user::Model,
    pub via_api_token: bool,
}

#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
}

#[derive(Serialize)]
struct ErrorEnvelope {
    error: ErrorBody,
}

#[derive(Serialize)]
struct ErrorBody {
    code: &'static str,
    message: String,
}

impl ApiError {
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code: "bad_request",
            message: message.into(),
        }
    }

    pub fn unauthorized() -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            code: "unauthorized",
            message: "Sign in to continue.".to_owned(),
        }
    }

    pub fn forbidden(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            code: "forbidden",
            message: message.into(),
        }
    }

    pub fn not_found() -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            code: "not_found",
            message: "Not found.".to_owned(),
        }
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            code: "conflict",
            message: message.into(),
        }
    }

    pub fn internal(error: impl std::fmt::Display) -> Self {
        tracing::error!(%error, "identity request failed");
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "internal_error",
            message: "The request could not be completed.".to_owned(),
        }
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ApiError {}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(ErrorEnvelope {
                error: ErrorBody {
                    code: self.code,
                    message: self.message,
                },
            }),
        )
            .into_response()
    }
}

impl From<sea_orm::DbErr> for ApiError {
    fn from(error: sea_orm::DbErr) -> Self {
        Self::internal(error)
    }
}

impl IdentityState {
    pub fn new(
        database: DatabaseConnection,
        settings: AuthSettings,
        public_url: Url,
    ) -> Result<Self, anyhow::Error> {
        if public_url.path() != "/"
            || public_url.query().is_some()
            || public_url.fragment().is_some()
            || !public_url.username().is_empty()
            || public_url.password().is_some()
        {
            return Err(anyhow::anyhow!(
                "public URL must be an origin without a path, query, fragment, or credentials"
            ));
        }
        let rp_id = public_url
            .host_str()
            .ok_or_else(|| anyhow::anyhow!("public URL must contain a host"))?;
        let webauthn = WebauthnBuilder::new(rp_id, &public_url)?
            .rp_name("Gitadel")
            .build()?;

        Ok(Self {
            database,
            settings,
            public_url,
            webauthn,
            registration_challenges: Arc::new(Mutex::new(HashMap::new())),
            authentication_challenges: Arc::new(Mutex::new(HashMap::new())),
            authorization_requests: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    pub fn database(&self) -> &DatabaseConnection {
        &self.database
    }

    pub fn webauthn(&self) -> &Webauthn {
        &self.webauthn
    }

    pub fn session_lifetime(&self) -> ChronoDuration {
        ChronoDuration::hours(self.settings.session_lifetime_hours)
    }

    pub fn invitation_lifetime(&self) -> ChronoDuration {
        ChronoDuration::hours(self.settings.invitation_lifetime_hours)
    }

    pub async fn registration_challenges(
        &self,
    ) -> tokio::sync::MutexGuard<'_, HashMap<String, RegistrationChallenge>> {
        let mut challenges = self.registration_challenges.lock().await;
        challenges.retain(|_, challenge| challenge.created_at.elapsed() < CHALLENGE_LIFETIME);
        challenges
    }

    pub async fn authentication_challenges(
        &self,
    ) -> tokio::sync::MutexGuard<'_, HashMap<String, AuthenticationChallenge>> {
        let mut challenges = self.authentication_challenges.lock().await;
        challenges.retain(|_, challenge| challenge.created_at.elapsed() < CHALLENGE_LIFETIME);
        challenges
    }

    async fn authorization_requests(
        &self,
    ) -> tokio::sync::MutexGuard<'_, HashMap<String, oauth::AuthorizationRequest>> {
        let mut requests = self.authorization_requests.lock().await;
        requests.retain(|_, request| request.created_at.elapsed() < CHALLENGE_LIFETIME);
        requests
    }

    pub async fn authenticate(
        &self,
        headers: &HeaderMap,
        jar: &CookieJar,
        required_scope: i32,
    ) -> Result<AuthenticatedUser, ApiError> {
        if let Some(authorization) = headers.get(header::AUTHORIZATION) {
            let authorization = authorization
                .to_str()
                .map_err(|_| ApiError::unauthorized())?;
            let token = authorization
                .strip_prefix("Bearer ")
                .or_else(|| authorization.strip_prefix("token "))
                .ok_or_else(ApiError::unauthorized)?;
            return self.authenticate_token(token, required_scope).await;
        }

        let raw_token = jar
            .get(SESSION_COOKIE)
            .map(Cookie::value)
            .ok_or_else(ApiError::unauthorized)?;
        let token_hash = hash_secret(raw_token);
        let now = Utc::now();
        let stored = session::Entity::find_by_id(token_hash)
            .one(&self.database)
            .await?
            .filter(|session| session.expires_at > now)
            .ok_or_else(ApiError::unauthorized)?;
        let account = self
            .enabled_user(stored.user_id)
            .await?
            .ok_or_else(ApiError::unauthorized)?;

        if now - stored.last_seen_at > ChronoDuration::minutes(5) {
            let mut active: session::ActiveModel = stored.into();
            active.last_seen_at = Set(now);
            active.update(&self.database).await?;
        }

        Ok(AuthenticatedUser {
            user: account,
            via_api_token: false,
        })
    }

    pub async fn authenticate_token(
        &self,
        token: &str,
        required_scope: i32,
    ) -> Result<AuthenticatedUser, ApiError> {
        let token_hash = hash_secret(token);
        let now = Utc::now();
        if let Some(stored) = api_token::Entity::find()
            .filter(api_token::Column::TokenHash.eq(&token_hash))
            .filter(api_token::Column::RevokedAt.is_null())
            .one(&self.database)
            .await?
            .filter(|token| token.expires_at.is_none_or(|expires_at| expires_at > now))
        {
            let required_api_scope = if required_scope & SCOPE_REPOSITORY_READ != 0 {
                (required_scope & !SCOPE_REPOSITORY_READ) | SCOPE_READ
            } else {
                required_scope
            };
            if stored.scopes & required_api_scope != required_api_scope {
                return Err(ApiError::forbidden(
                    "The API token does not have the required scope.",
                ));
            }
            let account = self
                .enabled_user(stored.user_id)
                .await?
                .ok_or_else(ApiError::unauthorized)?;
            if stored
                .last_used_at
                .is_none_or(|last_used_at| now - last_used_at > ChronoDuration::minutes(5))
            {
                let mut active: api_token::ActiveModel = stored.into();
                active.last_used_at = Set(Some(now));
                active.update(&self.database).await?;
            }
            return Ok(AuthenticatedUser {
                user: account,
                via_api_token: true,
            });
        }

        let stored = oauth_access_token::Entity::find_by_id(token_hash)
            .filter(oauth_access_token::Column::RevokedAt.is_null())
            .one(&self.database)
            .await?
            .ok_or_else(ApiError::unauthorized)?;
        if stored.scopes & required_scope != required_scope {
            return Err(ApiError::forbidden(
                "The OAuth token does not have the required scope.",
            ));
        }
        let account = self
            .enabled_user(stored.user_id)
            .await?
            .ok_or_else(ApiError::unauthorized)?;
        if stored
            .last_used_at
            .is_none_or(|last_used_at| now - last_used_at > ChronoDuration::minutes(5))
        {
            let mut active: oauth_access_token::ActiveModel = stored.into();
            active.last_used_at = Set(Some(now));
            active.update(&self.database).await?;
        }
        Ok(AuthenticatedUser {
            user: account,
            via_api_token: true,
        })
    }

    pub async fn authenticate_ssh_key(
        &self,
        fingerprint: &str,
    ) -> Result<Option<user::Model>, ApiError> {
        let Some(stored) = ssh_key::Entity::find()
            .filter(ssh_key::Column::Fingerprint.eq(fingerprint))
            .one(&self.database)
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
            active.update(&self.database).await?;
        }
        Ok(Some(account))
    }

    pub async fn session_user(&self, jar: &CookieJar) -> Result<Option<user::Model>, ApiError> {
        let Some(raw_token) = jar.get(SESSION_COOKIE).map(Cookie::value) else {
            return Ok(None);
        };
        let token_hash = hash_secret(raw_token);
        let now = Utc::now();
        let Some(stored) = session::Entity::find_by_id(token_hash)
            .one(&self.database)
            .await?
            .filter(|session| session.expires_at > now)
        else {
            return Ok(None);
        };
        self.enabled_user(stored.user_id).await
    }

    pub async fn optional_user(
        &self,
        headers: &HeaderMap,
        jar: &CookieJar,
        required_scope: i32,
    ) -> Result<Option<user::Model>, ApiError> {
        if headers.get(header::AUTHORIZATION).is_some() {
            return self
                .authenticate(headers, jar, required_scope)
                .await
                .map(|actor| Some(actor.user));
        }
        self.session_user(jar).await
    }

    async fn enabled_user(&self, id: Uuid) -> Result<Option<user::Model>, ApiError> {
        Ok(user::Entity::find_by_id(id)
            .filter(user::Column::DisabledAt.is_null())
            .one(&self.database)
            .await?)
    }

    pub async fn create_session(
        &self,
        user_id: Uuid,
    ) -> Result<(String, Cookie<'static>), ApiError> {
        self.create_session_on(&self.database, user_id).await
    }

    pub async fn create_session_on<C: ConnectionTrait>(
        &self,
        connection: &C,
        user_id: Uuid,
    ) -> Result<(String, Cookie<'static>), ApiError> {
        let raw_token = random_secret(32);
        let token_hash = hash_secret(&raw_token);
        let now = Utc::now();
        let expires_at = now + self.session_lifetime();
        session::ActiveModel {
            token_hash: Set(token_hash),
            user_id: Set(user_id),
            expires_at: Set(expires_at),
            created_at: Set(now),
            last_seen_at: Set(now),
        }
        .insert(connection)
        .await?;

        let cookie = Cookie::build((SESSION_COOKIE, raw_token.clone()))
            .path("/")
            .http_only(true)
            .same_site(SameSite::Lax)
            .secure(self.public_url.scheme() == "https")
            .max_age(TimeDuration::hours(self.settings.session_lifetime_hours))
            .build();
        Ok((raw_token, cookie))
    }

    pub async fn delete_session(&self, jar: &CookieJar) -> Result<Cookie<'static>, ApiError> {
        if let Some(raw_token) = jar.get(SESSION_COOKIE).map(Cookie::value) {
            session::Entity::delete_by_id(hash_secret(raw_token))
                .exec(&self.database)
                .await?;
        }
        Ok(Cookie::build(SESSION_COOKIE).path("/").build())
    }

    pub async fn audit(
        &self,
        actor_user_id: Option<Uuid>,
        action: impl Into<String>,
        target: Option<String>,
    ) -> Result<(), ApiError> {
        self.audit_on(&self.database, actor_user_id, action, target)
            .await
    }

    pub async fn audit_on<C: ConnectionTrait>(
        &self,
        connection: &C,
        actor_user_id: Option<Uuid>,
        action: impl Into<String>,
        target: Option<String>,
    ) -> Result<(), ApiError> {
        audit_event::ActiveModel {
            id: sea_orm::ActiveValue::NotSet,
            actor_user_id: Set(actor_user_id),
            action: Set(action.into()),
            target: Set(target),
            remote_address: Set(None),
            created_at: Set(Utc::now()),
        }
        .insert(connection)
        .await?;
        Ok(())
    }
}

pub async fn bootstrap_admin(
    database: &DatabaseConnection,
    username: &str,
    password: String,
) -> Result<user::Model, ApiError> {
    let username = validate_slug(username, "Username")?;
    let password_hash = hash_password(password).await?;
    let transaction = database.begin().await?;
    if user::Entity::find().count(&transaction).await? != 0 {
        return Err(ApiError::conflict(
            "The first administrator already exists.",
        ));
    }
    let now = Utc::now();
    let account = user::ActiveModel {
        id: Set(Uuid::new_v4()),
        username: Set(username.clone()),
        password_hash: Set(password_hash),
        is_admin: Set(true),
        disabled_at: Set(None),
        avatar_updated_at: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(&transaction)
    .await?;
    namespace::ActiveModel {
        slug: Set(username.clone()),
        kind: Set("user".to_owned()),
        user_id: Set(Some(account.id)),
        organization_id: Set(None),
        created_at: Set(now),
    }
    .insert(&transaction)
    .await?;
    audit_event::ActiveModel {
        id: sea_orm::ActiveValue::NotSet,
        actor_user_id: Set(Some(account.id)),
        action: Set("admin.bootstrap".to_owned()),
        target: Set(Some(username)),
        remote_address: Set(None),
        created_at: Set(now),
    }
    .insert(&transaction)
    .await?;
    transaction.commit().await?;
    Ok(account)
}

pub fn router() -> Router<IdentityState> {
    Router::new()
        .route("/auth/status", get(auth::status))
        .route("/instance", get(admin::public_instance_settings))
        .route(
            "/instance/favicon/{theme}",
            get(admin::public_instance_favicon),
        )
        .route("/setup", post(auth::setup))
        .route("/register", post(auth::register))
        .route("/auth/login", post(auth::login))
        .route("/auth/logout", post(auth::logout))
        .route("/users/{user_id}/avatar", get(avatar::public_avatar))
        .route(
            "/me/avatar",
            put(avatar::update_avatar)
                .delete(avatar::delete_avatar)
                .layer(DefaultBodyLimit::max(avatar::MAX_AVATAR_REQUEST_BYTES)),
        )
        .route("/me/username", put(auth::update_username))
        .route("/me/password", put(auth::update_password))
        .route(
            "/auth/passkeys/login/start",
            post(auth::start_passkey_login),
        )
        .route(
            "/auth/passkeys/login/finish",
            post(auth::finish_passkey_login),
        )
        .route("/me/passkeys", get(auth::list_passkeys))
        .route("/me/passkeys/{id}", delete(auth::delete_passkey))
        .route(
            "/me/passkeys/register/start",
            post(auth::start_passkey_registration),
        )
        .route(
            "/me/passkeys/register/finish",
            post(auth::finish_passkey_registration),
        )
        .route("/invitations", post(auth::create_invitation))
        .route(
            "/me/ssh-keys",
            get(resources::list_ssh_keys).post(resources::create_ssh_key),
        )
        .route("/me/ssh-keys/{id}", delete(resources::delete_ssh_key))
        .route(
            "/me/tokens",
            get(resources::list_tokens).post(resources::create_token),
        )
        .route("/me/tokens/{id}", delete(resources::revoke_token))
        .route(
            "/me/oauth-applications",
            get(oauth::list_applications).post(oauth::create_application),
        )
        .route(
            "/me/oauth-applications/{id}",
            delete(oauth::delete_application),
        )
        .route(
            "/organizations",
            get(resources::list_organizations).post(resources::create_organization),
        )
        .route(
            "/organizations/{slug}/members",
            get(resources::list_members).post(resources::add_member),
        )
        .route(
            "/organizations/{slug}/members/{username}",
            delete(resources::remove_member),
        )
        .route("/audit", get(resources::list_audit))
        .route(
            "/admin/instance",
            get(admin::get_instance_settings).put(admin::update_instance_settings),
        )
        .route(
            "/admin/instance/favicon/{theme}",
            put(admin::update_instance_favicon)
                .delete(admin::delete_instance_favicon)
                .layer(DefaultBodyLimit::max(admin::MAX_FAVICON_BYTES)),
        )
}

pub fn oauth_router() -> Router<IdentityState> {
    Router::new()
        .route(
            "/user/settings/applications",
            get(oauth::applications_settings_redirect),
        )
        .route(
            "/login/oauth/authorize",
            get(oauth::authorize).post(oauth::approve),
        )
        .route("/login/oauth/access_token", post(oauth::access_token))
}

pub fn random_secret(bytes: usize) -> String {
    let mut data = vec![0_u8; bytes];
    rand::rng().fill_bytes(&mut data);
    URL_SAFE_NO_PAD.encode(data)
}

pub fn hash_secret(secret: &str) -> String {
    let digest = Sha256::digest(secret.as_bytes());
    URL_SAFE_NO_PAD.encode(digest)
}

pub async fn hash_password(password: String) -> Result<String, ApiError> {
    validate_password(&password)?;
    tokio::task::spawn_blocking(move || {
        let mut salt = [0_u8; 16];
        rand::rng().fill_bytes(&mut salt);
        let salt = SaltString::encode_b64(&salt).map_err(ApiError::internal)?;
        Argon2::default()
            .hash_password(password.as_bytes(), &salt)
            .map(|hash| hash.to_string())
            .map_err(ApiError::internal)
    })
    .await
    .map_err(ApiError::internal)?
}

pub async fn verify_password(password: String, encoded: String) -> Result<bool, ApiError> {
    tokio::task::spawn_blocking(move || {
        let hash = PasswordHash::new(&encoded).map_err(ApiError::internal)?;
        Ok(Argon2::default()
            .verify_password(password.as_bytes(), &hash)
            .is_ok())
    })
    .await
    .map_err(ApiError::internal)?
}

pub fn validate_password(password: &str) -> Result<(), ApiError> {
    if password.len() < 12 {
        return Err(ApiError::bad_request(
            "Use at least 12 characters for the password.",
        ));
    }
    if password.len() > 1024 {
        return Err(ApiError::bad_request("The password is too long."));
    }
    Ok(())
}

pub fn validate_slug(value: &str, label: &str) -> Result<String, ApiError> {
    let normalized = value.trim().to_ascii_lowercase();
    let valid_length = (1..=39).contains(&normalized.len());
    let valid_edges = !normalized.starts_with('-') && !normalized.ends_with('-');
    let valid_chars = normalized
        .bytes()
        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
    if !valid_length || !valid_edges || !valid_chars {
        return Err(ApiError::bad_request(format!(
            "{label} must use 1 to 39 lowercase letters, numbers, or single hyphens."
        )));
    }
    Ok(normalized)
}

pub fn validate_name(value: &str, label: &str) -> Result<String, ApiError> {
    let value = value.trim();
    if value.is_empty() || value.len() > 128 {
        return Err(ApiError::bad_request(format!(
            "{label} must contain 1 to 128 characters."
        )));
    }
    Ok(value.to_owned())
}

#[derive(Serialize)]
pub struct UserResponse {
    pub id: Uuid,
    pub username: String,
    pub is_admin: bool,
    pub avatar_updated_at: Option<chrono::DateTime<Utc>>,
}

impl From<user::Model> for UserResponse {
    fn from(user: user::Model) -> Self {
        Self {
            id: user.id,
            username: user.username,
            is_admin: user.is_admin,
            avatar_updated_at: user.avatar_updated_at,
        }
    }
}

#[derive(Deserialize)]
pub struct Pagination {
    pub limit: Option<u64>,
}
