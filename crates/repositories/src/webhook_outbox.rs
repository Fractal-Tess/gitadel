//! Durable webhook outbox claims, retries and bounded outbound delivery.
use chrono::{DateTime, Utc};
use futures_util::TryStreamExt as _;
use gitadel_core::network::pinned_public_http_client;
use gitadel_db::entity::{repository_webhook, repository_webhook_delivery};
use gitadel_identity::IdentityError;
use gitadel_secrets::{SecretCipher, SecretKind};
use hmac::{Hmac, KeyInit as _, Mac};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, Condition, DatabaseConnection, EntityTrait, ExprTrait,
    QueryFilter, QueryOrder, QuerySelect, Set, sea_query::Expr,
};
use serde_json::Value;
use sha2::Sha256;
use std::{
    sync::{Arc, LazyLock},
    time::{Duration, Instant},
};
use tokio::sync::Semaphore;
use tokio_util::task::TaskTracker;
use url::Url;
use uuid::Uuid;
const WEBHOOK_DELIVERY_HISTORY_LIMIT: usize = 50;
const WEBHOOK_DELIVERY_BODY_LIMIT: usize = 2048;
const WEBHOOK_RESPONSE_BYTE_LIMIT: usize = WEBHOOK_DELIVERY_BODY_LIMIT * 4;
const WEBHOOK_DELIVERY_CONCURRENCY: usize = 8;
pub const WEBHOOK_MAX_ATTEMPTS: i32 = 5;
const WEBHOOK_STALE_AFTER_SECONDS: i64 = 60;
static WEBHOOK_DELIVERY_SLOTS: LazyLock<Arc<Semaphore>> =
    LazyLock::new(|| Arc::new(Semaphore::new(WEBHOOK_DELIVERY_CONCURRENCY)));
/// Construction starts no workers. The server supplies its drain tracker and
/// drives finite ticks; claimed deliveries join that tracker.
#[derive(Clone)]
pub struct WebhookOutbox {
    database: DatabaseConnection,
    cipher: SecretCipher,
    tasks: TaskTracker,
}
impl WebhookOutbox {
    pub fn task_tracker(&self) -> &TaskTracker {
        &self.tasks
    }
    pub fn new(database: DatabaseConnection, cipher: SecretCipher, tasks: TaskTracker) -> Self {
        Self {
            database,
            cipher,
            tasks,
        }
    }
    pub async fn insert_delivery(
        &self,
        hook: &repository_webhook::Model,
        event: &str,
        payload: &Value,
    ) -> Result<(), IdentityError> {
        repository_webhook_delivery::ActiveModel {
            id: Set(Uuid::new_v4()),
            webhook_id: Set(hook.id),
            event: Set(event.to_owned()),
            payload: Set(payload.to_string()),
            response_status: Set(None),
            response_body: Set(None),
            duration_ms: Set(0),
            created_at: Set(Utc::now()),
            status: Set("pending".to_owned()),
            attempt_count: Set(0),
            attempted_at: Set(None),
            next_attempt_at: Set(None),
        }
        .insert(&self.database)
        .await?;
        Ok(())
    }

    pub async fn process_pending_deliveries(&self) -> Result<(), IdentityError> {
        let database = &self.database;
        recover_stale_deliveries(database, Utc::now()).await?;
        loop {
            let Ok(permit) = Arc::clone(&WEBHOOK_DELIVERY_SLOTS).try_acquire_owned() else {
                break;
            };
            let Some(delivery) = next_due_delivery(database, Utc::now()).await? else {
                break;
            };
            let claimed = repository_webhook_delivery::Entity::update_many()
                .col_expr(
                    repository_webhook_delivery::Column::Status,
                    Expr::value("delivering"),
                )
                .col_expr(
                    repository_webhook_delivery::Column::AttemptCount,
                    Expr::col(repository_webhook_delivery::Column::AttemptCount).add(1),
                )
                .col_expr(
                    repository_webhook_delivery::Column::AttemptedAt,
                    Expr::value(Utc::now()),
                )
                .filter(repository_webhook_delivery::Column::Id.eq(delivery.id))
                .filter(repository_webhook_delivery::Column::Status.eq("pending"))
                .exec(database)
                .await?;
            if claimed.rows_affected == 0 {
                continue;
            }
            let delivery = repository_webhook_delivery::Entity::find_by_id(delivery.id)
                .one(database)
                .await?
                .ok_or_else(IdentityError::not_found)?;
            let Some(hook) = repository_webhook::Entity::find_by_id(delivery.webhook_id)
                .one(database)
                .await?
            else {
                continue;
            };
            let worker_state = self.clone();
            let task_state = worker_state.clone();
            task_state.tasks.spawn(async move {
            let _permit = permit;
            if let Err(error) = worker_state.deliver(&hook, &delivery).await {
                let message = error.to_string();
                if let Err(record_error) =
                    worker_state.finish_delivery(&hook, &delivery, None, 0, Some(message)).await
                {
                    tracing::warn!(%record_error, webhook_id = %hook.id, delivery_id = %delivery.id, "could not record failed webhook delivery");
                }
            }
        });
        }
        Ok(())
    }

