//! Repository-side integration wiring.
//!
//! A repository uses an integration only when a row here says so: enabling is
//! a per-repository decision, not something inherited wholesale. The common
//! configuration is still tiny - flip one switch - because the credential the
//! integration needs belongs to the owning namespace. The resource link and
//! deployment settings are opaque JSON whose meaning only the provider's
//! dispatcher knows, so new providers never need schema changes here.

use axum::{
    Json,
    extract::{Path as AxumPath, State},
    http::HeaderMap,
};
use axum_extra::extract::cookie::CookieJar;
use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, IntoActiveModel, QueryFilter, Set, TransactionTrait,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

use super::{Permission, RepositoryState, browser::read_git};
use crate::entity::{
    dokploy_source_binding, namespace_integration, repository, repository_integration,
};
use crate::identity::{ApiError, SCOPE_READ, SCOPE_WRITE};
use crate::integrations::{
    self, CreateEnvironmentRequest, CreateProjectRequest, CreateRemoteRequest, Credential,
    DomainRequest, EnablementRequest, Event, EventContext, LinkRemoteRequest, ProjectChoice,
    PushEvent, RemoteResourcesRequest, RepositoryIdentity, SetupRemoteRequest,
};

/// Distinguishes "field absent - leave stored value alone" from "field null -
/// clear the stored value" on optional request fields.
mod clearable {
    use serde::{Deserialize, Deserializer};

    pub(super) fn deserialize<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
    where
        D: Deserializer<'de>,
        T: Deserialize<'de>,
    {
        Ok(Some(Option::<T>::deserialize(deserializer)?))
    }
}

#[derive(Serialize, ToSchema)]
pub struct RepositoryIntegrationResponse {
    id: uuid::Uuid,
    connection_id: uuid::Uuid,
    provider: &'static str,
    provider_name: &'static str,
    name: String,
    connection_name: String,
    repository_setup: bool,
    configured: bool,
    enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    resource: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    config: Option<Value>,
}

#[derive(Serialize, ToSchema)]
pub struct RepositoryIntegrationConnectionResponse {
    id: uuid::Uuid,
    provider: &'static str,
    provider_name: &'static str,
    name: String,
    configured: bool,
}

#[derive(Serialize, ToSchema)]
pub struct RepositoryIntegrationProviderResponse {
    slug: &'static str,
    name: &'static str,
    description: &'static str,
    source_required: bool,
    icon: &'static str,
}

impl From<&'static integrations::Provider> for RepositoryIntegrationProviderResponse {
    fn from(provider: &'static integrations::Provider) -> Self {
        Self {
            slug: provider.slug,
            name: provider.name,
            description: provider.description,
            source_required: provider.source_required,
            icon: provider.icon,
        }
    }
}

#[derive(Serialize, ToSchema)]
pub struct RepositoryIntegrationsResponse {
    integrations: Vec<RepositoryIntegrationResponse>,
    connections: Vec<RepositoryIntegrationConnectionResponse>,
    providers: Vec<RepositoryIntegrationProviderResponse>,
}

#[derive(Deserialize, ToSchema)]
pub struct CreateRepositoryIntegrationRequest {
    integration_id: uuid::Uuid,
    name: String,
}

#[derive(Deserialize, ToSchema)]
pub struct UpdateRepositoryIntegrationRequest {
    name: Option<String>,
    enabled: Option<bool>,
    /// Provider-owned resource link; `null` unlinks, absent leaves it.
    #[serde(default, deserialize_with = "clearable::deserialize")]
    #[schema(value_type = Option<Object>, nullable)]
    resource: Option<Option<Value>>,
    #[serde(default, deserialize_with = "clearable::deserialize")]
    #[schema(value_type = Option<Object>, nullable)]
    config: Option<Option<Value>>,
}

async fn load_credential(
    state: &RepositoryState,
    namespace: &str,
    id: uuid::Uuid,
) -> Result<Option<namespace_integration::Model>, ApiError> {
    namespace_integration::Entity::find_by_id(id)
        .filter(namespace_integration::Column::Namespace.eq(namespace))
        .one(state.identity().database())
        .await?
        .map(|credential| {
            crate::identity::open_integration_api_key(state.identity().secret_cipher(), credential)
        })
        .transpose()
}

async fn load_source(
    state: &RepositoryState,
    integration_id: uuid::Uuid,
) -> Result<Option<dokploy_source_binding::Model>, ApiError> {
    Ok(dokploy_source_binding::Entity::find_by_id(integration_id)
        .one(state.identity().database())
        .await?)
}

fn source_id(source: Option<&dokploy_source_binding::Model>) -> Option<&str> {
    source.map(|source| source.gitea_id.as_str())
}

fn provider_credential<'a>(
    credential: &'a namespace_integration::Model,
    source: Option<&'a dokploy_source_binding::Model>,
) -> Credential<'a> {
    Credential {
        url: &credential.url,
        api_key: &credential.api_key,
        source_id: source_id(source),
    }
}

