use crate::openapi::ContractRouterExt;

use utoipa_axum::router::UtoipaMethodRouterExt;
mod admin;
pub(crate) mod admin_storage;
pub(crate) mod auth;
pub(crate) mod avatar;
pub mod commit_emails;
pub(crate) mod email;
pub(crate) mod integrations;
mod mirror_identities;
pub(crate) mod notifications;
pub(crate) mod oauth;
pub(crate) mod resources;
mod sso;
#[cfg(test)]
pub(crate) mod test_support;
pub(crate) mod two_factor;
pub(crate) mod users;

pub(crate) use admin::{require_admin, require_admin_principal};
pub(crate) use email::verified_mailbox;
pub(crate) use integrations::{
    authorize_namespace, open_api_key as open_integration_api_key,
    validate_name as validate_integration_name,
};
pub(crate) use mirror_identities::{
    load_secret as load_mirror_identity_secret, mark_identity_used as mark_repository_identity_used,
};
#[cfg(test)]
pub(crate) use notifications::NotificationPreferences;
pub(crate) use notifications::{NotificationKind, preferences as notification_preferences};
use std::{future::Future, net::IpAddr, sync::Arc};

use axum::{
    Json, Router,
    extract::DefaultBodyLimit,
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
#[cfg(test)]
use chrono::Duration as ChronoDuration;
use chrono::Utc;
use sea_orm::{ConnectionTrait, DatabaseConnection};
use serde::{Deserialize, Serialize};
use time::Duration as TimeDuration;
#[cfg(test)]
use tokio::sync::mpsc;
use tokio::sync::watch;
use url::Url;
use utoipa::ToSchema;
use uuid::Uuid;

#[cfg(test)]
use crate::{archive::MaintenanceAction, config::AuthSettings};
use crate::{blob_store::DomainStorage, config::Settings, entity::user, secrets::SecretCipher};

pub(crate) const SESSION_COOKIE: &str = "gitadel_session";
#[cfg(test)]
pub use gitadel_identity::random_secret;
pub use gitadel_identity::{
    AuthenticatedUser, SCOPE_READ, SCOPE_REPOSITORY_READ, SCOPE_SSH_KEYS, SCOPE_WRITE, hash_secret,
};
tokio::task_local! {
    static REMOTE_ADDRESS: Option<IpAddr>;
}

#[derive(Clone)]
pub struct IdentityState {
    pub storage_runtime: Arc<gitadel_storage::service::StorageRuntime>,
    pub backup_runtime: Arc<gitadel_backups::service::BackupRuntime>,
    pub auth_runtime: Arc<gitadel_identity::runtime::AuthRuntime>,
    database: DatabaseConnection,
    /// Read-only pool for hot read paths; the writer itself until
    /// [`Self::with_read_database`] supplies one.
    read_database: DatabaseConnection,
    /// Absent when the public URL is an IP address, which WebAuthn rejects.
    /// One-time first-run setup token, printed to the log while no account exists.
    runtime_settings: Option<Arc<Settings>>,
    integrity_settings_version: watch::Sender<u64>,
}

#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
}

#[derive(Serialize, utoipa::ToSchema)]
pub(crate) struct ErrorEnvelope {
    error: ErrorBody,
}

#[derive(Serialize, utoipa::ToSchema)]
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

    /// A remote service refused or failed the request. The message is shown
    /// to the person who asked, so it must not contain secrets.
    pub fn bad_gateway(message: impl Into<String>) -> Self {
        let message = message.into();
        tracing::warn!(%message, "remote service request failed");
        Self {
            status: StatusCode::BAD_GATEWAY,
            code: "remote_failed",
            message,
        }
    }

    pub fn internal(error: impl std::fmt::Display) -> Self {
        tracing::error!(%error, "request failed with an internal error");
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "internal_error",
            message: "The request could not be completed.".to_owned(),
        }
    }
}

