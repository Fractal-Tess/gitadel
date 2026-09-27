//! Time-based one-time passwords (RFC 6238) as a second factor for password
//! sign-in. Passkeys already prove possession of a device, so only the password
//! path consults this module.

use std::{net::IpAddr, time::Instant};

use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
};
use axum_extra::extract::cookie::CookieJar;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::Utc;
use hmac::{Hmac, KeyInit as _, Mac as _};
use rand::RngExt as _;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, Condition, ConnectionTrait, EntityTrait, PaginatorTrait,
    QueryFilter, Set, TransactionTrait, sea_query::Expr,
};
use serde::{Deserialize, Serialize};
use sha1::Sha1;
use subtle::ConstantTimeEq as _;
use uuid::Uuid;

use super::{
    AUTH_CHALLENGE_CAPACITY, AUTH_CHALLENGE_SOURCE_CAPACITY, ApiError, CHALLENGE_LIFETIME,
    IdentityState, SCOPE_READ, SCOPE_WRITE, hash_secret, remote_address, verify_password,
};
use crate::entity::{user_recovery_code, user_totp};

const TOTP_PERIOD_SECONDS: i64 = 30;
const TOTP_DIGITS: u32 = 6;
const TOTP_SECRET_BYTES: usize = 20;
const RECOVERY_CODE_COUNT: usize = 10;
const RECOVERY_CODE_LENGTH: usize = 10;
/// A pending sign-in is discarded after this many wrong codes, which forces
/// the password to be presented again.
const PENDING_LOGIN_ATTEMPTS: u8 = 5;
const BASE32_ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

type HmacSha1 = Hmac<Sha1>;

/// A password that was accepted for an account with two-factor
/// authentication, waiting for its second factor.
pub struct PendingLogin {
    user_id: Uuid,
    created_at: Instant,
    attempts: u8,
    remote_address: Option<IpAddr>,
}

impl IdentityState {
    pub(super) async fn insert_pending_login(&self, user_id: Uuid) -> Result<String, ApiError> {
        let source = remote_address();
        let mut pending = self.pending_logins.lock().await;
        pending.retain(|_, login| login.created_at.elapsed() < CHALLENGE_LIFETIME);
        if pending.len() >= AUTH_CHALLENGE_CAPACITY
            || pending
                .values()
                .filter(|login| login.remote_address == source)
                .count()
                >= AUTH_CHALLENGE_SOURCE_CAPACITY
        {
            return Err(ApiError::too_many_requests(
                "Too many authentication attempts. Try again later.",
            ));
        }
        let id = super::random_secret(24);
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

    pub(super) async fn pending_login_user(&self, id: &str) -> Option<Uuid> {
        let mut pending = self.pending_logins.lock().await;
        pending.retain(|_, login| login.created_at.elapsed() < CHALLENGE_LIFETIME);
        pending.get(id).map(|login| login.user_id)
    }

    pub(super) async fn record_pending_login_failure(&self, id: &str) {
        let mut pending = self.pending_logins.lock().await;
        let exhausted = pending.get_mut(id).is_some_and(|login| {
            login.attempts += 1;
            login.attempts >= PENDING_LOGIN_ATTEMPTS
        });
        if exhausted {
            pending.remove(id);
        }
    }

    pub(super) async fn finish_pending_login(&self, id: &str) {
        self.pending_logins.lock().await.remove(id);
    }
}

/// Which second factor completed a sign-in, recorded in the audit log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SecondFactor {
    Totp,
    RecoveryCode,
}

impl SecondFactor {
    pub fn label(self) -> &'static str {
        match self {
            Self::Totp => "totp",
            Self::RecoveryCode => "recovery_code",
        }
    }
}

pub async fn is_enabled<C: ConnectionTrait>(
    connection: &C,
    user_id: Uuid,
) -> Result<bool, ApiError> {
    Ok(user_totp::Entity::find_by_id(user_id)
        .filter(user_totp::Column::ConfirmedAt.is_not_null())
        .one(connection)
        .await?
        .is_some())
}