async fn ensure_source_ready(
    public_url: &str,
    provider: &'static integrations::Provider,
    credential: &namespace_integration::Model,
    source: Option<&dokploy_source_binding::Model>,
) -> Result<(), ApiError> {
    if !provider.source_required {
        return Ok(());
    }
    let source = source.ok_or_else(|| {
        ApiError::bad_request(format!(
            "{} must be connected to a Dokploy Gitea provider before repositories can use it.",
            credential.name
        ))
    })?;
    let sources = provider
        .integration
        .remote_sources(Credential {
            url: &credential.url,
            api_key: &credential.api_key,
            source_id: Some(&source.gitea_id),
        })
        .await
        .map_err(ApiError::bad_request)?;
    let remote = sources
        .iter()
        .find(|remote| remote.id == source.gitea_id)
        .ok_or_else(|| {
            ApiError::bad_request(format!(
                "The Dokploy provider bound to {} no longer exists. Choose another in the \
                 namespace's integration settings.",
                credential.name
            ))
        })?;
    match remote.problem(public_url) {
        Some(problem) => Err(ApiError::bad_request(format!(
            "{problem} Fix it in {}'s integration settings.",
            credential.name
        ))),
        None => Ok(()),
    }
}

fn response(
    provider: &'static integrations::Provider,
    credential: &namespace_integration::Model,
    row: repository_integration::Model,
) -> RepositoryIntegrationResponse {
    let parse = |stored: &Option<String>| {
        stored
            .as_ref()
            .and_then(|json| serde_json::from_str::<Value>(json).ok())
    };
    RepositoryIntegrationResponse {
        id: row.id,
        connection_id: credential.id,
        provider: provider.slug,
        provider_name: provider.name,
        name: row.name,
        connection_name: credential.name.clone(),
        repository_setup: provider.repository_setup,
        configured: credential.enabled,
        enabled: row.enabled,
        resource: parse(&row.resource),
        config: parse(&row.config),
    }
}
fn catalog(
    credentials: &[namespace_integration::Model],
    sources: &[dokploy_source_binding::Model],
    rows: Vec<repository_integration::Model>,
) -> RepositoryIntegrationsResponse {
    let connections = credentials
        .iter()
        .filter_map(|credential| {
            let provider = integrations::provider(&credential.provider)?;
            Some(RepositoryIntegrationConnectionResponse {
                id: credential.id,
                provider: provider.slug,
                provider_name: provider.name,
                name: credential.name.clone(),
                configured: credential.enabled
                    && (!provider.source_required
                        || sources
                            .iter()
                            .any(|source| source.integration_id == credential.id)),
            })
        })
        .collect();
    let integrations = rows
        .into_iter()
        .filter_map(|row| {
            let credential = credentials
                .iter()
                .find(|credential| credential.id == row.integration_id)?;
            let provider = integrations::provider(&credential.provider)?;
            Some(response(provider, credential, row))
        })
        .collect();
    RepositoryIntegrationsResponse {
        integrations,
        connections,
        providers: integrations::PROVIDERS
            .iter()
            .map(RepositoryIntegrationProviderResponse::from)
            .collect(),
    }
}

async fn integration_row(
    state: &RepositoryState,
    repository_id: uuid::Uuid,
    id: uuid::Uuid,
) -> Result<Option<repository_integration::Model>, ApiError> {
    Ok(repository_integration::Entity::find_by_id(id)
        .filter(repository_integration::Column::RepositoryId.eq(repository_id))
        .one(state.identity().database())
        .await?)
}

pub async fn list_integrations(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name)): AxumPath<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<RepositoryIntegrationsResponse>, ApiError> {
    let (_, repository_row) = state
        .authenticated_repository(
            &headers,
            &jar,
            &namespace,
            &name,
            Permission::Manage,
            SCOPE_READ,
        )
        .await?;
    let database = state.identity().database();
    let credentials = namespace_integration::Entity::find()
        .filter(namespace_integration::Column::Namespace.eq(repository_row.namespace.clone()))
        .all(database)
        .await?;
    let sources = dokploy_source_binding::Entity::find()
        .filter(
            dokploy_source_binding::Column::IntegrationId
                .is_in(credentials.iter().map(|credential| credential.id)),
        )
        .all(database)
        .await?;
    let rows = repository_integration::Entity::find()
        .filter(repository_integration::Column::RepositoryId.eq(repository_row.id))
        .all(database)
        .await?;
    Ok(Json(catalog(&credentials, &sources, rows)))
}

