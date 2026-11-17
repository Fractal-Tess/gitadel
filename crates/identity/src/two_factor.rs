//! TOTP verification and atomic consumption of authenticator/recovery credentials.
use crate::{IdentityError, hash_secret};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::Utc;
use gitadel_db::entity::{user_recovery_code, user_totp};
use gitadel_secrets::{SecretCipher, SecretKind};
use hmac::{Hmac, KeyInit as _, Mac as _};
use rand::RngExt as _;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, Condition, ConnectionTrait, EntityTrait, QueryFilter, Set,
    sea_query::Expr,
};
use sha1::Sha1;
use subtle::ConstantTimeEq as _;
use uuid::Uuid;

const TOTP_PERIOD_SECONDS: i64 = 30;
const TOTP_DIGITS: u32 = 6;
const RECOVERY_CODE_COUNT: usize = 10;
const RECOVERY_CODE_LENGTH: usize = 10;
const BASE32_ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
type HmacSha1 = Hmac<Sha1>;

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
) -> Result<bool, IdentityError> {
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
    cipher: &SecretCipher,
    user_id: Uuid,
    code: &str,
) -> Result<Option<SecondFactor>, IdentityError> {
    if verify_confirmed_totp(connection, cipher, user_id, code).await? {
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

/// Consume a current TOTP from a confirmed enrollment for reauthentication.
pub async fn verify_confirmed_totp<C: ConnectionTrait>(
    connection: &C,
    cipher: &SecretCipher,
    user_id: Uuid,
    code: &str,
) -> Result<bool, IdentityError> {
    let Some(stored) = user_totp::Entity::find_by_id(user_id)
        .filter(user_totp::Column::ConfirmedAt.is_not_null())
        .one(connection)
        .await?
    else {
        return Ok(false);
    };
    let secret = decode_secret(cipher, user_id, &stored.secret)?;
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
) -> Result<bool, IdentityError> {
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

pub async fn replace_recovery_codes<C: ConnectionTrait>(
    connection: &C,
    user_id: Uuid,
) -> Result<Vec<String>, IdentityError> {
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

/// Recovery codes carry 50 random bits each, which is why a fast hash is
/// adequate at rest: they cannot be guessed offline the way a password can.
pub fn recovery_code() -> String {
    let mut bytes = [0_u8; RECOVERY_CODE_LENGTH];
    rand::rng().fill(&mut bytes);
    let characters: String = bytes
        .iter()
        .map(|byte| char::from(BASE32_ALPHABET[usize::from(byte & 31)]).to_ascii_lowercase())
        .collect();
    let (left, right) = characters.split_at(RECOVERY_CODE_LENGTH / 2);
    format!("{left}-{right}")
}

pub fn normalize_recovery_code(code: &str) -> String {
    code.chars()
        .filter(|character| !character.is_whitespace() && *character != '-')
        .map(|character| character.to_ascii_lowercase())
        .collect()
}

/// The raw seed from its stored form: sealed by the instance key (or plain
/// text from before sealing existed), then base64.
pub fn decode_secret(
    cipher: &SecretCipher,
    user_id: Uuid,
    stored: &str,
) -> Result<Vec<u8>, IdentityError> {
    let encoded = cipher.open_text(SecretKind::TotpSecret, &user_id, stored)?;
    URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(IdentityError::internal)
}

pub fn hotp(secret: &[u8], counter: u64) -> Result<u32, IdentityError> {
    let mut mac = HmacSha1::new_from_slice(secret).map_err(IdentityError::internal)?;
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
pub fn verify_totp(
    secret: &[u8],
    code: &str,
    unix_time: i64,
    last_used_step: Option<i64>,
) -> Result<Option<i64>, IdentityError> {
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

use crate::{Principal, runtime::AuthService, verify_password};
use sea_orm::{PaginatorTrait, TransactionTrait};
const TOTP_SECRET_BYTES: usize = 20;
pub struct TwoFactorStatusResponse {
    pub enabled: bool,
    pub confirmed_at: Option<chrono::DateTime<Utc>>,
    pub recovery_codes_remaining: u64,
}

pub struct EnrollmentResponse {
    pub secret: String,
    pub otpauth_uri: String,
    pub qr_svg: String,
}

pub struct ConfirmEnrollmentRequest {
    pub code: String,
}

pub struct RecoveryCodesResponse {
    pub recovery_codes: Vec<String>,
}

pub struct ReauthenticationRequest {
    pub password: Option<String>,
    pub code: Option<String>,
}

pub async fn status(
    state: &AuthService<'_>,
    actor: &Principal,
) -> Result<TwoFactorStatusResponse, IdentityError> {
    let actor = state.require_actor(actor, false, crate::SCOPE_READ).await?;
    let confirmed_at = user_totp::Entity::find_by_id(actor.user.id)
        .one(state.database())
        .await?
        .and_then(|stored| stored.confirmed_at);
    let recovery_codes_remaining = user_recovery_code::Entity::find()
        .filter(user_recovery_code::Column::UserId.eq(actor.user.id))
        .filter(user_recovery_code::Column::UsedAt.is_null())
        .count(state.database())
        .await?;
    Ok(TwoFactorStatusResponse {
        enabled: confirmed_at.is_some(),
        confirmed_at,
        recovery_codes_remaining,
    })
}
pub async fn start_enrollment(
    state: &AuthService<'_>,
    actor: &Principal,
) -> Result<EnrollmentResponse, IdentityError> {
    let actor = state
        .require_actor(actor, false, crate::SCOPE_WRITE)
        .await?;
    crate::auth::require_browser_session(actor.via_api_token)?;
    let mut secret = [0_u8; TOTP_SECRET_BYTES];
    rand::rng().fill(&mut secret);

    let transaction = state.database().begin().await?;
    if is_enabled(&transaction, actor.user.id).await? {
        return Err(IdentityError::conflict(
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
        .map_err(IdentityError::internal)?;
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
    Ok(EnrollmentResponse {
        secret: encoded,
        otpauth_uri,
        qr_svg,
    })
}
pub async fn confirm_enrollment(
    state: &AuthService<'_>,
    actor: &Principal,
    request: ConfirmEnrollmentRequest,
) -> Result<RecoveryCodesResponse, IdentityError> {
    let actor = state
        .require_actor(actor, false, crate::SCOPE_WRITE)
        .await?;
    crate::auth::require_browser_session(actor.via_api_token)?;
    state
        .enforce_account_auth_rate_limit(&format!("two-factor:{}", actor.user.id), 8)
        .await?;
    let transaction = state.database().begin().await?;
    let stored = user_totp::Entity::find_by_id(actor.user.id)
        .one(&transaction)
        .await?
        .filter(|stored| stored.confirmed_at.is_none())
        .ok_or_else(|| IdentityError::bad_request("Start two-factor enrollment first."))?;
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
    Ok(RecoveryCodesResponse { recovery_codes })
}
pub async fn regenerate_recovery_codes(
    state: &AuthService<'_>,
    actor: &Principal,
    request: ReauthenticationRequest,
) -> Result<RecoveryCodesResponse, IdentityError> {
    let actor = state
        .require_actor(actor, false, crate::SCOPE_WRITE)
        .await?;
    crate::auth::require_browser_session(actor.via_api_token)?;
    let transaction = state.database().begin().await?;
    if !is_enabled(&transaction, actor.user.id).await? {
        return Err(IdentityError::bad_request(
            "Two-factor authentication is not enabled.",
        ));
    }
    reauthenticate(state, &transaction, &actor.user, request).await?;
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
    Ok(RecoveryCodesResponse { recovery_codes })
}
pub async fn disable(
    state: &AuthService<'_>,
    actor: &Principal,
    request: ReauthenticationRequest,
) -> Result<(), IdentityError> {
    let actor = state
        .require_actor(actor, false, crate::SCOPE_WRITE)
        .await?;
    crate::auth::require_browser_session(actor.via_api_token)?;
    let transaction = state.database().begin().await?;
    if !is_enabled(&transaction, actor.user.id).await? {
        return Err(IdentityError::bad_request(
            "Two-factor authentication is not enabled.",
        ));
    }
    reauthenticate(state, &transaction, &actor.user, request).await?;
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
    Ok(())
}
async fn reauthenticate<C: ConnectionTrait>(
    state: &AuthService<'_>,
    connection: &C,
    account: &gitadel_db::entity::user::Model,
    request: ReauthenticationRequest,
) -> Result<(), IdentityError> {
    state
        .enforce_account_auth_rate_limit(&format!("two-factor:{}", account.id), 8)
        .await?;
    if let Some(code) = request.code.filter(|code| !code.trim().is_empty()) {
        if crate::two_factor::verify_confirmed_totp(
            connection,
            state.secret_cipher(),
            account.id,
            &code,
        )
        .await?
        {
            return Ok(());
        }
        return Err(invalid_code());
    }
    if let Some(password) = request.password.filter(|password| !password.is_empty()) {
        if verify_password(password, account.password_hash.clone()).await? {
            return Ok(());
        }
        return Err(IdentityError::bad_request(
            "The current password is incorrect.",
        ));
    }
    Err(IdentityError::bad_request(
        "Enter your password or a current authentication code.",
    ))
}
pub fn base32(bytes: &[u8]) -> String {
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
pub fn otpauth_uri(secret: &str, username: &str, host: &str) -> Result<String, IdentityError> {
    let mut uri = url::Url::parse("otpauth://totp/").map_err(IdentityError::internal)?;
    uri.set_path(&format!("Gitadel:{username}@{host}"));
    uri.query_pairs_mut()
        .append_pair("secret", secret)
        .append_pair("issuer", "Gitadel")
        .append_pair("algorithm", "SHA1")
        .append_pair("digits", &TOTP_DIGITS.to_string())
        .append_pair("period", &TOTP_PERIOD_SECONDS.to_string());
    Ok(uri.into())
}
pub fn qr_svg(value: &str) -> Result<String, IdentityError> {
    let code = qrcode::QrCode::new(value.as_bytes()).map_err(IdentityError::internal)?;
    Ok(code
        .render::<qrcode::render::svg::Color<'_>>()
        .min_dimensions(192, 192)
        .dark_color(qrcode::render::svg::Color("#000000"))
        .light_color(qrcode::render::svg::Color("#ffffff"))
        .build())
}
pub fn invalid_code() -> IdentityError {
    IdentityError::InvalidTwoFactorCode
}