impl From<gitadel_identity::IdentityError> for ApiError {
    fn from(error: gitadel_identity::IdentityError) -> Self {
        use gitadel_identity::IdentityError;
        match error {
            IdentityError::BadRequest(message) => Self::bad_request(message),
            IdentityError::Unauthorized => Self::unauthorized(),
            IdentityError::InvalidCredentials => Self {
                status: StatusCode::UNAUTHORIZED,
                code: "invalid_credentials",
                message: "The username or credential was not accepted.".to_owned(),
            },
            IdentityError::InvalidTwoFactorCode => Self {
                status: StatusCode::UNAUTHORIZED,
                code: "invalid_two_factor_code",
                message: "The authentication code was not accepted.".to_owned(),
            },
            IdentityError::RateLimited(message) => Self::too_many_requests(message),
            IdentityError::NotFound => Self::not_found(),
            IdentityError::Forbidden(message) => Self::forbidden(message),
            IdentityError::Conflict(message) => Self::conflict(message),
            IdentityError::Internal(message) => Self::internal(message),
            IdentityError::SecretOpen(error) => Self::from(error),
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

impl From<crate::secrets::OpenError> for ApiError {
    /// A stored secret sealed under a different host key. The message names
    /// the setting so the operator knows what to enter again.
    fn from(error: crate::secrets::OpenError) -> Self {
        tracing::error!(%error, "could not open a stored secret");
        Self::conflict(error.to_string())
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

impl std::ops::Deref for IdentityState {
    type Target = gitadel_identity::runtime::AuthRuntime;
    fn deref(&self) -> &Self::Target {
        &self.auth_runtime
    }
}
impl IdentityState {
    /// Assemble HTTP state from server-owned service handles without starting workers.
    pub fn from_services(
        database: DatabaseConnection,
        auth_runtime: Arc<gitadel_identity::runtime::AuthRuntime>,
        backup_runtime: Arc<gitadel_backups::service::BackupRuntime>,
        storage_runtime: Arc<gitadel_storage::service::StorageRuntime>,
        integrity_settings_version: watch::Sender<u64>,
        runtime_settings: Option<Arc<Settings>>,
    ) -> Self {
        Self {
            read_database: database.clone(),
            database,
            auth_runtime,
            backup_runtime,
            storage_runtime,
            integrity_settings_version,
            runtime_settings,
        }
    }

    pub fn instance_service(&self) -> gitadel_instance::service::InstanceService<'_> {
        gitadel_instance::service::InstanceService::new(
            self.database(),
            gitadel_core::audit::AuditContext {
                remote_address: remote_address(),
            },
            &self.integrity_settings_version,
        )
    }
    pub fn backup_service(&self) -> gitadel_backups::service::BackupService<'_> {
        self.backup_runtime
            .service(gitadel_core::audit::AuditContext {
                remote_address: remote_address(),
            })
    }
    pub fn storage_service(&self) -> gitadel_storage::service::StorageService<'_> {
        self.storage_runtime
            .service(gitadel_core::audit::AuditContext {
                remote_address: remote_address(),
            })
    }
    pub fn auth_service(&self) -> gitadel_identity::runtime::AuthService<'_> {
        self.auth_runtime
            .service(gitadel_core::audit::AuditContext {
                remote_address: remote_address(),
            })
    }

    pub fn account_service(&self) -> gitadel_identity::AccountService<'_> {
        gitadel_identity::AccountService::new(
            self.database(),
            gitadel_core::audit::AuditContext {
                remote_address: remote_address(),
            },
        )
    }
    #[cfg(test)]
    pub fn new(
        database: DatabaseConnection,
        settings: AuthSettings,
        public_url: Url,
    ) -> Result<Self, anyhow::Error> {
        Self::build(database, settings, public_url, None, None)
    }

    #[cfg(test)]
    pub fn new_with_runtime(
        database: DatabaseConnection,
        settings: Settings,
        maintenance_sender: mpsc::Sender<MaintenanceAction>,
    ) -> Result<Self, anyhow::Error> {
        let auth = settings.auth.clone();
        let public_url = settings.server.public_url.clone();
        let state = Self::build(
            database,
            auth,
            public_url,
            Some(Arc::new(settings)),
            Some(maintenance_sender),
        )?;
        Ok(state)
    }