pub async fn create_integration(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name)): AxumPath<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<CreateRepositoryIntegrationRequest>,
) -> Result<Json<RepositoryIntegrationResponse>, ApiError> {
    let (actor, repository_row) = state
        .authenticated_repository(
            &headers,
            &jar,
            &namespace,
            &name,
            Permission::Manage,
            SCOPE_WRITE,
        )
        .await?;
    let credential = load_credential(&state, &repository_row.namespace, request.integration_id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let provider = integrations::provider(&credential.provider).ok_or_else(ApiError::not_found)?;
    let source = load_source(&state, credential.id).await?;
    ensure_source_ready(
        state.public_url().as_str(),
        provider,
        &credential,
        source.as_ref(),
    )
    .await?;
    let id = uuid::Uuid::new_v4();
    let row = repository_integration::Model {
        id,
        repository_id: repository_row.id,
        integration_id: credential.id,
        name: crate::identity::validate_integration_name(&request.name)?,
        enabled: credential.enabled,
        resource: None,
        config: None,
        updated_at: Utc::now(),
    };
    let transaction = state.identity().database().begin().await?;
    repository_integration::Entity::insert(row.clone().into_active_model())
        .exec_without_returning(&transaction)
        .await?;
    state
        .identity()
        .audit_on(
            &transaction,
            Some(actor.user.id),
            "repository.integration.create",
            Some(format!(
                "{}/{}/{}",
                repository_row.namespace, repository_row.name, id
            )),
        )
        .await?;
    transaction.commit().await?;
    Ok(Json(response(provider, &credential, row)))
}

pub async fn get_integration(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name, id)): AxumPath<(String, String, uuid::Uuid)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<RepositoryIntegrationResponse>, ApiError> {
    let (_, repository_row) = state
        .authenticated_repository(
            &headers,
            &jar,
            &namespace,
            &name,
            Permission::Manage,
            SCOPE_READ,
        )
        .await?;
    let row = integration_row(&state, repository_row.id, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let credential = load_credential(&state, &repository_row.namespace, row.integration_id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let provider = integrations::provider(&credential.provider).ok_or_else(ApiError::not_found)?;
    Ok(Json(response(provider, &credential, row)))
}

pub async fn delete_integration(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name, id)): AxumPath<(String, String, uuid::Uuid)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<axum::http::StatusCode, ApiError> {
    let (actor, repository_row) = state
        .authenticated_repository(
            &headers,
            &jar,
            &namespace,
            &name,
            Permission::Manage,
            SCOPE_WRITE,
        )
        .await?;
    let transaction = state.identity().database().begin().await?;
    let deleted = repository_integration::Entity::delete_many()
        .filter(repository_integration::Column::Id.eq(id))
        .filter(repository_integration::Column::RepositoryId.eq(repository_row.id))
        .exec(&transaction)
        .await?;
    if deleted.rows_affected == 0 {
        return Err(ApiError::not_found());
    }
    state
        .identity()
        .audit_on(
            &transaction,
            Some(actor.user.id),
            "repository.integration.delete",
            Some(format!(
                "{}/{}/{}",
                repository_row.namespace, repository_row.name, id
            )),
        )
        .await?;
    transaction.commit().await?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

pub async fn update_integration(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name, id)): AxumPath<(String, String, uuid::Uuid)>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<UpdateRepositoryIntegrationRequest>,
) -> Result<Json<RepositoryIntegrationResponse>, ApiError> {
    let (actor, repository_row) = state
        .authenticated_repository(
            &headers,
            &jar,
            &namespace,
            &name,
            Permission::Manage,
            SCOPE_WRITE,
        )
        .await?;
    let row = integration_row(&state, repository_row.id, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let credential = load_credential(&state, &repository_row.namespace, row.integration_id)
        .await?
        .filter(|credential| credential.enabled)
        .ok_or_else(ApiError::not_found)?;
    let provider = integrations::provider(&credential.provider).ok_or_else(ApiError::not_found)?;
    let source = load_source(&state, credential.id).await?;
    ensure_source_ready(
        state.public_url().as_str(),
        provider,
        &credential,
        source.as_ref(),
    )
    .await?;
    provider
        .integration
        .validate_repository_settings(
            request.resource.as_ref().and_then(Option::as_ref),
            request.config.as_ref().and_then(Option::as_ref),
        )
        .map_err(ApiError::bad_request)?;
    if let Some(enabled) = request.enabled {
        if let Some(stored) = row.resource.as_deref() {
            let resource = serde_json::from_str::<Value>(stored)
                .map_err(|error| ApiError::internal(error.to_string()))?;
            let request = enablement_request(
                provider_credential(&credential, source.as_ref()),
                &resource,
                enabled,
            )?;
            provider
                .integration
                .set_remote_enabled(request)
                .await
                .map_err(ApiError::bad_gateway)?;
        } else if enabled {
            return Err(ApiError::bad_request("Choose a deployment resource first."));
        }
    }

    let mut active = row.into_active_model();
    if let Some(name) = request.name {
        active.name = Set(crate::identity::validate_integration_name(&name)?);
    }
    if let Some(enabled) = request.enabled {
        active.enabled = Set(enabled);
    }
    if let Some(resource) = request.resource {
        active.resource = Set(resource.map(|value| value.to_string()));
    }
    if let Some(config) = request.config {
        active.config = Set(config.map(|value| value.to_string()));
    }
    active.updated_at = Set(Utc::now());

    let transaction = state.identity().database().begin().await?;
    let row = active.update(&transaction).await?;
    state
        .identity()
        .audit_on(
            &transaction,
            Some(actor.user.id),
            "repository.integration.update",
            Some(format!(
                "{}/{}/{}",
                repository_row.namespace, repository_row.name, id
            )),
        )
        .await?;
    transaction.commit().await?;
    Ok(Json(response(provider, &credential, row)))
}

/// Every deployable resource on the remote, for the configure page's picker.
pub async fn list_remote_resources(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name, integration_id)): AxumPath<(String, String, uuid::Uuid)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<Value>, ApiError> {
    let (_, repository_row) = state
        .authenticated_repository(
            &headers,
            &jar,
            &namespace,
            &name,
            Permission::Manage,
            SCOPE_READ,
        )
        .await?;
    let (_, credential, source) =
        require_instance(&state, &repository_row, integration_id, true).await?;
    let provider = integrations::provider(&credential.provider).ok_or_else(ApiError::not_found)?;
    if !provider.repository_setup {
        return Err(ApiError::not_found());
    }
    let resources = provider
        .integration
        .remote_resources(RemoteResourcesRequest {
            credential: provider_credential(&credential, source.as_ref()),
        })
        .await
        .map_err(ApiError::bad_gateway)?;
    Ok(Json(resources))
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProjectMutation {
    name: String,
    #[serde(default)]
    description: Option<String>,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct EnvironmentMutation {
    project_id: String,
    name: String,
    #[serde(default)]
    description: Option<String>,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct LinkMutation {
    kind: String,
    id: String,
    branch: String,
    #[serde(default)]
    repository_path: Option<String>,
    #[serde(default)]
    compose_path: Option<String>,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct CreateResourceMutation {
    kind: String,
    name: String,
    environment_id: String,
    branch: String,
    #[serde(default)]
    server_id: Option<String>,
    #[serde(default)]
    repository_path: Option<String>,
    #[serde(default)]
    compose_path: Option<String>,
}

fn repository_path(value: Option<String>) -> String {
    value.unwrap_or_else(|| "/".to_owned())
}

fn compose_path(value: Option<String>) -> String {
    value.unwrap_or_else(|| "./docker-compose.yml".to_owned())
}

#[derive(Deserialize)]
struct StoredResource {
    kind: String,
    id: String,
}

fn enablement_request<'a>(
    credential: Credential<'a>,
    resource: &Value,
    enabled: bool,
) -> Result<EnablementRequest<'a>, ApiError> {
    let resource: StoredResource = serde_json::from_value(resource.clone())
        .map_err(|error| ApiError::internal(error.to_string()))?;
    Ok(EnablementRequest {
        credential,
        kind: resource.kind,
        id: resource.id,
        enabled,
    })
}
#[derive(Deserialize)]
pub struct RemoteMutationRequest {
    #[serde(flatten)]
    body: Value,
}

#[expect(
    clippy::too_many_arguments,
    reason = "arguments combine the authorized integration persistence context"
)]
async fn store_remote_link(
    state: &RepositoryState,
    actor_id: uuid::Uuid,
    repository: &repository::Model,
    integration_id: uuid::Uuid,
    row: repository_integration::Model,
    credential: &namespace_integration::Model,
    source: Option<&dokploy_source_binding::Model>,
    provider: &'static integrations::Provider,
    link: Value,
) -> Result<Json<RepositoryIntegrationResponse>, ApiError> {
    provider
        .integration
        .validate_repository_settings(Some(&link), None)
        .map_err(ApiError::bad_request)?;
    let request = enablement_request(provider_credential(credential, source), &link, false)?;
    provider
        .integration
        .set_remote_enabled(request)
        .await
        .map_err(ApiError::bad_gateway)?;
    if row.enabled
        && let Some(stored) = row.resource.as_deref()
    {
        let previous = serde_json::from_str::<Value>(stored)
            .map_err(|error| ApiError::internal(error.to_string()))?;
        if previous != link {
            let request =
                enablement_request(provider_credential(credential, source), &previous, false)?;
            provider
                .integration
                .set_remote_enabled(request)
                .await
                .map_err(ApiError::bad_gateway)?;
        }
    }
    let name = link
        .get("name")
        .and_then(Value::as_str)
        .map(crate::identity::validate_integration_name)
        .transpose()?;

    let transaction = state.identity().database().begin().await?;
    let mut active = row.into_active_model();
    if let Some(name) = name {
        active.name = Set(name);
    }
    active.enabled = Set(false);
    active.resource = Set(Some(link.to_string()));
    active.config = Set(None);
    active.updated_at = Set(Utc::now());
    let row = active.update(&transaction).await?;

    state
        .identity()
        .audit_on(
            &transaction,
            Some(actor_id),
            "repository.integration.link",
            Some(format!(
                "{}/{}/{}",
                repository.namespace, repository.name, integration_id
            )),
        )
        .await?;
    transaction.commit().await?;

    Ok(Json(response(provider, credential, row)))
}

