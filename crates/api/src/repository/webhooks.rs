use std::{
    future::Future,
    sync::atomic::{AtomicI64, Ordering},
    time::Duration,
};

use axum::{
    Json,
    extract::{Path as AxumPath, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use axum_extra::extract::cookie::CookieJar;
use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder,
    QuerySelect, Set, TransactionTrait,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use url::Url;
use utoipa::ToSchema;
use uuid::Uuid;

use super::{Permission, RepositoryState};
use crate::{
    actions::ActionsState,
    entity::{
        oauth_access_token, oauth_refresh_token, repository, repository_webhook,
        repository_webhook_delivery, session, user,
    },
    identity::{ApiError, SCOPE_READ, SCOPE_WRITE},
    network::pinned_public_http_client,
    secrets::{SecretCipher, SecretKind},
};

/// Maximum number of delivery records kept per webhook.
const WEBHOOK_DELIVERY_HISTORY_LIMIT: usize = 50;

/// Event names a webhook may subscribe to, in display order.
///
/// `ping` is not listed: it is always delivered on creation and on request.
pub(crate) const WEBHOOK_EVENTS: &[&str] = &[
    "push",
    "create",
    "delete",
    "release",
    "issues",
    "issue_comment",
    "workflow_run",
];
/// Keep expired credential cleanup on the existing hourly cadence.
const SESSION_SWEEP_INTERVAL_SECONDS: i64 = 60 * 60;
static LAST_SESSION_SWEEP: AtomicI64 = AtomicI64::new(0);
const DEFAULT_WEBHOOK_EVENT: &str = "push";

#[derive(Serialize, ToSchema)]
struct WebhookConfigResponse {
    url: String,
    content_type: &'static str,
    insecure_ssl: &'static str,
}

#[derive(Serialize, ToSchema)]
struct WebhookLastResponse {
    code: Option<i32>,
    status: &'static str,
    message: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct WebhookResponse {
    id: Uuid,
    #[serde(rename = "type")]
    type_name: &'static str,
    name: &'static str,
    active: bool,
    events: Vec<String>,
    config: WebhookConfigResponse,
    url: String,
    ping_url: String,
    created_at: chrono::DateTime<Utc>,
    updated_at: chrono::DateTime<Utc>,
    last_delivery_at: Option<chrono::DateTime<Utc>>,
    last_response: WebhookLastResponse,
}

impl WebhookResponse {
    fn new(
        hook: repository_webhook::Model,
        state: &RepositoryState,
        repository: &repository::Model,
    ) -> Self {
        let url = api_hook_url(state, repository, hook.id);
        let events = hook_events(&hook).map(str::to_owned).collect();
        let last_response = WebhookLastResponse {
            code: hook.last_response_status,
            status: match (
                hook.last_response_status,
                hook.last_response_message.as_ref(),
            ) {
                (Some(200..=299), _) => "ok",
                (Some(_), _) | (None, Some(_)) => "failed",
                (None, None) => "unused",
            },
            message: hook.last_response_message,
        };
        Self {
            id: hook.id,
            type_name: "Repository",
            name: "web",
            active: hook.active,
            events,
            config: WebhookConfigResponse {
                url: hook.url,
                content_type: "json",
                insecure_ssl: "0",
            },
            url: url.clone(),
            ping_url: format!("{url}/pings"),
            created_at: hook.created_at,
            updated_at: hook.updated_at,
            last_delivery_at: hook.last_delivery_at,
            last_response,
        }
    }
}

#[derive(Serialize, ToSchema)]
pub struct WebhookDeliveryResponse {
    id: Uuid,
    event: String,
    status_code: Option<i32>,
    status: &'static str,
    delivered_at: chrono::DateTime<Utc>,
    duration_ms: i32,
    payload: Value,
    response_body: Option<String>,
    /// Attempts made so far, including the one in flight.
    attempts: i32,
    /// When a delivery that is waiting to be retried is next attempted.
    next_attempt_at: Option<chrono::DateTime<Utc>>,
}

impl WebhookDeliveryResponse {
    fn new(delivery: repository_webhook_delivery::Model) -> Self {
        let status = match delivery.status.as_str() {
            "pending" | "delivering" => "pending",
            "completed"
                if delivery
                    .response_status
                    .is_some_and(|code| (200..300).contains(&code)) =>
            {
                "ok"
            }
            _ => "failed",
        };
        Self {
            id: delivery.id,
            event: delivery.event,
            status_code: delivery.response_status,
            status,
            delivered_at: delivery.created_at,
            duration_ms: delivery.duration_ms,
            payload: serde_json::from_str(&delivery.payload).unwrap_or(Value::Null),
            response_body: delivery.response_body,
            attempts: delivery.attempt_count,
            next_attempt_at: delivery.next_attempt_at,
        }
    }
}

#[derive(Deserialize, ToSchema)]
pub struct WebhookConfigRequest {
    url: String,
    content_type: Option<String>,
    secret: Option<String>,
}

#[derive(Deserialize, ToSchema)]
pub struct CreateWebhookRequest {
    name: Option<String>,
    active: Option<bool>,
    events: Option<Vec<String>>,
    config: WebhookConfigRequest,
}

#[derive(Default, Deserialize, ToSchema)]
pub struct UpdateWebhookConfigRequest {
    url: Option<String>,
    content_type: Option<String>,
    secret: Option<String>,
}

#[derive(Deserialize, ToSchema)]
pub struct UpdateWebhookRequest {
    active: Option<bool>,
    events: Option<Vec<String>>,
    config: Option<UpdateWebhookConfigRequest>,
}

#[utoipa::path(
    get, path = "/repos/{namespace}/{name}/hooks",
    operation_id = "get_repos_namespace_name_hooks",
    summary = "List the repository's webhooks",
    tag = "Webhooks",
    params(
        ("namespace" = String, Path),
        ("name" = String, Path),
    ),
    responses(
        (status = 200, description = "Success", body = Vec<WebhookResponse>, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn list_webhooks(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name)): AxumPath<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<Vec<WebhookResponse>>, ApiError> {
    let (_, repository) = state
        .authenticated_repository(
            &headers,
            &jar,
            &namespace,
            &name,
            Permission::Manage,
            SCOPE_READ,
        )
        .await?;
    let hooks = repository_webhook::Entity::find()
        .filter(repository_webhook::Column::RepositoryId.eq(repository.id))
        .order_by_asc(repository_webhook::Column::CreatedAt)
        .all(state.identity().database())
        .await?;
    Ok(Json(
        hooks
            .into_iter()
            .map(|hook| WebhookResponse::new(hook, &state, &repository))
            .collect(),
    ))
}

#[utoipa::path(
    get, path = "/repos/{namespace}/{name}/hooks/{id}",
    operation_id = "get_repos_namespace_name_hooks_id",
    summary = "Get a webhook",
    tag = "Webhooks",
    params(
        ("namespace" = String, Path),
        ("name" = String, Path),
        ("id" = Uuid, Path),
    ),
    responses(
        (status = 200, description = "Success", body = WebhookResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn get_webhook(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name, id)): AxumPath<(String, String, Uuid)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<WebhookResponse>, ApiError> {
    let (_, repository) = state
        .authenticated_repository(
            &headers,
            &jar,
            &namespace,
            &name,
            Permission::Manage,
            SCOPE_READ,
        )
        .await?;
    let hook = find_hook(state.identity().database(), repository.id, id).await?;
    Ok(Json(WebhookResponse::new(hook, &state, &repository)))
}

#[utoipa::path(
    post, path = "/repos/{namespace}/{name}/hooks",
    operation_id = "post_repos_namespace_name_hooks",
    summary = "Create a webhook",
    tag = "Webhooks",

    params(
        ("namespace" = String, Path),
        ("name" = String, Path),
    ),
    responses(
        (status = 201, description = "Success", body = crate::repository::webhooks::WebhookResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn create_webhook(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name)): AxumPath<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<CreateWebhookRequest>,
) -> Result<impl IntoResponse + crate::openapi::TypedResponse, ApiError> {
    let (actor, repository) = state
        .authenticated_repository(
            &headers,
            &jar,
            &namespace,
            &name,
            Permission::Manage,
            SCOPE_WRITE,
        )
        .await?;
    validate_kind(request.name.as_deref())?;
    let events = normalize_events(request.events.as_deref())?
        .unwrap_or_else(|| DEFAULT_WEBHOOK_EVENT.to_owned());
    validate_content_type(request.config.content_type.as_deref())?;
    let endpoint = validate_endpoint(&request.config.url).await?;
    let secret = validate_secret(request.config.secret)?;
    let now = Utc::now();
    let id = Uuid::new_v4();
    let secret = seal_secret(state.identity().secret_cipher(), id, secret)?;
    let transaction = state.identity().database().begin().await?;
    let hook = repository_webhook::ActiveModel {
        id: Set(id),
        repository_id: Set(repository.id),
        url: Set(endpoint),
        secret: Set(secret),
        active: Set(request.active.unwrap_or(true)),
        created_at: Set(now),
        updated_at: Set(now),
        last_delivery_at: Set(None),
        last_response_status: Set(None),
        last_response_message: Set(None),
        events: Set(events),
    }
    .insert(&transaction)
    .await?;
    state
        .identity()
        .audit_on(
            &transaction,
            Some(actor.user.id),
            "repository.webhook.create",
            Some(format!("{namespace}/{name}/{}", hook.id)),
        )
        .await?;
    transaction.commit().await?;

    queue_ping(state.clone(), hook.clone(), repository.clone(), actor.user).await?;
    Ok((
        StatusCode::CREATED,
        Json(WebhookResponse::new(hook, &state, &repository)),
    ))
}

#[utoipa::path(
    patch, path = "/repos/{namespace}/{name}/hooks/{id}",
    operation_id = "patch_repos_namespace_name_hooks_id",
    summary = "Update a webhook",
    tag = "Webhooks",

    params(
        ("namespace" = String, Path),
        ("name" = String, Path),
        ("id" = Uuid, Path),
    ),
    responses(
        (status = 200, description = "Success", body = WebhookResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn update_webhook(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name, id)): AxumPath<(String, String, Uuid)>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<UpdateWebhookRequest>,
) -> Result<Json<WebhookResponse>, ApiError> {
    let (actor, repository) = state
        .authenticated_repository(
            &headers,
            &jar,
            &namespace,
            &name,
            Permission::Manage,
            SCOPE_WRITE,
        )
        .await?;
    let events = normalize_events(request.events.as_deref())?;
    let stored = find_hook(state.identity().database(), repository.id, id).await?;
    let mut active: repository_webhook::ActiveModel = stored.into();
    if let Some(enabled) = request.active {
        active.active = Set(enabled);
    }
    if let Some(events) = events {
        active.events = Set(events);
    }
    if let Some(config) = request.config {
        validate_content_type(config.content_type.as_deref())?;
        if let Some(endpoint) = config.url {
            active.url = Set(validate_endpoint(&endpoint).await?);
        }
        if let Some(secret) = config.secret {
            let secret = validate_secret(Some(secret))?;
            active.secret = Set(seal_secret(state.identity().secret_cipher(), id, secret)?);
        }
    }
    active.updated_at = Set(Utc::now());
    let hook = active.update(state.identity().database()).await?;
    state
        .identity()
        .audit(
            Some(actor.user.id),
            "repository.webhook.update",
            Some(format!("{namespace}/{name}/{id}")),
        )
        .await?;
    Ok(Json(WebhookResponse::new(hook, &state, &repository)))
}

#[utoipa::path(
    delete, path = "/repos/{namespace}/{name}/hooks/{id}",
    operation_id = "delete_repos_namespace_name_hooks_id",
    summary = "Delete a webhook",
    tag = "Webhooks",
    params(
        ("namespace" = String, Path),
        ("name" = String, Path),
        ("id" = Uuid, Path),
    ),
    responses(
        (status = 204, description = "Success"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn delete_webhook(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name, id)): AxumPath<(String, String, Uuid)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<StatusCode, ApiError> {
    let (actor, repository) = state
        .authenticated_repository(
            &headers,
            &jar,
            &namespace,
            &name,
            Permission::Manage,
            SCOPE_WRITE,
        )
        .await?;
    let hook = find_hook(state.identity().database(), repository.id, id).await?;
    let transaction = state.identity().database().begin().await?;
    repository_webhook::Entity::delete_by_id(hook.id)
        .exec(&transaction)
        .await?;
    state
        .identity()
        .audit_on(
            &transaction,
            Some(actor.user.id),
            "repository.webhook.delete",
            Some(format!("{namespace}/{name}/{id}")),
        )
        .await?;
    transaction.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    get, path = "/repos/{namespace}/{name}/hooks/{id}/deliveries",
    operation_id = "get_repos_namespace_name_hooks_id_deliveries",
    summary = "List a webhook's recent deliveries",
    tag = "Webhooks",
    params(
        ("namespace" = String, Path),
        ("name" = String, Path),
        ("id" = Uuid, Path),
    ),
    responses(
        (status = 200, description = "Success", body = Vec<WebhookDeliveryResponse>, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn list_webhook_deliveries(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name, id)): AxumPath<(String, String, Uuid)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<Vec<WebhookDeliveryResponse>>, ApiError> {
    let (_, repository) = state
        .authenticated_repository(
            &headers,
            &jar,
            &namespace,
            &name,
            Permission::Manage,
            SCOPE_READ,
        )
        .await?;
    find_hook(state.identity().database(), repository.id, id).await?;
    let deliveries = repository_webhook_delivery::Entity::find()
        .filter(repository_webhook_delivery::Column::WebhookId.eq(id))
        .order_by_desc(repository_webhook_delivery::Column::CreatedAt)
        .limit(WEBHOOK_DELIVERY_HISTORY_LIMIT as u64)
        .all(state.identity().database())
        .await?;
    Ok(Json(
        deliveries
            .into_iter()
            .map(WebhookDeliveryResponse::new)
            .collect(),
    ))
}

#[utoipa::path(
    get, path = "/repos/{namespace}/{name}/hooks/{id}/deliveries/{delivery_id}",
    operation_id = "get_repos_namespace_name_hooks_id_deliveries_delivery_id",
    summary = "Get a webhook delivery",
    tag = "Webhooks",
    params(
        ("namespace" = String, Path),
        ("name" = String, Path),
        ("id" = Uuid, Path),
        ("delivery_id" = Uuid, Path),
    ),
    responses(
        (status = 200, description = "Success", body = WebhookDeliveryResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn get_webhook_delivery(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name, id, delivery_id)): AxumPath<(String, String, Uuid, Uuid)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<WebhookDeliveryResponse>, ApiError> {
    let (_, repository) = state
        .authenticated_repository(
            &headers,
            &jar,
            &namespace,
            &name,
            Permission::Manage,
            SCOPE_READ,
        )
        .await?;
    let hook = find_hook(state.identity().database(), repository.id, id).await?;
    let delivery = find_delivery(state.identity().database(), hook.id, delivery_id).await?;
    Ok(Json(WebhookDeliveryResponse::new(delivery)))
}

#[utoipa::path(
    post, path = "/repos/{namespace}/{name}/hooks/{id}/deliveries/{delivery_id}/attempts",
    operation_id = "post_repos_namespace_name_hooks_id_deliveries_delivery_id_attempts",
    summary = "Redeliver a webhook delivery",
    tag = "Webhooks",
    params(
        ("namespace" = String, Path),
        ("name" = String, Path),
        ("id" = Uuid, Path),
        ("delivery_id" = Uuid, Path),
    ),
    responses(
        (status = 202, description = "Success"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn redeliver_webhook_delivery(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name, id, delivery_id)): AxumPath<(String, String, Uuid, Uuid)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<StatusCode, ApiError> {
    let (actor, repository) = state
        .authenticated_repository(
            &headers,
            &jar,
            &namespace,
            &name,
            Permission::Manage,
            SCOPE_WRITE,
        )
        .await?;
    let hook = find_hook(state.identity().database(), repository.id, id).await?;
    let delivery = find_delivery(state.identity().database(), hook.id, delivery_id).await?;
    let payload = serde_json::from_str(&delivery.payload).unwrap_or(Value::Null);
    state
        .identity()
        .audit(
            Some(actor.user.id),
            "repository.webhook.redeliver",
            Some(format!("{namespace}/{name}/{id}/{delivery_id}")),
        )
        .await?;
    queue_delivery(&state, &hook, &delivery.event, &payload).await?;
    Ok(StatusCode::ACCEPTED)
}

async fn find_delivery(
    database: &DatabaseConnection,
    webhook_id: Uuid,
    id: Uuid,
) -> Result<repository_webhook_delivery::Model, ApiError> {
    repository_webhook_delivery::Entity::find_by_id(id)
        .filter(repository_webhook_delivery::Column::WebhookId.eq(webhook_id))
        .one(database)
        .await?
        .ok_or_else(ApiError::not_found)
}

#[utoipa::path(
    post, path = "/repos/{namespace}/{name}/hooks/{id}/pings",
    operation_id = "post_repos_namespace_name_hooks_id_pings",
    summary = "Ping a webhook",
    tag = "Webhooks",
    params(
        ("namespace" = String, Path),
        ("name" = String, Path),
        ("id" = Uuid, Path),
    ),
    responses(
        (status = 204, description = "Success"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn ping_webhook(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name, id)): AxumPath<(String, String, Uuid)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<StatusCode, ApiError> {
    let (actor, repository) = state
        .authenticated_repository(
            &headers,
            &jar,
            &namespace,
            &name,
            Permission::Manage,
            SCOPE_WRITE,
        )
        .await?;
    let hook = find_hook(state.identity().database(), repository.id, id).await?;
    state
        .identity()
        .audit(
            Some(actor.user.id),
            "repository.webhook.ping",
            Some(format!("{namespace}/{name}/{id}")),
        )
        .await?;
    queue_ping(state, hook, repository, actor.user).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(super) use gitadel_repositories::post_write::{RefSnapshot, snapshot_refs};

struct LegacyPostWrite<'a> {
    repository: &'a RepositoryState,
    actions: &'a ActionsState,
}
#[async_trait::async_trait]
impl gitadel_repositories::post_write::PostWriteEffects for LegacyPostWrite<'_> {
    async fn deployments_enabled(
        &self,
        repository_id: Uuid,
    ) -> Result<bool, gitadel_identity::IdentityError> {
        super::integrations::has_enabled(self.repository, repository_id)
            .await
            .map_err(|error| gitadel_identity::IdentityError::internal(error.to_string()))
    }
    async fn actions(
        &self,
        repository: &repository::Model,
        actor_id: Uuid,
        change: &gitadel_repositories::post_write::PreparedRef,
    ) -> Result<(), String> {
        crate::actions::workflow::ingest_push(
            self.actions,
            repository,
            actor_id,
            &change.reference,
            &change.before,
            &change.after,
            &change.zero,
            change.changed_paths.clone(),
        )
        .await
        .map_err(|error| error.to_string())
    }
    fn deploy(
        &self,
        repository: &repository::Model,
        change: &gitadel_repositories::post_write::PreparedRef,
    ) {
        super::integrations::dispatch_push(
            self.repository,
            repository,
            &change.reference,
            change.deleted(),
            change.payload.clone(),
        );
    }
}
pub(super) async fn dispatch_push(
    state: &RepositoryState,
    actions: &ActionsState,
    repository: &repository::Model,
    actor_user_id: Uuid,
    before: RefSnapshot,
) -> Result<(), ApiError> {
    let service = gitadel_repositories::post_write::PostWriteService::new(
        state.identity().database().clone(),
        gitadel_repositories::content::RepositoryContent::new(
            state.repository_root.as_ref().clone(),
        ),
        state.webhook_outbox.clone(),
    );
    let presentation = gitadel_repositories::post_write::PushPresentation {
        public_url: state.public_url.as_ref().clone(),
        http_clone_url: state.http_clone_url(repository),
        ssh_clone_url: state.ssh_clone_url(repository),
    };
    let effects = LegacyPostWrite {
        repository: state,
        actions,
    };
    let prepared = service
        .prepare(repository, actor_user_id, before, &presentation, &effects)
        .await?;
    Ok(service.deliver(prepared, &effects).await?)
}

/// Queue `event` for every active hook of the repository subscribed to it.
///
/// The payload is built only when at least one hook subscribes, so callers can
/// emit unconditionally without paying for unused payloads.
pub(super) async fn dispatch_event<F, Fut>(
    state: &RepositoryState,
    repository_id: Uuid,
    event: &str,
    payload: F,
) -> Result<(), ApiError>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<Value, ApiError>>,
{
    let hooks = active_hooks(state, repository_id)
        .await?
        .into_iter()
        .filter(|hook| subscribes(hook, event))
        .collect::<Vec<_>>();
    if hooks.is_empty() {
        return Ok(());
    }
    let payload = payload().await?;
    for hook in &hooks {
        insert_delivery(state, hook, event, &payload).await?;
    }
    process_pending_deliveries(state).await
}

/// Whether any active hook of the repository subscribes to `event`.
pub(super) async fn has_subscribers(
    state: &RepositoryState,
    repository_id: Uuid,
    event: &str,
) -> Result<bool, ApiError> {
    Ok(active_hooks(state, repository_id)
        .await?
        .iter()
        .any(|hook| subscribes(hook, event)))
}

async fn active_hooks(
    state: &RepositoryState,
    repository_id: Uuid,
) -> Result<Vec<repository_webhook::Model>, ApiError> {
    Ok(repository_webhook::Entity::find()
        .filter(repository_webhook::Column::RepositoryId.eq(repository_id))
        .filter(repository_webhook::Column::Active.eq(true))
        .all(state.identity().database())
        .await?)
}

fn hook_events(hook: &repository_webhook::Model) -> impl Iterator<Item = &str> {
    hook.events
        .split(',')
        .map(str::trim)
        .filter(|event| !event.is_empty())
}

fn subscribes(hook: &repository_webhook::Model, event: &str) -> bool {
    hook_events(hook).any(|subscribed| subscribed == event)
}

/// Payload for the `create` and `delete` events, shaped like GitHub and Gitea.
async fn queue_ping(
    state: RepositoryState,
    hook: repository_webhook::Model,
    repository: repository::Model,
    actor: user::Model,
) -> Result<(), ApiError> {
    let payload = json!({
        "zen": "Design for failure.",
        "hook_id": hook.id,
        "hook": WebhookResponse::new(hook.clone(), &state, &repository),
        "repository": repository_payload(&state, &repository),
        "sender": user_payload(&actor),
    });
    queue_delivery(&state, &hook, "ping", &payload).await
}

async fn queue_delivery(
    state: &RepositoryState,
    hook: &repository_webhook::Model,
    event: &str,
    payload: &Value,
) -> Result<(), ApiError> {
    insert_delivery(state, hook, event, payload).await?;
    process_pending_deliveries(state).await
}

/// Record a pending delivery in the outbox without starting delivery.
async fn insert_delivery(
    state: &RepositoryState,
    hook: &repository_webhook::Model,
    event: &str,
    payload: &Value,
) -> Result<(), ApiError> {
    Ok(outbox(state).insert_delivery(hook, event, payload).await?)
}

pub(super) async fn process_pending_deliveries(state: &RepositoryState) -> Result<(), ApiError> {
    sweep_expired_sessions(state.identity().database()).await;
    Ok(outbox(state).process_pending_deliveries().await?)
}

/// Remove expired browser sessions and OAuth access tokens at most once per
/// [`SESSION_SWEEP_INTERVAL_SECONDS`].
///
/// Neither has a worker of its own. The outbox is drained from the
/// repository scheduler every thirty seconds, which makes it the nearest
/// periodic tick; a failure is logged and never holds up deliveries.
async fn sweep_expired_sessions(database: &DatabaseConnection) {
    let now = Utc::now();
    let last = LAST_SESSION_SWEEP.load(Ordering::Relaxed);
    if now.timestamp() - last < SESSION_SWEEP_INTERVAL_SECONDS
        || LAST_SESSION_SWEEP
            .compare_exchange(last, now.timestamp(), Ordering::Relaxed, Ordering::Relaxed)
            .is_err()
    {
        return;
    }
    match session::delete_expired(database, now).await {
        Ok(0) => {}
        Ok(removed) => tracing::debug!(removed, "removed expired sessions"),
        Err(error) => tracing::warn!(%error, "could not remove expired sessions"),
    }
    match oauth_access_token::delete_expired(database, now).await {
        Ok(0) => {}
        Ok(removed) => tracing::debug!(removed, "removed expired OAuth access tokens"),
        Err(error) => tracing::warn!(%error, "could not remove expired OAuth access tokens"),
    }
    match oauth_refresh_token::delete_expired(database, now).await {
        Ok(0) => {}
        Ok(removed) => tracing::debug!(removed, "removed expired OAuth refresh tokens"),
        Err(error) => tracing::warn!(%error, "could not remove expired OAuth refresh tokens"),
    }
}

async fn find_hook(
    database: &sea_orm::DatabaseConnection,
    repository_id: Uuid,
    id: Uuid,
) -> Result<repository_webhook::Model, ApiError> {
    repository_webhook::Entity::find_by_id(id)
        .filter(repository_webhook::Column::RepositoryId.eq(repository_id))
        .one(database)
        .await?
        .ok_or_else(ApiError::not_found)
}

fn validate_kind(name: Option<&str>) -> Result<(), ApiError> {
    if name.is_some_and(|name| name != "web") {
        return Err(ApiError::bad_request("Webhook name must be web."));
    }
    Ok(())
}

/// Validate requested events and return their stored, canonical form.
///
/// `*` subscribes to every supported event, as on GitHub.
fn normalize_events(events: Option<&[String]>) -> Result<Option<String>, ApiError> {
    let Some(events) = events else {
        return Ok(None);
    };
    if let Some(unknown) = events
        .iter()
        .map(|event| event.trim())
        .find(|event| *event != "*" && !WEBHOOK_EVENTS.contains(event))
    {
        return Err(ApiError::bad_request(format!(
            "Unsupported webhook event `{unknown}`. Supported events: {}.",
            WEBHOOK_EVENTS.join(", ")
        )));
    }
    let wildcard = events.iter().any(|event| event.trim() == "*");
    let selected = WEBHOOK_EVENTS
        .iter()
        .copied()
        .filter(|supported| wildcard || events.iter().any(|event| event.trim() == *supported))
        .collect::<Vec<_>>();
    if selected.is_empty() {
        return Err(ApiError::bad_request("Select at least one webhook event."));
    }
    Ok(Some(selected.join(",")))
}

fn validate_content_type(content_type: Option<&str>) -> Result<(), ApiError> {
    if content_type.is_some_and(|content_type| content_type != "json") {
        return Err(ApiError::bad_request("Webhook content type must be json."));
    }
    Ok(())
}

async fn validate_endpoint(value: &str) -> Result<String, ApiError> {
    let endpoint = Url::parse(value.trim())
        .map_err(|_| ApiError::bad_request("Webhook URL must be a valid HTTP or HTTPS URL."))?;
    if !matches!(endpoint.scheme(), "http" | "https")
        || endpoint.host_str().is_none()
        || !endpoint.username().is_empty()
        || endpoint.password().is_some()
        || endpoint.fragment().is_some()
        || endpoint.as_str().len() > 2048
    {
        return Err(ApiError::bad_request(
            "Webhook URL must be an HTTP or HTTPS URL without credentials or a fragment.",
        ));
    }
    pinned_public_http_client(&endpoint, Duration::from_secs(10))
        .await
        .map_err(ApiError::bad_request)?;
    Ok(endpoint.to_string())
}

/// The stored form of a webhook secret: sealed to the hook, or absent.
fn seal_secret(
    cipher: &SecretCipher,
    hook_id: Uuid,
    secret: Option<String>,
) -> Result<Option<String>, ApiError> {
    secret
        .map(|secret| {
            cipher
                .seal_text(SecretKind::WebhookSecret, &hook_id, &secret)
                .map_err(ApiError::internal)
        })
        .transpose()
}

fn validate_secret(secret: Option<String>) -> Result<Option<String>, ApiError> {
    let secret = secret.filter(|secret| !secret.is_empty());
    if secret.as_ref().is_some_and(|secret| secret.len() > 256) {
        return Err(ApiError::bad_request(
            "Webhook secrets must be at most 256 characters.",
        ));
    }
    Ok(secret)
}

pub(super) fn repository_payload(state: &RepositoryState, repository: &repository::Model) -> Value {
    let full_name = format!("{}/{}", repository.namespace, repository.name);
    json!({
        "id": repository.id,
        "name": repository.name,
        "full_name": full_name,
        "private": repository.visibility == "private",
        "owner": { "login": repository.namespace, "type": "User" },
        "html_url": public_url(state, &format!("/{full_name}")),
        "url": public_url(state, &format!("/api/v1/repos/{full_name}")),
        "clone_url": state.http_clone_url(repository),
        "ssh_url": state.ssh_clone_url(repository),
        "default_branch": repository.default_branch,
        "archived": repository.archived_at.is_some(),
    })
}

pub(super) fn user_payload(actor: &user::Model) -> Value {
    json!({
        "id": actor.id,
        "login": actor.username,
        "type": "User",
    })
}

fn api_hook_url(state: &RepositoryState, repository: &repository::Model, id: Uuid) -> String {
    public_url(
        state,
        &format!(
            "/api/v1/repos/{}/{}/hooks/{id}",
            repository.namespace, repository.name
        ),
    )
}

pub(super) fn public_url(state: &RepositoryState, path: &str) -> String {
    let mut url = state.public_url.as_ref().clone();
    url.set_path(path);
    url.set_query(None);
    url.set_fragment(None);
    url.to_string()
}

pub(super) fn webhook_client() -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::DateTime;

    fn events(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn webhook_events_are_validated_and_canonicalised() {
        assert_eq!(normalize_events(None).unwrap(), None);
        assert_eq!(
            normalize_events(Some(&events(&["issues", "push", "issues"]))).unwrap(),
            Some("push,issues".to_owned())
        );
        assert_eq!(
            normalize_events(Some(&events(&["*"]))).unwrap(),
            Some(WEBHOOK_EVENTS.join(","))
        );
        assert!(normalize_events(Some(&[])).is_err());
        assert!(normalize_events(Some(&events(&["pull_request"]))).is_err());
    }

    #[test]
    fn webhook_secrets_are_stored_sealed_to_their_hook() {
        let cipher = SecretCipher::for_tests();
        let hook_id = Uuid::new_v4();
        assert_eq!(seal_secret(&cipher, hook_id, None).unwrap(), None);
        let stored = seal_secret(&cipher, hook_id, Some("hunter2".to_owned()))
            .unwrap()
            .unwrap();
        assert!(crate::secrets::is_sealed(&stored));
        assert_eq!(
            cipher
                .open_text(SecretKind::WebhookSecret, &hook_id, &stored)
                .unwrap(),
            "hunter2"
        );
        assert!(
            cipher
                .open_text(SecretKind::WebhookSecret, &Uuid::new_v4(), &stored)
                .is_err()
        );
    }

    #[test]
    fn hooks_match_only_subscribed_events() {
        let now = Utc::now();
        let hook = repository_webhook::Model {
            id: Uuid::new_v4(),
            repository_id: Uuid::new_v4(),
            url: "https://example.com/hook".to_owned(),
            secret: None,
            active: true,
            created_at: now,
            updated_at: now,
            last_delivery_at: None,
            last_response_status: None,
            last_response_message: None,
            events: "push,release".to_owned(),
        };
        assert!(subscribes(&hook, "push"));
        assert!(subscribes(&hook, "release"));
        assert!(!subscribes(&hook, "issues"));
        assert!(!subscribes(&hook, "pus"));
    }

    #[test]
    fn pending_webhook_delivery_is_visible_as_pending() {
        let delivery = repository_webhook_delivery::Model {
            id: Uuid::new_v4(),
            webhook_id: Uuid::new_v4(),
            event: "push".to_owned(),
            payload: "{}".to_owned(),
            response_status: None,
            response_body: None,
            duration_ms: 0,
            created_at: Utc::now(),
            status: "pending".to_owned(),
            attempt_count: 0,
            attempted_at: None,
            next_attempt_at: None,
        };

        let response = WebhookDeliveryResponse::new(delivery);

        assert_eq!(response.status, "pending");
    }

    #[test]
    fn retries_back_off_for_about_an_hour_then_stop() {
        let delays = (1..=WEBHOOK_MAX_ATTEMPTS)
            .map(|attempt| retry_delay(attempt).map(|delay| delay.num_minutes()))
            .collect::<Vec<_>>();
        assert_eq!(delays, [Some(1), Some(5), Some(15), Some(45), None]);
        assert_eq!(retry_delay(WEBHOOK_MAX_ATTEMPTS + 3), None);
    }

    #[test]
    fn only_transient_failures_are_retried() {
        for status in [None, Some(408), Some(429), Some(500), Some(502), Some(503)] {
            assert!(is_retryable("push", status), "{status:?}");
        }
        for status in [
            Some(301),
            Some(400),
            Some(401),
            Some(404),
            Some(410),
            Some(422),
        ] {
            assert!(!is_retryable("push", status), "{status:?}");
        }
        assert!(!is_retryable("ping", None));
        assert!(!is_retryable("ping", Some(503)));
    }

    async fn hook(database: &DatabaseConnection) -> repository_webhook::Model {
        let owner = crate::actions::test_support::owner(database, "owner").await;
        let repository =
            crate::actions::test_support::repository(database, &owner, "project").await;
        let now = Utc::now();
        repository_webhook::ActiveModel {
            id: Set(Uuid::new_v4()),
            repository_id: Set(repository.id),
            url: Set("https://example.com/hook".to_owned()),
            secret: Set(None),
            active: Set(true),
            created_at: Set(now),
            updated_at: Set(now),
            last_delivery_at: Set(None),
            last_response_status: Set(None),
            last_response_message: Set(None),
            events: Set("push".to_owned()),
        }
        .insert(database)
        .await
        .unwrap()
    }

    async fn delivery(
        database: &DatabaseConnection,
        hook: &repository_webhook::Model,
        event: &str,
        created_at: DateTime<Utc>,
    ) -> repository_webhook_delivery::Model {
        repository_webhook_delivery::ActiveModel {
            id: Set(Uuid::new_v4()),
            webhook_id: Set(hook.id),
            event: Set(event.to_owned()),
            payload: Set("{}".to_owned()),
            response_status: Set(None),
            response_body: Set(None),
            duration_ms: Set(0),
            created_at: Set(created_at),
            status: Set("pending".to_owned()),
            attempt_count: Set(0),
            attempted_at: Set(None),
            next_attempt_at: Set(None),
        }
        .insert(database)
        .await
        .unwrap()
    }

    /// What the worker does when it picks a delivery up.
    async fn claim(
        database: &DatabaseConnection,
        delivery: repository_webhook_delivery::Model,
        now: DateTime<Utc>,
    ) -> repository_webhook_delivery::Model {
        let attempt_count = delivery.attempt_count + 1;
        let mut active: repository_webhook_delivery::ActiveModel = delivery.into();
        active.status = Set("delivering".to_owned());
        active.attempt_count = Set(attempt_count);
        active.attempted_at = Set(Some(now));
        active.update(database).await.unwrap()
    }

    async fn reload(database: &DatabaseConnection, id: Uuid) -> repository_webhook_delivery::Model {
        repository_webhook_delivery::Entity::find_by_id(id)
            .one(database)
            .await
            .unwrap()
            .unwrap()
    }

    #[tokio::test]
    async fn failed_delivery_is_retried_with_backoff_until_attempts_run_out() {
        let database = crate::actions::test_support::database().await;
        let hook = hook(&database).await;
        let mut now = Utc::now();
        let queued = delivery(&database, &hook, "push", now).await;
        let id = queued.id;

        for (attempt, minutes) in [(1, 1), (2, 5), (3, 15), (4, 45)] {
            let due = next_due_delivery(&database, now).await.ok().unwrap();
            assert_eq!(
                due.as_ref().map(|due| due.id),
                Some(id),
                "attempt {attempt}"
            );
            let claimed = claim(&database, due.unwrap(), now).await;
            record_attempt(
                &database,
                &hook,
                &claimed,
                Some(503),
                12,
                Some("down".to_owned()),
                now,
            )
            .await
            .ok()
            .unwrap();

            let waiting = reload(&database, id).await;
            assert_eq!(waiting.status, "pending");
            assert_eq!(waiting.attempt_count, attempt);
            assert_eq!(waiting.response_status, Some(503));
            assert_eq!(
                waiting.next_attempt_at,
                Some(now + chrono::Duration::minutes(minutes))
            );
            assert_eq!(WebhookDeliveryResponse::new(waiting).status, "pending");
            // Not due a moment before the scheduled time, due at it.
            let early = now + chrono::Duration::minutes(minutes) - chrono::Duration::seconds(1);
            assert!(
                next_due_delivery(&database, early)
                    .await
                    .ok()
                    .unwrap()
                    .is_none()
            );
            now += chrono::Duration::minutes(minutes);
        }

        let due = next_due_delivery(&database, now)
            .await
            .ok()
            .unwrap()
            .unwrap();
        let claimed = claim(&database, due, now).await;
        record_attempt(
            &database,
            &hook,
            &claimed,
            None,
            0,
            Some("timed out".to_owned()),
            now,
        )
        .await
        .ok()
        .unwrap();
        let failed = reload(&database, id).await;
        assert_eq!(failed.status, "failed");
        assert_eq!(failed.attempt_count, WEBHOOK_MAX_ATTEMPTS);
        assert_eq!(failed.next_attempt_at, None);
        assert!(
            next_due_delivery(&database, now + chrono::Duration::days(1))
                .await
                .ok()
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn retried_delivery_completes_when_the_receiver_recovers() {
        let database = crate::actions::test_support::database().await;
        let hook = hook(&database).await;
        let now = Utc::now();
        let queued = delivery(&database, &hook, "push", now).await;
        let claimed = claim(&database, queued, now).await;
        record_attempt(
            &database,
            &hook,
            &claimed,
            None,
            0,
            Some("refused".to_owned()),
            now,
        )
        .await
        .ok()
        .unwrap();

        let later = now + chrono::Duration::minutes(1);
        let due = next_due_delivery(&database, later)
            .await
            .ok()
            .unwrap()
            .unwrap();
        let claimed = claim(&database, due, later).await;
        record_attempt(&database, &hook, &claimed, Some(204), 8, None, later)
            .await
            .ok()
            .unwrap();

        let completed = reload(&database, claimed.id).await;
        assert_eq!(completed.status, "completed");
        assert_eq!(completed.attempt_count, 2);
        assert_eq!(completed.next_attempt_at, None);
        assert_eq!(WebhookDeliveryResponse::new(completed).status, "ok");
    }

    #[tokio::test]
    async fn rejected_deliveries_and_pings_fail_without_retry() {
        let database = crate::actions::test_support::database().await;
        let hook = hook(&database).await;
        let now = Utc::now();
        for (event, status) in [("push", Some(404)), ("ping", Some(503)), ("ping", None)] {
            let queued = delivery(&database, &hook, event, now).await;
            let claimed = claim(&database, queued, now).await;
            record_attempt(&database, &hook, &claimed, status, 3, None, now)
                .await
                .ok()
                .unwrap();
            let finished = reload(&database, claimed.id).await;
            assert_eq!(finished.status, "failed", "{event} {status:?}");
            assert_eq!(finished.next_attempt_at, None);
        }
    }

    #[tokio::test]
    async fn interrupted_deliveries_are_requeued_only_while_attempts_remain() {
        let database = crate::actions::test_support::database().await;
        let hook = hook(&database).await;
        let now = Utc::now();
        let started = now - chrono::Duration::minutes(5);

        let recoverable = claim(
            &database,
            delivery(&database, &hook, "push", started).await,
            started,
        )
        .await;
        let mut exhausted: repository_webhook_delivery::ActiveModel =
            delivery(&database, &hook, "push", started).await.into();
        exhausted.status = Set("delivering".to_owned());
        exhausted.attempt_count = Set(WEBHOOK_MAX_ATTEMPTS);
        exhausted.attempted_at = Set(Some(started));
        let exhausted = exhausted.update(&database).await.unwrap();
        let in_flight = claim(
            &database,
            delivery(&database, &hook, "push", now).await,
            now,
        )
        .await;

        recover_stale_deliveries(&database, now).await.ok().unwrap();

        assert_eq!(reload(&database, recoverable.id).await.status, "pending");
        assert_eq!(reload(&database, exhausted.id).await.status, "failed");
        assert_eq!(reload(&database, in_flight.id).await.status, "delivering");
        assert_eq!(
            next_due_delivery(&database, now)
                .await
                .ok()
                .unwrap()
                .map(|due| due.id),
            Some(recoverable.id)
        );
    }
}

fn outbox(state: &RepositoryState) -> &gitadel_repositories::webhook_outbox::WebhookOutbox {
    &state.webhook_outbox
}
#[cfg(test)]
use gitadel_repositories::webhook_outbox::{
    WEBHOOK_MAX_ATTEMPTS, is_retryable, next_due_delivery, record_attempt,
    recover_stale_deliveries, retry_delay,
};
