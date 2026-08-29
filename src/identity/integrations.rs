//! Namespace-owned integration instances.
//!
//! A namespace may connect any number of instances of one provider. Repository
//! settings reference an instance by ID, so labels and credentials can change
//! without breaking repository links.

use std::collections::HashMap;

use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode, header},
};
use axum_extra::extract::cookie::CookieJar;
use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, IntoActiveModel, QueryFilter,
    QueryOrder, Set, TransactionTrait,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::entity::{
    dokploy_source_binding, namespace, namespace_integration, oauth_application,
    organization_member,
};
use crate::identity::{hash_secret, random_secret};
use crate::integrations::{
    Credential, PROVIDERS, Provider, RemoteSource, SourceApplication, TestReport, provider,
};

use super::{ApiError, AuthenticatedUser, IdentityState, SCOPE_READ, SCOPE_WRITE};

const MAX_API_KEY_LENGTH: usize = 512;
const MAX_NAME_LENGTH: usize = 80;

#[derive(Serialize)]
pub struct ProviderResponse {
    slug: &'static str,
    name: &'static str,
    description: &'static str,
    icon: &'static str,
    source_required: bool,
}

impl From<&'static Provider> for ProviderResponse {
    fn from(provider: &'static Provider) -> Self {
        Self {
            slug: provider.slug,
            name: provider.name,
            description: provider.description,
            icon: provider.icon,
            source_required: provider.source_required,
        }
    }
}

#[derive(Serialize)]
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
    source: Option<SourceBindingSummary>,
}

#[derive(Serialize)]
pub struct SourceBindingSummary {
    id: String,
    name: String,
    managed: bool,
}

impl From<&dokploy_source_binding::Model> for SourceBindingSummary {
    fn from(binding: &dokploy_source_binding::Model) -> Self {
        Self {
            id: binding.gitea_id.clone(),
            name: "Gitea provider".to_owned(),
            managed: binding.managed,
        }
    }
}

impl IntegrationResponse {
    fn new(
        provider: &'static Provider,
        integration: namespace_integration::Model,
        binding: Option<&dokploy_source_binding::Model>,
        default_internal_url: &str,
    ) -> Self {
        let internal_url = integration
            .dokploy_internal_url
            .clone()
            .unwrap_or_else(|| default_internal_url.to_owned());
        Self {
            id: integration.id,
            provider: provider.slug,
            provider_name: provider.name,
            name: integration.name,
            enabled: integration.enabled,
            url: integration.url,
            internal_url,
            api_key_set: true,
            source: binding.map(SourceBindingSummary::from),
        }
    }
}

#[derive(Serialize)]
pub struct NamespaceIntegrationsResponse {
    namespace: String,
    providers: Vec<ProviderResponse>,
    integrations: Vec<IntegrationResponse>,
}
#[derive(Serialize)]
pub struct IntegrationCredentialResponse {
    api_key: String,
}

#[derive(Serialize)]
pub struct RemoteSourceResponse {
    id: String,
    name: String,
    ready: bool,
}

impl From<&RemoteSource> for RemoteSourceResponse {
    fn from(source: &RemoteSource) -> Self {
        Self {
            id: source.id.clone(),
            name: source.name.clone(),
            ready: source.ready,
        }
    }
}

#[derive(Serialize)]
pub struct SourceConnectionResponse {
    required: bool,
    binding: Option<SourceBindingStatus>,
    sources: Vec<RemoteSourceResponse>,
}

