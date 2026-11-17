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
