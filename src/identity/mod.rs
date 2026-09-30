mod admin;
mod admin_storage;
mod auth;
mod avatar;
pub mod commit_emails;
mod email;
mod integrations;
mod mirror_identities;
mod notifications;
mod oauth;
mod resources;
mod sso;
mod two_factor;
mod users;

pub(crate) use admin::require_admin;
pub(crate) use email::verified_mailbox;
pub(crate) use integrations::{authorize_namespace, validate_name as validate_integration_name};
pub(crate) use mirror_identities::{
    load_secret as load_mirror_identity_secret, mark_identity_used as mark_repository_identity_used,
};
#[cfg(test)]
pub(crate) use notifications::NotificationPreferences;
pub(crate) use notifications::{NotificationKind, preferences as notification_preferences};
use std::{
    collections::{HashMap, VecDeque},
    future::Future,
    net::IpAddr,
    path::PathBuf,
    sync::{Arc, LazyLock},
    time::{Duration, Instant},
};

use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
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
use rand::RngExt as _;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait,
    PaginatorTrait, QueryFilter, Set, TransactionTrait,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use time::Duration as TimeDuration;
use tokio::sync::{Mutex, Semaphore, mpsc, watch};
use url::Url;
use uuid::Uuid;
use webauthn_rs::{
    Webauthn, WebauthnBuilder,
    prelude::{DiscoverableAuthentication, PasskeyRegistration},
};

use crate::{
    archive::MaintenanceAction,
    backup_provider::{BackupProvider, BackupProviderConfig},
    blob_store::{DomainStorage, targets::MeasuredUsage},
    config::{AuthSettings, Settings},
    entity::{api_token, audit_event, namespace, oauth_access_token, session, ssh_key, user},
};

const SESSION_COOKIE: &str = "gitadel_session";
pub const SCOPE_READ: i32 = 1;
pub const SCOPE_WRITE: i32 = 1 << 1;
pub const SCOPE_SSH_KEYS: i32 = 1 << 2;
pub const SCOPE_REPOSITORY_READ: i32 = 1 << 3;
const VALIDATED_BACKUP_LIFETIME: Duration = Duration::from_secs(30 * 60);
const TESTED_BACKUP_PROVIDER_LIFETIME: Duration = Duration::from_secs(10 * 60);
const CHALLENGE_LIFETIME: Duration = Duration::from_secs(5 * 60);
const AUTH_CHALLENGE_CAPACITY: usize = 1_024;
const AUTH_CHALLENGE_SOURCE_CAPACITY: usize = 16;
const AUTH_RATE_LIMIT_CAPACITY: usize = 4_096;
const AUTH_RATE_LIMIT_WINDOW: Duration = Duration::from_secs(60);
const PASSWORD_WORK_CONCURRENCY: usize = 4;
static PASSWORD_WORK: LazyLock<Semaphore> =
    LazyLock::new(|| Semaphore::new(PASSWORD_WORK_CONCURRENCY));

tokio::task_local! {
    static REMOTE_ADDRESS: Option<IpAddr>;
}

#[derive(Clone)]
pub struct IdentityState {
    database: DatabaseConnection,
    settings: AuthSettings,
    public_url: Url,
    /// Absent when the public URL is an IP address, which WebAuthn rejects.
    webauthn: Option<Webauthn>,
    registration_challenges: Arc<Mutex<HashMap<String, RegistrationChallenge>>>,
    authentication_challenges: Arc<Mutex<HashMap<String, AuthenticationChallenge>>>,
    authorization_requests: Arc<Mutex<HashMap<String, oauth::AuthorizationRequest>>>,
    oidc_authorizations: Arc<sso::OidcAuthorizations>,
    auth_rate_limits: Arc<Mutex<HashMap<String, VecDeque<Instant>>>>,
    pending_logins: Arc<Mutex<HashMap<String, two_factor::PendingLogin>>>,
    /// One-time first-run setup token, printed to the log while no account exists.
    setup_token: Arc<Mutex<Option<String>>>,
    runtime_settings: Option<Arc<Settings>>,
    maintenance_sender: Option<mpsc::Sender<MaintenanceAction>>,
    maintenance_pending: Arc<Mutex<bool>>,
    lfs_storage: Arc<tokio::sync::RwLock<Option<Arc<DomainStorage>>>>,
    registry_storage: Arc<tokio::sync::RwLock<Option<Arc<DomainStorage>>>>,
    validated_backups: Arc<Mutex<HashMap<Uuid, ValidatedBackup>>>,
    tested_backup_providers: Arc<Mutex<HashMap<Uuid, TestedBackupProvider>>>,
    measured_storage: Arc<Mutex<HashMap<Uuid, MeasuredUsage>>>,
    integrity_settings_version: watch::Sender<u64>,
    dummy_password_hash: Arc<String>,
    mailer: crate::mail::Mailer,
}