#[derive(Serialize)]
pub struct SourceBindingStatus {
    id: String,
    name: String,
    managed: bool,
    ready: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    authorization_url: Option<String>,
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum ConfigureSourceRequest {
    Bind { source_id: String },
    Create { name: String },
}

#[derive(Deserialize)]
pub struct CreateIntegrationRequest {
    provider: String,
    name: String,
    url: String,
    api_key: String,
    internal_url: String,
}

#[derive(Deserialize)]
pub struct UpdateIntegrationRequest {
    name: Option<String>,
    enabled: Option<bool>,
    url: Option<String>,
    api_key: Option<String>,
    internal_url: Option<String>,
}

#[derive(Deserialize)]
pub struct TestIntegrationRequest {
    provider: String,
    url: String,
    api_key: String,
}

#[derive(Serialize)]
pub struct IntegrationTestResponse {
    account: Option<String>,
    warnings: Vec<String>,
}

pub async fn list_integrations(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(slug): Path<String>,
) -> Result<Json<NamespaceIntegrationsResponse>, ApiError> {
    let actor = state.authenticate(&headers, &jar, SCOPE_READ).await?;
    authorize_namespace(&state, &actor, &slug).await?;
    let stored = namespace_integration::Entity::find()
        .filter(namespace_integration::Column::Namespace.eq(slug.clone()))
        .order_by_asc(namespace_integration::Column::CreatedAt)
        .all(state.database())
        .await?;
    let bindings = dokploy_source_binding::Entity::find()
        .filter(
            dokploy_source_binding::Column::IntegrationId
                .is_in(stored.iter().map(|integration| integration.id)),
        )
        .all(state.database())
        .await?
        .into_iter()
        .map(|binding| (binding.integration_id, binding))
        .collect::<HashMap<_, _>>();
    let integrations = stored
        .into_iter()
        .filter_map(|row| {
            provider(&row.provider).map(|provider| {
                let binding = bindings.get(&row.id);
                IntegrationResponse::new(provider, row, binding, state.public_url.as_str())
            })
        })
        .collect();
    Ok(Json(NamespaceIntegrationsResponse {
        namespace: slug,
        providers: PROVIDERS.iter().map(ProviderResponse::from).collect(),
        integrations,
    }))
}

pub async fn create_integration(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(slug): Path<String>,
    Json(request): Json<CreateIntegrationRequest>,
) -> Result<(StatusCode, Json<IntegrationResponse>), ApiError> {
    let actor = state.authenticate(&headers, &jar, SCOPE_WRITE).await?;
    authorize_namespace(&state, &actor, &slug).await?;
    let provider = provider(&request.provider).ok_or_else(ApiError::not_found)?;
    let name = validate_name(&request.name)?;
    let url = validate_url(&request.url, provider.name)?;
    let api_key = validate_api_key(&request.api_key, true)?.expect("required key was validated");
    let internal_url = validate_url(&request.internal_url, "Gitadel")?;
    let now = Utc::now();
    let transaction = state.database().begin().await?;
    let integration = namespace_integration::ActiveModel {
        id: Set(Uuid::new_v4()),
        namespace: Set(slug.clone()),
        provider: Set(provider.slug.to_owned()),
        name: Set(name),
        enabled: Set(true),
        url: Set(url),
        api_key: Set(api_key),
        dokploy_internal_url: Set(Some(internal_url)),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(&transaction)
    .await?;
    state
        .audit_on(
            &transaction,
            Some(actor.user.id),
            "integration.create",
            Some(format!("{slug}/{}", integration.id)),
        )
        .await?;
    transaction.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(IntegrationResponse::new(
            provider,
            integration,
            None,
            state.public_url.as_str(),
        )),
    ))
}

