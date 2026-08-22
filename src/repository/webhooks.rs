use std::{collections::BTreeMap, path::Path, process::Stdio, time::Duration};

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
    ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, QueryOrder, Set, TransactionTrait,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::Sha256;
use tokio::process::Command;
use url::Url;
use uuid::Uuid;

use super::{Permission, RepositoryState};
use crate::{
    entity::{repository, repository_webhook, user},
    identity::{ApiError, SCOPE_READ, SCOPE_WRITE},
};

pub(super) type RefSnapshot = BTreeMap<String, String>;

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
    repository: &repository::Model,
    actor_user_id: Uuid,
    before: RefSnapshot,
) -> Result<(), ApiError> {
    let hooks = repository_webhook::Entity::find()
        .filter(repository_webhook::Column::RepositoryId.eq(repository.id))
        .filter(repository_webhook::Column::Active.eq(true))
        .all(state.identity().database())
        .await?;
    if hooks.is_empty() {
        return Ok(());
    }
    let after = snapshot_refs(&state.repository_path(repository)).await?;
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
        let payload = push_payload(
            state, repository, &actor, reference, old_oid, new_oid, &zero,
        );
        for hook in &hooks {
            queue_delivery(state.clone(), hook.clone(), "push", payload.clone());
        }
    }
    Ok(())
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
    queue_delivery(state, hook, "ping", payload);
}

fn queue_delivery(
    state: RepositoryState,
    hook: repository_webhook::Model,
    event: &'static str,
    payload: Value,
) {
    tokio::spawn(async move {
        if let Err(error) = deliver(&state, &hook, event, &payload).await {
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

    let delivered_at = Utc::now();
    let (status, message) = match request.send().await {
        Ok(response) => {
            let status = i32::from(response.status().as_u16());
            let message = response
                .text()
                .await
                .unwrap_or_default()
                .chars()
                .take(512)
                .collect::<String>();
            (Some(status), (!message.is_empty()).then_some(message))
        }
        Err(error) => (None, Some(error.to_string())),
    };
    let mut active: repository_webhook::ActiveModel = hook.clone().into();
    active.last_delivery_at = Set(Some(delivered_at));
    active.last_response_status = Set(status);
    active.last_response_message = Set(message);
    active.update(state.identity().database()).await?;
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

fn push_payload(
    state: &RepositoryState,
    repository: &repository::Model,
    actor: &user::Model,
    reference: &str,
    before: &str,
    after: &str,
    zero: &str,
) -> Value {
    json!({
        "ref": reference,
        "before": before,
        "after": after,
        "created": before == zero,
        "deleted": after == zero,
        "forced": false,
        "base_ref": null,
        "compare": null,
        "commits": [],
        "head_commit": null,
        "repository": repository_payload(state, repository),
        "pusher": { "name": actor.username },
        "sender": user_payload(actor),
    })
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