    pub async fn deliver(
        &self,
        hook: &repository_webhook::Model,
        delivery: &repository_webhook_delivery::Model,
    ) -> Result<(), IdentityError> {
        let body = delivery.payload.as_bytes();
        let endpoint = Url::parse(&hook.url).map_err(IdentityError::internal)?;
        let client = pinned_public_http_client(&endpoint, Duration::from_secs(10))
            .await
            .map_err(IdentityError::bad_request)?;
        let mut request = client
            .post(endpoint)
            .header("content-type", "application/json")
            .header("user-agent", "Gitadel-Hookshot/1.0")
            .header("x-github-event", &delivery.event)
            .header("x-github-delivery", delivery.id.to_string())
            .header("x-github-hook-id", hook.id.to_string())
            .header("x-gitadel-event", &delivery.event)
            .body(body.to_vec());
        if let Some(secret) = &hook.secret {
            let secret = self
                .cipher
                .open_text(SecretKind::WebhookSecret, &hook.id, secret)?;
            request = request.header("x-hub-signature-256", signature(&secret, body)?);
        }

        let started = Instant::now();
        let (status, response_body) = match request.send().await {
            Ok(response) => {
                let status = i32::from(response.status().as_u16());
                let mut stream = response.bytes_stream();
                let mut bytes = Vec::with_capacity(WEBHOOK_RESPONSE_BYTE_LIMIT);
                while bytes.len() < WEBHOOK_RESPONSE_BYTE_LIMIT {
                    let Some(chunk) = stream.try_next().await.map_err(IdentityError::internal)?
                    else {
                        break;
                    };
                    let remaining = WEBHOOK_RESPONSE_BYTE_LIMIT - bytes.len();
                    bytes.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
                }
                let text = String::from_utf8_lossy(&bytes).into_owned();
                (Some(status), (!text.is_empty()).then_some(text))
            }
            Err(error) => (None, Some(error.to_string())),
        };
        let duration_ms = i32::try_from(started.elapsed().as_millis()).unwrap_or(i32::MAX);
        self.finish_delivery(hook, delivery, status, duration_ms, response_body)
            .await
    }

    pub async fn finish_delivery(
        &self,
        hook: &repository_webhook::Model,
        delivery: &repository_webhook_delivery::Model,
        response_status: Option<i32>,
        duration_ms: i32,
        response_body: Option<String>,
    ) -> Result<(), IdentityError> {
        record_attempt(
            &self.database,
            hook,
            delivery,
            response_status,
            duration_ms,
            response_body,
            Utc::now(),
        )
        .await
    }
}
/// How long to wait after the given number of failed attempts, or `None` once
/// the delivery has used every attempt. Five attempts span about an hour:
/// immediately, then after 1, 5, 15, and 45 minutes.
pub fn retry_delay(attempt_count: i32) -> Option<chrono::Duration> {
    let minutes = match attempt_count {
        ..=1 => 1,
        2 => 5,
        3 => 15,
        4 => 45,
        _ => return None,
    };
    (attempt_count < WEBHOOK_MAX_ATTEMPTS).then(|| chrono::Duration::minutes(minutes))
}

/// Whether another attempt could plausibly succeed. A receiver that answers
/// with a client error has seen the payload and refused it, so resending the
/// same bytes is pointless; a timeout, connection failure, rate limit, or
/// server error is worth waiting out.
pub fn is_retryable(event: &str, response_status: Option<i32>) -> bool {
    // A ping is a manual connectivity check whose result should be immediate.
    if event == "ping" {
        return false;
    }
    match response_status {
        None => true,
        Some(status) => matches!(status, 408 | 425 | 429) || status >= 500,
    }
}