/// Create a Dokploy project and its default production environment.
pub async fn create_remote_project(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name, integration_id)): AxumPath<(String, String, uuid::Uuid)>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<RemoteMutationRequest>,
) -> Result<Json<Value>, ApiError> {
    let (actor, repository_row) = state
        .authenticated_repository(
            &headers,
            &jar,
            &namespace,
            &name,
            Permission::Manage,
            SCOPE_WRITE,
        )
        .await?;
    let (_, credential, source) =
        require_instance(&state, &repository_row, integration_id, true).await?;
    let provider = integrations::provider(&credential.provider).ok_or_else(ApiError::not_found)?;
    if !provider.repository_setup {
        return Err(ApiError::not_found());
    }
    let body: ProjectMutation = serde_json::from_value(request.body)
        .map_err(|error| ApiError::bad_request(format!("Invalid request: {error}")))?;
    let project = provider
        .integration
        .create_remote_project(CreateProjectRequest {
            credential: provider_credential(&credential, source.as_ref()),
            name: body.name,
            description: body.description,
        })
        .await
        .map_err(ApiError::bad_gateway)?;
    state
        .identity()
        .audit(
            Some(actor.user.id),
            "repository.integration.project.create",
            Some(format!(
                "{}/{}/{}",
                repository_row.namespace, repository_row.name, integration_id
            )),
        )
        .await?;
    Ok(Json(project))
}