    #[cfg(test)]
    fn build(
        database: DatabaseConnection,
        settings: AuthSettings,
        public_url: Url,
        runtime_settings: Option<Arc<Settings>>,
        maintenance_sender: Option<mpsc::Sender<MaintenanceAction>>,
    ) -> Result<Self, anyhow::Error> {
        let mailer = match runtime_settings
            .as_deref()
            .and_then(|settings| settings.smtp.as_ref())
        {
            Some(smtp) => crate::mail::Mailer::start(smtp)?,
            None => crate::mail::Mailer::disabled(),
        };
        let secret_cipher = match runtime_settings.as_deref() {
            Some(settings) => {
                let host_key = crate::repository::load_ssh_host_key(&settings.ssh.host_key)?;
                SecretCipher::from_host_key(&host_key)
                    .map_err(|error| anyhow::anyhow!("could not derive the secret key: {error}"))?
            }
            // Only tests build a state without runtime settings.
            None => SecretCipher::from_material(b"gitadel test secret cipher")?,
        };
        let auth_runtime = Arc::new(gitadel_identity::runtime::AuthRuntime::new(
            database.clone(),
            settings,
            public_url,
            mailer,
            Arc::new(secret_cipher),
        )?);
        let backup_runtime = Arc::new(gitadel_backups::service::BackupRuntime::new(
            database.clone(),
            runtime_settings.clone(),
            maintenance_sender,
        ));
        let storage_runtime = Arc::new(gitadel_storage::service::StorageRuntime::new(
            database.clone(),
            runtime_settings.clone(),
            None,
            None,
        ));
        Ok(Self {
            storage_runtime,
            backup_runtime,
            auth_runtime,
            read_database: database.clone(),
            database,
            runtime_settings,
            integrity_settings_version: watch::channel(0).0,
        })
    }

    /// Seals and opens stored credentials.
    pub(crate) fn secret_cipher(&self) -> &SecretCipher {
        &self.secret_cipher
    }

    pub fn database(&self) -> &DatabaseConnection {
        &self.database
    }

    /// The read-only pool. Reads that must observe an uncommitted write of
    /// their own transaction stay on [`Self::database`].
    pub fn read_database(&self) -> &DatabaseConnection {
        &self.read_database
    }

    /// Route hot read paths through `read_database`, a pool opened by
    /// [`gitadel_db::connect_reader`].
    pub fn with_read_database(mut self, read_database: DatabaseConnection) -> Self {
        self.read_database = read_database;
        self
    }

    /// Outgoing mail; disabled unless `[smtp]` is configured.
    pub fn mailer(&self) -> &crate::mail::Mailer {
        &self.mailer
    }

    #[cfg(test)]
    pub(crate) fn with_mailer(mut self, mailer: crate::mail::Mailer) -> Self {
        Arc::make_mut(&mut self.auth_runtime).mailer = mailer;
        self
    }
    #[cfg(test)]
    pub(crate) async fn initialize_lfs_storage(
        &self,
        settings: &crate::config::StorageSettings,
    ) -> Result<(), anyhow::Error> {
        let manager = crate::storage::start(&self.database, settings).await?;
        *self.storage_runtime.lfs_storage.write().await = Some(manager);
        Ok(())
    }

    #[cfg(test)]
    pub(crate) async fn initialize_registry_storage(
        &self,
        settings: &crate::config::StorageSettings,
    ) -> Result<(), anyhow::Error> {
        let manager = crate::registry::storage::start(&self.database, settings).await?;
        *self.storage_runtime.registry_storage.write().await = Some(manager);
        Ok(())
    }

    pub(crate) async fn registry_storage(&self) -> Option<Arc<DomainStorage>> {
        self.storage_runtime.registry_storage().await
    }

    pub(crate) async fn lfs_storage(&self) -> Option<Arc<DomainStorage>> {
        self.storage_runtime.lfs_storage().await
    }

    pub(crate) fn subscribe_integrity_settings(&self) -> watch::Receiver<u64> {
        self.integrity_settings_version.subscribe()
    }

    pub fn public_url(&self) -> &Url {
        &self.public_url
    }

    pub fn runtime_settings(&self) -> Result<&Settings, ApiError> {
        self.runtime_settings
            .as_deref()
            .ok_or_else(|| ApiError::internal("runtime settings are unavailable"))
    }

    #[cfg(test)]
    pub fn session_lifetime(&self) -> ChronoDuration {
        self.auth_service().session_lifetime()
    }

