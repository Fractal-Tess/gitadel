use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    process::Stdio,
    time::{Duration, Instant},
};

use axum::{
    Json,
    extract::{Path as AxumPath, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use axum_extra::extract::cookie::CookieJar;
use chrono::Utc;
use hmac::{Hmac, Mac};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder,
    QuerySelect, Set, TransactionTrait,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::Sha256;
use tokio::process::Command;
use url::Url;
use uuid::Uuid;

use super::{Permission, RepositoryState};
use crate::{
    actions::ActionsState,
    entity::{repository, repository_webhook, repository_webhook_delivery, user},
    identity::{ApiError, SCOPE_READ, SCOPE_WRITE},
};

pub(super) type RefSnapshot = BTreeMap<String, String>;

/// Maximum number of delivery records kept per webhook.
const WEBHOOK_DELIVERY_HISTORY_LIMIT: usize = 50;

/// Maximum number of response body characters stored per delivery.
const WEBHOOK_DELIVERY_BODY_LIMIT: usize = 2048;

#[derive(Serialize)]
struct WebhookConfigResponse {
    url: String,
    content_type: &'static str,
    insecure_ssl: &'static str,
}

#[derive(Serialize)]
struct WebhookLastResponse {
    code: Option<i32>,
    status: &'static str,
    message: Option<String>,
}

#[derive(Serialize)]
pub struct WebhookResponse {
    id: Uuid,
    #[serde(rename = "type")]
    type_name: &'static str,
    name: &'static str,
    active: bool,
    events: [&'static str; 1],
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
            events: ["push"],
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

#[derive(Serialize)]
pub struct WebhookDeliveryResponse {
    id: Uuid,
    event: String,
    status_code: Option<i32>,
    status: &'static str,
    delivered_at: chrono::DateTime<Utc>,
    duration_ms: i32,
    payload: Value,
    response_body: Option<String>,
}

impl WebhookDeliveryResponse {
    fn new(delivery: repository_webhook_delivery::Model) -> Self {
        let status = if delivery
            .response_status
            .is_some_and(|code| (200..300).contains(&code))
        {
            "ok"
        } else {
            "failed"
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
        }
    }
}

#[derive(Deserialize)]
pub struct WebhookConfigRequest {
    url: String,
    content_type: Option<String>,
    secret: Option<String>,
}

#[derive(Deserialize)]
pub struct CreateWebhookRequest {
    name: Option<String>,
    active: Option<bool>,
    events: Option<Vec<String>>,
    config: WebhookConfigRequest,
}

#[derive(Default, Deserialize)]
pub struct UpdateWebhookConfigRequest {
    url: Option<String>,
    content_type: Option<String>,
    secret: Option<String>,
}

#[derive(Deserialize)]
pub struct UpdateWebhookRequest {
    active: Option<bool>,
    events: Option<Vec<String>>,
    config: Option<UpdateWebhookConfigRequest>,
}

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

pub async fn create_webhook(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name)): AxumPath<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<CreateWebhookRequest>,
) -> Result<impl IntoResponse, ApiError> {
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
    validate_kind(request.name.as_deref(), request.events.as_deref())?;
    validate_content_type(request.config.content_type.as_deref())?;
    let endpoint = validate_endpoint(&request.config.url)?;
    let secret = validate_secret(request.config.secret)?;
    let now = Utc::now();
    let transaction = state.identity().database().begin().await?;
    let hook = repository_webhook::ActiveModel {
        id: Set(Uuid::new_v4()),
        repository_id: Set(repository.id),
        url: Set(endpoint),
        secret: Set(secret),
        active: Set(request.active.unwrap_or(true)),
        created_at: Set(now),
        updated_at: Set(now),
        last_delivery_at: Set(None),
        last_response_status: Set(None),
        last_response_message: Set(None),
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

    queue_ping(state.clone(), hook.clone(), repository.clone(), actor.user);
    Ok((
        StatusCode::CREATED,
        Json(WebhookResponse::new(hook, &state, &repository)),
    ))
}

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
    validate_kind(None, request.events.as_deref())?;
    let stored = find_hook(state.identity().database(), repository.id, id).await?;
    let mut active: repository_webhook::ActiveModel = stored.into();
    if let Some(enabled) = request.active {
        active.active = Set(enabled);
    }
    if let Some(config) = request.config {
        validate_content_type(config.content_type.as_deref())?;
        if let Some(endpoint) = config.url {
            active.url = Set(validate_endpoint(&endpoint)?);
        }
        if let Some(secret) = config.secret {
            active.secret = Set(validate_secret(Some(secret))?);
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
    queue_delivery(state, hook, delivery.event, payload);
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
    queue_ping(state, hook, repository, actor.user);
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn snapshot_refs(path: &Path) -> Result<RefSnapshot, ApiError> {
    let output = Command::new("git")
        .arg("--git-dir")
        .arg(path)
        .args([
            "for-each-ref",
            "--format=%(refname) %(objectname)",
            "refs/heads",
            "refs/tags",
        ])
        .stdin(Stdio::null())
        .output()
        .await
        .map_err(ApiError::internal)?;
    if !output.status.success() {
        return Err(ApiError::internal(String::from_utf8_lossy(&output.stderr)));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.split_once(' '))
        .map(|(name, oid)| (name.to_owned(), oid.to_owned()))
        .collect())
}

pub(super) async fn dispatch_push(
    state: &RepositoryState,
    actions: &ActionsState,
    repository: &repository::Model,
    actor_user_id: Uuid,
    before: RefSnapshot,
) -> Result<(), ApiError> {
    let hooks = repository_webhook::Entity::find()
        .filter(repository_webhook::Column::RepositoryId.eq(repository.id))
        .filter(repository_webhook::Column::Active.eq(true))
        .all(state.identity().database())
        .await?;
    let integrations_enabled = super::integrations::has_enabled(state, repository.id).await?;
    let path = state.repository_path(repository);
    let after = snapshot_refs(&path).await?;
    let actor = user::Entity::find_by_id(actor_user_id)
        .one(state.identity().database())
        .await?
        .ok_or_else(ApiError::not_found)?;
    let zero = if repository.object_format == "sha256" {
        "0".repeat(64)
    } else {
        "0".repeat(40)
    };

    for (reference, old_oid, new_oid) in changed_refs(&before, &after, &zero) {
        let commits = commit_entries(&path, old_oid, new_oid, &zero).await;
        let changed_paths = pushed_paths(&commits);
        let pushed = PushedRef {
            reference,
            before: old_oid,
            after: new_oid,
            zero: &zero,
        };
        let payload = push_payload(state, repository, &actor, &pushed, commits);
        if let Err(error) = crate::actions::workflow::ingest_push(
            actions,
            repository,
            actor_user_id,
            reference,
            old_oid,
            new_oid,
            &zero,
            changed_paths,
        )
        .await
        {
            tracing::error!(
                %error,
                repository_id = %repository.id,
                reference,
                commit = new_oid,
                "could not enqueue Actions workflows"
            );
        }
        for hook in &hooks {
            queue_delivery(
                state.clone(),
                hook.clone(),
                String::from("push"),
                payload.clone(),
            );
        }
        if integrations_enabled {
            super::integrations::dispatch_push(
                state,
                repository,
                reference,
                new_oid == zero,
                payload,
            );
        }
    }
    Ok(())
}

fn pushed_paths(commits: &[Value]) -> Vec<String> {
    let mut paths = BTreeSet::new();
    for commit in commits {
        for field in ["added", "modified", "removed"] {
            if let Some(values) = commit.get(field).and_then(Value::as_array) {
                paths.extend(values.iter().filter_map(Value::as_str).map(str::to_owned));
            }
        }
    }
    paths.into_iter().collect()
}

fn queue_ping(
    state: RepositoryState,
    hook: repository_webhook::Model,
    repository: repository::Model,
    actor: user::Model,
) {
    let payload = json!({
        "zen": "Design for failure.",
        "hook_id": hook.id,
        "hook": WebhookResponse::new(hook.clone(), &state, &repository),
        "repository": repository_payload(&state, &repository),
        "sender": user_payload(&actor),
    });
    queue_delivery(state, hook, String::from("ping"), payload);
}

fn queue_delivery(
    state: RepositoryState,
    hook: repository_webhook::Model,
    event: String,
    payload: Value,
) {
    let task_state = state.clone();
    task_state.spawn_task(async move {
        if let Err(error) = deliver(&state, &hook, &event, &payload).await {
            tracing::warn!(%error, webhook_id = %hook.id, %event, "webhook delivery failed");
        }
    });
}

async fn deliver(
    state: &RepositoryState,
    hook: &repository_webhook::Model,
    event: &str,
    payload: &Value,
) -> Result<(), ApiError> {
    let body = serde_json::to_vec(payload).map_err(ApiError::internal)?;
    let delivery_id = Uuid::new_v4();
    let mut request = state
        .webhook_client()
        .post(&hook.url)
        .header("content-type", "application/json")
        .header("user-agent", "Gitadel-Hookshot/1.0")
        .header("x-github-event", event)
        .header("x-github-delivery", delivery_id.to_string())
        .header("x-github-hook-id", hook.id.to_string())
        .header("x-gitadel-event", event)
        .body(body.clone());
    if let Some(secret) = &hook.secret {
        request = request.header("x-hub-signature-256", signature(secret, &body)?);
    }

    let started = Instant::now();
    let (status, response_body) = match request.send().await {
        Ok(response) => {
            let status = i32::from(response.status().as_u16());
            let text = response.text().await.unwrap_or_default();
            (Some(status), (!text.is_empty()).then_some(text))
        }
        Err(error) => (None, Some(error.to_string())),
    };
    let duration_ms = i32::try_from(started.elapsed().as_millis()).unwrap_or(i32::MAX);
    record_delivery(
        state,
        hook.id,
        event,
        payload,
        status,
        duration_ms,
        &response_body,
    )
    .await?;

    let mut active: repository_webhook::ActiveModel = hook.clone().into();
    active.last_delivery_at = Set(Some(Utc::now()));
    active.last_response_status = Set(status);
    active.last_response_message = Set(response_body
        .as_ref()
        .map(|body| body.chars().take(512).collect::<String>()));
    active.update(state.identity().database()).await?;
    Ok(())
}

async fn record_delivery(
    state: &RepositoryState,
    webhook_id: Uuid,
    event: &str,
    payload: &Value,
    status: Option<i32>,
    duration_ms: i32,
    response_body: &Option<String>,
) -> Result<(), ApiError> {
    let database = state.identity().database();
    repository_webhook_delivery::ActiveModel {
        id: Set(Uuid::new_v4()),
        webhook_id: Set(webhook_id),
        event: Set(event.to_owned()),
        payload: Set(payload.to_string()),
        response_status: Set(status),
        response_body: Set(response_body.as_ref().map(|body| {
            body.chars()
                .take(WEBHOOK_DELIVERY_BODY_LIMIT)
                .collect::<String>()
        })),
        duration_ms: Set(duration_ms),
        created_at: Set(Utc::now()),
    }
    .insert(database)
    .await?;
    prune_deliveries(database, webhook_id).await?;
    Ok(())
}

async fn prune_deliveries(database: &DatabaseConnection, webhook_id: Uuid) -> Result<(), ApiError> {
    let keep = repository_webhook_delivery::Entity::find()
        .filter(repository_webhook_delivery::Column::WebhookId.eq(webhook_id))
        .order_by_desc(repository_webhook_delivery::Column::CreatedAt)
        .limit(WEBHOOK_DELIVERY_HISTORY_LIMIT as u64)
        .all(database)
        .await?;
    if keep.len() < WEBHOOK_DELIVERY_HISTORY_LIMIT {
        return Ok(());
    }
    repository_webhook_delivery::Entity::delete_many()
        .filter(repository_webhook_delivery::Column::WebhookId.eq(webhook_id))
        .filter(
            repository_webhook_delivery::Column::Id
                .is_not_in(keep.into_iter().map(|d| d.id).collect::<Vec<_>>()),
        )
        .exec(database)
        .await?;
    Ok(())
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

fn validate_kind(name: Option<&str>, events: Option<&[String]>) -> Result<(), ApiError> {
    if name.is_some_and(|name| name != "web") {
        return Err(ApiError::bad_request("Webhook name must be web."));
    }
    if events.is_some_and(|events| events != ["push"]) {
        return Err(ApiError::bad_request(
            "This version supports the push webhook event only.",
        ));
    }
    Ok(())
}

fn validate_content_type(content_type: Option<&str>) -> Result<(), ApiError> {
    if content_type.is_some_and(|content_type| content_type != "json") {
        return Err(ApiError::bad_request("Webhook content type must be json."));
    }
    Ok(())
}

fn validate_endpoint(value: &str) -> Result<String, ApiError> {
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
    Ok(endpoint.to_string())
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

fn changed_refs<'a>(
    before: &'a RefSnapshot,
    after: &'a RefSnapshot,
    zero: &'a str,
) -> Vec<(&'a str, &'a str, &'a str)> {
    let mut names = before.keys().chain(after.keys()).collect::<Vec<_>>();
    names.sort_unstable();
    names.dedup();
    names
        .into_iter()
        .filter_map(|name| {
            let old_oid = before.get(name).map_or(zero, String::as_str);
            let new_oid = after.get(name).map_or(zero, String::as_str);
            (old_oid != new_oid).then_some((name.as_str(), old_oid, new_oid))
        })
        .collect()
}

/// Maximum commits enumerated per pushed ref.
///
/// Gitea caps its own push payloads similarly; a branch created from a large
/// import would otherwise walk the entire history.
const PUSH_COMMIT_LIMIT: usize = 50;

/// The four values describing one pushed ref, which always travel together.
struct PushedRef<'a> {
    reference: &'a str,
    before: &'a str,
    after: &'a str,
    zero: &'a str,
}

fn push_payload(
    state: &RepositoryState,
    repository: &repository::Model,
    actor: &user::Model,
    pushed: &PushedRef<'_>,
    commits: Vec<Value>,
) -> Value {
    let PushedRef {
        reference,
        before,
        after,
        zero,
    } = *pushed;
    json!({
        "ref": reference,
        "before": before,
        "after": after,
        "created": before == zero,
        "deleted": after == zero,
        "forced": false,
        "base_ref": null,
        "compare": null,
        "head_commit": commits.last().cloned(),
        "commits": commits,
        "repository": repository_payload(state, repository),
        "pusher": { "name": actor.username },
        "sender": user_payload(actor),
    })
}

/// Enumerate the commits a push introduced, with their changed paths.
///
/// Consumers filter on these paths — Dokploy's watch paths are exactly this —
/// so an empty list silently disables that filtering. Failures degrade to an
/// empty list rather than blocking the push.
async fn commit_entries(path: &Path, before: &str, after: &str, zero: &str) -> Vec<Value> {
    if after == zero {
        return Vec::new();
    }
    // A new ref has no baseline, so walk back from its tip instead of a range.
    let range = if before == zero {
        after.to_owned()
    } else {
        format!("{before}..{after}")
    };
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args([
            "log",
            &format!("--max-count={PUSH_COMMIT_LIMIT}"),
            // STX starts each record so name-status lines stay newline-split.
            "--format=%x02%H%x1f%s%x1f%an%x1f%ae%x1f%cI",
            "--name-status",
            "--no-color",
            &range,
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await;
    let output = match output {
        Ok(output) if output.status.success() => output,
        Ok(output) => {
            tracing::warn!(
                stderr = %String::from_utf8_lossy(&output.stderr).trim(),
                "listing pushed commits failed"
            );
            return Vec::new();
        }
        Err(error) => {
            tracing::warn!(%error, "listing pushed commits failed");
            return Vec::new();
        }
    };

    let text = String::from_utf8_lossy(&output.stdout);
    let mut commits = text
        .split('\u{2}')
        .skip(1)
        .filter_map(parse_commit_record)
        .collect::<Vec<_>>();
    // git log is newest first; push payloads list commits oldest first.
    commits.reverse();
    commits
}

fn parse_commit_record(record: &str) -> Option<Value> {
    let (header, body) = record.split_once('\n').unwrap_or((record, ""));
    let mut fields = header.split('\u{1f}');
    let id = fields.next()?;
    let message = fields.next().unwrap_or_default();
    let author_name = fields.next().unwrap_or_default();
    let author_email = fields.next().unwrap_or_default();
    let timestamp = fields.next().unwrap_or_default();

    let mut added = Vec::new();
    let mut modified = Vec::new();
    let mut removed = Vec::new();
    for line in body.lines().filter(|line| !line.is_empty()) {
        let mut columns = line.split('\t');
        let Some(status) = columns.next() else {
            continue;
        };
        let Some(target) = columns.next() else {
            continue;
        };
        // Renames and copies report the source first, then the destination.
        match status.as_bytes().first() {
            Some(b'A') => added.push(target.to_owned()),
            Some(b'D') => removed.push(target.to_owned()),
            Some(b'R' | b'C') => {
                removed.push(target.to_owned());
                if let Some(destination) = columns.next() {
                    added.push(destination.to_owned());
                }
            }
            Some(_) => modified.push(target.to_owned()),
            None => {}
        }
    }

    Some(json!({
        "id": id,
        "message": message,
        "timestamp": timestamp,
        "author": { "name": author_name, "email": author_email },
        "committer": { "name": author_name, "email": author_email },
        "added": added,
        "modified": modified,
        "removed": removed,
    }))
}

fn repository_payload(state: &RepositoryState, repository: &repository::Model) -> Value {
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

fn user_payload(actor: &user::Model) -> Value {
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

fn public_url(state: &RepositoryState, path: &str) -> String {
    let mut url = state.public_url.as_ref().clone();
    url.set_path(path);
    url.set_query(None);
    url.set_fragment(None);
    url.to_string()
}

fn signature(secret: &str, body: &[u8]) -> Result<String, ApiError> {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).map_err(ApiError::internal)?;
    mac.update(body);
    let digest = mac.finalize().into_bytes();
    let mut encoded = String::with_capacity(7 + digest.len() * 2);
    encoded.push_str("sha256=");
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(encoded, "{byte:02x}");
    }
    Ok(encoded)
}

pub(super) fn webhook_client() -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .build()
}