/// Create a Dokploy environment inside an existing project.
pub async fn create_remote_environment(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name, integration_id)): AxumPath<(String, String, uuid::Uuid)>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<RemoteMutationRequest>,
) -> Result<Json<Value>, ApiError> {
    let (actor, repository_row) = state
        .authenticated_repository(
            &headers,
            &jar,
            &namespace,
            &name,
            Permission::Manage,
            SCOPE_WRITE,
        )
        .await?;
    let (_, credential, source) =
        require_instance(&state, &repository_row, integration_id, true).await?;
    let provider = integrations::provider(&credential.provider).ok_or_else(ApiError::not_found)?;
    if !provider.repository_setup {
        return Err(ApiError::not_found());
    }
    let body: EnvironmentMutation = serde_json::from_value(request.body)
        .map_err(|error| ApiError::bad_request(format!("Invalid request: {error}")))?;
    let environment = provider
        .integration
        .create_remote_environment(CreateEnvironmentRequest {
            credential: provider_credential(&credential, source.as_ref()),
            project_id: body.project_id,
            name: body.name,
            description: body.description,
        })
        .await
        .map_err(ApiError::bad_gateway)?;
    state
        .identity()
        .audit(
            Some(actor.user.id),
            "repository.integration.environment.create",
            Some(format!(
                "{}/{}/{}",
                repository_row.namespace, repository_row.name, integration_id
            )),
        )
        .await?;
    Ok(Json(environment))
}

/// Link an existing remote resource, configuring its Gitadel source when it is
/// currently unconfigured.
pub async fn link_remote_resource(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name, integration_id)): AxumPath<(String, String, uuid::Uuid)>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<RemoteMutationRequest>,
) -> Result<Json<RepositoryIntegrationResponse>, ApiError> {
    let (actor, repository_row) = state
        .authenticated_repository(
            &headers,
            &jar,
            &namespace,
            &name,
            Permission::Manage,
            SCOPE_WRITE,
        )
        .await?;
    let (row, credential, source) =
        require_instance(&state, &repository_row, integration_id, true).await?;
    let provider = integrations::provider(&credential.provider).ok_or_else(ApiError::not_found)?;
    if !provider.repository_setup {
        return Err(ApiError::not_found());
    }
    let body: LinkMutation = serde_json::from_value(request.body)
        .map_err(|error| ApiError::bad_request(format!("Invalid request: {error}")))?;
    let link = provider
        .integration
        .link_remote(LinkRemoteRequest {
            repository: RepositoryIdentity {
                namespace: &repository_row.namespace,
                name: &repository_row.name,
            },
            credential: provider_credential(&credential, source.as_ref()),
            kind: body.kind,
            id: body.id,
            branch: body.branch,
            repository_path: repository_path(body.repository_path),
            compose_path: compose_path(body.compose_path),
        })
        .await
        .map_err(ApiError::bad_gateway)?;
    store_remote_link(
        &state,
        actor.user.id,
        &repository_row,
        integration_id,
        row,
        &credential,
        source.as_ref(),
        provider,
        link,
    )
    .await
}

/// Create a new resource on the remote, wire it to this repository, and store
/// the resulting link.
pub async fn create_remote_resource(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name, integration_id)): AxumPath<(String, String, uuid::Uuid)>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<RemoteMutationRequest>,
) -> Result<Json<RepositoryIntegrationResponse>, ApiError> {
    let (actor, repository_row) = state
        .authenticated_repository(
            &headers,
            &jar,
            &namespace,
            &name,
            Permission::Manage,
            SCOPE_WRITE,
        )
        .await?;
    let (row, credential, source) =
        require_instance(&state, &repository_row, integration_id, true).await?;
    let provider = integrations::provider(&credential.provider).ok_or_else(ApiError::not_found)?;
    if !provider.repository_setup {
        return Err(ApiError::not_found());
    }
    let body: CreateResourceMutation = serde_json::from_value(request.body)
        .map_err(|error| ApiError::bad_request(format!("Invalid request: {error}")))?;
    let link = provider
        .integration
        .create_remote(CreateRemoteRequest {
            repository: RepositoryIdentity {
                namespace: &repository_row.namespace,
                name: &repository_row.name,
            },
            credential: provider_credential(&credential, source.as_ref()),
            kind: body.kind,
            name: body.name,
            environment_id: body.environment_id,
            branch: body.branch,
            server_id: body.server_id,
            repository_path: repository_path(body.repository_path),
            compose_path: compose_path(body.compose_path),
        })
        .await
        .map_err(ApiError::bad_gateway)?;
    store_remote_link(
        &state,
        actor.user.id,
        &repository_row,
        integration_id,
        row,
        &credential,
        source.as_ref(),
        provider,
        link,
    )
    .await
}