    pub async fn authenticate_principal(
        &self,
        headers: &HeaderMap,
        jar: &CookieJar,
        required_scope: i32,
    ) -> Result<gitadel_identity::Principal, ApiError> {
        if let Some(authorization) = headers.get(header::AUTHORIZATION) {
            let authorization = authorization
                .to_str()
                .map_err(|_| ApiError::unauthorized())?;
            let token = authorization
                .strip_prefix("Bearer ")
                .or_else(|| authorization.strip_prefix("token "))
                .ok_or_else(ApiError::unauthorized)?;
            return Ok(gitadel_identity::IdentityService::new(self.database())
                .authenticate_token_principal(token, required_scope)
                .await?);
        }

        let raw_token = jar
            .get(SESSION_COOKIE)
            .map(Cookie::value)
            .ok_or_else(ApiError::unauthorized)?;
        Ok(gitadel_identity::IdentityService::new(&self.database)
            .authenticate_session_principal(raw_token)
            .await?)
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
        Ok(gitadel_identity::IdentityService::new(&self.database)
            .authenticate_session(raw_token)
            .await?)
    }

    pub async fn authenticate_token(
        &self,
        token: &str,
        required_scope: i32,
    ) -> Result<AuthenticatedUser, ApiError> {
        Ok(gitadel_identity::IdentityService::new(&self.database)
            .authenticate_token(token, required_scope)
            .await?)
    }

    pub async fn authenticate_ssh_key(
        &self,
        fingerprint: &str,
    ) -> Result<Option<user::Model>, ApiError> {
        Ok(gitadel_identity::IdentityService::new(&self.database)
            .authenticate_ssh_key(fingerprint)
            .await?)
    }

    pub async fn session_user(&self, jar: &CookieJar) -> Result<Option<user::Model>, ApiError> {
        let Some(raw_token) = jar.get(SESSION_COOKIE).map(Cookie::value) else {
            return Ok(None);
        };
        Ok(gitadel_identity::IdentityService::new(&self.database)
            .session_user(raw_token)
            .await?)
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

    /// Whether the account exists and is not disabled. Callers that keep a
    /// principal beyond one request (an open SSH connection, an LFS token)
    /// re-check it here before each use.
    pub(crate) async fn user_is_enabled(&self, id: Uuid) -> Result<bool, ApiError> {
        Ok(self.enabled_user(id).await?.is_some())
    }

    async fn enabled_user(&self, id: Uuid) -> Result<Option<user::Model>, ApiError> {
        Ok(gitadel_identity::IdentityService::new(&self.database)
            .enabled_user(id)
            .await?)
    }

    #[cfg(test)]
    pub async fn create_session_on<C: ConnectionTrait>(
        &self,
        connection: &C,
        user_id: Uuid,
    ) -> Result<(String, Cookie<'static>), ApiError> {
        let raw_token =
            gitadel_identity::create_session_on(connection, user_id, self.session_lifetime())
                .await?;

        let cookie = self.session_cookie(raw_token.clone());
        Ok((raw_token, cookie))
    }

    pub fn session_cookie(&self, raw_token: String) -> Cookie<'static> {
        Cookie::build((SESSION_COOKIE, raw_token))
            .path("/")
            .http_only(true)
            .same_site(SameSite::Lax)
            .secure(self.public_url.scheme() == "https")
            .max_age(TimeDuration::hours(self.settings.session_lifetime_hours))
            .build()
    }

    pub async fn delete_session(&self, jar: &CookieJar) -> Result<Cookie<'static>, ApiError> {
        if let Some(raw_token) = jar.get(SESSION_COOKIE).map(Cookie::value) {
            gitadel_identity::delete_session(&self.database, raw_token).await?;
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
        Ok(gitadel_db::audit::record(
            connection,
            actor_user_id,
            action,
            target,
            gitadel_core::audit::AuditContext {
                remote_address: remote_address(),
            },
        )
        .await?)
    }
}

pub async fn bootstrap_admin(
    database: &DatabaseConnection,
    username: &str,
    password: String,
) -> Result<user::Model, ApiError> {
    Ok(gitadel_identity::bootstrap_admin(database, username, password).await?)
}

pub fn router() -> Router<IdentityState> {
    contract_router().into()
}