pub(crate) struct ValidatedBackup {
    pub key: String,
    pub path: PathBuf,
    pub provider: BackupProvider,
    validated_at: Instant,
}

struct TestedBackupProvider {
    user_id: Uuid,
    config: BackupProviderConfig,
    tested_at: Instant,
}

pub struct RegistrationChallenge {
    pub user_id: Uuid,
    pub name: String,
    pub state: PasskeyRegistration,
    pub created_at: Instant,
}

pub struct AuthenticationChallenge {
    pub state: DiscoverableAuthentication,
    pub created_at: Instant,
    pub remote_address: Option<IpAddr>,
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

    pub fn too_many_requests(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::TOO_MANY_REQUESTS,
            code: "rate_limited",
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

pub async fn with_remote_address<F: Future>(address: Option<IpAddr>, future: F) -> F::Output {
    REMOTE_ADDRESS.scope(address, future).await
}

pub fn remote_address() -> Option<IpAddr> {
    REMOTE_ADDRESS.try_with(|address| *address).ok().flatten()
}

impl IdentityState {
    #[cfg(test)]
    pub fn new(
        database: DatabaseConnection,
        settings: AuthSettings,
        public_url: Url,
    ) -> Result<Self, anyhow::Error> {
        Self::build(database, settings, public_url, None, None)
    }

    pub fn new_with_runtime(
        database: DatabaseConnection,
        settings: Settings,
        maintenance_sender: mpsc::Sender<MaintenanceAction>,
    ) -> Result<Self, anyhow::Error> {
        let auth = settings.auth.clone();
        let public_url = settings.server.public_url.clone();
        Self::build(
            database,
            auth,
            public_url,
            Some(Arc::new(settings)),
            Some(maintenance_sender),
        )
    }

    fn build(
        database: DatabaseConnection,
        settings: AuthSettings,
        public_url: Url,
        runtime_settings: Option<Arc<Settings>>,
        maintenance_sender: Option<mpsc::Sender<MaintenanceAction>>,
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
        let webauthn = match public_url.host() {
            Some(url::Host::Domain(rp_id)) => Some(
                WebauthnBuilder::new(rp_id, &public_url)?
                    .rp_name("Gitadel")
                    .build()?,
            ),
            Some(url::Host::Ipv4(_) | url::Host::Ipv6(_)) => {
                tracing::warn!(
                    %public_url,
                    "passkeys are disabled because the public URL is an IP address; use a hostname to enable them"
                );
                None
            }
            None => return Err(anyhow::anyhow!("public URL must contain a host")),
        };

        let dummy_password_hash = Argon2::default()
            .hash_password(b"gitadel-invalid-login")
            .map_err(|error| anyhow::anyhow!("could not prepare password verification: {error}"))?
            .to_string();
        let mailer = match runtime_settings
            .as_deref()
            .and_then(|settings| settings.smtp.as_ref())
        {
            Some(smtp) => crate::mail::Mailer::start(smtp)?,
            None => crate::mail::Mailer::disabled(),
        };
        Ok(Self {
            database,
            settings,
            public_url,
            webauthn,
            registration_challenges: Arc::new(Mutex::new(HashMap::new())),
            authentication_challenges: Arc::new(Mutex::new(HashMap::new())),
            authorization_requests: Arc::new(Mutex::new(HashMap::new())),
            oidc_authorizations: Arc::new(Mutex::new(HashMap::new())),
            auth_rate_limits: Arc::new(Mutex::new(HashMap::new())),
            pending_logins: Arc::new(Mutex::new(HashMap::new())),
            setup_token: Arc::new(Mutex::new(None)),
            runtime_settings,
            maintenance_sender,
            maintenance_pending: Arc::new(Mutex::new(false)),
            lfs_storage: Arc::new(tokio::sync::RwLock::new(None)),
            registry_storage: Arc::new(tokio::sync::RwLock::new(None)),
            validated_backups: Arc::new(Mutex::new(HashMap::new())),
            tested_backup_providers: Arc::new(Mutex::new(HashMap::new())),
            measured_storage: Arc::new(Mutex::new(HashMap::new())),
            integrity_settings_version: watch::channel(0).0,
            dummy_password_hash: Arc::new(dummy_password_hash),
            mailer,
        })
    }

    pub fn database(&self) -> &DatabaseConnection {
        &self.database
    }

    /// Outgoing mail; disabled unless `[smtp]` is configured.
    pub fn mailer(&self) -> &crate::mail::Mailer {
        &self.mailer
    }

    #[cfg(test)]
    pub(crate) fn with_mailer(mut self, mailer: crate::mail::Mailer) -> Self {
        self.mailer = mailer;
        self
    }
    pub(crate) async fn initialize_lfs_storage(
        &self,
        settings: &crate::config::StorageSettings,
    ) -> Result<(), anyhow::Error> {
        let manager = crate::storage::start(&self.database, settings).await?;
        *self.lfs_storage.write().await = Some(manager);
        Ok(())
    }

    pub(crate) async fn initialize_registry_storage(
        &self,
        settings: &crate::config::StorageSettings,
    ) -> Result<(), anyhow::Error> {
        let manager = crate::registry::storage::start(&self.database, settings).await?;
        *self.registry_storage.write().await = Some(manager);
        Ok(())
    }

    pub(crate) async fn registry_storage(&self) -> Option<Arc<DomainStorage>> {
        self.registry_storage.read().await.clone()
    }

    pub(crate) async fn lfs_storage(&self) -> Option<Arc<DomainStorage>> {
        self.lfs_storage.read().await.clone()
    }

    pub(crate) fn subscribe_integrity_settings(&self) -> watch::Receiver<u64> {
        self.integrity_settings_version.subscribe()
    }

    pub(crate) fn notify_integrity_settings_changed(&self) {
        self.integrity_settings_version
            .send_modify(|version| *version = version.wrapping_add(1));
    }

    pub fn public_url(&self) -> &Url {
        &self.public_url
    }

    /// Issues a first-run setup link when the instance has no accounts yet.
    /// The token lives only in memory, so every restart prints a fresh link.
    pub(crate) async fn prepare_setup_link(&self) -> Result<Option<Url>, ApiError> {
        if user::Entity::find().count(&self.database).await? != 0 {
            return Ok(None);
        }
        let token = random_secret(32);
        let mut link = self
            .public_url
            .join("register")
            .map_err(ApiError::internal)?;
        link.query_pairs_mut().append_pair("setup", &token);
        *self.setup_token.lock().await = Some(token);
        Ok(Some(link))
    }

    pub(crate) async fn setup_token_matches(&self, candidate: &str) -> bool {
        use subtle::ConstantTimeEq as _;
        self.setup_token
            .lock()
            .await
            .as_deref()
            .is_some_and(|expected| {
                bool::from(
                    hash_secret(expected)
                        .as_bytes()
                        .ct_eq(hash_secret(candidate).as_bytes()),
                )
            })
    }

    /// Retires the setup link once the first administrator exists.
    pub(crate) async fn clear_setup_token(&self) {
        *self.setup_token.lock().await = None;
    }

    fn oidc_authorizations(&self) -> &sso::OidcAuthorizations {
        &self.oidc_authorizations
    }

    pub fn runtime_settings(&self) -> Result<&Settings, ApiError> {
        self.runtime_settings
            .as_deref()
            .ok_or_else(|| ApiError::internal("runtime settings are unavailable"))
    }

    pub async fn cache_validated_backup(
        &self,
        key: String,
        path: PathBuf,
        provider: BackupProvider,
    ) -> Uuid {
        let mut backups = self.validated_backups.lock().await;
        backups.retain(|_, backup| {
            if backup.validated_at.elapsed() <= VALIDATED_BACKUP_LIFETIME {
                true
            } else {
                let _ = std::fs::remove_file(&backup.path);
                false
            }
        });
        let token = Uuid::new_v4();
        backups.insert(
            token,
            ValidatedBackup {
                key,
                path,
                provider,
                validated_at: Instant::now(),
            },
        );
        token
    }

    pub async fn take_validated_backup(&self, token: Uuid) -> Option<ValidatedBackup> {
        let backup = self.validated_backups.lock().await.remove(&token)?;
        if backup.validated_at.elapsed() > VALIDATED_BACKUP_LIFETIME {
            let _ = std::fs::remove_file(backup.path);
            return None;
        }
        Some(backup)
    }

    pub async fn cache_tested_backup_provider(
        &self,
        user_id: Uuid,
        config: BackupProviderConfig,
    ) -> Uuid {
        let mut tested = self.tested_backup_providers.lock().await;
        tested.retain(|_, proof| proof.tested_at.elapsed() <= TESTED_BACKUP_PROVIDER_LIFETIME);
        let token = Uuid::new_v4();
        tested.insert(
            token,
            TestedBackupProvider {
                user_id,
                config,
                tested_at: Instant::now(),
            },
        );
        token
    }

    pub async fn consume_tested_backup_provider(
        &self,
        token: Uuid,
        user_id: Uuid,
        config: &BackupProviderConfig,
    ) -> bool {
        let Some(proof) = self.tested_backup_providers.lock().await.remove(&token) else {
            return false;
        };
        proof.tested_at.elapsed() <= TESTED_BACKUP_PROVIDER_LIFETIME
            && proof.user_id == user_id
            && proof.config == *config
    }

    /// Scanning a target costs a full listing, so the result is kept until an
    /// administrator asks for a fresh one. It is deliberately not persisted: a
    /// figure that survives a restart would outlive the state it described.
    pub(crate) async fn record_measured_storage(&self, target_id: Uuid, usage: MeasuredUsage) {
        self.measured_storage.lock().await.insert(target_id, usage);
    }

    pub(crate) async fn measured_storage(&self) -> HashMap<Uuid, MeasuredUsage> {
        self.measured_storage.lock().await.clone()
    }

    pub(crate) async fn forget_measured_storage(&self, target_id: Uuid) {
        self.measured_storage.lock().await.remove(&target_id);
    }

    pub async fn schedule_maintenance(&self, action: MaintenanceAction) -> Result<(), ApiError> {
        let sender = self
            .maintenance_sender
            .clone()
            .ok_or_else(|| ApiError::internal("maintenance operations are unavailable"))?;
        let mut pending = self.maintenance_pending.lock().await;
        if *pending {
            return Err(ApiError::conflict(
                "Another backup or restore operation is already starting.",
            ));
        }
        *pending = true;
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(750)).await;
            if sender.send(action).await.is_err() {
                tracing::error!("could not schedule maintenance operation");
            }
        });
        Ok(())
    }

    pub fn webauthn(&self) -> Result<&Webauthn, ApiError> {
        self.webauthn.as_ref().ok_or_else(|| {
            ApiError::bad_request(
                "Passkeys need a public URL with a hostname rather than an IP address.",
            )
        })
    }

    pub fn passkeys_available(&self) -> bool {
        self.webauthn.is_some()
    }

    pub fn session_lifetime(&self) -> ChronoDuration {
        ChronoDuration::hours(self.settings.session_lifetime_hours)
    }

    pub fn invitation_lifetime(&self) -> ChronoDuration {
        ChronoDuration::hours(self.settings.invitation_lifetime_hours)
    }
    pub fn dummy_password_hash(&self) -> String {
        self.dummy_password_hash.as_ref().clone()
    }

    pub async fn enforce_auth_rate_limit(
        &self,
        bucket: &str,
        limit: usize,
    ) -> Result<(), ApiError> {
        let source = remote_address()
            .map(|address| address.to_string())
            .unwrap_or_else(|| "unknown".to_owned());
        self.enforce_rate_limit(format!("source:{source}:{bucket}"), limit)
            .await
    }

    pub async fn enforce_account_auth_rate_limit(
        &self,
        account: &str,
        limit: usize,
    ) -> Result<(), ApiError> {
        self.enforce_rate_limit(format!("account:{account}"), limit)
            .await
    }

    async fn enforce_rate_limit(&self, key: String, limit: usize) -> Result<(), ApiError> {
        let mut limits = self.auth_rate_limits.lock().await;
        let now = Instant::now();
        limits.retain(|_, attempts| {
            attempts.retain(|attempt| now.duration_since(*attempt) < AUTH_RATE_LIMIT_WINDOW);
            !attempts.is_empty()
        });
        if !limits.contains_key(&key) && limits.len() >= AUTH_RATE_LIMIT_CAPACITY {
            return Err(ApiError::too_many_requests(
                "Too many authentication attempts. Try again later.",
            ));
        }
        let attempts = limits.entry(key).or_default();
        if attempts.len() >= limit {
            return Err(ApiError::too_many_requests(
                "Too many authentication attempts. Try again later.",
            ));
        }
        attempts.push_back(now);
        Ok(())
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

    pub async fn insert_authentication_challenge(
        &self,
        id: String,
        state: DiscoverableAuthentication,
    ) -> Result<(), ApiError> {
        let source = remote_address();
        let mut challenges = self.authentication_challenges.lock().await;
        challenges.retain(|_, challenge| challenge.created_at.elapsed() < CHALLENGE_LIFETIME);
        if challenges.len() >= AUTH_CHALLENGE_CAPACITY
            || challenges
                .values()
                .filter(|challenge| challenge.remote_address == source)
                .count()
                >= AUTH_CHALLENGE_SOURCE_CAPACITY
        {
            return Err(ApiError::too_many_requests(
                "Too many authentication attempts. Try again later.",
            ));
        }
        challenges.insert(
            id,
            AuthenticationChallenge {
                state,
                created_at: Instant::now(),
                remote_address: source,
            },
        );
        Ok(())
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
            remote_address: Set(remote_address().map(|address| address.to_string())),
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
    if user::Entity::find().count(database).await? != 0 {
        return Err(ApiError::conflict(
            "The first administrator already exists.",
        ));
    }
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
        default_repository_visibility: Set("private".to_owned()),
        theme_preference: Set("system".to_owned()),
        motion_preference: Set("system".to_owned()),
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
        .route("/auth/oidc/{provider_id}/start", get(sso::start))
        .route("/auth/oidc/{provider_id}/callback", get(sso::callback))
        .route(
            "/admin/authentication",
            get(sso::admin_configuration).put(sso::update_configuration),
        )
        .route(
            "/admin/authentication/providers",
            get(sso::list_providers).post(sso::create_provider),
        )
        .route(
            "/admin/authentication/providers/{provider_id}",
            put(sso::update_provider).delete(sso::delete_provider),
        )
        .route("/instance", get(admin::public_instance_settings))
        .route("/admin/smtp", get(email::smtp_status))
        .route("/admin/smtp/test", post(email::send_test_email))
        .route(
            "/me/email",
            get(email::get_email)
                .put(email::set_email)
                .delete(email::delete_email),
        )
        .route("/me/email/verification", post(email::resend_verification))
        .route("/auth/email/verify", post(email::verify_email))
        .route(
            "/me/notifications",
            get(notifications::get_notifications).put(notifications::update_notifications),
        )
        .route("/auth/password-reset", post(email::request_password_reset))
        .route(
            "/auth/password-reset/confirm",
            post(email::confirm_password_reset),
        )
        .route(
            "/instance/favicon/{theme}",
            get(admin::public_instance_favicon),
        )
        .route(
            "/admin/backup/providers",
            get(admin::list_backup_providers).post(admin::create_backup_provider),
        )
        .route(
            "/admin/backup/providers/test",
            post(admin::test_backup_provider),
        )
        .route(
            "/admin/backup/providers/{provider_id}",
            put(admin::update_backup_provider).delete(admin::delete_backup_provider),
        )
        .route(
            "/admin/backup/providers/{provider_id}/schedule",
            put(admin::update_backup_schedule),
        )
        .route(
            "/admin/backup/providers/{provider_id}/backups",
            get(admin::list_backups).post(admin::create_backup),
        )
        .route(
            "/admin/backup/providers/{provider_id}/backups/object",
            get(admin::download_backup).delete(admin::delete_backup),
        )
        .route(
            "/admin/backups/progress/{operation_id}",
            get(admin::backup_progress),
        )
        .route(
            "/admin/backup/providers/{provider_id}/backups/preflight",
            post(admin::preflight_restore),
        )
        .route(
            "/admin/backup/providers/{provider_id}/backups/restore",
            post(admin::restore_backup),
        )
        .route(
            "/admin/storage/targets",
            get(admin::list_storage_targets).post(admin::create_storage_target),
        )
        .route(
            "/admin/storage/targets/test",
            post(admin::test_storage_target),
        )
        .route("/admin/storage/domains", get(admin_storage::list_domains))
        .route(
            "/admin/storage/domains/{domain}",
            get(admin_storage::domain_status),
        )
        .route(
            "/admin/storage/domains/{domain}/repositories",
            get(admin_storage::domain_repositories),
        )
        .route(
            "/admin/storage/domains/{domain}/migrate",
            post(admin_storage::migrate_domain),
        )
        .route(
            "/admin/storage/domains/{domain}/migrations/{operation_id}",
            get(admin_storage::domain_migration),
        )
        .route(
            "/admin/storage/domains/{domain}/migrations/{operation_id}/events",
            get(admin_storage::domain_migration_events),
        )
        // Deprecated per-domain aliases of the routes above; remove after one
        // release.
        .route(
            "/admin/storage/lfs/status",
            get(admin_storage::legacy::lfs_status),
        )
        .route(
            "/admin/storage/lfs/repositories",
            get(admin_storage::legacy::lfs_repositories),
        )
        .route(
            "/admin/storage/migrations",
            post(admin_storage::legacy::lfs_migrate),
        )
        .route(
            "/admin/storage/progress/{operation_id}",
            get(admin_storage::legacy::lfs_progress),
        )
        .route(
            "/admin/storage/registry/status",
            get(admin_storage::legacy::registry_status),
        )
        .route(
            "/admin/storage/registry/repositories",
            get(admin_storage::legacy::registry_repositories),
        )
        .route(
            "/admin/storage/registry/migrate",
            post(admin_storage::legacy::registry_migrate),
        )
        .route(
            "/admin/storage/registry/migrations/{operation_id}/events",
            get(admin_storage::legacy::registry_events),
        )
        .route(
            "/admin/storage/targets/{target_id}",
            delete(admin::delete_storage_target),
        )
        .route(
            "/admin/storage/targets/{target_id}/test",
            post(admin::test_saved_storage_target),
        )
        .route(
            "/admin/storage/targets/{target_id}/usage",
            post(admin::measure_storage_target),
        )
        .route("/register", post(auth::register))
        .route("/setup", post(auth::setup))
        .route("/auth/login", post(auth::login))
        .route(
            "/auth/login/two-factor",
            post(auth::finish_two_factor_login),
        )
        .route("/auth/logout", post(auth::logout))
        .route("/users/{user_id}/avatar", get(avatar::public_avatar))
        .route(
            "/me/avatar",
            put(avatar::update_avatar)
                .delete(avatar::delete_avatar)
                .layer(DefaultBodyLimit::max(avatar::MAX_AVATAR_REQUEST_BYTES)),
        )
        .route("/me/username", put(auth::update_username))
        .route(
            "/me/repository-preferences",
            put(auth::update_repository_preferences),
        )
        .route("/me/theme-preference", put(auth::update_theme_preference))
        .route("/me/motion-preference", put(auth::update_motion_preference))
        .route(
            "/me/commit-identity",
            get(commit_emails::get_identity).put(commit_emails::update_identity),
        )
        .route("/me/commit-emails", post(commit_emails::add_email))
        .route(
            "/me/commit-emails/{email}",
            delete(commit_emails::remove_email),
        )
        .route("/me/password", put(auth::update_password))
        .route("/me/two-factor", get(two_factor::status))
        .route("/me/two-factor/enroll", post(two_factor::start_enrollment))
        .route(
            "/me/two-factor/confirm",
            post(two_factor::confirm_enrollment),
        )
        .route(
            "/me/two-factor/recovery-codes",
            post(two_factor::regenerate_recovery_codes),
        )
        .route("/me/two-factor/disable", post(two_factor::disable))
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
        .route(
            "/invitations",
            get(users::list_invitations).post(auth::create_invitation),
        )
        .route("/invitations/{id}", delete(users::revoke_invitation))
        .route("/admin/users", get(users::list_users))
        .route("/admin/users/{username}", delete(users::delete_user))
        .route("/admin/users/{username}/disable", post(users::disable_user))
        .route("/admin/users/{username}/enable", post(users::enable_user))
        .route(
            "/admin/users/{username}/two-factor",
            delete(users::reset_two_factor),
        )
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
            "/organizations/{slug}",
            put(resources::update_organization).delete(resources::delete_organization),
        )
        .route("/namespaces/{slug}", get(resources::get_namespace))
        .route(
            "/organizations/{slug}/avatar",
            get(avatar::public_organization_avatar)
                .put(avatar::update_organization_avatar)
                .delete(avatar::delete_organization_avatar)
                .layer(DefaultBodyLimit::max(avatar::MAX_AVATAR_REQUEST_BYTES)),
        )
        .route(
            "/organizations/{slug}/members",
            get(resources::list_members).post(resources::add_member),
        )
        .route(
            "/organizations/{slug}/member-suggestions",
            get(resources::list_member_suggestions),
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
            "/admin/integrity",
            get(admin::get_integrity_settings).put(admin::update_integrity_settings),
        )
        .route(
            "/admin/instance/favicon/{theme}",
            put(admin::update_instance_favicon)
                .delete(admin::delete_instance_favicon)
                .layer(DefaultBodyLimit::max(admin::MAX_FAVICON_BYTES)),
        )
        .route(
            "/namespaces/{slug}/integrations",
            get(integrations::list_integrations).post(integrations::create_integration),
        )
        .route(
            "/namespaces/{slug}/integrations/test",
            post(integrations::test_new_integration),
        )
        .route(
            "/namespaces/{slug}/integrations/{id}",
            put(integrations::update_integration).delete(integrations::delete_integration),
        )
        .route(
            "/namespaces/{slug}/integrations/{id}/credential",
            get(integrations::get_integration_credential),
        )
        .route(
            "/namespaces/{slug}/integrations/{id}/test",
            post(integrations::test_integration),
        )
        .route(
            "/namespaces/{slug}/integrations/{id}/source",
            get(integrations::get_integration_source)
                .post(integrations::configure_integration_source)
                .delete(integrations::disconnect_integration_source),
        )
        .merge(mirror_identities::router())
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
    rand::rng().fill(&mut data);
    URL_SAFE_NO_PAD.encode(data)
}

