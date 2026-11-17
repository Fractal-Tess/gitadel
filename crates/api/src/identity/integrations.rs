//! Namespace-owned integration instances.
//!
//! A namespace may connect any number of instances of one provider. Repository
//! settings reference an instance by ID, so labels and credentials can change
//! without breaking repository links.

use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode, header},
};
use axum_extra::extract::cookie::CookieJar;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use super::oauth::{ACCESS_TOKEN_LIFETIME, TokenPair};
use super::{ApiError, AuthenticatedUser, IdentityState, SCOPE_READ, SCOPE_WRITE};
use crate::secrets::SecretCipher;

#[derive(Serialize, ToSchema)]
pub struct ProviderResponse {
    slug: &'static str,
    name: &'static str,
    description: &'static str,
    icon: &'static str,
    source_required: bool,
}

#[derive(Serialize, ToSchema)]
pub struct IntegrationResponse {
    id: Uuid,
    provider: &'static str,
    provider_name: &'static str,
    name: String,
    enabled: bool,
    url: String,
    internal_url: String,
    api_key_set: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    source: Option<SourceBindingSummary>,
}

#[derive(Serialize, ToSchema)]
pub struct SourceBindingSummary {
    id: String,
    name: String,
    managed: bool,
}

#[derive(Serialize, ToSchema)]
pub struct NamespaceIntegrationsResponse {
    namespace: String,
    providers: Vec<ProviderResponse>,
    integrations: Vec<IntegrationResponse>,
}
#[derive(Serialize, ToSchema)]
pub struct IntegrationCredentialResponse {
    api_key: String,
}

#[derive(Serialize, ToSchema)]
pub struct RemoteSourceResponse {
    id: String,
    name: String,
    ready: bool,
    url: Option<String>,
    /// False when the provider was configured for another forge.
    targets_instance: bool,
    /// Why this provider cannot be used by this Gitadel, if it cannot.
    problem: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct SourceConnectionResponse {
    required: bool,
    binding: Option<SourceBindingStatus>,
    sources: Vec<RemoteSourceResponse>,
    /// Set when this response follows a reachability check.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    check: Option<SourceCheckResponse>,
}

/// Whether Dokploy can list repositories through the bound provider.
#[derive(Serialize, ToSchema)]
pub struct SourceCheckResponse {
    reachable: bool,
    /// The Gitadel address Dokploy uses after the check.
    internal_url: String,
    repositories: Option<usize>,
    /// Addresses that failed, in the order they were tried.
    attempts: Vec<SourceAttempt>,
}

#[derive(Serialize, ToSchema)]
pub struct SourceAttempt {
    url: String,
    error: String,
}

#[derive(Serialize, ToSchema)]
pub struct SourceBindingStatus {
    id: String,
    name: String,
    managed: bool,
    ready: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    authorization_url: Option<String>,
    url: Option<String>,
    targets_instance: bool,
    problem: Option<String>,
}

#[derive(Deserialize, ToSchema)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum ConfigureSourceRequest {
    Bind { source_id: String },
    Create { name: String },
}

#[derive(Deserialize, ToSchema)]
pub struct CreateIntegrationRequest {
    provider: String,
    name: String,
    url: String,
    api_key: String,
    internal_url: String,
}

#[derive(Deserialize, ToSchema)]
pub struct UpdateIntegrationRequest {
    name: Option<String>,
    enabled: Option<bool>,
    url: Option<String>,
    api_key: Option<String>,
    internal_url: Option<String>,
}

#[derive(Deserialize, ToSchema)]
pub struct TestIntegrationRequest {
    provider: String,
    url: String,
    api_key: String,
}

#[derive(Serialize, ToSchema)]
pub struct IntegrationTestResponse {
    account: Option<String>,
    warnings: Vec<String>,
}