/// How this repository can deploy, read from its default branch.
pub async fn deployment_suggestions(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name, integration_id)): AxumPath<(String, String, uuid::Uuid)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<DeploymentSuggestionsResponse>, ApiError> {
    let (_, repository_row) = state
        .authenticated_repository(
            &headers,
            &jar,
            &namespace,
            &name,
            Permission::Manage,
            SCOPE_READ,
        )
        .await?;
    integration_row(&state, repository_row.id, integration_id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    suggestions_for(&state, &repository_row).await
}

/// The same suggestions before any integration exists, for clients that
/// decide how to deploy before setting one up.
pub async fn repository_deployment_suggestions(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name)): AxumPath<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<DeploymentSuggestionsResponse>, ApiError> {
    let (_, repository_row) = state
        .authenticated_repository(
            &headers,
            &jar,
            &namespace,
            &name,
            Permission::Manage,
            SCOPE_READ,
        )
        .await?;
    suggestions_for(&state, &repository_row).await
}

/// What a repository can deploy, read from its default branch.
#[derive(Serialize, ToSchema)]
pub(crate) struct DeploymentSuggestionsResponse {
    branch: Option<String>,
    /// Default name for a new remote project.
    project_name: String,
    suggestions: Vec<super::deploy_detect::BuildSuggestion>,
}

/// The outcome of a one-step deployment setup.
#[derive(Serialize, ToSchema)]
pub(crate) struct SetupDeploymentResponse {
    integration: RepositoryIntegrationResponse,
    /// What was done, in order.
    steps: Vec<String>,
    /// A later step that failed after the resource was linked.
    warning: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub(crate) struct DeployResponse {
    /// The provider's description of what it started.
    summary: String,
}

async fn suggestions_for(
    state: &RepositoryState,
    repository_row: &repository::Model,
) -> Result<Json<DeploymentSuggestionsResponse>, ApiError> {
    let suggestions = match repository_row.default_branch.clone() {
        Some(branch) => read_git(state.repository_path(repository_row), move |git| {
            super::deploy_detect::suggest(git, &branch)
        })
        .await
        .unwrap_or_default(),
        None => Vec::new(),
    };
    Ok(Json(DeploymentSuggestionsResponse {
        branch: repository_row.default_branch.clone(),
        project_name: repository_row.name.clone(),
        suggestions,
    }))
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct DomainMutation {
    host: String,
    port: u16,
    #[serde(default)]
    https: bool,
    #[serde(default)]
    service: Option<String>,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct SetupMutation {
    kind: String,
    name: String,
    branch: String,
    #[serde(default)]
    server_id: Option<String>,
    #[serde(default)]
    repository_path: Option<String>,
    #[serde(default)]
    compose_path: Option<String>,
    #[serde(default)]
    dockerfile: Option<String>,
    /// An existing remote project; otherwise `project_name` is created.
    #[serde(default)]
    project_id: Option<String>,
    #[serde(default)]
    project_name: Option<String>,
    environment: String,
    #[serde(default)]
    domain: Option<DomainMutation>,
    #[serde(default)]
    deploy: bool,
    #[serde(default)]
    deploy_on_push: bool,
}

/// Create, configure, link, and optionally deploy a resource in one request,
/// reporting each completed step.
pub async fn setup_deployment(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name, integration_id)): AxumPath<(String, String, uuid::Uuid)>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<RemoteMutationRequest>,
) -> Result<Json<SetupDeploymentResponse>, ApiError> {
    let (actor, repository_row) = state
        .authenticated_repository(
            &headers,
            &jar,
            &namespace,
            &name,
            Permission::Manage,
            SCOPE_WRITE,
        )
        .await?;
    let (row, credential, source) =
        require_instance(&state, &repository_row, integration_id, true).await?;
    let provider = integrations::provider(&credential.provider).ok_or_else(ApiError::not_found)?;
    if !provider.repository_setup {
        return Err(ApiError::not_found());
    }
    let body: SetupMutation = serde_json::from_value(request.body)
        .map_err(|error| ApiError::bad_request(format!("Invalid request: {error}")))?;
    let project = match (body.project_id, body.project_name) {
        (Some(id), _) if !id.trim().is_empty() => ProjectChoice::Existing(id),
        (_, Some(name)) if !name.trim().is_empty() => ProjectChoice::New(name),
        _ => return Err(ApiError::bad_request("Choose a project or name a new one.")),
    };
    if let Some(domain) = body.domain.as_ref()
        && (domain.host.trim().is_empty() || domain.port == 0)
    {
        return Err(ApiError::bad_request(
            "A domain needs a host name and a port.",
        ));
    }
    let outcome = provider
        .integration
        .setup_remote(SetupRemoteRequest {
            repository: RepositoryIdentity {
                namespace: &repository_row.namespace,
                name: &repository_row.name,
            },
            credential: provider_credential(&credential, source.as_ref()),
            kind: body.kind,
            name: body.name,
            branch: body.branch,
            server_id: body.server_id.filter(|id| !id.trim().is_empty()),
            repository_path: repository_path(body.repository_path),
            compose_path: compose_path(body.compose_path),
            dockerfile: body.dockerfile.filter(|file| !file.trim().is_empty()),
            project,
            environment: body.environment,
            domain: body.domain.map(|domain| DomainRequest {
                host: domain.host,
                port: domain.port,
                https: domain.https,
                service: domain.service,
            }),
        })
        .await
        .map_err(ApiError::bad_gateway)?;
    let mut steps = outcome.steps;
    let Json(mut linked) = store_remote_link(
        &state,
        actor.user.id,
        &repository_row,
        integration_id,
        row,
        &credential,
        source.as_ref(),
        provider,
        outcome.link.clone(),
    )
    .await?;
    steps.push("Linked it to this repository".to_owned());

    // Later failures leave a working, linked resource; report them as the
    // step that failed rather than discarding the setup.
    let mut warning = None;
    if body.deploy_on_push {
        let request = enablement_request(
            provider_credential(&credential, source.as_ref()),
            &outcome.link,
            true,
        )?;
        match provider.integration.set_remote_enabled(request).await {
            Ok(()) => {
                let stored = integration_row(&state, repository_row.id, integration_id)
                    .await?
                    .ok_or_else(ApiError::not_found)?;
                let mut active = stored.into_active_model();
                active.enabled = Set(true);
                active.updated_at = Set(Utc::now());
                let stored = active.update(state.identity().database()).await?;
                linked = response(provider, &credential, stored);
                steps.push("Turned on deploy on push".to_owned());
            }
            Err(error) => warning = Some(format!("Could not turn on deploy on push: {error}")),
        }
    }
    if body.deploy && warning.is_none() {
        let resource = outcome.link.to_string();
        match provider
            .integration
            .handle_event(EventContext {
                event: Event::Manual,
                resource: Some(&resource),
                credential: provider_credential(&credential, source.as_ref()),
            })
            .await
        {
            Ok(_) => steps.push("Started the first deployment".to_owned()),
            Err(error) => warning = Some(format!("Could not start the first deployment: {error}")),
        }
    }
    state
        .identity()
        .audit(
            Some(actor.user.id),
            "repository.integration.setup",
            Some(format!(
                "{}/{}/{}",
                repository_row.namespace, repository_row.name, integration_id
            )),
        )
        .await?;
    Ok(Json(SetupDeploymentResponse {
        integration: linked,
        steps,
        warning,
    }))
}

/// The newest deployment of the linked resource.
pub async fn deployment_status(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name, integration_id)): AxumPath<(String, String, uuid::Uuid)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<Value>, ApiError> {
    let (_, repository_row) = state
        .authenticated_repository(
            &headers,
            &jar,
            &namespace,
            &name,
            Permission::Manage,
            SCOPE_READ,
        )
        .await?;
    let (row, credential, source) =
        require_instance(&state, &repository_row, integration_id, false).await?;
    let provider = integrations::provider(&credential.provider).ok_or_else(ApiError::not_found)?;
    let resource = row
        .resource
        .as_deref()
        .ok_or_else(|| ApiError::bad_request("Set up a deployment first."))?;
    let status = provider
        .integration
        .deployment_status(provider_credential(&credential, source.as_ref()), resource)
        .await
        .map_err(ApiError::bad_gateway)?;
    Ok(Json(status))
}