pub fn hash_secret(secret: &str) -> String {
    let digest = Sha256::digest(secret.as_bytes());
    URL_SAFE_NO_PAD.encode(digest)
}

pub async fn hash_password(password: String) -> Result<String, ApiError> {
    let _permit = PASSWORD_WORK.acquire().await.map_err(ApiError::internal)?;
    validate_password(&password)?;
    tokio::task::spawn_blocking(move || {
        Argon2::default()
            .hash_password(password.as_bytes())
            .map(|hash| hash.to_string())
            .map_err(ApiError::internal)
    })
    .await
    .map_err(ApiError::internal)?
}

pub async fn verify_password(password: String, encoded: String) -> Result<bool, ApiError> {
    let _permit = PASSWORD_WORK.acquire().await.map_err(ApiError::internal)?;
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
    pub default_repository_visibility: String,
    pub theme_preference: String,
    pub motion_preference: String,
    pub avatar_updated_at: Option<chrono::DateTime<Utc>>,
}

impl From<user::Model> for UserResponse {
    fn from(user: user::Model) -> Self {
        Self {
            id: user.id,
            username: user.username,
            is_admin: user.is_admin,
            default_repository_visibility: user.default_repository_visibility,
            theme_preference: user.theme_preference,
            motion_preference: user.motion_preference,
            avatar_updated_at: user.avatar_updated_at,
        }
    }
}

#[derive(Deserialize)]
pub struct Pagination {
    pub limit: Option<u64>,
}
