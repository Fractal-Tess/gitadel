//! Deprecated per-domain storage routes, kept for one release as thin aliases
//! of the `/admin/storage/domains/{domain}` API. Responses keep their original
//! shapes; new clients should use the domain API.

use std::convert::Infallible;

use axum::{
    Json,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::sse::{Event, Sse},
};
use axum_extra::extract::cookie::CookieJar;
use futures_util::Stream;
use serde::Serialize;
use uuid::Uuid;

use super::{
    Domain, MigrationRequest, MigrationStarted, RepositoryUsagePage, RepositoryUsageQuery,
    RepositoryUsageRow, UsageTotals, entry, migration_events, repositories, start_migration,
    status,
};
use crate::identity::{ApiError, IdentityState, SCOPE_READ, SCOPE_WRITE, require_admin};

const LFS: &str = "lfs";
const REGISTRY: &str = "registry";

#[derive(Debug, Serialize)]
pub struct LfsStatus {
    object_count: u64,
    total_bytes: u64,
}

/// Deprecated alias of `GET /admin/storage/domains/lfs`.
pub async fn lfs_status(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<LfsStatus>, ApiError> {
    require_admin(&state, &headers, &jar, SCOPE_READ).await?;
    let status = status(&state, &Domain::find(&state, LFS).await?).await?;
    Ok(Json(LfsStatus {
        object_count: status.usage.usage.object_count,
        total_bytes: status.usage.usage.total_bytes,
    }))
}

/// Deprecated alias of `GET /admin/storage/domains/lfs/repositories`.
pub async fn lfs_repositories(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Query(query): Query<RepositoryUsageQuery>,
) -> Result<Json<RepositoryUsagePage>, ApiError> {
    require_admin(&state, &headers, &jar, SCOPE_READ).await?;
    let domain = Domain::find(&state, LFS).await?;
    Ok(Json(repositories(&state, &domain, &query).await?))
}

/// Deprecated alias of `POST /admin/storage/domains/lfs/migrate`.
pub async fn lfs_migrate(
    state: State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<MigrationRequest>,
) -> Result<(StatusCode, Json<MigrationStarted>), ApiError> {
    migrate(state, headers, jar, LFS, request).await
}

/// Deprecated alias of `GET /admin/storage/domains/lfs/migrations/{id}/events`.
pub async fn lfs_progress(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(operation_id): Path<Uuid>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    require_admin(&state, &headers, &jar, SCOPE_READ).await?;
    migration_events(&state, entry(LFS)?, operation_id).await
}

#[derive(Debug, Serialize)]
pub struct RegistryStatus {
    active_target_id: Option<Uuid>,
    #[serde(flatten)]
    usage: UsageTotals,
}

/// Deprecated alias of `GET /admin/storage/domains/registry`. The registry
/// counters, nested under `details` in the domain API, stay top-level here.
pub async fn registry_status(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&state, &headers, &jar, SCOPE_READ).await?;
    let status = status(&state, &Domain::find(&state, REGISTRY).await?).await?;
    let mut value = serde_json::to_value(RegistryStatus {
        active_target_id: status.active_target_id,
        usage: status.usage,
    })
    .map_err(ApiError::internal)?;
    flatten_details(&mut value);
    Ok(Json(value))
}

/// Deprecated alias of `GET /admin/storage/domains/registry/repositories`.
pub async fn registry_repositories(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Query(query): Query<RepositoryUsageQuery>,
) -> Result<Json<RepositoryUsagePage<serde_json::Value>>, ApiError> {
    require_admin(&state, &headers, &jar, SCOPE_READ).await?;
    let domain = Domain::find(&state, REGISTRY).await?;
    let page = repositories(&state, &domain, &query).await?;
    let rows = page
        .repositories
        .into_iter()
        .map(|row: RepositoryUsageRow| {
            let mut value = serde_json::to_value(row)?;
            flatten_details(&mut value);
            Ok(value)
        })
        .collect::<Result<Vec<_>, serde_json::Error>>()
        .map_err(ApiError::internal)?;
    Ok(Json(RepositoryUsagePage {
        repositories: rows,
        total: page.total,
        limit: page.limit,
        offset: page.offset,
    }))
}

/// Deprecated alias of `POST /admin/storage/domains/registry/migrate`.
pub async fn registry_migrate(
    state: State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<MigrationRequest>,
) -> Result<(StatusCode, Json<MigrationStarted>), ApiError> {
    migrate(state, headers, jar, REGISTRY, request).await
}

/// Deprecated alias of
/// `GET /admin/storage/domains/registry/migrations/{id}/events`.
pub async fn registry_events(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(operation_id): Path<Uuid>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    require_admin(&state, &headers, &jar, SCOPE_READ).await?;
    migration_events(&state, entry(REGISTRY)?, operation_id).await
}

async fn migrate(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    domain: &str,
    request: MigrationRequest,
) -> Result<(StatusCode, Json<MigrationStarted>), ApiError> {
    let actor = require_admin(&state, &headers, &jar, SCOPE_WRITE).await?;
    let domain = Domain::find(&state, domain).await?;
    let started = start_migration(&state, &domain, actor.user.id, &request).await?;
    Ok((StatusCode::ACCEPTED, Json(started)))
}

/// Moves the entries of a `details` object up into its parent.
pub(super) fn flatten_details(value: &mut serde_json::Value) {
    let Some(object) = value.as_object_mut() else {
        return;
    };
    if let Some(serde_json::Value::Object(details)) = object.remove("details") {
        object.extend(details);
    }
}
