use std::{collections::HashMap, convert::Infallible, time::Duration};

use axum::{
    Json,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::sse::{Event, Sse},
};
use axum_extra::extract::cookie::CookieJar;
use futures_util::stream;
use sea_orm::EntityTrait as _;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{ApiError, IdentityState, SCOPE_READ, SCOPE_WRITE, require_admin};
use crate::{
    entity::{namespace, organization, registry_storage_migration, repository},
    registry::{storage, store::RegistryUsage},
};

#[derive(Debug, Serialize)]
pub struct RegistryStatusResponse {
    active_target_id: Option<Uuid>,
    #[serde(flatten)]
    usage: RegistryUsage,
}

#[derive(Debug, Deserialize)]
pub struct RegistryRepositoryUsageQuery {
    search: Option<String>,
    owner: Option<String>,
    owner_type: Option<String>,
    min_bytes: Option<u64>,
    max_bytes: Option<u64>,
    sort: Option<String>,
    limit: Option<u64>,
    offset: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct RegistryRepositoryUsage {
    repository_id: Uuid,
    repository_name: String,
    owner_name: String,
    owner_type: String,
    object_count: u64,
    total_bytes: u64,
    image_count: u64,
    blob_count: u64,
    manifest_count: u64,
    tag_count: u64,
}

#[derive(Debug, Serialize)]
pub struct RegistryRepositoryUsageResponse {
    repositories: Vec<RegistryRepositoryUsage>,
    total: u64,
    limit: u64,
    offset: u64,
}

async fn usage(
    state: &IdentityState,
) -> Result<(Option<Uuid>, Vec<(repository::Model, RegistryUsage)>), ApiError> {
    let manager = state
        .registry_storage()
        .await
        .ok_or_else(|| ApiError::internal("Registry storage is unavailable."))?;
    let active = manager.active();
    let rows = storage::usage_by_repository(
        state.database(),
        &state.runtime_settings()?.storage,
        &active,
    )
    .await
    .map_err(ApiError::internal)?;
    Ok((active.map(|active| active.id), rows))
}

pub async fn registry_status(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<RegistryStatusResponse>, ApiError> {
    require_admin(&state, &headers, &jar, SCOPE_READ).await?;
    let (active_target_id, rows) = usage(&state).await?;
    let mut total = RegistryUsage::default();
    for (_, usage) in rows {
        total += usage;
    }
    Ok(Json(RegistryStatusResponse {
        active_target_id,
        usage: total,
    }))
}

pub async fn list_registry_repository_usage(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Query(query): Query<RegistryRepositoryUsageQuery>,
) -> Result<Json<RegistryRepositoryUsageResponse>, ApiError> {
    require_admin(&state, &headers, &jar, SCOPE_READ).await?;
    let limit = query.limit.unwrap_or(10);
    if !(1..=100).contains(&limit) {
        return Err(ApiError::bad_request(
            "The registry usage page size must be between 1 and 100.",
        ));
    }
    if query
        .min_bytes
        .zip(query.max_bytes)
        .is_some_and(|(minimum, maximum)| minimum > maximum)
    {
        return Err(ApiError::bad_request(
            "The minimum registry usage cannot exceed the maximum.",
        ));
    }
    let owner_type = query.owner_type.as_deref().unwrap_or("");
    if !matches!(owner_type, "" | "user" | "organization") {
        return Err(ApiError::bad_request(
            "The registry owner type must be user or organization.",
        ));
    }
    let sort = query.sort.as_deref().unwrap_or("bytes_desc");
    if !matches!(sort, "bytes_desc" | "bytes_asc" | "name") {
        return Err(ApiError::bad_request(
            "The registry usage sort must be bytes_desc, bytes_asc, or name.",
        ));
    }
    let search = query.search.as_deref().unwrap_or("").trim().to_lowercase();
    let owner_search = query.owner.as_deref().unwrap_or("").trim().to_lowercase();
    let namespaces: HashMap<_, _> = namespace::Entity::find()
        .all(state.database())
        .await?
        .into_iter()
        .map(|owner| (owner.slug.clone(), owner))
        .collect();
    let organizations: HashMap<_, _> = organization::Entity::find()
        .all(state.database())
        .await?
        .into_iter()
        .map(|owner| (owner.id, owner.display_name.to_lowercase()))
        .collect();
    let (_, usages) = usage(&state).await?;
    let mut rows = Vec::new();
    for (repository, usage) in usages {
        if repository.deleted_at.is_some()
            || usage.object_count == 0
            || (!search.is_empty() && !repository.name.to_lowercase().contains(&search))
            || query
                .min_bytes
                .is_some_and(|minimum| usage.total_bytes < minimum)
            || query
                .max_bytes
                .is_some_and(|maximum| usage.total_bytes > maximum)
        {
            continue;
        }
        let owner = namespaces
            .get(&repository.namespace)
            .ok_or_else(|| ApiError::internal("Repository namespace is missing."))?;
        if !owner_type.is_empty() && owner.kind != owner_type {
            continue;
        }
        if !owner_search.is_empty()
            && !owner.slug.to_lowercase().contains(&owner_search)
            && !owner
                .organization_id
                .and_then(|id| organizations.get(&id))
                .is_some_and(|name| name.contains(&owner_search))
        {
            continue;
        }
        rows.push(RegistryRepositoryUsage {
            repository_id: repository.id,
            repository_name: repository.name,
            owner_name: repository.namespace,
            owner_type: owner.kind.clone(),
            object_count: usage.object_count,
            total_bytes: usage.total_bytes,
            image_count: usage.image_count,
            blob_count: usage.blob_count,
            manifest_count: usage.manifest_count,
            tag_count: usage.tag_count,
        });
    }
    rows.sort_by(|left, right| {
        let ordering = match sort {
            "bytes_desc" => right.total_bytes.cmp(&left.total_bytes),
            "bytes_asc" => left.total_bytes.cmp(&right.total_bytes),
            _ => std::cmp::Ordering::Equal,
        };
        ordering
            .then_with(|| left.repository_name.cmp(&right.repository_name))
            .then_with(|| left.owner_name.cmp(&right.owner_name))
            .then_with(|| left.repository_id.cmp(&right.repository_id))
    });
    let total = rows.len() as u64;
    let offset = query.offset.unwrap_or(0);
    let repositories = rows
        .into_iter()
        .skip(usize::try_from(offset).unwrap_or(usize::MAX))
        .take(limit as usize)
        .collect();
    Ok(Json(RegistryRepositoryUsageResponse {
        repositories,
        total,
        limit,
        offset,
    }))
}

#[derive(Debug, Deserialize)]
pub struct RegistryMigrationRequest {
    target_id: Uuid,
    #[serde(default = "default_batch_size")]
    batch_size: usize,
}
fn default_batch_size() -> usize {
    100
}

#[derive(Debug, Serialize)]
pub struct RegistryMigrationResponse {
    operation_id: Uuid,
    target_id: Uuid,
    message: &'static str,
}

pub async fn migrate_registry(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<RegistryMigrationRequest>,
) -> Result<(StatusCode, Json<RegistryMigrationResponse>), ApiError> {
    let actor = require_admin(&state, &headers, &jar, SCOPE_WRITE).await?;
    if request.batch_size == 0 {
        return Err(ApiError::bad_request(
            "The registry migration batch size must be greater than zero.",
        ));
    }
    if !request.target_id.is_nil() {
        crate::blob_store::targets::find(state.database(), request.target_id)
            .await
            .map_err(|_| ApiError::bad_request("The storage target does not exist."))?;
    }
    let manager = state
        .registry_storage()
        .await
        .ok_or_else(|| ApiError::internal("Registry storage is unavailable."))?;
    let migration_guard = manager
        .try_lock_migration()
        .ok_or_else(|| ApiError::conflict("Another registry migration is already in progress."))?;
    let operation_id = Uuid::new_v4();
    let settings = state.runtime_settings()?.storage.clone();
    storage::reserve_migration(
        state.database(),
        request.target_id,
        operation_id,
        manager.target_id(),
    )
    .await
    .map_err(|error| ApiError::bad_request(format!("Could not start migration: {error:#}")))?;
    if let Err(error) = state
        .audit(
            Some(actor.user.id),
            "registry.migration.start",
            Some(request.target_id.to_string()),
        )
        .await
    {
        storage::mark_failed(state.database(), operation_id, &anyhow::anyhow!("{error}")).await;
        return Err(error);
    }
    let database = state.database().clone();
    tokio::spawn(async move {
        if let Err(error) = storage::migrate_online(
            &database,
            &settings,
            operation_id,
            request.batch_size,
            manager,
            migration_guard,
        )
        .await
        {
            tracing::error!(%error, %operation_id, "online registry storage migration failed");
        }
    });
    Ok((
        StatusCode::ACCEPTED,
        Json(RegistryMigrationResponse {
            operation_id,
            target_id: request.target_id,
            message: "Container registry storage migration started without restarting Gitadel.",
        }),
    ))
}

pub async fn registry_migration_events(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(operation_id): Path<Uuid>,
) -> Result<Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>>, ApiError> {
    require_admin(&state, &headers, &jar, SCOPE_WRITE).await?;
    registry_storage_migration::Entity::find_by_id(operation_id)
        .one(state.database())
        .await?
        .ok_or_else(ApiError::not_found)?;
    let events = stream::unfold(
        (state.database().clone(), false),
        move |(database, done)| async move {
            if done {
                return None;
            }
            let (payload, terminal) = match registry_storage_migration::Entity::find_by_id(
                operation_id,
            )
            .one(&database)
            .await
            {
                Ok(Some(row)) => {
                    let terminal = matches!(row.state.as_str(), "completed" | "failed");
                    let message = match row.phase.as_str() {
                        "scheduled" => "Container registry migration is starting.",
                        "checking_destination" => "Checking the destination storage target.",
                        "copying_registry" => "Copying and verifying container registry objects.",
                        "writing_metadata" => "Selecting the verified registry storage target.",
                        "completed" => "Container registry storage migration completed.",
                        "failed" => row
                            .error
                            .as_deref()
                            .unwrap_or("Container registry migration failed."),
                        _ => "Migrating container registry objects.",
                    };
                    (
                        serde_json::json!({
                            "operation_id": row.id, "key": row.last_key.as_deref().unwrap_or(""),
                            "operation": "registry_migrate", "phase": row.phase, "message": message,
                            "processed_bytes": row.copied_bytes.max(0), "total_bytes": row.total_bytes,
                        }),
                        terminal,
                    )
                }
                result => {
                    tracing::error!(?result, %operation_id, "could not read registry migration progress");
                    (
                        serde_json::json!({
                            "operation_id": operation_id, "key": "", "operation": "registry_migrate",
                            "phase": "failed", "message": "Could not read registry migration progress.",
                            "processed_bytes": null, "total_bytes": null,
                        }),
                        true,
                    )
                }
            };
            let event = Event::default().data(payload.to_string());
            if !terminal {
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            Some((Ok(event), (database, terminal)))
        },
    );
    Ok(Sse::new(events))
}