pub async fn update_integration(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((slug, id)): Path<(String, Uuid)>,
    Json(request): Json<UpdateIntegrationRequest>,
) -> Result<Json<IntegrationResponse>, ApiError> {
    let actor = state.authenticate(&headers, &jar, SCOPE_WRITE).await?;
    authorize_namespace(&state, &actor, &slug).await?;
    let existing = owned_integration(state.database(), &slug, id).await?;
    let provider = provider(&existing.provider).ok_or_else(ApiError::not_found)?;
    let binding = source_binding(state.database(), id).await?;
    let url = request
        .url
        .as_deref()
        .map(|url| validate_url(url, provider.name))
        .transpose()?;
    let api_key = validate_api_key(request.api_key.as_deref().unwrap_or_default(), false)?;
    let internal_url = request
        .internal_url
        .as_deref()
        .map(|url| validate_url(url, "Gitadel"))
        .transpose()?;
    if binding.is_some()
        && (url.as_ref().is_some_and(|url| url != &existing.url)
            || api_key
                .as_ref()
                .is_some_and(|api_key| api_key != &existing.api_key))
    {
        return Err(ApiError::conflict(
            "Disconnect the Dokploy source before changing this connection's URL or API key.",
        ));
    }
    if let Some(binding) = binding.as_ref().filter(|binding| binding.managed)
        && internal_url.as_ref().is_some_and(|internal_url| {
            internal_url
                != existing
                    .dokploy_internal_url
                    .as_deref()
                    .unwrap_or(state.public_url.as_str())
        })
    {
        provider
            .integration
            .update_remote_source_internal_url(
                Credential {
                    url: &existing.url,
                    api_key: &existing.api_key,
                    source_id: Some(&binding.gitea_id),
                },
                &binding.gitea_id,
                internal_url.as_deref().expect("change was checked"),
            )
            .await
            .map_err(ApiError::bad_request)?;
    }

    let transaction = state.database().begin().await?;
    let mut active = owned_integration(&transaction, &slug, id)
        .await?
        .into_active_model();
    if let Some(name) = request.name {
        active.name = Set(validate_name(&name)?);
    }
    if let Some(enabled) = request.enabled {
        active.enabled = Set(enabled);
    }
    if let Some(url) = url {
        active.url = Set(url);
    }
    if let Some(api_key) = api_key {
        active.api_key = Set(api_key);
    }
    if let Some(internal_url) = internal_url {
        active.dokploy_internal_url = Set(Some(internal_url));
    }
    active.updated_at = Set(Utc::now());
    let integration = active.update(&transaction).await?;
    state
        .audit_on(
            &transaction,
            Some(actor.user.id),
            "integration.update",
            Some(format!("{slug}/{id}")),
        )
        .await?;
    transaction.commit().await?;
    Ok(Json(IntegrationResponse::new(
        provider,
        integration,
        binding.as_ref(),
        state.public_url.as_str(),
    )))
}

pub async fn delete_integration(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((slug, id)): Path<(String, Uuid)>,
) -> Result<StatusCode, ApiError> {
    let actor = state.authenticate(&headers, &jar, SCOPE_WRITE).await?;
    authorize_namespace(&state, &actor, &slug).await?;
    let integration = owned_integration(state.database(), &slug, id).await?;
    let binding = source_binding(state.database(), id).await?;
    if let Some(binding) = binding.as_ref() {
        let provider = provider(&integration.provider).ok_or_else(ApiError::not_found)?;
        cleanup_managed_source(provider, &integration, binding).await;
    }
    let transaction = state.database().begin().await?;
    namespace_integration::Entity::delete_by_id(id)
        .exec(&transaction)
        .await?;
    if let Some(application_id) = binding.and_then(|binding| binding.oauth_application_id) {
        oauth_application::Entity::delete_by_id(application_id)
            .exec(&transaction)
            .await?;
    }
    state
        .audit_on(
            &transaction,
            Some(actor.user.id),
            "integration.delete",
            Some(format!("{slug}/{id}")),
        )
        .await?;
    transaction.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}
/// Return one stored key only to the namespace owner and prevent caching.
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
    let actor = state.authenticate(&headers, &jar, SCOPE_WRITE).await?;
    authorize_namespace(&state, &actor, &slug).await?;
    let stored = owned_integration(state.database(), &slug, id).await?;
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Json(IntegrationCredentialResponse {
            api_key: stored.api_key,
        }),
    ))
}

pub async fn get_integration_source(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((slug, id)): Path<(String, Uuid)>,
) -> Result<Json<SourceConnectionResponse>, ApiError> {
    let actor = state.authenticate(&headers, &jar, SCOPE_READ).await?;
    authorize_namespace(&state, &actor, &slug).await?;
    let integration = owned_integration(state.database(), &slug, id).await?;
    let provider = provider(&integration.provider).ok_or_else(ApiError::not_found)?;
    if !provider.source_required {
        return Err(ApiError::not_found());
    }
    let binding = source_binding(state.database(), id).await?;
    let sources = provider
        .integration
        .remote_sources(Credential {
            url: &integration.url,
            api_key: &integration.api_key,
            source_id: binding.as_ref().map(|binding| binding.gitea_id.as_str()),
        })
        .await
        .map_err(ApiError::bad_request)?;
    Ok(Json(source_connection_response(
        provider,
        binding.as_ref(),
        &sources,
    )))
}

