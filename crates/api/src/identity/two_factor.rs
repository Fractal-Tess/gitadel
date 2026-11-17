//! Time-based one-time passwords (RFC 6238) as a second factor for password
//! sign-in. Passkeys already prove possession of a device, so only the password
//! path consults this module.

use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
};
use axum_extra::extract::cookie::CookieJar;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::{ApiError, IdentityState, SCOPE_READ, SCOPE_WRITE};

#[cfg(test)]
const RECOVERY_CODE_LENGTH: usize = 10;
#[cfg(test)]
pub async fn is_enabled<C: ConnectionTrait>(
    connection: &C,
    user_id: Uuid,
) -> Result<bool, ApiError> {
    Ok(gitadel_identity::two_factor::is_enabled(connection, user_id).await?)
}
#[cfg(test)]
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
#[cfg(test)]
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
    let actor = state
        .authenticate_principal(&headers, &jar, SCOPE_READ)
        .await?;
    Ok(Json(
        domain::status(&state.auth_service(), &actor).await?.into(),
    ))
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
    let actor = state
        .authenticate_principal(&headers, &jar, SCOPE_WRITE)
        .await?;
    Ok(Json(
        domain::start_enrollment(&state.auth_service(), &actor)
            .await?
            .into(),
    ))
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
    let actor = state
        .authenticate_principal(&headers, &jar, SCOPE_WRITE)
        .await?;
    Ok((
        StatusCode::CREATED,
        Json(
            domain::confirm_enrollment(
                &state.auth_service(),
                &actor,
                domain::ConfirmEnrollmentRequest { code: request.code },
            )
            .await?
            .into(),
        ),
    ))
}

/// Either proof is enough to change an enabled second factor: the password
/// covers a lost authenticator and the code covers a forgotten password.
#[derive(Deserialize, ToSchema)]
pub struct ReauthenticationRequest {
    password: Option<String>,
    code: Option<String>,
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
    let actor = state
        .authenticate_principal(&headers, &jar, SCOPE_WRITE)
        .await?;
    Ok(Json(
        domain::regenerate_recovery_codes(
            &state.auth_service(),
            &actor,
            domain::ReauthenticationRequest {
                password: request.password,
                code: request.code,
            },
        )
        .await?
        .into(),
    ))
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
    let actor = state
        .authenticate_principal(&headers, &jar, SCOPE_WRITE)
        .await?;
    domain::disable(
        &state.auth_service(),
        &actor,
        domain::ReauthenticationRequest {
            password: request.password,
            code: request.code,
        },
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
async fn replace_recovery_codes<C: ConnectionTrait>(
    connection: &C,
    user_id: Uuid,
) -> Result<Vec<String>, ApiError> {
    Ok(gitadel_identity::two_factor::replace_recovery_codes(connection, user_id).await?)
}

#[cfg(test)]
use gitadel_identity::two_factor::{hotp, normalize_recovery_code, recovery_code};

#[cfg(test)]
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

#[cfg(test)]
fn base32(bytes: &[u8]) -> String {
    domain::base32(bytes)
}

#[cfg(test)]
fn otpauth_uri(secret: &str, username: &str, host: &str) -> Result<String, ApiError> {
    Ok(domain::otpauth_uri(secret, username, host)?)
}

use gitadel_identity::two_factor as domain;
impl From<domain::TwoFactorStatusResponse> for TwoFactorStatusResponse {
    fn from(value: domain::TwoFactorStatusResponse) -> Self {
        Self {
            enabled: value.enabled,
            confirmed_at: value.confirmed_at,
            recovery_codes_remaining: value.recovery_codes_remaining,
        }
    }
}
impl From<domain::EnrollmentResponse> for EnrollmentResponse {
    fn from(value: domain::EnrollmentResponse) -> Self {
        Self {
            secret: value.secret,
            otpauth_uri: value.otpauth_uri,
            qr_svg: value.qr_svg,
        }
    }
}
impl From<domain::RecoveryCodesResponse> for RecoveryCodesResponse {
    fn from(value: domain::RecoveryCodesResponse) -> Self {
        Self {
            recovery_codes: value.recovery_codes,
        }
    }
}

#[cfg(test)]
use crate::entity::user_recovery_code;
#[cfg(test)]
use crate::entity::user_totp;
#[cfg(test)]
use crate::secrets::SecretCipher;
#[cfg(test)]
use crate::secrets::SecretKind;
#[cfg(test)]
use base64::Engine as _;
#[cfg(test)]
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
#[cfg(test)]
use gitadel_identity::two_factor::SecondFactor;
#[cfg(test)]
use sea_orm::ActiveModelTrait;
#[cfg(test)]
use sea_orm::ConnectionTrait;
#[cfg(test)]
use sea_orm::EntityTrait;
#[cfg(test)]
use sea_orm::PaginatorTrait;
#[cfg(test)]
use sea_orm::Set;
#[cfg(test)]
use uuid::Uuid;

#[cfg(test)]
const TOTP_PERIOD_SECONDS: i64 = 30;

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
