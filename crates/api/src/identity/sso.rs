use axum::{
    Json,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Redirect, Response},
};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{ApiError, IdentityState, SCOPE_READ, SCOPE_WRITE};

const OIDC_TRANSACTION_COOKIE: &str = "gitadel_oidc_transaction";
#[derive(Clone, Serialize, utoipa::ToSchema)]
pub struct PublicOidcProvider {
    id: Uuid,
    name: String,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct AuthenticationConfiguration {
    pub(super) password_enabled: bool,
    pub(super) passkey_enabled: bool,
    providers: Vec<PublicOidcProvider>,
}

#[derive(Deserialize, utoipa::ToSchema, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct StartQuery {
    #[serde(default, rename = "returnTo")]
    return_to: Option<String>,
}

#[utoipa::path(
    get, path = "/auth/oidc/{provider_id}/start",
    operation_id = "get_auth_oidc_provider_id_start",
    summary = "Start",
    tag = "Authentication",
    params(
        ("provider_id" = Uuid, Path),
        StartQuery,
    ),
    security(),
    responses(
        (status = 307, description = "Success"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn start(
    State(state): State<IdentityState>,
    Path(provider_id): Path<Uuid>,
    jar: CookieJar,
    Query(query): Query<StartQuery>,
) -> Result<impl IntoResponse + crate::openapi::TypedResponse, ApiError> {
    let (transaction_secret, authorization_url) = domain::start(
        &state.auth_service(),
        provider_id,
        domain::StartQuery {
            return_to: query.return_to,
        },
    )
    .await?;
    let cookie = Cookie::build((OIDC_TRANSACTION_COOKIE, transaction_secret))
        .path("/api/v1/auth/oidc")
        .http_only(true)
        .same_site(SameSite::Lax)
        .secure(state.public_url().scheme() == "https")
        .max_age(time::Duration::minutes(10))
        .build();
    Ok((jar.add(cookie), Redirect::to(&authorization_url)))
}

#[derive(Deserialize, utoipa::ToSchema, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

#[utoipa::path(
    get, path = "/auth/oidc/{provider_id}/callback",
    operation_id = "get_auth_oidc_provider_id_callback",
    summary = "Callback",
    tag = "Authentication",
    params(
        ("provider_id" = Uuid, Path),
        CallbackQuery,
    ),
    security(),
    responses(
        (status = 303, description = "Success"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn callback(
    State(state): State<IdentityState>,
    Path(provider_id): Path<Uuid>,
    jar: CookieJar,
    Query(query): Query<CallbackQuery>,
) -> Response {
    let transaction_secret = jar
        .get(OIDC_TRANSACTION_COOKIE)
        .map(|cookie| cookie.value().to_owned());
    let removal = Cookie::build(OIDC_TRANSACTION_COOKIE)
        .path("/api/v1/auth/oidc")
        .build();
    match complete_callback(&state, provider_id, query, transaction_secret.as_deref()).await {
        Ok((session, return_to)) => {
            (jar.remove(removal).add(session), Redirect::to(&return_to)).into_response()
        }
        Err(error) => (jar.remove(removal), error).into_response(),
    }
}

async fn complete_callback(
    state: &IdentityState,
    provider_id: Uuid,
    query: CallbackQuery,
    transaction_secret: Option<&str>,
) -> Result<(Cookie<'static>, String), ApiError> {
    let (token, return_to) = domain::complete_callback(
        &state.auth_service(),
        provider_id,
        domain::CallbackQuery {
            state: query.state,
            code: query.code,
            error: query.error,
            error_description: query.error_description,
        },
        transaction_secret,
    )
    .await?;
    Ok((state.session_cookie(token), return_to))
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct AdminOidcProvider {
    id: Uuid,
    name: String,
    issuer_url: String,
    client_id: String,
    has_client_secret: bool,
    enabled: bool,
    auto_provision: bool,
    callback_url: String,
    created_at: chrono::DateTime<Utc>,
    updated_at: chrono::DateTime<Utc>,
}

#[utoipa::path(
    get, path = "/admin/authentication",
    operation_id = "get_admin_authentication",
    summary = "Admin configuration",
    tag = "Administration",
    responses(
        (status = 200, description = "Success", body = AuthenticationConfiguration, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn admin_configuration(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<AuthenticationConfiguration>, ApiError> {
    let actor = super::admin::require_admin_principal(&state, &headers, &jar, SCOPE_READ).await?;
    Ok(Json(
        domain::admin_configuration(&state.auth_service(), &actor)
            .await?
            .into(),
    ))
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct UpdateAuthenticationConfiguration {
    password_enabled: bool,
    passkey_enabled: bool,
}

#[utoipa::path(
    put, path = "/admin/authentication",
    operation_id = "put_admin_authentication",
    summary = "Update configuration",
    tag = "Administration",

    responses(
        (status = 200, description = "Success", body = AuthenticationConfiguration, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn update_configuration(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<UpdateAuthenticationConfiguration>,
) -> Result<Json<AuthenticationConfiguration>, ApiError> {
    let actor = super::admin::require_admin_principal(&state, &headers, &jar, SCOPE_WRITE).await?;
    Ok(Json(
        domain::update_configuration(
            &state.auth_service(),
            &actor,
            domain::UpdateAuthenticationConfiguration {
                password_enabled: request.password_enabled,
                passkey_enabled: request.passkey_enabled,
            },
        )
        .await?
        .into(),
    ))
}

#[utoipa::path(
    get, path = "/admin/authentication/providers",
    operation_id = "get_admin_authentication_providers",
    summary = "List providers",
    tag = "Administration",
    responses(
        (status = 200, description = "Success", body = Vec<AdminOidcProvider>, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn list_providers(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<Vec<AdminOidcProvider>>, ApiError> {
    let actor = super::admin::require_admin_principal(&state, &headers, &jar, SCOPE_READ).await?;
    Ok(Json(
        domain::list_providers(&state.auth_service(), &actor)
            .await?
            .into_iter()
            .map(Into::into)
            .collect(),
    ))
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct SaveProviderRequest {
    name: String,
    issuer_url: String,
    client_id: String,
    client_secret: Option<String>,
    enabled: bool,
    auto_provision: bool,
}

#[utoipa::path(
    post, path = "/admin/authentication/providers",
    operation_id = "post_admin_authentication_providers",
    summary = "Create provider",
    tag = "Administration",

    responses(
        (status = 201, description = "Success", body = AdminOidcProvider, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn create_provider(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<SaveProviderRequest>,
) -> Result<(StatusCode, Json<AdminOidcProvider>), ApiError> {
    let actor = super::admin::require_admin_principal(&state, &headers, &jar, SCOPE_WRITE).await?;
    Ok((
        StatusCode::CREATED,
        Json(
            domain::create_provider(
                &state.auth_service(),
                &actor,
                domain::SaveProviderRequest {
                    name: request.name,
                    issuer_url: request.issuer_url,
                    client_id: request.client_id,
                    client_secret: request.client_secret,
                    enabled: request.enabled,
                    auto_provision: request.auto_provision,
                },
            )
            .await?
            .into(),
        ),
    ))
}

#[utoipa::path(
    put, path = "/admin/authentication/providers/{provider_id}",
    operation_id = "put_admin_authentication_providers_provider_id",
    summary = "Update provider",
    tag = "Administration",

    params(
        ("provider_id" = Uuid, Path),
    ),
    responses(
        (status = 200, description = "Success", body = AdminOidcProvider, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn update_provider(
    State(state): State<IdentityState>,
    Path(provider_id): Path<Uuid>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<SaveProviderRequest>,
) -> Result<Json<AdminOidcProvider>, ApiError> {
    let actor = super::admin::require_admin_principal(&state, &headers, &jar, SCOPE_WRITE).await?;
    Ok(Json(
        domain::update_provider(
            &state.auth_service(),
            &actor,
            provider_id,
            domain::SaveProviderRequest {
                name: request.name,
                issuer_url: request.issuer_url,
                client_id: request.client_id,
                client_secret: request.client_secret,
                enabled: request.enabled,
                auto_provision: request.auto_provision,
            },
        )
        .await?
        .into(),
    ))
}

#[utoipa::path(
    delete, path = "/admin/authentication/providers/{provider_id}",
    operation_id = "delete_admin_authentication_providers_provider_id",
    summary = "Delete provider",
    tag = "Administration",
    params(
        ("provider_id" = Uuid, Path),
    ),
    responses(
        (status = 204, description = "Success"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn delete_provider(
    State(state): State<IdentityState>,
    Path(provider_id): Path<Uuid>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<axum::http::StatusCode, ApiError> {
    let actor = super::admin::require_admin_principal(&state, &headers, &jar, SCOPE_WRITE).await?;
    domain::delete_provider(&state.auth_service(), &actor, provider_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// The stored form of a client secret: sealed to this provider, or empty for
/// a public client.
use gitadel_identity::sso as domain;
impl From<domain::PublicOidcProvider> for PublicOidcProvider {
    fn from(value: domain::PublicOidcProvider) -> Self {
        Self {
            id: value.id,
            name: value.name,
        }
    }
}
impl From<domain::AuthenticationConfiguration> for AuthenticationConfiguration {
    fn from(value: domain::AuthenticationConfiguration) -> Self {
        Self {
            password_enabled: value.password_enabled,
            passkey_enabled: value.passkey_enabled,
            providers: value.providers.into_iter().map(Into::into).collect(),
        }
    }
}
impl From<domain::AdminOidcProvider> for AdminOidcProvider {
    fn from(value: domain::AdminOidcProvider) -> Self {
        Self {
            id: value.id,
            name: value.name,
            issuer_url: value.issuer_url,
            client_id: value.client_id,
            has_client_secret: value.has_client_secret,
            enabled: value.enabled,
            auto_provision: value.auto_provision,
            callback_url: value.callback_url,
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}
