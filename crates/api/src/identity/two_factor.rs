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
use rand::RngExt as _;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, PaginatorTrait, QueryFilter, Set,
    TransactionTrait,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use super::{
    AUTH_CHALLENGE_CAPACITY, AUTH_CHALLENGE_SOURCE_CAPACITY, ApiError, CHALLENGE_LIFETIME,
    IdentityState, SCOPE_READ, SCOPE_WRITE, remote_address, verify_password,
};
use crate::{
    entity::{user_recovery_code, user_totp},
    secrets::{SecretCipher, SecretKind},
};

const TOTP_PERIOD_SECONDS: i64 = 30;
const TOTP_DIGITS: u32 = 6;
const TOTP_SECRET_BYTES: usize = 20;
#[cfg(test)]
const RECOVERY_CODE_LENGTH: usize = 10;
/// A pending sign-in is discarded after this many wrong codes, which forces
/// the password to be presented again.
const PENDING_LOGIN_ATTEMPTS: u8 = 5;
const BASE32_ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

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

pub use gitadel_identity::two_factor::SecondFactor;

pub async fn is_enabled<C: ConnectionTrait>(
    connection: &C,
    user_id: Uuid,
) -> Result<bool, ApiError> {
    Ok(gitadel_identity::two_factor::is_enabled(connection, user_id).await?)
}
pub async fn verify_second_factor<C: ConnectionTrait>(
    connection: &C,
    cipher: &SecretCipher,
    user_id: Uuid,
    code: &str,
) -> Result<Option<SecondFactor>, ApiError> {
    Ok(
        gitadel_identity::two_factor::verify_second_factor(connection, cipher, user_id, code)
            .await?,
    )
}
pub async fn remove_on<C: ConnectionTrait>(
    connection: &C,
    user_id: Uuid,
) -> Result<bool, ApiError> {
    Ok(gitadel_identity::two_factor::remove_on(connection, user_id).await?)
}

#[derive(Serialize, ToSchema)]
pub struct TwoFactorStatusResponse {
    enabled: bool,
    confirmed_at: Option<chrono::DateTime<Utc>>,
    recovery_codes_remaining: u64,
}

#[utoipa::path(
    get, path = "/me/two-factor",
    operation_id = "get_me_two_factor",
    summary = "Get two-factor authentication status",
    tag = "Account",
    responses(
        (status = 200, description = "Success", body = TwoFactorStatusResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
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

#[derive(Serialize, ToSchema)]
pub struct EnrollmentResponse {
    secret: String,
    otpauth_uri: String,
    qr_svg: String,
}

#[utoipa::path(
    post, path = "/me/two-factor/enroll",
    operation_id = "post_me_two_factor_enroll",
    summary = "Start two-factor enrollment",
    tag = "Account",
    responses(
        (status = 200, description = "Success", body = EnrollmentResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
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
    let sealed = state
        .secret_cipher()
        .seal_text(
            SecretKind::TotpSecret,
            &actor.user.id,
            &URL_SAFE_NO_PAD.encode(secret),
        )
        .map_err(ApiError::internal)?;
    user_totp::ActiveModel {
        user_id: Set(actor.user.id),
        secret: Set(sealed),
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

#[derive(Deserialize, ToSchema)]
pub struct ConfirmEnrollmentRequest {
    code: String,
}

#[derive(Serialize, ToSchema)]
pub struct RecoveryCodesResponse {
    recovery_codes: Vec<String>,
}

#[utoipa::path(
    post, path = "/me/two-factor/confirm",
    operation_id = "post_me_two_factor_confirm",
    summary = "Confirm two-factor enrollment",
    tag = "Account",

    responses(
        (status = 201, description = "Success", body = RecoveryCodesResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
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
    let secret = decode_secret(state.secret_cipher(), actor.user.id, &stored.secret)?;
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
#[derive(Deserialize, ToSchema)]
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
        if verify_confirmed_totp(connection, state.secret_cipher(), account.id, &code).await? {
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

#[utoipa::path(
    post, path = "/me/two-factor/recovery-codes",
    operation_id = "post_me_two_factor_recovery_codes",
    summary = "Regenerate recovery codes",
    tag = "Account",

    responses(
        (status = 200, description = "Success", body = RecoveryCodesResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
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

#[utoipa::path(
    post, path = "/me/two-factor/disable",
    operation_id = "post_me_two_factor_disable",
    summary = "Disable two-factor authentication",
    tag = "Account",

    responses(
        (status = 204, description = "Success"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
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
    Ok(gitadel_identity::two_factor::replace_recovery_codes(connection, user_id).await?)
}

pub(super) fn invalid_code() -> ApiError {
    ApiError {
        status: StatusCode::UNAUTHORIZED,
        code: "invalid_two_factor_code",
        message: "The authentication code was not accepted.".to_owned(),
    }
}

#[cfg(test)]
use gitadel_identity::two_factor::{hotp, normalize_recovery_code, recovery_code};
fn decode_secret(cipher: &SecretCipher, user_id: Uuid, stored: &str) -> Result<Vec<u8>, ApiError> {
    Ok(gitadel_identity::two_factor::decode_secret(
        cipher, user_id, stored,
    )?)
}
fn verify_totp(
    secret: &[u8],
    code: &str,
    unix_time: i64,
    last_used_step: Option<i64>,
) -> Result<Option<i64>, ApiError> {
    Ok(gitadel_identity::two_factor::verify_totp(
        secret,
        code,
        unix_time,
        last_used_step,
    )?)
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
            motion_preference: Set("system".to_owned()),
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

        let cipher = SecretCipher::for_tests();
        let first = verify_second_factor(&database, &cipher, user_id, &codes[0])
            .await
            .unwrap();
        let replay = verify_second_factor(&database, &cipher, user_id, &codes[0])
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

        let cipher = SecretCipher::for_tests();
        let first = verify_second_factor(&database, &cipher, user_id, &code)
            .await
            .unwrap();
        let replay = verify_second_factor(&database, &cipher, user_id, &code)
            .await
            .unwrap();
        assert_eq!((first, replay), (Some(SecondFactor::Totp), None));
    }

    #[tokio::test]
    async fn sealed_totp_secrets_verify_and_a_replaced_key_names_the_setting() {
        let (database, user_id) = database_with_user().await;
        let cipher = SecretCipher::for_tests();
        user_totp::ActiveModel {
            user_id: Set(user_id),
            secret: Set(cipher
                .seal_text(
                    SecretKind::TotpSecret,
                    &user_id,
                    &URL_SAFE_NO_PAD.encode(RFC_SECRET),
                )
                .unwrap()),
            confirmed_at: Set(Some(Utc::now())),
            last_used_step: Set(None),
            created_at: Set(Utc::now()),
        }
        .insert(&database)
        .await
        .unwrap();
        let step = Utc::now().timestamp().div_euclid(TOTP_PERIOD_SECONDS);
        let code = format!("{:06}", hotp(RFC_SECRET, step.unsigned_abs()).unwrap());

        let rotated = SecretCipher::from_material(b"replaced host key").unwrap();
        let error = verify_second_factor(&database, &rotated, user_id, &code)
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains(&format!("authenticator secret of user {user_id}")),
            "{error}"
        );

        let verified = verify_second_factor(&database, &cipher, user_id, &code)
            .await
            .unwrap();
        assert_eq!(verified, Some(SecondFactor::Totp));
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
