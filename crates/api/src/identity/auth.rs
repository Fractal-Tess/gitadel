use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use axum_extra::extract::cookie::CookieJar;
use chrono::Utc;
#[cfg(test)]
use sea_orm::ActiveModelTrait;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;
use webauthn_rs::prelude::{
    CreationChallengeResponse, PublicKeyCredential, RegisterPublicKeyCredential,
    RequestChallengeResponse,
};

use super::{ApiError, IdentityState, UserResponse};

#[derive(Serialize, utoipa::ToSchema)]
pub struct AuthStatusResponse {
    setup_required: bool,
    authenticated: bool,
    user: Option<UserResponse>,
    authentication: super::sso::AuthenticationConfiguration,
    /// Whether outgoing email is configured; gates password reset and
    /// email settings in the interface.
    email_enabled: bool,
}

#[utoipa::path(
    get, path = "/auth/status",
    operation_id = "get_auth_status",
    summary = "Status",
    tag = "Authentication",
    security(),
    responses(
        (status = 200, description = "Success", body = AuthStatusResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn status(
    State(state): State<IdentityState>,
    jar: CookieJar,
) -> Result<Json<AuthStatusResponse>, ApiError> {
    let status = domain::status(
        &state.auth_service(),
        jar.get(super::SESSION_COOKIE).map(|cookie| cookie.value()),
    )
    .await?;
    Ok(Json(AuthStatusResponse {
        setup_required: status.setup_required,
        authenticated: status.account.is_some(),
        user: status.account.map(Into::into),
        authentication: status.authentication.into(),
        email_enabled: status.email_enabled,
    }))
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct CredentialsRequest {
    username: String,
    password: String,
}

#[derive(Serialize, ToSchema)]
pub struct AuthResponse {
    user: UserResponse,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct TwoFactorChallengeResponse {
    two_factor_required: bool,
    challenge_id: String,
}

#[derive(serde::Serialize, utoipa::ToSchema)]
#[serde(untagged)]
pub enum LoginResponse {
    Authenticated(AuthResponse),
    TwoFactor(TwoFactorChallengeResponse),
}

#[utoipa::path(
    post, path = "/auth/login",
    operation_id = "post_auth_login",
    summary = "Login",
    tag = "Authentication",

    security(),
    responses(
        (status = 200, description = "Success", body = LoginResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn login(
    State(state): State<IdentityState>,
    jar: CookieJar,
    Json(request): Json<CredentialsRequest>,
) -> Result<(CookieJar, Json<LoginResponse>), ApiError> {
    match domain::login(
        &state.auth_service(),
        domain::CredentialsRequest {
            username: request.username,
            password: request.password,
        },
    )
    .await?
    {
        domain::LoginOutcome::Authenticated(grant) => Ok((
            jar.add(state.session_cookie(grant.token)),
            Json(LoginResponse::Authenticated(AuthResponse {
                user: grant.user.into(),
            })),
        )),
        domain::LoginOutcome::TwoFactor { challenge_id } => Ok((
            jar,
            Json(LoginResponse::TwoFactor(TwoFactorChallengeResponse {
                two_factor_required: true,
                challenge_id,
            })),
        )),
    }
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct TwoFactorLoginRequest {
    challenge_id: String,
    code: String,
}

#[utoipa::path(
    post, path = "/auth/login/two-factor",
    operation_id = "post_auth_login_two_factor",
    summary = "Finish two factor login",
    tag = "Authentication",

    security(),
    responses(
        (status = 200, description = "Success", body = AuthResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn finish_two_factor_login(
    State(state): State<IdentityState>,
    jar: CookieJar,
    Json(request): Json<TwoFactorLoginRequest>,
) -> Result<impl IntoResponse + crate::openapi::TypedResponse, ApiError> {
    let grant = domain::finish_two_factor_login(
        &state.auth_service(),
        domain::TwoFactorLoginRequest {
            challenge_id: request.challenge_id,
            code: request.code,
        },
    )
    .await?;
    Ok((
        jar.add(state.session_cookie(grant.token)),
        Json(AuthResponse {
            user: grant.user.into(),
        }),
    ))
}

#[utoipa::path(
    post, path = "/auth/logout",
    operation_id = "post_auth_logout",
    summary = "Logout",
    tag = "Authentication",
    responses(
        (status = 204, description = "Success"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn logout(
    State(state): State<IdentityState>,
    jar: CookieJar,
) -> Result<impl IntoResponse + crate::openapi::TypedResponse, ApiError> {
    let actor = state.session_user(&jar).await?;
    let removal = state.delete_session(&jar).await?;
    if let Some(actor) = actor {
        state.audit(Some(actor.id), "auth.logout", None).await?;
    }
    Ok((StatusCode::NO_CONTENT, jar.remove(removal)))
}

#[derive(Deserialize, ToSchema)]
pub struct UpdateUsernameRequest {
    username: String,
}

#[utoipa::path(
    put, path = "/me/username",
    operation_id = "put_me_username",
    summary = "Change the username",
    tag = "Account",

    responses(
        (status = 200, description = "Success", body = AuthResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn update_username(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<UpdateUsernameRequest>,
) -> Result<Json<AuthResponse>, ApiError> {
    let actor = state
        .authenticate_principal(&headers, &jar, super::SCOPE_WRITE)
        .await?;
    Ok(Json(AuthResponse {
        user: domain::update_username(
            &state.auth_service(),
            &actor,
            domain::UpdateUsernameRequest {
                username: request.username,
            },
        )
        .await?
        .into(),
    }))
}

#[derive(Deserialize, ToSchema)]
pub struct UpdateRepositoryPreferencesRequest {
    default_repository_visibility: String,
}

#[utoipa::path(
    put, path = "/me/repository-preferences",
    operation_id = "put_me_repository_preferences",
    summary = "Update repository preferences",
    tag = "Account",

    responses(
        (status = 200, description = "Success", body = AuthResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn update_repository_preferences(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<UpdateRepositoryPreferencesRequest>,
) -> Result<Json<AuthResponse>, ApiError> {
    let actor = state
        .authenticate_principal(&headers, &jar, super::SCOPE_WRITE)
        .await?;
    Ok(Json(AuthResponse {
        user: domain::update_repository_preferences(
            &state.auth_service(),
            &actor,
            domain::UpdateRepositoryPreferencesRequest {
                default_repository_visibility: request.default_repository_visibility,
            },
        )
        .await?
        .into(),
    }))
}

#[derive(Deserialize, ToSchema)]
pub struct UpdateThemePreferenceRequest {
    theme_preference: String,
}

#[utoipa::path(
    put, path = "/me/theme-preference",
    operation_id = "put_me_theme_preference",
    summary = "Update the theme preference",
    tag = "Account",

    responses(
        (status = 200, description = "Success", body = AuthResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn update_theme_preference(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<UpdateThemePreferenceRequest>,
) -> Result<Json<AuthResponse>, ApiError> {
    let actor = state
        .authenticate_principal(&headers, &jar, super::SCOPE_WRITE)
        .await?;
    Ok(Json(AuthResponse {
        user: domain::update_theme_preference(
            &state.auth_service(),
            &actor,
            domain::UpdateThemePreferenceRequest {
                theme_preference: request.theme_preference,
            },
        )
        .await?
        .into(),
    }))
}

#[derive(Deserialize, ToSchema)]
pub struct UpdateMotionPreferenceRequest {
    motion_preference: String,
}

/// Whether the interface animates: `system` follows the browser's
/// reduced-motion setting, `reduce` turns animation off everywhere.
#[utoipa::path(
    put, path = "/me/motion-preference",
    operation_id = "put_me_motion_preference",
    summary = "Update the motion preference",
    tag = "Account",

    responses(
        (status = 200, description = "Success", body = AuthResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn update_motion_preference(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<UpdateMotionPreferenceRequest>,
) -> Result<Json<AuthResponse>, ApiError> {
    let actor = state
        .authenticate_principal(&headers, &jar, super::SCOPE_WRITE)
        .await?;
    Ok(Json(AuthResponse {
        user: domain::update_motion_preference(
            &state.auth_service(),
            &actor,
            domain::UpdateMotionPreferenceRequest {
                motion_preference: request.motion_preference,
            },
        )
        .await?
        .into(),
    }))
}

#[derive(Deserialize, ToSchema)]
pub struct UpdatePasswordRequest {
    current_password: String,
    new_password: String,
}

#[utoipa::path(
    put, path = "/me/password",
    operation_id = "put_me_password",
    summary = "Change the password",
    tag = "Account",

    responses(
        (status = 204, description = "Success"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn update_password(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<UpdatePasswordRequest>,
) -> Result<StatusCode, ApiError> {
    let actor = state
        .authenticate_principal(&headers, &jar, super::SCOPE_WRITE)
        .await?;
    let current_session_hash = jar
        .get(super::SESSION_COOKIE)
        .map(|cookie| super::hash_secret(cookie.value()));
    domain::update_password(
        &state.auth_service(),
        &actor,
        domain::UpdatePasswordRequest {
            current_password: request.current_password,
            new_password: request.new_password,
        },
        current_session_hash,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
pub(super) fn require_browser_session(via_api_token: bool) -> Result<(), ApiError> {
    Ok(domain::require_browser_session(via_api_token)?)
}

/// Rename a user and their personal namespace inside the caller's transaction.
#[cfg(test)]
use gitadel_identity::accounts::rename_user_namespace;

#[derive(Deserialize, utoipa::ToSchema)]
pub struct SetupRequest {
    token: String,
    username: String,
    password: String,
}

/// Creates the first administrator from the one-time link printed at startup.
#[utoipa::path(
    post, path = "/setup",
    operation_id = "post_setup",
    summary = "Setup",
    tag = "Instance",

    security(),
    responses(
        (status = 201, description = "Success", body = AuthResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn setup(
    State(state): State<IdentityState>,
    jar: CookieJar,
    Json(request): Json<SetupRequest>,
) -> Result<impl IntoResponse + crate::openapi::TypedResponse, ApiError> {
    let grant = domain::setup(
        &state.auth_service(),
        domain::SetupRequest {
            token: request.token,
            username: request.username,
            password: request.password,
        },
    )
    .await?;
    Ok((
        StatusCode::CREATED,
        jar.add(state.session_cookie(grant.token)),
        Json(AuthResponse {
            user: grant.user.into(),
        }),
    ))
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct InvitationRegistrationRequest {
    token: String,
    username: String,
    password: String,
}

#[utoipa::path(
    post, path = "/register",
    operation_id = "post_register",
    summary = "Register",
    tag = "Authentication",

    security(),
    responses(
        (status = 201, description = "Success", body = AuthResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn register(
    State(state): State<IdentityState>,
    jar: CookieJar,
    Json(request): Json<InvitationRegistrationRequest>,
) -> Result<impl IntoResponse + crate::openapi::TypedResponse, ApiError> {
    let grant = domain::register(
        &state.auth_service(),
        domain::InvitationRegistrationRequest {
            token: request.token,
            username: request.username,
            password: request.password,
        },
    )
    .await?;
    Ok((
        StatusCode::CREATED,
        jar.add(state.session_cookie(grant.token)),
        Json(AuthResponse {
            user: grant.user.into(),
        }),
    ))
}

#[derive(Deserialize, ToSchema)]
pub struct CreateInvitationRequest {
    expires_in_hours: Option<i64>,
}

#[derive(Serialize, ToSchema)]
pub struct InvitationResponse {
    token: String,
    expires_at: chrono::DateTime<Utc>,
}

#[utoipa::path(
    post, path = "/invitations",
    operation_id = "post_invitations",
    summary = "Create an invitation",
    tag = "Account",

    responses(
        (status = 201, description = "Success", body = crate::identity::auth::InvitationResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn create_invitation(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<CreateInvitationRequest>,
) -> Result<impl IntoResponse + crate::openapi::TypedResponse, ApiError> {
    let actor = state
        .authenticate_principal(&headers, &jar, super::SCOPE_WRITE)
        .await?;
    Ok((
        StatusCode::CREATED,
        Json::<InvitationResponse>(
            domain::create_invitation(
                &state.auth_service(),
                &actor,
                domain::CreateInvitationRequest {
                    expires_in_hours: request.expires_in_hours,
                },
            )
            .await?
            .into(),
        ),
    ))
}

#[derive(Serialize, ToSchema)]
pub struct PasskeyResponse {
    id: Uuid,
    name: String,
    created_at: chrono::DateTime<Utc>,
    last_used_at: Option<chrono::DateTime<Utc>>,
}

#[utoipa::path(
    get, path = "/me/passkeys",
    operation_id = "get_me_passkeys",
    summary = "List passkeys",
    tag = "Account",
    responses(
        (status = 200, description = "Success", body = Vec<PasskeyResponse>, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn list_passkeys(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<Vec<PasskeyResponse>>, ApiError> {
    let actor = state
        .authenticate_principal(&headers, &jar, super::SCOPE_READ)
        .await?;
    Ok(Json(
        domain::list_passkeys(&state.auth_service(), &actor)
            .await?
            .into_iter()
            .map(Into::into)
            .collect(),
    ))
}

#[derive(Deserialize, ToSchema)]
pub struct StartPasskeyRegistrationRequest {
    name: String,
}

#[derive(Serialize, ToSchema)]
pub struct PasskeyCreationResponse {
    challenge_id: String,
    /// WebAuthn `PublicKeyCredentialCreationOptions`, wrapped in `publicKey`.
    #[schema(value_type = Object)]
    options: CreationChallengeResponse,
}

#[utoipa::path(
    post, path = "/me/passkeys/register/start",
    operation_id = "post_me_passkeys_register_start",
    summary = "Start registering a passkey",
    tag = "Account",

    responses(
        (status = 200, description = "Success", body = PasskeyCreationResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn start_passkey_registration(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<StartPasskeyRegistrationRequest>,
) -> Result<Json<PasskeyCreationResponse>, ApiError> {
    domain::check_passkey_registration(&state.auth_service()).await?;

    let actor = state
        .authenticate_principal(&headers, &jar, super::SCOPE_WRITE)
        .await?;
    Ok(Json(
        domain::start_passkey_registration(
            &state.auth_service(),
            &actor,
            domain::StartPasskeyRegistrationRequest { name: request.name },
        )
        .await?
        .into(),
    ))
}

#[derive(Deserialize, ToSchema)]
pub struct FinishPasskeyRegistrationRequest {
    challenge_id: String,
    /// The WebAuthn `PublicKeyCredential` the browser produced.
    #[schema(value_type = Object)]
    credential: RegisterPublicKeyCredential,
}

#[utoipa::path(
    post, path = "/me/passkeys/register/finish",
    operation_id = "post_me_passkeys_register_finish",
    summary = "Finish registering a passkey",
    tag = "Account",

    responses(
        (status = 201, description = "Registered"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn finish_passkey_registration(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<FinishPasskeyRegistrationRequest>,
) -> Result<impl IntoResponse + crate::openapi::TypedResponse, ApiError> {
    let actor = state
        .authenticate_principal(&headers, &jar, super::SCOPE_WRITE)
        .await?;
    domain::finish_passkey_registration(
        &state.auth_service(),
        &actor,
        domain::FinishPasskeyRegistrationRequest {
            challenge_id: request.challenge_id,
            credential: request.credential,
        },
    )
    .await?;
    Ok(StatusCode::CREATED)
}

#[utoipa::path(
    delete, path = "/me/passkeys/{id}",
    operation_id = "delete_me_passkeys_id",
    summary = "Delete a passkey",
    tag = "Account",
    params(
        ("id" = Uuid, Path),
    ),
    responses(
        (status = 204, description = "Success"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn delete_passkey(
    State(state): State<IdentityState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<StatusCode, ApiError> {
    let actor = state
        .authenticate_principal(&headers, &jar, super::SCOPE_WRITE)
        .await?;
    domain::delete_passkey(&state.auth_service(), &actor, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct PasskeyRequestResponse {
    challenge_id: String,
    /// WebAuthn request options from webauthn-rs; external browser protocol.
    #[schema(value_type = Object)]
    options: RequestChallengeResponse,
}

#[utoipa::path(
    post, path = "/auth/passkeys/login/start",
    operation_id = "post_auth_passkeys_login_start",
    summary = "Start passkey login",
    tag = "Authentication",
    security(),
    responses(
        (status = 200, description = "Success", body = PasskeyRequestResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn start_passkey_login(
    State(state): State<IdentityState>,
) -> Result<Json<PasskeyRequestResponse>, ApiError> {
    Ok(Json(
        domain::start_passkey_login(&state.auth_service())
            .await?
            .into(),
    ))
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct FinishPasskeyLoginRequest {
    challenge_id: String,
    /// WebAuthn credential from the browser; external browser protocol.
    #[schema(value_type = Object)]
    credential: PublicKeyCredential,
}

#[utoipa::path(
    post, path = "/auth/passkeys/login/finish",
    operation_id = "post_auth_passkeys_login_finish",
    summary = "Finish passkey login",
    tag = "Authentication",

    security(),
    responses(
        (status = 200, description = "Success", body = AuthResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn finish_passkey_login(
    State(state): State<IdentityState>,
    jar: CookieJar,
    Json(request): Json<FinishPasskeyLoginRequest>,
) -> Result<impl IntoResponse + crate::openapi::TypedResponse, ApiError> {
    let grant = domain::finish_passkey_login(
        &state.auth_service(),
        domain::FinishPasskeyLoginRequest {
            challenge_id: request.challenge_id,
            credential: request.credential,
        },
    )
    .await?;
    Ok((
        jar.add(state.session_cookie(grant.token)),
        Json(AuthResponse {
            user: grant.user.into(),
        }),
    ))
}

use gitadel_identity::auth as domain;
impl From<domain::InvitationResponse> for InvitationResponse {
    fn from(value: domain::InvitationResponse) -> Self {
        Self {
            token: value.token,
            expires_at: value.expires_at,
        }
    }
}

#[cfg(test)]
use super::hash_secret;
#[cfg(test)]
use crate::entity::action_runner;
#[cfg(test)]
use crate::entity::action_runner_registration_token;
#[cfg(test)]
use crate::entity::action_secret;
#[cfg(test)]
use crate::entity::action_variable;
#[cfg(test)]
use crate::entity::namespace;
#[cfg(test)]
use crate::entity::namespace_integration;
#[cfg(test)]
use crate::entity::namespace_mirror_identity;
#[cfg(test)]
use crate::entity::namespace_pin;
#[cfg(test)]
use crate::entity::repository;
#[cfg(test)]
use crate::entity::repository_alias;
#[cfg(test)]
use crate::entity::user;
#[cfg(test)]
use sea_orm::EntityTrait;
#[cfg(test)]
use sea_orm::Set;
#[cfg(test)]
use sea_orm::TransactionTrait;
impl From<domain::PasskeyResponse> for PasskeyResponse {
    fn from(value: domain::PasskeyResponse) -> Self {
        Self {
            id: value.id,
            name: value.name,
            created_at: value.created_at,
            last_used_at: value.last_used_at,
        }
    }
}
impl From<domain::PasskeyCreationResponse> for PasskeyCreationResponse {
    fn from(value: domain::PasskeyCreationResponse) -> Self {
        Self {
            challenge_id: value.challenge_id,
            options: value.options,
        }
    }
}
impl From<domain::PasskeyRequestResponse> for PasskeyRequestResponse {
    fn from(value: domain::PasskeyRequestResponse) -> Self {
        Self {
            challenge_id: value.challenge_id,
            options: value.options,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::entity::session;
    use sea_orm::{DatabaseConnection, NotSet};

    use super::*;
    use crate::actions::test_support;

    use gitadel_db::test_support::database;

    /// Every namespace-scoped table a personal namespace can own rows in.
    async fn namespace_resources(database: &DatabaseConnection, owner: &user::Model) {
        let now = Utc::now();
        let slug = owner.username.clone();
        let repository = test_support::repository(database, owner, "project").await;
        repository_alias::ActiveModel {
            namespace: Set(slug.clone()),
            name: Set("former-name".to_owned()),
            repository_id: Set(repository.id),
            created_at: Set(now),
        }
        .insert(database)
        .await
        .unwrap();
        namespace_pin::ActiveModel {
            namespace: Set(slug.clone()),
            repository_id: Set(repository.id),
            position: Set(0),
            created_at: Set(now),
        }
        .insert(database)
        .await
        .unwrap();
        namespace_integration::ActiveModel {
            id: Set(Uuid::new_v4()),
            namespace: Set(slug.clone()),
            provider: Set("dokploy".to_owned()),
            name: Set("Dokploy".to_owned()),
            enabled: Set(true),
            url: Set("https://dokploy.example".to_owned()),
            api_key: Set("secret".to_owned()),
            dokploy_internal_url: Set(None),
            created_at: Set(now),
            updated_at: Set(now),
        }
        .insert(database)
        .await
        .unwrap();
        namespace_mirror_identity::ActiveModel {
            id: Set(Uuid::new_v4()),
            namespace: Set(slug.clone()),
            name: Set("Mirror token".to_owned()),
            kind: Set("token".to_owned()),
            username: Set(None),
            secret: Set("secret".to_owned()),
            provider: Set(Some("github".to_owned())),
            instance_url: Set(Some("https://api.github.com".to_owned())),
            public_key: Set(None),
            fingerprint: Set(None),
            last_used_at: Set(None),
            created_at: Set(now),
            updated_at: Set(now),
        }
        .insert(database)
        .await
        .unwrap();
        action_runner::ActiveModel {
            id: NotSet,
            uuid: Set(Uuid::new_v4().to_string()),
            namespace: Set(Some(slug.clone())),
            name: Set("runner".to_owned()),
            token_hash: Set("runner-token".to_owned()),
            approved_labels: Set("[]".to_owned()),
            version: Set("test".to_owned()),
            ephemeral: Set(false),
            disabled_at: Set(None),
            deleted_at: Set(None),
            last_seen_at: Set(None),
            created_by: Set(Some(owner.id)),
            created_at: Set(now),
        }
        .insert(database)
        .await
        .unwrap();
        action_runner_registration_token::ActiveModel {
            id: Set(Uuid::new_v4()),
            token_hash: Set("registration-token".to_owned()),
            namespace: Set(Some(slug.clone())),
            runner_name: Set("runner".to_owned()),
            approved_labels: Set("[]".to_owned()),
            expires_at: Set(now),
            used_at: Set(None),
            created_by: Set(Some(owner.id)),
            created_at: Set(now),
        }
        .insert(database)
        .await
        .unwrap();
        action_secret::ActiveModel {
            id: NotSet,
            repository_id: Set(None),
            namespace: Set(Some(slug.clone())),
            name: Set("TOKEN".to_owned()),
            sealed: Set(vec![1, 2, 3]),
            updated_by: Set(Some(owner.id)),
            created_at: Set(now),
            updated_at: Set(now),
        }
        .insert(database)
        .await
        .unwrap();
        action_variable::ActiveModel {
            id: NotSet,
            repository_id: Set(None),
            namespace: Set(Some(slug)),
            name: Set("REGION".to_owned()),
            value: Set("eu".to_owned()),
            updated_by: Set(Some(owner.id)),
            created_at: Set(now),
            updated_at: Set(now),
        }
        .insert(database)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn user_rename_moves_every_namespace_resource() {
        let database = database().await;
        let owner = test_support::owner(&database, "old-name").await;
        namespace_resources(&database, &owner).await;

        let transaction = database.begin().await.unwrap();
        let renamed = rename_user_namespace(&transaction, owner, "new-name")
            .await
            .unwrap();
        // Deferred foreign keys are checked here: a missed table fails the commit.
        transaction.commit().await.unwrap();

        assert_eq!(renamed.username, "new-name");
        assert!(
            namespace::Entity::find_by_id("old-name")
                .one(&database)
                .await
                .unwrap()
                .is_none()
        );
        macro_rules! namespace_of {
            ($entity:ident) => {
                $entity::Entity::find()
                    .one(&database)
                    .await
                    .unwrap()
                    .map(|row| Option::<String>::from(row.namespace))
            };
        }
        let moved = Some(Some("new-name".to_owned()));
        assert_eq!(namespace_of!(repository), moved);
        assert_eq!(namespace_of!(repository_alias), moved);
        assert_eq!(namespace_of!(namespace_pin), moved);
        assert_eq!(namespace_of!(namespace_integration), moved);
        assert_eq!(namespace_of!(namespace_mirror_identity), moved);
        assert_eq!(namespace_of!(action_runner), moved);
        assert_eq!(namespace_of!(action_runner_registration_token), moved);
        assert_eq!(namespace_of!(action_secret), moved);
        assert_eq!(namespace_of!(action_variable), moved);
    }

    #[tokio::test]
    async fn expired_sessions_are_swept_and_live_ones_kept() {
        let database = database().await;
        let owner = test_support::owner(&database, "owner").await;
        let now = Utc::now();
        for (token, expires_at) in [
            ("expired", now - chrono::Duration::seconds(1)),
            ("long-expired", now - chrono::Duration::days(90)),
            ("live", now + chrono::Duration::hours(1)),
        ] {
            session::ActiveModel {
                token_hash: Set(token.to_owned()),
                user_id: Set(owner.id),
                expires_at: Set(expires_at),
                created_at: Set(now),
                last_seen_at: Set(now),
            }
            .insert(&database)
            .await
            .unwrap();
        }

        let removed = session::delete_expired(&database, now).await.unwrap();

        let remaining = session::Entity::find().all(&database).await.unwrap();
        assert_eq!(removed, 2);
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].token_hash, "live");
    }

    #[test]
    fn credential_changes_refuse_api_tokens() {
        assert!(require_browser_session(false).is_ok());
        assert!(require_browser_session(true).is_err());
    }

    #[tokio::test]
    async fn user_rename_refuses_a_slug_another_namespace_owns() {
        let database = database().await;
        let owner = test_support::owner(&database, "old-name").await;
        test_support::owner(&database, "taken").await;

        let transaction = database.begin().await.unwrap();
        assert!(
            rename_user_namespace(&transaction, owner, "taken")
                .await
                .is_err()
        );
    }

    mod handlers {
        use axum::http::{Method, StatusCode, header};
        use sea_orm::{ActiveModelTrait, IntoActiveModel, Set};
        use serde_json::{Value, json};

        use super::super::*;
        use crate::{
            entity::api_token,
            identity::{
                SCOPE_READ, SCOPE_WRITE, SESSION_COOKIE,
                test_support::{Harness, PASSWORD},
            },
        };

        async fn login(harness: &Harness, username: &str, password: &str) -> (StatusCode, Value) {
            harness
                .request(
                    Method::POST,
                    "/api/v1/auth/login",
                    None,
                    Some(json!({ "username": username, "password": password })),
                )
                .await
        }

        #[tokio::test]
        async fn login_accepts_the_password_and_refuses_everything_else() {
            let harness = Harness::new().await;
            let alice = harness.user("alice").await;

            let request = axum::http::Request::builder()
                .method(Method::POST)
                .uri("/api/v1/auth/login")
                .header(header::CONTENT_TYPE, "application/json")
                .body(axum::body::Body::from(
                    json!({ "username": "Alice", "password": PASSWORD }).to_string(),
                ))
                .unwrap();
            let response = harness.send(request).await;
            assert_eq!(response.status(), StatusCode::OK);
            let cookie = response
                .headers()
                .get(header::SET_COOKIE)
                .expect("a session cookie is set")
                .to_str()
                .unwrap()
                .to_owned();
            assert!(cookie.starts_with(&format!("{SESSION_COOKIE}=")));
            assert!(cookie.contains("HttpOnly"));
            let body = crate::identity::test_support::json_body(response).await;
            assert_eq!(body["user"]["username"], "alice");

            // The cookie signs the user in.
            let session = cookie.split(';').next().unwrap().to_owned();
            let (status, body) = harness
                .request(
                    Method::GET,
                    "/api/v1/me/commit-identity",
                    Some(&session),
                    None,
                )
                .await;
            assert_eq!(status, StatusCode::OK, "{body}");

            let (status, body) = login(&harness, "alice", "not the password").await;
            assert_eq!(status, StatusCode::UNAUTHORIZED);
            assert_eq!(body["error"]["code"], "invalid_credentials");
            let (status, body) = login(&harness, "nobody", PASSWORD).await;
            assert_eq!(
                status,
                StatusCode::UNAUTHORIZED,
                "unknown users look the same"
            );
            assert_eq!(body["error"]["code"], "invalid_credentials");
            let (status, _) = login(&harness, "not a slug!", PASSWORD).await;
            assert_eq!(status, StatusCode::BAD_REQUEST);

            let mut disabled = alice.into_active_model();
            disabled.disabled_at = Set(Some(Utc::now()));
            disabled.update(&harness.database).await.unwrap();
            let (status, _) = login(&harness, "alice", PASSWORD).await;
            assert_eq!(
                status,
                StatusCode::UNAUTHORIZED,
                "disabled accounts cannot sign in"
            );
        }

        #[tokio::test]
        async fn repeated_failures_lock_the_account_even_for_the_right_password() {
            let harness = Harness::new().await;
            harness.user("alice").await;
            harness.user("bob").await;
            for _ in 0..8 {
                let (status, _) = login(&harness, "alice", "wrong password").await;
                assert_eq!(status, StatusCode::UNAUTHORIZED);
            }
            let (status, body) = login(&harness, "alice", PASSWORD).await;
            assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{body}");
            assert_eq!(body["error"]["code"], "rate_limited");
            // The lock is per account, not per instance.
            let (status, _) = login(&harness, "bob", PASSWORD).await;
            assert_eq!(status, StatusCode::OK);
        }

        #[tokio::test]
        async fn changing_the_password_keeps_this_session_and_ends_the_others() {
            let harness = Harness::new().await;
            let alice = harness.user("alice").await;
            let current = harness.login(&alice).await;
            let other = harness.login(&alice).await;
            let new_password = "a different long password";

            let (status, body) = harness
                .request(
                    Method::PUT,
                    "/api/v1/me/password",
                    Some(&current),
                    Some(json!({ "current_password": "wrong", "new_password": new_password })),
                )
                .await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
            let (status, body) = harness
                .request(
                    Method::PUT,
                    "/api/v1/me/password",
                    Some(&current),
                    Some(json!({ "current_password": PASSWORD, "new_password": "short" })),
                )
                .await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

            let (status, body) = harness
                .request(
                    Method::PUT,
                    "/api/v1/me/password",
                    Some(&current),
                    Some(json!({ "current_password": PASSWORD, "new_password": new_password })),
                )
                .await;
            assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

            let (status, _) = harness
                .request(
                    Method::GET,
                    "/api/v1/me/commit-identity",
                    Some(&current),
                    None,
                )
                .await;
            assert_eq!(status, StatusCode::OK, "the session that changed it stays");
            let (status, _) = harness
                .request(
                    Method::GET,
                    "/api/v1/me/commit-identity",
                    Some(&other),
                    None,
                )
                .await;
            assert_eq!(
                status,
                StatusCode::UNAUTHORIZED,
                "other sessions are signed out"
            );
            let (status, _) = login(&harness, "alice", PASSWORD).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED);
            let (status, _) = login(&harness, "alice", new_password).await;
            assert_eq!(status, StatusCode::OK);

            // API tokens cannot change credentials.
            let raw = "gat_test_token";
            api_token::ActiveModel {
                id: Set(Uuid::new_v4()),
                user_id: Set(alice.id),
                name: Set("script".to_owned()),
                token_hash: Set(hash_secret(raw)),
                scopes: Set(SCOPE_READ | SCOPE_WRITE),
                expires_at: Set(None),
                created_at: Set(Utc::now()),
                last_used_at: Set(None),
                revoked_at: Set(None),
            }
            .insert(&harness.database)
            .await
            .unwrap();
            let request = axum::http::Request::builder()
                .method(Method::PUT)
                .uri("/api/v1/me/password")
                .header(header::AUTHORIZATION, format!("Bearer {raw}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(axum::body::Body::from(
                    json!({ "current_password": new_password, "new_password": PASSWORD })
                        .to_string(),
                ))
                .unwrap();
            assert_eq!(harness.send(request).await.status(), StatusCode::FORBIDDEN);
        }

        #[tokio::test]
        async fn renaming_moves_the_personal_namespace_and_refuses_taken_names() {
            let harness = Harness::new().await;
            let alice = harness.user("alice").await;
            harness.user("taken").await;
            let cookie = harness.login(&alice).await;

            let (status, body) = harness
                .request(
                    Method::PUT,
                    "/api/v1/me/username",
                    Some(&cookie),
                    Some(json!({ "username": "Alice-Two" })),
                )
                .await;
            assert_eq!(status, StatusCode::OK, "{body}");
            assert_eq!(body["user"]["username"], "alice-two");
            let (status, body) = harness
                .request(Method::GET, "/api/v1/namespaces/alice-two", None, None)
                .await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(body["kind"], "user");
            let (status, _) = harness
                .request(Method::GET, "/api/v1/namespaces/alice", None, None)
                .await;
            assert_eq!(status, StatusCode::NOT_FOUND);
            // The session survives the rename.
            let (status, _) = harness
                .request(
                    Method::GET,
                    "/api/v1/me/commit-identity",
                    Some(&cookie),
                    None,
                )
                .await;
            assert_eq!(status, StatusCode::OK);

            let (status, body) = harness
                .request(
                    Method::PUT,
                    "/api/v1/me/username",
                    Some(&cookie),
                    Some(json!({ "username": "taken" })),
                )
                .await;
            assert_eq!(status, StatusCode::CONFLICT, "{body}");
            let (status, _) = harness
                .request(
                    Method::PUT,
                    "/api/v1/me/username",
                    Some(&cookie),
                    Some(json!({ "username": "-bad-" })),
                )
                .await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
            let (status, body) = login(&harness, "alice-two", PASSWORD).await;
            assert_eq!(status, StatusCode::OK, "{body}");
        }
    }
}