/// Trigger a deployment right now without changing push delivery.
pub async fn deploy_integration(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name, integration_id)): AxumPath<(String, String, uuid::Uuid)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<DeployResponse>, ApiError> {
    let (actor, repository_row) = state
        .authenticated_repository(
            &headers,
            &jar,
            &namespace,
            &name,
            Permission::Manage,
            SCOPE_WRITE,
        )
        .await?;
    let (row, credential, source) =
        require_instance(&state, &repository_row, integration_id, true).await?;
    let provider = integrations::provider(&credential.provider).ok_or_else(ApiError::not_found)?;
    if !provider.repository_setup {
        return Err(ApiError::not_found());
    }
    let integration = provider.integration;

    let context = EventContext {
        event: Event::Manual,
        resource: row.resource.as_deref(),
        credential: provider_credential(&credential, source.as_ref()),
    };
    let summary = integration
        .handle_event(context)
        .await
        .map_err(ApiError::bad_gateway)?;

    state
        .identity()
        .audit(
            Some(actor.user.id),
            "repository.integration.deploy",
            Some(format!(
                "{}/{}/{}",
                repository_row.namespace, repository_row.name, integration_id
            )),
        )
        .await?;
    Ok(Json(DeployResponse { summary }))
}

async fn require_instance(
    state: &RepositoryState,
    repository: &repository::Model,
    id: uuid::Uuid,
    require_enabled: bool,
) -> Result<
    (
        repository_integration::Model,
        namespace_integration::Model,
        Option<dokploy_source_binding::Model>,
    ),
    ApiError,