/// Record the outcome of one attempt: completed, failed for good, or back in
/// the outbox with the time of its next attempt.
pub async fn record_attempt(
    database: &DatabaseConnection,
    hook: &repository_webhook::Model,
    delivery: &repository_webhook_delivery::Model,
    response_status: Option<i32>,
    duration_ms: i32,
    response_body: Option<String>,
    now: DateTime<Utc>,
) -> Result<(), IdentityError> {
    let successful = response_status.is_some_and(|status| (200..300).contains(&status));
    let retry_at = (!successful && is_retryable(&delivery.event, response_status))
        .then(|| retry_delay(delivery.attempt_count))
        .flatten()
        .map(|delay| now + delay);
    let status = if successful {
        "completed"
    } else if retry_at.is_some() {
        "pending"
    } else {
        "failed"
    };
    let mut active: repository_webhook_delivery::ActiveModel = delivery.clone().into();
    active.status = Set(status.to_owned());
    active.next_attempt_at = Set(retry_at);
    active.response_status = Set(response_status);
    active.response_body = Set(response_body.as_ref().map(|body| {
        body.chars()
            .take(WEBHOOK_DELIVERY_BODY_LIMIT)
            .collect::<String>()
    }));
    active.duration_ms = Set(duration_ms);
    active.update(database).await?;

    let mut active_hook: repository_webhook::ActiveModel = hook.clone().into();
    active_hook.last_delivery_at = Set(Some(now));
    active_hook.last_response_status = Set(response_status);
    active_hook.last_response_message = Set(response_body
        .as_ref()
        .map(|body| body.chars().take(512).collect::<String>()));
    active_hook.update(database).await?;
    prune_deliveries(database, hook.id).await
}

/// Return deliveries whose worker died mid-request to the outbox, and give up
/// on those that have already used every attempt so a delivery that keeps
/// killing its worker cannot be retried forever.
pub async fn recover_stale_deliveries(
    database: &DatabaseConnection,
    now: DateTime<Utc>,
) -> Result<(), IdentityError> {
    let stale = now - chrono::Duration::seconds(WEBHOOK_STALE_AFTER_SECONDS);
    repository_webhook_delivery::Entity::update_many()
        .col_expr(
            repository_webhook_delivery::Column::Status,
            Expr::value("failed"),
        )
        .col_expr(
            repository_webhook_delivery::Column::ResponseBody,
            Expr::value("The delivery was interrupted before it finished."),
        )
        .filter(repository_webhook_delivery::Column::Status.eq("delivering"))
        .filter(repository_webhook_delivery::Column::AttemptedAt.lt(stale))
        .filter(repository_webhook_delivery::Column::AttemptCount.gte(WEBHOOK_MAX_ATTEMPTS))
        .exec(database)
        .await?;
    repository_webhook_delivery::Entity::update_many()
        .col_expr(
            repository_webhook_delivery::Column::Status,
            Expr::value("pending"),
        )
        .col_expr(
            repository_webhook_delivery::Column::NextAttemptAt,
            Expr::value(Option::<DateTime<Utc>>::None),
        )
        .filter(repository_webhook_delivery::Column::Status.eq("delivering"))
        .filter(repository_webhook_delivery::Column::AttemptedAt.lt(stale))
        .exec(database)
        .await?;
    Ok(())
}

/// The oldest pending delivery whose next attempt is due.
pub async fn next_due_delivery(
    database: &DatabaseConnection,
    now: DateTime<Utc>,
) -> Result<Option<repository_webhook_delivery::Model>, IdentityError> {
    Ok(repository_webhook_delivery::Entity::find()
        .filter(repository_webhook_delivery::Column::Status.eq("pending"))
        .filter(
            Condition::any()
                .add(repository_webhook_delivery::Column::NextAttemptAt.is_null())
                .add(repository_webhook_delivery::Column::NextAttemptAt.lte(now)),
        )
        .order_by_asc(repository_webhook_delivery::Column::CreatedAt)
        .one(database)
        .await?)
}

pub async fn prune_deliveries(
    database: &DatabaseConnection,
    webhook_id: Uuid,
) -> Result<(), IdentityError> {
    let terminal = ["completed", "failed"];
    let keep = repository_webhook_delivery::Entity::find()
        .filter(repository_webhook_delivery::Column::WebhookId.eq(webhook_id))
        .filter(repository_webhook_delivery::Column::Status.is_in(terminal))
        .order_by_desc(repository_webhook_delivery::Column::CreatedAt)
        .limit(WEBHOOK_DELIVERY_HISTORY_LIMIT as u64)
        .all(database)
        .await?;
    if keep.len() < WEBHOOK_DELIVERY_HISTORY_LIMIT {
        return Ok(());
    }
    repository_webhook_delivery::Entity::delete_many()
        .filter(repository_webhook_delivery::Column::WebhookId.eq(webhook_id))
        .filter(repository_webhook_delivery::Column::Status.is_in(terminal))
        .filter(
            repository_webhook_delivery::Column::Id.is_not_in(
                keep.into_iter()
                    .map(|delivery| delivery.id)
                    .collect::<Vec<_>>(),
            ),
        )
        .exec(database)
        .await?;
    Ok(())
}

pub fn signature(secret: &str, body: &[u8]) -> Result<String, IdentityError> {
    let mut mac =
        Hmac::<Sha256>::new_from_slice(secret.as_bytes()).map_err(IdentityError::internal)?;
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