/// Checks a submitted code against the confirmed authenticator first and the
/// unused recovery codes second, consuming whichever one matched.
pub async fn verify_second_factor<C: ConnectionTrait>(
    connection: &C,
    user_id: Uuid,
    code: &str,
) -> Result<Option<SecondFactor>, ApiError> {
    if verify_confirmed_totp(connection, user_id, code).await? {
        return Ok(Some(SecondFactor::Totp));
    }
    let normalized = normalize_recovery_code(code);
    if normalized.len() != RECOVERY_CODE_LENGTH {
        return Ok(None);
    }
    let consumed = user_recovery_code::Entity::update_many()
        .col_expr(
            user_recovery_code::Column::UsedAt,
            Expr::value(Some(Utc::now())),
        )
        .filter(user_recovery_code::Column::CodeHash.eq(hash_secret(&normalized)))
        .filter(user_recovery_code::Column::UserId.eq(user_id))
        .filter(user_recovery_code::Column::UsedAt.is_null())
        .exec(connection)
        .await?;
    Ok((consumed.rows_affected == 1).then_some(SecondFactor::RecoveryCode))
}

async fn verify_confirmed_totp<C: ConnectionTrait>(
    connection: &C,
    user_id: Uuid,
    code: &str,
) -> Result<bool, ApiError> {
    let Some(stored) = user_totp::Entity::find_by_id(user_id)
        .filter(user_totp::Column::ConfirmedAt.is_not_null())
        .one(connection)
        .await?
    else {
        return Ok(false);
    };
    let secret = decode_secret(&stored.secret)?;
    let Some(step) = verify_totp(&secret, code, Utc::now().timestamp(), stored.last_used_step)?
    else {
        return Ok(false);
    };
    // The conditional update makes a code usable once even when two requests
    // race with the same value.
    let claimed = user_totp::Entity::update_many()
        .col_expr(user_totp::Column::LastUsedStep, Expr::value(Some(step)))
        .filter(user_totp::Column::UserId.eq(user_id))
        .filter(
            Condition::any()
                .add(user_totp::Column::LastUsedStep.is_null())
                .add(user_totp::Column::LastUsedStep.lt(step)),
        )
        .exec(connection)
        .await?;
    Ok(claimed.rows_affected == 1)
}

/// Removes the authenticator and every recovery code for an account.
pub async fn remove_on<C: ConnectionTrait>(
    connection: &C,
    user_id: Uuid,
) -> Result<bool, ApiError> {
    user_recovery_code::Entity::delete_many()
        .filter(user_recovery_code::Column::UserId.eq(user_id))
        .exec(connection)
        .await?;
    let removed = user_totp::Entity::delete_many()
        .filter(user_totp::Column::UserId.eq(user_id))
        .filter(user_totp::Column::ConfirmedAt.is_not_null())
        .exec(connection)
        .await?;
    user_totp::Entity::delete_by_id(user_id)
        .exec(connection)
        .await?;
    Ok(removed.rows_affected == 1)
}

#[derive(Serialize)]
pub struct TwoFactorStatusResponse {
    enabled: bool,
    confirmed_at: Option<chrono::DateTime<Utc>>,
    recovery_codes_remaining: u64,
}