pub async fn configure_integration_source(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((slug, id)): Path<(String, Uuid)>,
    Json(request): Json<ConfigureSourceRequest>,
) -> Result<(StatusCode, Json<SourceConnectionResponse>), ApiError> {
    let actor = state.authenticate(&headers, &jar, SCOPE_WRITE).await?;
    authorize_namespace(&state, &actor, &slug).await?;
    let integration = owned_integration(state.database(), &slug, id).await?;
    let provider = provider(&integration.provider).ok_or_else(ApiError::not_found)?;
    if !provider.source_required {
        return Err(ApiError::not_found());
    }
    if source_binding(state.database(), id).await?.is_some() {
        return Err(ApiError::conflict(
            "Disconnect the current Dokploy source before choosing another one.",
        ));
    }

    let credential = Credential {
        url: &integration.url,
        api_key: &integration.api_key,
        source_id: None,
    };
    let (binding, sources) = match request {
        ConfigureSourceRequest::Bind { source_id } => {
            let source_id = source_id.trim();
            if source_id.is_empty() {
                return Err(ApiError::bad_request("Choose a Dokploy Gitea provider."));
            }
            let sources = provider
                .integration
                .remote_sources(credential)
                .await
                .map_err(ApiError::bad_request)?;
            let source = sources
                .iter()
                .find(|source| source.id == source_id && source.ready)
                .ok_or_else(|| {
                    ApiError::bad_request(
                        "That Dokploy Gitea provider is not connected or is no longer available.",
                    )
                })?;
            let now = Utc::now();
            let transaction = state.database().begin().await?;
            let binding = dokploy_source_binding::ActiveModel {
                integration_id: Set(id),
                gitea_id: Set(source.id.clone()),
                git_provider_id: Set(source.parent_id.clone()),
                oauth_application_id: Set(None),
                managed: Set(false),
                created_at: Set(now),
                updated_at: Set(now),
            }
            .insert(&transaction)
            .await?;
            state
                .audit_on(
                    &transaction,
                    Some(actor.user.id),
                    "integration.source.connect",
                    Some(format!("{slug}/{id}/{}", binding.gitea_id)),
                )
                .await?;
            transaction.commit().await?;
            (binding, sources)
        }
        ConfigureSourceRequest::Create { name } => {
            if actor.via_api_token {
                return Err(ApiError::forbidden(
                    "Create and authorize Dokploy source providers from a browser session.",
                ));
            }
            let name = validate_name(&name)?;
            let redirect_uri = format!(
                "{}/api/providers/gitea/callback",
                integration.url.trim_end_matches('/')
            );
            let client_id = format!("goc_{}", random_secret(24));
            let client_secret = format!("gcs_{}", random_secret(32));
            let application_id = Uuid::new_v4();
            let internal_url = integration
                .dokploy_internal_url
                .as_deref()
                .unwrap_or(state.public_url.as_str());
            let provisioned = match provider
                .integration
                .provision_remote_source(
                    credential,
                    SourceApplication {
                        name: &name,
                        client_id: &client_id,
                        client_secret: &client_secret,
                        redirect_uri: &redirect_uri,
                        public_url: state.public_url.as_str(),
                        internal_url,
                    },
                )
                .await
            {
                Ok(provisioned) => provisioned,
                Err(error) => return Err(ApiError::bad_request(error)),
            };
            let transaction = state.database().begin().await?;
            let now = Utc::now();
            let binding_result: Result<dokploy_source_binding::Model, ApiError> = async {
                let application = oauth_application::ActiveModel {
                    id: Set(application_id),
                    user_id: Set(actor.user.id),
                    name: Set(format!("Dokploy: {}", integration.name)),
                    client_id: Set(client_id),
                    client_secret_hash: Set(hash_secret(&client_secret)),
                    redirect_uri: Set(redirect_uri),
                    created_at: Set(Utc::now()),
                }
                .insert(&transaction)
                .await?;
                let binding = dokploy_source_binding::ActiveModel {
                    integration_id: Set(id),
                    gitea_id: Set(provisioned.id.clone()),
                    git_provider_id: Set(provisioned.parent_id.clone()),
                    oauth_application_id: Set(Some(application.id)),
                    managed: Set(true),
                    created_at: Set(now),
                    updated_at: Set(now),
                }
                .insert(&transaction)
                .await?;
                state
                    .audit_on(
                        &transaction,
                        Some(actor.user.id),
                        "integration.source.connect",
                        Some(format!("{slug}/{id}/{}", binding.gitea_id)),
                    )
                    .await?;
                transaction.commit().await?;
                Ok(binding)
            }
            .await;
            let binding = match binding_result {
                Ok(binding) => binding,
                Err(error) => {
                    if let Err(cleanup_error) = provider
                        .integration
                        .remove_remote_source(credential, &provisioned.parent_id)
                        .await
                    {
                        tracing::warn!(%cleanup_error, integration_id = %id, "could not remove orphaned Dokploy source");
                    }
                    return Err(error);
                }
            };
            let source = RemoteSource {
                id: provisioned.id,
                parent_id: provisioned.parent_id,
                name,
                ready: false,
                authorization_url: provisioned.authorization_url,
            };
            (binding, vec![source])
        }
    };
    Ok((
        StatusCode::CREATED,
        Json(source_connection_response(
            provider,
            Some(&binding),
            &sources,
        )),
    ))
}

