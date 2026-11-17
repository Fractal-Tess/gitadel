//! Stateful authentication, independent of HTTP request state.
use crate::{AccountService, IdentityError, hash_secret, random_secret};
use argon2::{Argon2, PasswordHasher};
use chrono::Duration as ChronoDuration;
use gitadel_config::AuthSettings;
use gitadel_core::audit::AuditContext;
use gitadel_db::entity::user;
use gitadel_secrets::SecretCipher;
use sea_orm::{ConnectionTrait, DatabaseConnection, EntityTrait, PaginatorTrait};
use std::{
    collections::{HashMap, VecDeque},
    net::IpAddr,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;
use url::Url;
use uuid::Uuid;
use webauthn_rs::{
    Webauthn, WebauthnBuilder,
    prelude::{DiscoverableAuthentication, PasskeyRegistration},
};
const CHALLENGE_LIFETIME: Duration = Duration::from_secs(5 * 60);
const AUTH_CHALLENGE_CAPACITY: usize = 1_024;
const AUTH_CHALLENGE_SOURCE_CAPACITY: usize = 16;
const AUTH_RATE_LIMIT_CAPACITY: usize = 4_096;
const AUTH_RATE_LIMIT_WINDOW: Duration = Duration::from_secs(60);
/// How many attempts an account tolerates per window across every source, as
/// a multiple of its per-source limit. Distributed guessing is still slowed,
/// while one source failing on purpose cannot lock the account's owner out.
const ACCOUNT_RATE_LIMIT_GLOBAL_FACTOR: usize = 5;

/// Sliding-window attempt counters keyed by source and account.
#[derive(Default)]
struct AuthRateLimiter {
    buckets: HashMap<String, VecDeque<Instant>>,
}

impl AuthRateLimiter {
    /// Records an attempt under `key` unless `limit` attempts already landed
    /// inside the window. A full table evicts its quietest bucket instead of
    /// refusing new keys, so a flood of fresh source addresses cannot lock
    /// out everyone whose bucket does not exist yet.
    fn try_acquire(&mut self, key: String, limit: usize, now: Instant) -> bool {
        self.buckets.retain(|_, attempts| {
            attempts.retain(|attempt| now.duration_since(*attempt) < AUTH_RATE_LIMIT_WINDOW);
            !attempts.is_empty()
        });
        if !self.buckets.contains_key(&key) && self.buckets.len() >= AUTH_RATE_LIMIT_CAPACITY {
            let quietest = self
                .buckets
                .iter()
                .min_by_key(|(_, attempts)| attempts.back().copied())
                .map(|(key, _)| key.clone());
            if let Some(quietest) = quietest {
                self.buckets.remove(&quietest);
            }
        }
        let attempts = self.buckets.entry(key).or_default();
        if attempts.len() >= limit {
            return false;
        }
        attempts.push_back(now);
        true
    }
}

/// The rate-limit bucket of a source address. IPv6 is bucketed by /64: a
/// single subscriber usually holds a whole /64 and can rotate through it.
fn source_bucket(address: Option<IpAddr>) -> String {
    match address {
        Some(IpAddr::V4(address)) => address.to_string(),
        Some(IpAddr::V6(address)) => {
            let prefix = u128::from(address) & !((1u128 << 64) - 1);
            format!("{}/64", std::net::Ipv6Addr::from(prefix))
        }
        None => "unknown".to_owned(),
    }
}

#[cfg(test)]
mod rate_limit_tests {
    use super::*;

    #[test]
    fn a_full_limiter_evicts_its_quietest_bucket_instead_of_refusing() {
        let mut limiter = AuthRateLimiter::default();
        let start = Instant::now();
        for index in 0..AUTH_RATE_LIMIT_CAPACITY {
            let at = start + Duration::from_millis(index as u64);
            assert!(limiter.try_acquire(format!("source:{index}"), 1, at));
        }
        let later = start + Duration::from_millis(AUTH_RATE_LIMIT_CAPACITY as u64);
        assert!(
            limiter.try_acquire("source:newcomer".to_owned(), 1, later),
            "a new key is admitted when the table is full"
        );
        assert!(limiter.buckets.len() <= AUTH_RATE_LIMIT_CAPACITY);
        assert!(
            !limiter.buckets.contains_key("source:0"),
            "the bucket with the oldest attempt made room"
        );
        assert!(limiter.buckets.contains_key("source:1"));
    }

    #[test]
    fn attempts_outside_the_window_no_longer_count() {
        let mut limiter = AuthRateLimiter::default();
        let start = Instant::now();
        assert!(limiter.try_acquire("k".to_owned(), 1, start));
        assert!(!limiter.try_acquire("k".to_owned(), 1, start));
        assert!(limiter.try_acquire("k".to_owned(), 1, start + AUTH_RATE_LIMIT_WINDOW));
    }

    #[test]
    fn ipv6_sources_share_a_bucket_per_64() {
        let same = |a: &str| source_bucket(Some(a.parse().unwrap()));
        assert_eq!(same("2001:db8:1:2::1"), same("2001:db8:1:2:ffff::9"));
        assert_ne!(same("2001:db8:1:2::1"), same("2001:db8:1:3::1"));
        assert_eq!(same("2001:db8:1:2::1"), "2001:db8:1:2::/64");
        assert_eq!(same("203.0.113.7"), "203.0.113.7");
        assert_eq!(source_bucket(None), "unknown");
    }
}

#[derive(Clone)]
pub struct AuthRuntime {
    pub database: DatabaseConnection,
    pub oidc_authorizations: Arc<crate::sso::OidcAuthorizations>,
    pub authorization_requests: Arc<Mutex<HashMap<String, crate::oauth::AuthorizationRequest>>>,
    pub settings: AuthSettings,
    pub public_url: Url,
    pub webauthn: Option<Webauthn>,
    pub registration_challenges: Arc<Mutex<HashMap<String, RegistrationChallenge>>>,
    pub authentication_challenges: Arc<Mutex<HashMap<String, AuthenticationChallenge>>>,
    auth_rate_limits: Arc<Mutex<AuthRateLimiter>>,
    pub pending_logins: Arc<Mutex<HashMap<String, PendingLogin>>>,
    pub setup_token: Arc<Mutex<Option<String>>>,
    pub dummy_password_hash: Arc<String>,
    pub mailer: gitadel_mail::Mailer,
    pub secret_cipher: Arc<SecretCipher>,
}
impl AuthRuntime {
    pub fn new(
        database: DatabaseConnection,
        settings: AuthSettings,
        public_url: Url,
        mailer: gitadel_mail::Mailer,
        secret_cipher: Arc<SecretCipher>,
    ) -> Result<Self, IdentityError> {
        if public_url.path() != "/"
            || public_url.query().is_some()
            || public_url.fragment().is_some()
            || !public_url.username().is_empty()
            || public_url.password().is_some()
        {
            return Err(IdentityError::internal(
                "public URL must be an origin without a path, query, fragment, or credentials",
            ));
        }
        let webauthn = match public_url.host() {
            Some(url::Host::Domain(rp_id)) => Some(
                WebauthnBuilder::new(rp_id, &public_url)
                    .map_err(IdentityError::internal)?
                    .rp_name("Gitadel")
                    .build()
                    .map_err(IdentityError::internal)?,
            ),
            Some(url::Host::Ipv4(_) | url::Host::Ipv6(_)) => {
                tracing::warn!(%public_url, "passkeys are disabled because the public URL is an IP address; use a hostname to enable them");
                None
            }
            None => return Err(IdentityError::internal("public URL must contain a host")),
        };
        let dummy_password_hash = Argon2::default()
            .hash_password(b"gitadel-invalid-login")
            .map_err(IdentityError::internal)?
            .to_string();
        Ok(Self {
            database,
            settings,
            public_url,
            webauthn,
            oidc_authorizations: Default::default(),
            authorization_requests: Default::default(),
            registration_challenges: Default::default(),
            authentication_challenges: Default::default(),
            auth_rate_limits: Default::default(),
            pending_logins: Default::default(),
            setup_token: Default::default(),
            dummy_password_hash: Arc::new(dummy_password_hash),
            mailer,
            secret_cipher,
        })
    }
    pub fn service(&self, audit: AuditContext) -> AuthService<'_> {
        AuthService {
            runtime: self,
            audit,
        }
    }
}
pub struct AuthService<'a> {
    pub runtime: &'a AuthRuntime,
    pub audit: AuditContext,
}
impl std::ops::Deref for AuthService<'_> {
    type Target = AuthRuntime;
    fn deref(&self) -> &AuthRuntime {
        self.runtime
    }
}
impl AuthService<'_> {
    pub fn database(&self) -> &DatabaseConnection {
        &self.database
    }
    pub fn account_service(&self) -> AccountService<'_> {
        AccountService::new(&self.database, self.audit)
    }
    pub fn public_url(&self) -> &Url {
        &self.public_url
    }
    pub fn secret_cipher(&self) -> &SecretCipher {
        &self.secret_cipher
    }
    pub fn mailer(&self) -> &gitadel_mail::Mailer {
        &self.mailer
    }
    pub fn oidc_authorizations(&self) -> &crate::sso::OidcAuthorizations {
        &self.oidc_authorizations
    }
    pub async fn session_user(
        &self,
        raw: Option<&str>,
    ) -> Result<Option<user::Model>, IdentityError> {
        match raw {
            Some(raw) => {
                crate::IdentityService::new(self.database())
                    .session_user(raw)
                    .await
            }
            None => Ok(None),
        }
    }
    pub async fn audit(
        &self,
        actor: Option<Uuid>,
        action: impl Into<String>,
        target: Option<String>,
    ) -> Result<(), IdentityError> {
        self.audit_on(self.database(), actor, action, target).await
    }
    pub async fn audit_on<C: ConnectionTrait>(
        &self,
        connection: &C,
        actor: Option<Uuid>,
        action: impl Into<String>,
        target: Option<String>,
    ) -> Result<(), IdentityError> {
        self.account_service()
            .audit_on(connection, actor, action, target)
            .await
    }
    pub async fn create_session_on<C: ConnectionTrait>(
        &self,
        connection: &C,
        id: Uuid,
    ) -> Result<String, IdentityError> {
        crate::create_session_on(connection, id, self.session_lifetime()).await
    }
    pub async fn require_actor(
        &self,
        actor: &crate::Principal,
        admin: bool,
        scope: i32,
    ) -> Result<crate::AuthenticatedUser, IdentityError> {
        self.account_service()
            .require_actor(actor, admin, scope)
            .await
    }
    pub async fn prepare_setup_link(&self) -> Result<Option<Url>, IdentityError> {
        if user::Entity::find().count(&self.database).await? != 0 {
            return Ok(None);
        }
        let token = random_secret(32);
        let mut link = self
            .public_url
            .join("register")
            .map_err(IdentityError::internal)?;
        link.query_pairs_mut().append_pair("setup", &token);
        *self.setup_token.lock().await = Some(token);
        Ok(Some(link))
    }
    pub async fn setup_token_matches(&self, candidate: &str) -> bool {
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
    pub async fn clear_setup_token(&self) {
        *self.setup_token.lock().await = None;
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
    ) -> Result<(), IdentityError> {
        let source = source_bucket(self.audit.remote_address);
        self.enforce_rate_limit(format!("source:{source}:{bucket}"), limit)
            .await
    }
    pub async fn enforce_account_auth_rate_limit(
        &self,
        account: &str,
        limit: usize,
    ) -> Result<(), IdentityError> {
        let source = source_bucket(self.audit.remote_address);
        self.enforce_rate_limit(format!("account:{account}:{source}"), limit)
            .await?;
        self.enforce_rate_limit(
            format!("account:{account}"),
            limit.saturating_mul(ACCOUNT_RATE_LIMIT_GLOBAL_FACTOR),
        )
        .await
    }
    pub async fn enforce_rate_limit(&self, key: String, limit: usize) -> Result<(), IdentityError> {
        let mut limiter = self.auth_rate_limits.lock().await;
        if limiter.try_acquire(key, limit, Instant::now()) {
            Ok(())
        } else {
            Err(IdentityError::too_many_requests(
                "Too many authentication attempts. Try again later.",
            ))
        }
    }

    pub async fn insert_authentication_challenge(
        &self,
        id: String,
        state: DiscoverableAuthentication,
    ) -> Result<(), IdentityError> {
        let source = self.audit.remote_address;
        let mut challenges = self.authentication_challenges.lock().await;
        challenges.retain(|_, challenge| challenge.created_at.elapsed() < CHALLENGE_LIFETIME);
        if challenges.len() >= AUTH_CHALLENGE_CAPACITY
            || challenges
                .values()
                .filter(|challenge| challenge.remote_address == source)
                .count()
                >= AUTH_CHALLENGE_SOURCE_CAPACITY
        {
            return Err(IdentityError::too_many_requests(
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
}
impl AuthRuntime {
    pub fn webauthn(&self) -> Result<&Webauthn, IdentityError> {
        self.webauthn.as_ref().ok_or_else(|| {
            IdentityError::bad_request(
                "Passkeys need a public URL with a hostname rather than an IP address.",
            )
        })
    }
}
impl AuthRuntime {
    pub async fn registration_challenges(
        &self,
    ) -> tokio::sync::MutexGuard<'_, HashMap<String, RegistrationChallenge>> {
        let mut challenges = self.registration_challenges.lock().await;
        challenges.retain(|_, challenge| challenge.created_at.elapsed() < CHALLENGE_LIFETIME);
        challenges
    }
}
impl AuthRuntime {
    pub async fn authentication_challenges(
        &self,
    ) -> tokio::sync::MutexGuard<'_, HashMap<String, AuthenticationChallenge>> {
        let mut challenges = self.authentication_challenges.lock().await;
        challenges.retain(|_, challenge| challenge.created_at.elapsed() < CHALLENGE_LIFETIME);
        challenges
    }
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
pub struct PendingLogin {
    user_id: Uuid,
    created_at: Instant,
    attempts: u8,
    remote_address: Option<IpAddr>,
}
impl AuthService<'_> {
    pub async fn insert_pending_login(&self, user_id: Uuid) -> Result<String, IdentityError> {
        let source = self.audit.remote_address;
        let mut pending = self.pending_logins.lock().await;
        pending.retain(|_, login| login.created_at.elapsed() < CHALLENGE_LIFETIME);
        if pending.len() >= AUTH_CHALLENGE_CAPACITY
            || pending
                .values()
                .filter(|login| login.remote_address == source)
                .count()
                >= AUTH_CHALLENGE_SOURCE_CAPACITY
        {
            return Err(IdentityError::too_many_requests(
                "Too many authentication attempts. Try again later.",
            ));
        }
        let id = random_secret(24);
        pending.insert(
            id.clone(),
            PendingLogin {
                user_id,
                created_at: Instant::now(),
                attempts: 0,
                remote_address: source,
            },
        );
        Ok(id)
    }

    pub async fn pending_login_user(&self, id: &str) -> Option<Uuid> {
        let mut pending = self.pending_logins.lock().await;
        pending.retain(|_, login| login.created_at.elapsed() < CHALLENGE_LIFETIME);
        pending.get(id).map(|login| login.user_id)
    }

    pub async fn record_pending_login_failure(&self, id: &str) {
        let mut pending = self.pending_logins.lock().await;
        let exhausted = pending.get_mut(id).is_some_and(|login| {
            login.attempts += 1;
            login.attempts >= PENDING_LOGIN_ATTEMPTS
        });
        if exhausted {
            pending.remove(id);
        }
    }

    pub async fn finish_pending_login(&self, id: &str) {
        self.pending_logins.lock().await.remove(id);
    }
}

const PENDING_LOGIN_ATTEMPTS: u8 = 5;

impl AuthRuntime {
    pub async fn authorization_requests(
        &self,
    ) -> tokio::sync::MutexGuard<'_, HashMap<String, crate::oauth::AuthorizationRequest>> {
        let mut requests = self.authorization_requests.lock().await;
        requests.retain(|_, request| request.created_at.elapsed() < CHALLENGE_LIFETIME);
        requests
    }
}