pub async fn status(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<TwoFactorStatusResponse>, ApiError> {
    let actor = state.authenticate(&headers, &jar, SCOPE_READ).await?;
    let confirmed_at = user_totp::Entity::find_by_id(actor.user.id)
        .one(state.database())
        .await?
        .and_then(|stored| stored.confirmed_at);
    let recovery_codes_remaining = user_recovery_code::Entity::find()
        .filter(user_recovery_code::Column::UserId.eq(actor.user.id))
        .filter(user_recovery_code::Column::UsedAt.is_null())
        .count(state.database())
        .await?;
    Ok(Json(TwoFactorStatusResponse {
        enabled: confirmed_at.is_some(),
        confirmed_at,
        recovery_codes_remaining,
    }))
}

#[derive(Serialize)]
pub struct EnrollmentResponse {
    secret: String,
    otpauth_uri: String,
    qr_svg: String,
}

pub async fn start_enrollment(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<EnrollmentResponse>, ApiError> {
    let actor = state.authenticate(&headers, &jar, SCOPE_WRITE).await?;
    super::auth::require_browser_session(actor.via_api_token)?;
    let mut secret = [0_u8; TOTP_SECRET_BYTES];
    rand::rng().fill(&mut secret);

    let transaction = state.database().begin().await?;
    if is_enabled(&transaction, actor.user.id).await? {
        return Err(ApiError::conflict(
            "Two-factor authentication is already enabled.",
        ));
    }
    // Starting again replaces an unconfirmed secret, so an abandoned QR code
    // never becomes valid later.
    user_totp::Entity::delete_by_id(actor.user.id)
        .exec(&transaction)
        .await?;
    user_totp::ActiveModel {
        user_id: Set(actor.user.id),
        secret: Set(URL_SAFE_NO_PAD.encode(secret)),
        confirmed_at: Set(None),
        last_used_step: Set(None),
        created_at: Set(Utc::now()),
    }
    .insert(&transaction)
    .await?;
    transaction.commit().await?;

    let encoded = base32(&secret);
    let host = state.public_url().host_str().unwrap_or("gitadel");
    let otpauth_uri = otpauth_uri(&encoded, &actor.user.username, host)?;
    let qr_svg = qr_svg(&otpauth_uri)?;
    Ok(Json(EnrollmentResponse {
        secret: encoded,
        otpauth_uri,
        qr_svg,
    }))
}

#[derive(Deserialize)]
pub struct ConfirmEnrollmentRequest {
    code: String,
}

#[derive(Serialize)]
pub struct RecoveryCodesResponse {
    recovery_codes: Vec<String>,
}

pub async fn confirm_enrollment(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<ConfirmEnrollmentRequest>,
) -> Result<(StatusCode, Json<RecoveryCodesResponse>), ApiError> {
    let actor = state.authenticate(&headers, &jar, SCOPE_WRITE).await?;
    super::auth::require_browser_session(actor.via_api_token)?;
    state
        .enforce_account_auth_rate_limit(&format!("two-factor:{}", actor.user.id), 8)
        .await?;
    let transaction = state.database().begin().await?;
    let stored = user_totp::Entity::find_by_id(actor.user.id)
        .one(&transaction)
        .await?
        .filter(|stored| stored.confirmed_at.is_none())
        .ok_or_else(|| ApiError::bad_request("Start two-factor enrollment first."))?;
    let secret = decode_secret(&stored.secret)?;
    let Some(step) = verify_totp(&secret, &request.code, Utc::now().timestamp(), None)? else {
        return Err(invalid_code());
    };
    let mut active: user_totp::ActiveModel = stored.into();
    active.confirmed_at = Set(Some(Utc::now()));
    active.last_used_step = Set(Some(step));
    active.update(&transaction).await?;
    let recovery_codes = replace_recovery_codes(&transaction, actor.user.id).await?;
    state
        .audit_on(
            &transaction,
            Some(actor.user.id),
            "account.two_factor.enable",
            None,
        )
        .await?;
    transaction.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(RecoveryCodesResponse { recovery_codes }),
    ))
}

/// Either proof is enough to change an enabled second factor: the password
/// covers a lost authenticator and the code covers a forgotten password.
#[derive(Deserialize)]
pub struct ReauthenticationRequest {
    password: Option<String>,
    code: Option<String>,
}

async fn reauthenticate<C: ConnectionTrait>(
    state: &IdentityState,
    connection: &C,
    account: &crate::entity::user::Model,
    request: ReauthenticationRequest,
) -> Result<(), ApiError> {
    state
        .enforce_account_auth_rate_limit(&format!("two-factor:{}", account.id), 8)
        .await?;
    if let Some(code) = request.code.filter(|code| !code.trim().is_empty()) {
        if verify_confirmed_totp(connection, account.id, &code).await? {
            return Ok(());
        }
        return Err(invalid_code());
    }
    if let Some(password) = request.password.filter(|password| !password.is_empty()) {
        if verify_password(password, account.password_hash.clone()).await? {
            return Ok(());
        }
        return Err(ApiError::bad_request("The current password is incorrect."));
    }
    Err(ApiError::bad_request(
        "Enter your password or a current authentication code.",
    ))
}