pub fn contract_router() -> utoipa_axum::router::OpenApiRouter<IdentityState> {
    utoipa_axum::router::OpenApiRouter::new()
        .api_routes(crate::openapi::routes!(crate::identity::auth::status))
        .api_routes(crate::openapi::routes!(crate::identity::sso::start))
        .api_routes(crate::openapi::routes!(crate::identity::sso::callback))
        .api_routes(crate::openapi::routes!(
            crate::identity::sso::admin_configuration,
            crate::identity::sso::update_configuration
        ))
        .api_routes(crate::openapi::routes!(
            crate::identity::sso::list_providers,
            crate::identity::sso::create_provider
        ))
        .api_routes(crate::openapi::routes!(
            crate::identity::sso::update_provider,
            crate::identity::sso::delete_provider
        ))
        .api_routes(crate::openapi::routes!(
            crate::identity::admin::public_instance_settings
        ))
        .api_routes(crate::openapi::routes!(crate::identity::email::smtp_status))
        .api_routes(crate::openapi::routes!(
            crate::identity::email::send_test_email
        ))
        .api_routes(crate::openapi::routes!(
            crate::identity::email::get_email,
            crate::identity::email::set_email,
            crate::identity::email::delete_email
        ))
        .api_routes(crate::openapi::routes!(
            crate::identity::email::resend_verification
        ))
        .api_routes(crate::openapi::routes!(
            crate::identity::email::verify_email
        ))
        .api_routes(crate::openapi::routes!(
            crate::identity::notifications::get_notifications,
            crate::identity::notifications::update_notifications
        ))
        .api_routes(crate::openapi::routes!(
            crate::identity::email::request_password_reset
        ))
        .api_routes(crate::openapi::routes!(
            crate::identity::email::confirm_password_reset
        ))
        .api_routes(crate::openapi::routes!(
            crate::identity::admin::public_instance_favicon
        ))
        .api_routes(crate::openapi::routes!(
            crate::identity::admin::list_backup_providers,
            crate::identity::admin::create_backup_provider
        ))
        .api_routes(crate::openapi::routes!(
            crate::identity::admin::test_backup_provider
        ))
        .api_routes(crate::openapi::routes!(
            crate::identity::admin::update_backup_provider,
            crate::identity::admin::delete_backup_provider
        ))
        .api_routes(crate::openapi::routes!(
            crate::identity::admin::update_backup_schedule
        ))
        .api_routes(crate::openapi::routes!(
            crate::identity::admin::list_backups,
            crate::identity::admin::create_backup
        ))
        .api_routes(crate::openapi::routes!(
            crate::identity::admin::download_backup,
            crate::identity::admin::delete_backup
        ))
        .api_routes(crate::openapi::routes!(
            crate::identity::admin::backup_progress
        ))
        .api_routes(crate::openapi::routes!(
            crate::identity::admin::preflight_restore
        ))
        .api_routes(crate::openapi::routes!(
            crate::identity::admin::restore_backup
        ))
        .api_routes(crate::openapi::routes!(
            crate::identity::admin::list_storage_targets,
            crate::identity::admin::create_storage_target
        ))
        .api_routes(crate::openapi::routes!(
            crate::identity::admin::test_storage_target
        ))
        .api_routes(crate::openapi::routes!(
            crate::identity::admin_storage::overview::storage_overview
        ))
        .api_routes(crate::openapi::routes!(
            crate::identity::admin_storage::list_domains
        ))
        .api_routes(crate::openapi::routes!(
            crate::identity::admin_storage::domain_status
        ))
        .api_routes(crate::openapi::routes!(
            crate::identity::admin_storage::domain_repositories
        ))
        .api_routes(crate::openapi::routes!(
            crate::identity::admin_storage::migrate_domain
        ))
        .api_routes(crate::openapi::routes!(
            crate::identity::admin_storage::domain_migration
        ))
        .api_routes(crate::openapi::routes!(
            crate::identity::admin_storage::domain_migration_events
        ))
        // Deprecated per-domain aliases of the routes above; remove after one
        // release.
        .api_routes(crate::openapi::routes!(
            crate::identity::admin_storage::legacy::lfs_status
        ))
        .api_routes(crate::openapi::routes!(
            crate::identity::admin_storage::legacy::lfs_repositories
        ))
        .api_routes(crate::openapi::routes!(
            crate::identity::admin_storage::legacy::lfs_migrate
        ))
        .api_routes(crate::openapi::routes!(
            crate::identity::admin_storage::legacy::lfs_progress
        ))
        .api_routes(crate::openapi::routes!(
            crate::identity::admin_storage::legacy::registry_status
        ))
        .api_routes(crate::openapi::routes!(
            crate::identity::admin_storage::legacy::registry_repositories
        ))
        .api_routes(crate::openapi::routes!(
            crate::identity::admin_storage::legacy::registry_migrate
        ))
        .api_routes(crate::openapi::routes!(
            crate::identity::admin_storage::legacy::registry_events
        ))
        .api_routes(crate::openapi::routes!(
            crate::identity::admin::delete_storage_target
        ))
        .api_routes(crate::openapi::routes!(
            crate::identity::admin::test_saved_storage_target
        ))
        .api_routes(crate::openapi::routes!(
            crate::identity::admin::measure_storage_target
        ))
        .api_routes(crate::openapi::routes!(crate::identity::auth::register))
        .api_routes(crate::openapi::routes!(crate::identity::auth::setup))
        .api_routes(crate::openapi::routes!(crate::identity::auth::login))
        .api_routes(crate::openapi::routes!(
            crate::identity::auth::finish_two_factor_login
        ))
        .api_routes(crate::openapi::routes!(crate::identity::auth::logout))
        .api_routes(crate::openapi::routes!(
            crate::identity::avatar::public_avatar
        ))
        .api_routes(
            crate::openapi::routes!(
                crate::identity::avatar::update_avatar,
                crate::identity::avatar::delete_avatar
            )
            .layer(DefaultBodyLimit::max(avatar::MAX_AVATAR_REQUEST_BYTES)),
        )
        .api_routes(crate::openapi::routes!(
            crate::identity::auth::update_username
        ))
        .api_routes(crate::openapi::routes!(
            crate::identity::auth::update_repository_preferences
        ))
        .api_routes(crate::openapi::routes!(
            crate::identity::auth::update_theme_preference
        ))
        .api_routes(crate::openapi::routes!(
            crate::identity::auth::update_motion_preference
        ))
        .api_routes(crate::openapi::routes!(
            crate::identity::commit_emails::get_identity,
            crate::identity::commit_emails::update_identity
        ))
        .api_routes(crate::openapi::routes!(
            crate::identity::commit_emails::add_email
        ))
        .api_routes(crate::openapi::routes!(
            crate::identity::commit_emails::remove_email
        ))
        .api_routes(crate::openapi::routes!(
            crate::identity::commit_emails::resend_verification
        ))
        .api_routes(crate::openapi::routes!(
            crate::identity::auth::update_password
        ))
        .api_routes(crate::openapi::routes!(crate::identity::two_factor::status))
        .api_routes(crate::openapi::routes!(
            crate::identity::two_factor::start_enrollment
        ))
        .api_routes(crate::openapi::routes!(
            crate::identity::two_factor::confirm_enrollment
        ))
        .api_routes(crate::openapi::routes!(
            crate::identity::two_factor::regenerate_recovery_codes
        ))
        .api_routes(crate::openapi::routes!(
            crate::identity::two_factor::disable
        ))
        .api_routes(crate::openapi::routes!(
            crate::identity::auth::start_passkey_login
        ))
        .api_routes(crate::openapi::routes!(
            crate::identity::auth::finish_passkey_login
        ))
        .api_routes(crate::openapi::routes!(
            crate::identity::auth::list_passkeys
        ))
        .api_routes(crate::openapi::routes!(
            crate::identity::auth::delete_passkey
        ))
        .api_routes(crate::openapi::routes!(
            crate::identity::auth::start_passkey_registration
        ))
        .api_routes(crate::openapi::routes!(
            crate::identity::auth::finish_passkey_registration
        ))
        .api_routes(crate::openapi::routes!(
            crate::identity::users::list_invitations,
            crate::identity::auth::create_invitation
        ))
        .api_routes(crate::openapi::routes!(
            crate::identity::users::revoke_invitation
        ))
        .api_routes(crate::openapi::routes!(crate::identity::users::list_users))
        .api_routes(crate::openapi::routes!(crate::identity::users::delete_user))
        .api_routes(crate::openapi::routes!(
            crate::identity::users::disable_user
        ))
        .api_routes(crate::openapi::routes!(crate::identity::users::enable_user))
        .api_routes(crate::openapi::routes!(
            crate::identity::users::reset_two_factor
        ))
        .api_routes(crate::openapi::routes!(
            crate::identity::resources::list_ssh_keys,
            crate::identity::resources::create_ssh_key
        ))
        .api_routes(crate::openapi::routes!(
            crate::identity::resources::delete_ssh_key
        ))
        .api_routes(crate::openapi::routes!(
            crate::identity::resources::list_tokens,
            crate::identity::resources::create_token
        ))
        .api_routes(crate::openapi::routes!(
            crate::identity::resources::revoke_token
        ))
        .api_routes(crate::openapi::routes!(
            crate::identity::oauth::list_applications,
            crate::identity::oauth::create_application
        ))
        .api_routes(crate::openapi::routes!(
            crate::identity::oauth::delete_application
        ))
        .api_routes(crate::openapi::routes!(
            crate::identity::oauth::list_authorizations
        ))
        .api_routes(crate::openapi::routes!(
            crate::identity::oauth::revoke_authorization
        ))
        .api_routes(crate::openapi::routes!(
            crate::identity::resources::list_organizations,
            crate::identity::resources::create_organization
        ))
        .api_routes(crate::openapi::routes!(
            crate::identity::resources::update_organization,
            crate::identity::resources::delete_organization
        ))
        .api_routes(crate::openapi::routes!(
            crate::identity::resources::get_namespace
        ))
        .api_routes(
            crate::openapi::routes!(
                crate::identity::avatar::public_organization_avatar,
                crate::identity::avatar::update_organization_avatar,
                crate::identity::avatar::delete_organization_avatar
            )
            .layer(DefaultBodyLimit::max(avatar::MAX_AVATAR_REQUEST_BYTES)),
        )
        .api_routes(crate::openapi::routes!(
            crate::identity::resources::list_members,
            crate::identity::resources::add_member
        ))
        .api_routes(crate::openapi::routes!(
            crate::identity::resources::list_member_suggestions
        ))
        .api_routes(crate::openapi::routes!(
            crate::identity::resources::remove_member
        ))
        .api_routes(crate::openapi::routes!(
            crate::identity::resources::list_audit
        ))
        .api_routes(crate::openapi::routes!(
            crate::identity::admin::get_instance_settings,
            crate::identity::admin::update_instance_settings
        ))
        .api_routes(crate::openapi::routes!(
            crate::identity::admin::get_integrity_settings,
            crate::identity::admin::update_integrity_settings
        ))
        .api_routes(
            crate::openapi::routes!(
                crate::identity::admin::update_instance_favicon,
                crate::identity::admin::delete_instance_favicon
            )
            .layer(DefaultBodyLimit::max(admin::MAX_FAVICON_BYTES)),
        )
        .api_routes(crate::openapi::routes!(
            crate::identity::integrations::list_integrations,
            crate::identity::integrations::create_integration
        ))
        .api_routes(crate::openapi::routes!(
            crate::identity::integrations::test_new_integration
        ))
        .api_routes(crate::openapi::routes!(
            crate::identity::integrations::update_integration,
            crate::identity::integrations::delete_integration
        ))
        .api_routes(crate::openapi::routes!(
            crate::identity::integrations::get_integration_credential
        ))
        .api_routes(crate::openapi::routes!(
            crate::identity::integrations::test_integration
        ))
        .api_routes(crate::openapi::routes!(
            crate::identity::integrations::get_integration_source,
            crate::identity::integrations::configure_integration_source,
            crate::identity::integrations::disconnect_integration_source
        ))
        .api_routes(crate::openapi::routes!(
            crate::identity::integrations::verify_integration_source
        ))
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

pub fn validate_slug(value: &str, label: &str) -> Result<String, ApiError> {
    Ok(gitadel_identity::validate_slug(value, label)?)
}
pub fn validate_name(value: &str, label: &str) -> Result<String, ApiError> {
    Ok(gitadel_identity::validate_name(value, label)?)
}

#[derive(Serialize, ToSchema)]
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

#[derive(Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct Pagination {
    pub limit: Option<u64>,
}