> {
    let row = integration_row(state, repository.id, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let credential = load_credential(state, &repository.namespace, row.integration_id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if require_enabled && !credential.enabled {
        return Err(ApiError::bad_request(format!(
            "{} is disabled by {}.",
            credential.name, repository.namespace
        )));
    }
    let provider = integrations::provider(&credential.provider).ok_or_else(ApiError::not_found)?;
    let source = load_source(state, credential.id).await?;
    ensure_source_ready(
        state.public_url().as_str(),
        provider,
        &credential,
        source.as_ref(),
    )
    .await?;
    Ok((row, credential, source))
}

/// Deliver a push event to every enabled repository integration.
///
/// Delivery runs after the Git operation succeeds. Provider failures are
/// isolated to logs and cannot fail the push.
pub(super) fn dispatch_push(
    state: &RepositoryState,
    repository: &repository::Model,
    reference: &str,
    deleted: bool,
    payload: Value,
) {
    let task_state = state.clone();
    let state = state.clone();
    let repository = repository.clone();
    let reference = reference.to_owned();
    task_state.spawn_task(async move {
        if let Err(error) = trigger_push(&state, &repository, &reference, deleted, &payload).await {
            tracing::warn!(
                repository = %format!("{}/{}", repository.namespace, repository.name),
                reference,
                %error,
                "Integration event dispatch failed"
            );
        }
    });
}

/// Cheap preflight used before constructing a commit-rich push payload.
pub(super) async fn has_enabled(
    state: &RepositoryState,
    repository_id: uuid::Uuid,
) -> Result<bool, ApiError> {
    Ok(repository_integration::Entity::find()
        .filter(repository_integration::Column::RepositoryId.eq(repository_id))
        .filter(repository_integration::Column::Enabled.eq(true))
        .one(state.identity().database())
        .await?
        .is_some())
}

pub(super) async fn trigger_push(
    state: &RepositoryState,
    repository: &repository::Model,
    reference: &str,
    deleted: bool,
    payload: &Value,
) -> Result<(), String> {
    let database = state.identity().database();
    // API keys are stored sealed; open them before they reach the provider.
    let connected = namespace_integration::Entity::find()
        .filter(namespace_integration::Column::Namespace.eq(repository.namespace.clone()))
        .filter(namespace_integration::Column::Enabled.eq(true))
        .all(database)
        .await
        .map_err(|error| error.to_string())?
        .into_iter()
        .map(|connection| {
            crate::identity::open_integration_api_key(state.identity().secret_cipher(), connection)
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    if connected.is_empty() {
        return Ok(());
    }
    let sources = dokploy_source_binding::Entity::find()
        .filter(
            dokploy_source_binding::Column::IntegrationId
                .is_in(connected.iter().map(|connection| connection.id)),
        )
        .all(database)
        .await
        .map_err(|error| error.to_string())?;
    let rows = repository_integration::Entity::find()
        .filter(repository_integration::Column::RepositoryId.eq(repository.id))
        .filter(repository_integration::Column::Enabled.eq(true))
        .all(database)
        .await
        .map_err(|error| error.to_string())?;

    for row in rows {
        let Some(connection) = connected
            .iter()
            .find(|connection| connection.id == row.integration_id)
        else {
            continue;
        };
        let Some(provider) = integrations::provider(&connection.provider) else {
            tracing::warn!(
                provider = %connection.provider,
                integration = %row.name,
                "No implementation for stored integration"
            );
            continue;
        };
        let source = sources
            .iter()
            .find(|source| source.integration_id == connection.id);
        if provider.source_required && source.is_none() {
            tracing::warn!(
                provider = %connection.provider,
                integration = %row.name,
                "Integration has no Dokploy source binding"
            );
            continue;
        }
        let context = EventContext {
            event: Event::Push(PushEvent {
                reference,
                deleted,
                payload,
            }),
            resource: row.resource.as_deref(),
            credential: provider_credential(connection, source),
        };
        match provider.integration.handle_event(context).await {
            Ok(summary) if !summary.is_empty() => tracing::info!(
                repository = %format!("{}/{}", repository.namespace, repository.name),
                reference,
                provider = %connection.provider,
                integration = %row.name,
                "{summary}"
            ),
            Ok(_) => {}
            Err(error) => tracing::warn!(
                repository = %format!("{}/{}", repository.namespace, repository.name),
                provider = %connection.provider,
                integration = %row.name,
                %error,
                "Integration event handler failed"
            ),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_keeps_multiple_repository_instances_for_one_connection() {
        let now = Utc::now();
        let connection_id = uuid::Uuid::new_v4();
        let credential = namespace_integration::Model {
            id: connection_id,
            namespace: "acme".to_owned(),
            provider: "dokploy".to_owned(),
            name: "Production Dokploy".to_owned(),
            enabled: true,
            url: "https://dokploy.example.com".to_owned(),
            api_key: "secret".to_owned(),
            dokploy_internal_url: None,
            created_at: now,
            updated_at: now,
        };
        let source = dokploy_source_binding::Model {
            integration_id: connection_id,
            gitea_id: "gitea-1".to_owned(),
            git_provider_id: "provider-1".to_owned(),
            oauth_application_id: None,
            managed: false,
            created_at: now,
            updated_at: now,
        };
        let first_id = uuid::Uuid::new_v4();
        let second_id = uuid::Uuid::new_v4();
        let row = |id, name: &str| repository_integration::Model {
            id,
            repository_id: uuid::Uuid::new_v4(),
            integration_id: connection_id,
            name: name.to_owned(),
            enabled: true,
            resource: None,
            config: None,
            updated_at: now,
        };

        let result = catalog(
            std::slice::from_ref(&credential),
            &[source],
            vec![row(first_id, "Web"), row(second_id, "Worker")],
        );

        assert_eq!(result.connections.len(), 1);
        assert!(result.connections[0].configured);
        assert_eq!(result.integrations.len(), 2);
        assert_eq!(result.integrations[0].id, first_id);
        assert_eq!(result.integrations[0].connection_id, connection_id);
        assert_eq!(result.integrations[0].name, "Web");
        assert_eq!(result.integrations[1].id, second_id);
        assert_eq!(result.integrations[1].connection_id, connection_id);
        assert_eq!(result.integrations[1].name, "Worker");

        let unbound = catalog(&[credential], &[], Vec::new());
        assert!(!unbound.connections[0].configured);
    }
}