pub async fn regenerate_recovery_codes(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<ReauthenticationRequest>,
) -> Result<Json<RecoveryCodesResponse>, ApiError> {
    let actor = state.authenticate(&headers, &jar, SCOPE_WRITE).await?;
    super::auth::require_browser_session(actor.via_api_token)?;
    let transaction = state.database().begin().await?;
    if !is_enabled(&transaction, actor.user.id).await? {
        return Err(ApiError::bad_request(
            "Two-factor authentication is not enabled.",
        ));
    }
    reauthenticate(&state, &transaction, &actor.user, request).await?;
    let recovery_codes = replace_recovery_codes(&transaction, actor.user.id).await?;
    state
        .audit_on(
            &transaction,
            Some(actor.user.id),
            "account.two_factor.recovery_codes.regenerate",
            None,
        )
        .await?;
    transaction.commit().await?;
    Ok(Json(RecoveryCodesResponse { recovery_codes }))
}

pub async fn disable(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<ReauthenticationRequest>,
) -> Result<StatusCode, ApiError> {
    let actor = state.authenticate(&headers, &jar, SCOPE_WRITE).await?;
    super::auth::require_browser_session(actor.via_api_token)?;
    let transaction = state.database().begin().await?;
    if !is_enabled(&transaction, actor.user.id).await? {
        return Err(ApiError::bad_request(
            "Two-factor authentication is not enabled.",
        ));
    }
    reauthenticate(&state, &transaction, &actor.user, request).await?;
    remove_on(&transaction, actor.user.id).await?;
    state
        .audit_on(
            &transaction,
            Some(actor.user.id),
            "account.two_factor.disable",
            None,
        )
        .await?;
    transaction.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn replace_recovery_codes<C: ConnectionTrait>(
    connection: &C,
    user_id: Uuid,
) -> Result<Vec<String>, ApiError> {
    user_recovery_code::Entity::delete_many()
        .filter(user_recovery_code::Column::UserId.eq(user_id))
        .exec(connection)
        .await?;
    let now = Utc::now();
    let mut codes = Vec::with_capacity(RECOVERY_CODE_COUNT);
    for _ in 0..RECOVERY_CODE_COUNT {
        let code = recovery_code();
        user_recovery_code::ActiveModel {
            code_hash: Set(hash_secret(&normalize_recovery_code(&code))),
            user_id: Set(user_id),
            used_at: Set(None),
            created_at: Set(now),
        }
        .insert(connection)
        .await?;
        codes.push(code);
    }
    Ok(codes)
}

pub(super) fn invalid_code() -> ApiError {
    ApiError {
        status: StatusCode::UNAUTHORIZED,
        code: "invalid_two_factor_code",
        message: "The authentication code was not accepted.".to_owned(),
    }
}

/// Recovery codes carry 50 random bits each, which is why a fast hash is
/// adequate at rest: they cannot be guessed offline the way a password can.
fn recovery_code() -> String {
    let mut bytes = [0_u8; RECOVERY_CODE_LENGTH];
    rand::rng().fill(&mut bytes);
    let characters: String = bytes
        .iter()
        .map(|byte| char::from(BASE32_ALPHABET[usize::from(byte & 31)]).to_ascii_lowercase())
        .collect();
    let (left, right) = characters.split_at(RECOVERY_CODE_LENGTH / 2);
    format!("{left}-{right}")
}

fn normalize_recovery_code(code: &str) -> String {
    code.chars()
        .filter(|character| !character.is_whitespace() && *character != '-')
        .map(|character| character.to_ascii_lowercase())
        .collect()
}

fn decode_secret(encoded: &str) -> Result<Vec<u8>, ApiError> {
    URL_SAFE_NO_PAD.decode(encoded).map_err(ApiError::internal)
}

fn hotp(secret: &[u8], counter: u64) -> Result<u32, ApiError> {
    let mut mac = HmacSha1::new_from_slice(secret).map_err(ApiError::internal)?;
    mac.update(&counter.to_be_bytes());
    let digest = mac.finalize().into_bytes();
    let offset = usize::from(digest[digest.len() - 1] & 0x0f);
    let binary = u32::from_be_bytes([
        digest[offset] & 0x7f,
        digest[offset + 1],
        digest[offset + 2],
        digest[offset + 3],
    ]);
    Ok(binary % 10_u32.pow(TOTP_DIGITS))
}

/// Accepts the current step and one on either side for clock drift, and
/// returns the matched step so it can be recorded against replay.
fn verify_totp(
    secret: &[u8],
    code: &str,
    unix_time: i64,
    last_used_step: Option<i64>,
) -> Result<Option<i64>, ApiError> {
    let code: String = code
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect();
    if code.len() != TOTP_DIGITS as usize || !code.bytes().all(|byte| byte.is_ascii_digit()) {
        return Ok(None);
    }
    let current = unix_time.div_euclid(TOTP_PERIOD_SECONDS);
    for step in [current - 1, current, current + 1] {
        if step < 0 || last_used_step.is_some_and(|last| step <= last) {
            continue;
        }
        let expected = format!("{:06}", hotp(secret, step.unsigned_abs())?);
        if bool::from(expected.as_bytes().ct_eq(code.as_bytes())) {
            return Ok(Some(step));
        }
    }
    Ok(None)
}

fn base32(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(bytes.len().div_ceil(5) * 8);
    let mut buffer = 0_u32;
    let mut bits = 0_u32;
    for &byte in bytes {
        buffer = (buffer << 8) | u32::from(byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            encoded.push(char::from(
                BASE32_ALPHABET[((buffer >> bits) & 31) as usize],
            ));
        }
        buffer &= (1 << bits) - 1;
    }
    if bits > 0 {
        encoded.push(char::from(
            BASE32_ALPHABET[((buffer << (5 - bits)) & 31) as usize],
        ));
    }
    encoded
}

fn otpauth_uri(secret: &str, username: &str, host: &str) -> Result<String, ApiError> {
    let mut uri = url::Url::parse("otpauth://totp/").map_err(ApiError::internal)?;
    uri.set_path(&format!("Gitadel:{username}@{host}"));
    uri.query_pairs_mut()
        .append_pair("secret", secret)
        .append_pair("issuer", "Gitadel")
        .append_pair("algorithm", "SHA1")
        .append_pair("digits", &TOTP_DIGITS.to_string())
        .append_pair("period", &TOTP_PERIOD_SECONDS.to_string());
    Ok(uri.into())
}

fn qr_svg(value: &str) -> Result<String, ApiError> {
    let code = qrcode::QrCode::new(value.as_bytes()).map_err(ApiError::internal)?;
    Ok(code
        .render::<qrcode::render::svg::Color<'_>>()
        .min_dimensions(192, 192)
        .dark_color(qrcode::render::svg::Color("#000000"))
        .light_color(qrcode::render::svg::Color("#ffffff"))
        .build())
}

#[cfg(test)]
mod tests {
    use sea_orm::{ConnectOptions, Database, DatabaseConnection};
    use sea_orm_migration::MigratorTrait;

    use super::*;
    use crate::{entity::user, migration::Migrator};

    const RFC_SECRET: &[u8] = b"12345678901234567890";

    #[test]
    fn totp_matches_rfc_6238_sha1_vectors() {
        let codes = [59, 1_111_111_109, 1_234_567_890, 2_000_000_000]
            .map(|time| format!("{:06}", hotp(RFC_SECRET, (time / 30) as u64).unwrap()));
        assert_eq!(codes, ["287082", "081804", "005924", "279037"]);
    }

    #[test]
    fn totp_accepts_adjacent_steps_and_rejects_replay() {
        let now = 1_111_111_109;
        let previous = format!("{:06}", hotp(RFC_SECRET, (now / 30 - 1) as u64).unwrap());
        let step = verify_totp(RFC_SECRET, &previous, now, None).unwrap();
        assert_eq!(step, Some(now / 30 - 1));
        assert_eq!(verify_totp(RFC_SECRET, &previous, now, step).unwrap(), None);
    }

    #[test]
    fn totp_rejects_malformed_codes() {
        assert_eq!(
            verify_totp(RFC_SECRET, "08180", 1_111_111_109, None).unwrap(),
            None
        );
        assert_eq!(
            verify_totp(RFC_SECRET, "abcdef", 1_111_111_109, None).unwrap(),
            None
        );
    }

    #[test]
    fn base32_encodes_without_padding() {
        assert_eq!(base32(b"foobar"), "MZXW6YTBOI");
        assert_eq!(base32(RFC_SECRET), "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ");
    }

    #[test]
    fn otpauth_uri_encodes_label_and_parameters() {
        let uri = otpauth_uri("ABC", "alice", "git.example.com").unwrap();
        assert_eq!(
            uri,
            "otpauth://totp/Gitadel:alice@git.example.com?secret=ABC&issuer=Gitadel&algorithm=SHA1&digits=6&period=30"
        );
    }

    #[test]
    fn recovery_codes_normalize_formatting() {
        let code = recovery_code();
        assert_eq!(code.len(), RECOVERY_CODE_LENGTH + 1);
        assert_eq!(
            normalize_recovery_code(&format!(" {} ", code.to_uppercase())),
            code.replace('-', "")
        );
    }

    async fn database_with_user() -> (DatabaseConnection, Uuid) {
        let mut options = ConnectOptions::new("sqlite::memory:");
        options.max_connections(1).sqlx_logging(false);
        let database = Database::connect(options).await.unwrap();
        Migrator::up(&database, None).await.unwrap();
        let now = Utc::now();
        let user_id = Uuid::new_v4();
        user::ActiveModel {
            id: Set(user_id),
            username: Set("alice".to_owned()),
            password_hash: Set("hash".to_owned()),
            is_admin: Set(false),
            default_repository_visibility: Set("private".to_owned()),
            theme_preference: Set("system".to_owned()),
            disabled_at: Set(None),
            avatar_updated_at: Set(None),
            created_at: Set(now),
            updated_at: Set(now),
        }
        .insert(&database)
        .await
        .unwrap();
        (database, user_id)
    }

    #[tokio::test]
    async fn recovery_codes_are_hashed_and_single_use() {
        let (database, user_id) = database_with_user().await;
        user_totp::ActiveModel {
            user_id: Set(user_id),
            secret: Set(URL_SAFE_NO_PAD.encode(RFC_SECRET)),
            confirmed_at: Set(Some(Utc::now())),
            last_used_step: Set(None),
            created_at: Set(Utc::now()),
        }
        .insert(&database)
        .await
        .unwrap();
        let codes = replace_recovery_codes(&database, user_id).await.unwrap();
        let stored = user_recovery_code::Entity::find()
            .all(&database)
            .await
            .unwrap();
        assert!(stored.iter().all(|row| !codes.contains(&row.code_hash)));

        let first = verify_second_factor(&database, user_id, &codes[0])
            .await
            .unwrap();
        let replay = verify_second_factor(&database, user_id, &codes[0])
            .await
            .unwrap();
        assert_eq!((first, replay), (Some(SecondFactor::RecoveryCode), None));
    }

    #[tokio::test]
    async fn confirmed_totp_code_is_consumed_once() {
        let (database, user_id) = database_with_user().await;
        user_totp::ActiveModel {
            user_id: Set(user_id),
            secret: Set(URL_SAFE_NO_PAD.encode(RFC_SECRET)),
            confirmed_at: Set(Some(Utc::now())),
            last_used_step: Set(None),
            created_at: Set(Utc::now()),
        }
        .insert(&database)
        .await
        .unwrap();
        let step = Utc::now().timestamp().div_euclid(TOTP_PERIOD_SECONDS);
        let code = format!("{:06}", hotp(RFC_SECRET, step.unsigned_abs()).unwrap());

        let first = verify_second_factor(&database, user_id, &code)
            .await
            .unwrap();
        let replay = verify_second_factor(&database, user_id, &code)
            .await
            .unwrap();
        assert_eq!((first, replay), (Some(SecondFactor::Totp), None));
    }

    #[tokio::test]
    async fn removing_two_factor_clears_recovery_codes() {
        let (database, user_id) = database_with_user().await;
        user_totp::ActiveModel {
            user_id: Set(user_id),
            secret: Set(URL_SAFE_NO_PAD.encode(RFC_SECRET)),
            confirmed_at: Set(Some(Utc::now())),
            last_used_step: Set(None),
            created_at: Set(Utc::now()),
        }
        .insert(&database)
        .await
        .unwrap();
        replace_recovery_codes(&database, user_id).await.unwrap();

        let removed = remove_on(&database, user_id).await.unwrap();
        let remaining = user_recovery_code::Entity::find()
            .count(&database)
            .await
            .unwrap();
        assert_eq!(
            (
                removed,
                remaining,
                is_enabled(&database, user_id).await.unwrap()
            ),
            (true, 0, false)
        );
    }
}