#[utoipa::path(
    get, path = "/namespaces/{slug}/integrations",
    operation_id = "get_namespaces_slug_integrations",
    summary = "List the namespace's connections and available providers",
    tag = "Deployments",
    params(
        ("slug" = String, Path),
    ),
    responses(
        (status = 200, description = "Success", body = NamespaceIntegrationsResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn list_integrations(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(slug): Path<String>,
) -> Result<Json<NamespaceIntegrationsResponse>, ApiError> {
    let _actor = state.authenticate(&headers, &jar, SCOPE_READ).await?;
    Ok(Json(NamespaceIntegrationsResponse::from(
        service(&state)
            .list_integrations(&crate::repository::principal(&headers, &jar)?, slug)
            .await?,
    )))
}
#[utoipa::path(
    post, path = "/namespaces/{slug}/integrations",
    operation_id = "post_namespaces_slug_integrations",
    summary = "Add a connection",
    tag = "Deployments",

    params(
        ("slug" = String, Path),
    ),
    responses(
        (status = 201, description = "Success", body = IntegrationResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn create_integration(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(slug): Path<String>,
    Json(request): Json<CreateIntegrationRequest>,
) -> Result<(StatusCode, Json<IntegrationResponse>), ApiError> {
    let _actor = state.authenticate(&headers, &jar, SCOPE_WRITE).await?;
    Ok((
        StatusCode::CREATED,
        Json(IntegrationResponse::from(
            service(&state)
                .create_integration(
                    &crate::repository::principal(&headers, &jar)?,
                    slug,
                    request.into(),
                )
                .await?,
        )),
    ))
}
#[utoipa::path(
    put, path = "/namespaces/{slug}/integrations/{id}",
    operation_id = "put_namespaces_slug_integrations_id",
    summary = "Update a connection",
    tag = "Deployments",

    params(
        ("slug" = String, Path),
        ("id" = Uuid, Path),
    ),
    responses(
        (status = 200, description = "Success", body = IntegrationResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn update_integration(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((slug, id)): Path<(String, Uuid)>,
    Json(request): Json<UpdateIntegrationRequest>,
) -> Result<Json<IntegrationResponse>, ApiError> {
    let _actor = state.authenticate(&headers, &jar, SCOPE_WRITE).await?;
    Ok(Json(IntegrationResponse::from(
        service(&state)
            .update_integration(
                &crate::repository::principal(&headers, &jar)?,
                slug,
                id,
                request.into(),
            )
            .await?,
    )))
}
#[utoipa::path(
    delete, path = "/namespaces/{slug}/integrations/{id}",
    operation_id = "delete_namespaces_slug_integrations_id",
    summary = "Remove a connection and the provider Gitadel created for it",
    tag = "Deployments",
    params(
        ("slug" = String, Path),
        ("id" = Uuid, Path),
    ),
    responses(
        (status = 204, description = "Success"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn delete_integration(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((slug, id)): Path<(String, Uuid)>,
) -> Result<StatusCode, ApiError> {
    let _actor = state.authenticate(&headers, &jar, SCOPE_WRITE).await?;
    service(&state)
        .delete_integration(&crate::repository::principal(&headers, &jar)?, slug, id)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
/// Return one stored key only to the namespace owner and prevent caching.
#[utoipa::path(
    get, path = "/namespaces/{slug}/integrations/{id}/credential",
    operation_id = "get_namespaces_slug_integrations_id_credential",
    summary = "Reveal a connection's stored API key",
    tag = "Deployments",
    params(
        ("slug" = String, Path),
        ("id" = Uuid, Path),
    ),
    responses(
        (status = 200, description = "Success", body = IntegrationCredentialResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn get_integration_credential(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((slug, id)): Path<(String, Uuid)>,
) -> Result<
    (
        [(axum::http::HeaderName, &'static str); 1],
        Json<IntegrationCredentialResponse>,
    ),
    ApiError,
> {
    let _actor = state.authenticate(&headers, &jar, SCOPE_WRITE).await?;
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Json(IntegrationCredentialResponse::from(
            service(&state)
                .get_integration_credential(
                    &crate::repository::principal(&headers, &jar)?,
                    slug,
                    id,
                )
                .await?,
        )),
    ))
}
#[utoipa::path(
    get, path = "/namespaces/{slug}/integrations/{id}/source",
    operation_id = "get_namespaces_slug_integrations_id_source",
    summary = "Show the connection's repository source and the providers it could use",
    tag = "Deployments",
    params(
        ("slug" = String, Path),
        ("id" = Uuid, Path),
    ),
    responses(
        (status = 200, description = "Success", body = SourceConnectionResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn get_integration_source(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((slug, id)): Path<(String, Uuid)>,
) -> Result<Json<SourceConnectionResponse>, ApiError> {
    let _actor = state.authenticate(&headers, &jar, SCOPE_READ).await?;
    Ok(Json(SourceConnectionResponse::from(
        service(&state)
            .get_integration_source(&crate::repository::principal(&headers, &jar)?, slug, id)
            .await?,
    )))
}
#[utoipa::path(
    post, path = "/namespaces/{slug}/integrations/{id}/source",
    operation_id = "post_namespaces_slug_integrations_id_source",
    summary = "Connect a repository source, creating a provider with Gitadel-issued tokens or binding an existing one",
    tag = "Deployments",

    params(
        ("slug" = String, Path),
        ("id" = Uuid, Path),
    ),
    responses(
        (status = 201, description = "Success", body = SourceConnectionResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn configure_integration_source(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((slug, id)): Path<(String, Uuid)>,
    Json(request): Json<ConfigureSourceRequest>,
) -> Result<(StatusCode, Json<SourceConnectionResponse>), ApiError> {
    let _actor = state.authenticate(&headers, &jar, SCOPE_WRITE).await?;
    Ok((
        StatusCode::CREATED,
        Json(SourceConnectionResponse::from(
            service(&state)
                .configure_integration_source(
                    &crate::repository::principal(&headers, &jar)?,
                    slug,
                    id,
                    request.into(),
                )
                .await?,
        )),
    ))
}
/// Check that Dokploy reaches Gitadel through the bound provider. When it
/// cannot and Gitadel manages the provider, try the other addresses this
/// instance answers on and keep the first one that works.
#[utoipa::path(
    post, path = "/namespaces/{slug}/integrations/{id}/source/verify",
    operation_id = "post_namespaces_slug_integrations_id_source_verify",
    summary = "Check that the remote reaches Gitadel through the source, trying other addresses if needed",
    tag = "Deployments",
    params(
        ("slug" = String, Path),
        ("id" = Uuid, Path),
    ),
    responses(
        (status = 200, description = "Success", body = SourceCheckResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn verify_integration_source(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((slug, id)): Path<(String, Uuid)>,
) -> Result<Json<SourceCheckResponse>, ApiError> {
    let _actor = state.authenticate(&headers, &jar, SCOPE_WRITE).await?;
    Ok(Json(SourceCheckResponse::from(
        service(&state)
            .verify_integration_source(&crate::repository::principal(&headers, &jar)?, slug, id)
            .await?,
    )))
}
#[utoipa::path(
    delete, path = "/namespaces/{slug}/integrations/{id}/source",
    operation_id = "delete_namespaces_slug_integrations_id_source",
    summary = "Disconnect the repository source",
    tag = "Deployments",
    params(
        ("slug" = String, Path),
        ("id" = Uuid, Path),
    ),
    responses(
        (status = 204, description = "Success"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn disconnect_integration_source(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((slug, id)): Path<(String, Uuid)>,
) -> Result<StatusCode, ApiError> {
    let _actor = state.authenticate(&headers, &jar, SCOPE_WRITE).await?;
    service(&state)
        .disconnect_integration_source(&crate::repository::principal(&headers, &jar)?, slug, id)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
/// Verify credentials before storing a new namespace integration.
#[utoipa::path(
    post, path = "/namespaces/{slug}/integrations/test",
    operation_id = "post_namespaces_slug_integrations_test",
    summary = "Test credentials before saving a connection",
    tag = "Deployments",

    params(
        ("slug" = String, Path),
    ),
    responses(
        (status = 200, description = "Success", body = IntegrationTestResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn test_new_integration(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(slug): Path<String>,
    Json(request): Json<TestIntegrationRequest>,
) -> Result<Json<IntegrationTestResponse>, ApiError> {
    let _actor = state.authenticate(&headers, &jar, SCOPE_WRITE).await?;
    Ok(Json(IntegrationTestResponse::from(
        service(&state)
            .test_new_integration(
                &crate::repository::principal(&headers, &jar)?,
                slug,
                request.into(),
            )
            .await?,
    )))
}
/// Verify one stored credential without returning the key in the response.
#[utoipa::path(
    post, path = "/namespaces/{slug}/integrations/{id}/test",
    operation_id = "post_namespaces_slug_integrations_id_test",
    summary = "Test a saved connection",
    tag = "Deployments",
    params(
        ("slug" = String, Path),
        ("id" = Uuid, Path),
    ),
    responses(
        (status = 200, description = "Success", body = IntegrationTestResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn test_integration(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((slug, id)): Path<(String, Uuid)>,
) -> Result<Json<IntegrationTestResponse>, ApiError> {
    let _actor = state.authenticate(&headers, &jar, SCOPE_READ).await?;
    Ok(Json(IntegrationTestResponse::from(
        service(&state)
            .test_integration(&crate::repository::principal(&headers, &jar)?, slug, id)
            .await?,
    )))
}
/// Only a namespace's owner may read or change its credentials.
pub(crate) async fn authorize_namespace(
    state: &IdentityState,
    actor: &AuthenticatedUser,
    slug: &str,
) -> Result<(), ApiError> {
    Ok(gitadel_identity::permissions::authorize_namespace(state.database(), actor, slug).await?)
}

fn service(state: &IdentityState) -> gitadel_integrations::connections::ConnectionService<'_> {
    gitadel_integrations::connections::ConnectionService::new(
        state.database(),
        state.secret_cipher(),
        &state.public_url,
        &LegacySourceTokens,
        gitadel_core::audit::AuditContext {
            remote_address: super::remote_address(),
        },
    )
}
struct LegacySourceTokens;
#[async_trait::async_trait]
impl gitadel_integrations::connections::SourceTokenPort for LegacySourceTokens {
    fn generate(&self) -> gitadel_integrations::connections::SourceTokens {
        let pair = TokenPair::generate();
        gitadel_integrations::connections::SourceTokens {
            access_token: pair.access_token,
            refresh_token: pair.refresh_token,
        }
    }
    fn access_token_lifetime(&self) -> chrono::Duration {
        ACCESS_TOKEN_LIFETIME
    }
    async fn store(
        &self,
        tokens: &gitadel_integrations::connections::SourceTokens,
        transaction: &sea_orm::DatabaseTransaction,
        application_id: Uuid,
        user_id: Uuid,
        scope: &str,
    ) -> Result<(), gitadel_identity::IdentityError> {
        TokenPair {
            access_token: tokens.access_token.clone(),
            refresh_token: tokens.refresh_token.clone(),
        }
        .store(transaction, application_id, user_id, scope)
        .await?;
        Ok(())
    }
}
pub(crate) fn open_api_key(
    cipher: &SecretCipher,
    integration: crate::entity::namespace_integration::Model,
) -> Result<crate::entity::namespace_integration::Model, ApiError> {
    Ok(gitadel_integrations::connections::open_api_key(
        cipher,
        integration,
    )?)
}
pub(crate) fn validate_name(value: &str) -> Result<String, ApiError> {
    Ok(gitadel_integrations::connections::validate_name(value)?)
}

impl From<gitadel_integrations::connections::ProviderResponse> for ProviderResponse {
    fn from(value: gitadel_integrations::connections::ProviderResponse) -> Self {
        Self {
            slug: value.slug,
            name: value.name,
            description: value.description,
            icon: value.icon,
            source_required: value.source_required,
        }
    }
}

impl From<gitadel_integrations::connections::IntegrationResponse> for IntegrationResponse {
    fn from(value: gitadel_integrations::connections::IntegrationResponse) -> Self {
        Self {
            id: value.id,
            provider: value.provider,
            provider_name: value.provider_name,
            name: value.name,
            enabled: value.enabled,
            url: value.url,
            internal_url: value.internal_url,
            api_key_set: value.api_key_set,
            source: value.source.map(Into::into),
        }
    }
}

impl From<gitadel_integrations::connections::SourceBindingSummary> for SourceBindingSummary {
    fn from(value: gitadel_integrations::connections::SourceBindingSummary) -> Self {
        Self {
            id: value.id,
            name: value.name,
            managed: value.managed,
        }
    }
}

impl From<gitadel_integrations::connections::NamespaceIntegrationsResponse>
    for NamespaceIntegrationsResponse
{
    fn from(value: gitadel_integrations::connections::NamespaceIntegrationsResponse) -> Self {
        Self {
            namespace: value.namespace,
            providers: value.providers.into_iter().map(Into::into).collect(),
            integrations: value.integrations.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<gitadel_integrations::connections::IntegrationCredentialResponse>
    for IntegrationCredentialResponse
{
    fn from(value: gitadel_integrations::connections::IntegrationCredentialResponse) -> Self {
        Self {
            api_key: value.api_key,
        }
    }
}

impl From<gitadel_integrations::connections::RemoteSourceResponse> for RemoteSourceResponse {
    fn from(value: gitadel_integrations::connections::RemoteSourceResponse) -> Self {
        Self {
            id: value.id,
            name: value.name,
            ready: value.ready,
            url: value.url,
            targets_instance: value.targets_instance,
            problem: value.problem,
        }
    }
}

impl From<gitadel_integrations::connections::SourceConnectionResponse>
    for SourceConnectionResponse
{
    fn from(value: gitadel_integrations::connections::SourceConnectionResponse) -> Self {
        Self {
            required: value.required,
            binding: value.binding.map(Into::into),
            sources: value.sources.into_iter().map(Into::into).collect(),
            check: value.check.map(Into::into),
        }
    }
}

impl From<gitadel_integrations::connections::SourceCheckResponse> for SourceCheckResponse {
    fn from(value: gitadel_integrations::connections::SourceCheckResponse) -> Self {
        Self {
            reachable: value.reachable,
            internal_url: value.internal_url,
            repositories: value.repositories,
            attempts: value.attempts.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<gitadel_integrations::connections::SourceAttempt> for SourceAttempt {
    fn from(value: gitadel_integrations::connections::SourceAttempt) -> Self {
        Self {
            url: value.url,
            error: value.error,
        }
    }
}

impl From<gitadel_integrations::connections::SourceBindingStatus> for SourceBindingStatus {
    fn from(value: gitadel_integrations::connections::SourceBindingStatus) -> Self {
        Self {
            id: value.id,
            name: value.name,
            managed: value.managed,
            ready: value.ready,
            authorization_url: value.authorization_url,
            url: value.url,
            targets_instance: value.targets_instance,
            problem: value.problem,
        }
    }
}

impl From<CreateIntegrationRequest>
    for gitadel_integrations::connections::CreateIntegrationRequest
{
    fn from(value: CreateIntegrationRequest) -> Self {
        Self {
            provider: value.provider,
            name: value.name,
            url: value.url,
            api_key: value.api_key,
            internal_url: value.internal_url,
        }
    }
}

impl From<UpdateIntegrationRequest>
    for gitadel_integrations::connections::UpdateIntegrationRequest
{
    fn from(value: UpdateIntegrationRequest) -> Self {
        Self {
            name: value.name,
            enabled: value.enabled,
            url: value.url,
            api_key: value.api_key,
            internal_url: value.internal_url,
        }
    }
}

impl From<TestIntegrationRequest> for gitadel_integrations::connections::TestIntegrationRequest {
    fn from(value: TestIntegrationRequest) -> Self {
        Self {
            provider: value.provider,
            url: value.url,
            api_key: value.api_key,
        }
    }
}

impl From<gitadel_integrations::connections::IntegrationTestResponse> for IntegrationTestResponse {
    fn from(value: gitadel_integrations::connections::IntegrationTestResponse) -> Self {
        Self {
            account: value.account,
            warnings: value.warnings,
        }
    }
}
impl From<ConfigureSourceRequest> for gitadel_integrations::connections::ConfigureSourceRequest {
    fn from(value: ConfigureSourceRequest) -> Self {
        match value {
            ConfigureSourceRequest::Bind { source_id } => Self::Bind { source_id },
            ConfigureSourceRequest::Create { name } => Self::Create { name },
        }
    }
}