pub async fn disconnect_integration_source(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((slug, id)): Path<(String, Uuid)>,
) -> Result<StatusCode, ApiError> {
    let actor = state.authenticate(&headers, &jar, SCOPE_WRITE).await?;
    authorize_namespace(&state, &actor, &slug).await?;
    let integration = owned_integration(state.database(), &slug, id).await?;
    let provider = provider(&integration.provider).ok_or_else(ApiError::not_found)?;
    let binding = source_binding(state.database(), id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    cleanup_managed_source(provider, &integration, &binding).await;
    let transaction = state.database().begin().await?;
    dokploy_source_binding::Entity::delete_by_id(id)
        .exec(&transaction)
        .await?;
    if let Some(application_id) = binding.oauth_application_id {
        oauth_application::Entity::delete_by_id(application_id)
            .exec(&transaction)
            .await?;
    }
    state
        .audit_on(
            &transaction,
            Some(actor.user.id),
            "integration.source.disconnect",
            Some(format!("{slug}/{id}/{}", binding.gitea_id)),
        )
        .await?;
    transaction.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Verify credentials before storing a new namespace integration.
pub async fn test_new_integration(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(slug): Path<String>,
    Json(request): Json<TestIntegrationRequest>,
) -> Result<Json<IntegrationTestResponse>, ApiError> {
    let actor = state.authenticate(&headers, &jar, SCOPE_WRITE).await?;
    authorize_namespace(&state, &actor, &slug).await?;
    let provider = provider(&request.provider).ok_or_else(ApiError::not_found)?;
    let url = validate_url(&request.url, provider.name)?;
    let api_key = validate_api_key(&request.api_key, true)?.expect("required key was validated");
    test_credential(provider, &url, &api_key, None).await
}

/// Verify one stored credential without returning the key in the response.
pub async fn test_integration(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((slug, id)): Path<(String, Uuid)>,
) -> Result<Json<IntegrationTestResponse>, ApiError> {
    let actor = state.authenticate(&headers, &jar, SCOPE_READ).await?;
    authorize_namespace(&state, &actor, &slug).await?;
    let stored = owned_integration(state.database(), &slug, id).await?;
    let provider = provider(&stored.provider).ok_or_else(ApiError::not_found)?;
    let binding = source_binding(state.database(), id).await?;
    test_credential(
        provider,
        &stored.url,
        &stored.api_key,
        binding.as_ref().map(|binding| binding.gitea_id.as_str()),
    )
    .await
}

async fn test_credential(
    provider: &'static Provider,
    url: &str,
    api_key: &str,
    source_id: Option<&str>,
) -> Result<Json<IntegrationTestResponse>, ApiError> {
    let TestReport { account, warnings } = provider
        .integration
        .test(Credential {
            url,
            api_key,
            source_id,
        })
        .await
        .map_err(|error| {
            ApiError::bad_request(format!(
                "{} rejected the credential: {error}",
                provider.name
            ))
        })?;
    Ok(Json(IntegrationTestResponse { account, warnings }))
}
fn source_connection_response(
    provider: &'static Provider,
    binding: Option<&dokploy_source_binding::Model>,
    sources: &[RemoteSource],
) -> SourceConnectionResponse {
    let binding = binding.map(|binding| {
        let remote = sources.iter().find(|source| source.id == binding.gitea_id);
        SourceBindingStatus {
            id: binding.gitea_id.clone(),
            name: remote
                .map(|source| source.name.clone())
                .unwrap_or_else(|| "Gitea provider".to_owned()),
            managed: binding.managed,
            ready: remote.is_some_and(|source| source.ready),
            authorization_url: remote.map(|source| source.authorization_url.clone()),
        }
    });
    SourceConnectionResponse {
        required: provider.source_required,
        binding,
        sources: sources.iter().map(RemoteSourceResponse::from).collect(),
    }
}

async fn cleanup_managed_source(
    provider: &'static Provider,
    integration: &namespace_integration::Model,
    binding: &dokploy_source_binding::Model,
) {
    if !binding.managed {
        return;
    }
    if let Err(error) = provider
        .integration
        .remove_remote_source(
            Credential {
                url: &integration.url,
                api_key: &integration.api_key,
                source_id: Some(&binding.gitea_id),
            },
            &binding.git_provider_id,
        )
        .await
    {
        tracing::warn!(
            %error,
            integration_id = %integration.id,
            source_id = %binding.gitea_id,
            "could not remove managed Dokploy source"
        );
    }
}

async fn source_binding<C>(
    database: &C,
    integration_id: Uuid,
) -> Result<Option<dokploy_source_binding::Model>, ApiError>
where
    C: ConnectionTrait,
{
    Ok(dokploy_source_binding::Entity::find_by_id(integration_id)
        .one(database)
        .await?)
}

async fn owned_integration<C>(
    database: &C,
    namespace: &str,
    id: Uuid,
) -> Result<namespace_integration::Model, ApiError>
where
    C: sea_orm::ConnectionTrait,
{
    namespace_integration::Entity::find_by_id(id)
        .filter(namespace_integration::Column::Namespace.eq(namespace))
        .one(database)
        .await?
        .ok_or_else(ApiError::not_found)
}

/// Only a namespace's owner may read or change its credentials.
pub(crate) async fn authorize_namespace(
    state: &IdentityState,
    actor: &AuthenticatedUser,
    slug: &str,
) -> Result<(), ApiError> {
    let owner = namespace::Entity::find_by_id(slug)
        .one(state.database())
        .await?
        .ok_or_else(ApiError::not_found)?;
    let permitted = match owner.kind.as_str() {
        "user" => owner.user_id == Some(actor.user.id),
        "organization" => match owner.organization_id {
            Some(organization_id) => {
                organization_member::Entity::find_by_id((organization_id, actor.user.id))
                    .filter(organization_member::Column::Role.eq("owner"))
                    .one(state.database())
                    .await?
                    .is_some()
            }
            None => false,
        },
        _ => false,
    };
    if permitted {
        Ok(())
    } else {
        Err(ApiError::not_found())
    }
}

pub(crate) fn validate_name(value: &str) -> Result<String, ApiError> {
    let name = value.trim();
    if name.is_empty() || name.len() > MAX_NAME_LENGTH {
        return Err(ApiError::bad_request(
            "Integration names must contain between 1 and 80 characters.",
        ));
    }
    Ok(name.to_owned())
}

fn validate_api_key(value: &str, required: bool) -> Result<Option<String>, ApiError> {
    let key = value.trim();
    if key.is_empty() {
        return if required {
            Err(ApiError::bad_request("An API key is required."))
        } else {
            Ok(None)
        };
    }
    if key.len() > MAX_API_KEY_LENGTH {
        return Err(ApiError::bad_request(
            "API keys cannot exceed 512 characters.",
        ));
    }
    Ok(Some(key.to_owned()))
}

fn validate_url(value: &str, name: &str) -> Result<String, ApiError> {
    let url = url::Url::parse(value.trim()).map_err(|_| {
        ApiError::bad_request(format!("The {name} URL must be a valid HTTP or HTTPS URL."))
    })?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.as_str().len() > 2048
    {
        return Err(ApiError::bad_request(format!(
            "The {name} URL must be an HTTP or HTTPS URL without credentials."
        )));
    }
    Ok(url.as_str().trim_end_matches('/').to_owned())
}
