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
    RepositoryUsageRow, entry, migration_events, repositories, start_migration, status,
};
use crate::identity::{ApiError, IdentityState, SCOPE_READ, SCOPE_WRITE, require_admin};
use crate::registry::store::RegistryUsage;

const LFS: &str = "lfs";
const REGISTRY: &str = "registry";

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct LfsStatus {
    object_count: u64,
    total_bytes: u64,
}

/// Deprecated alias of `GET /admin/storage/domains/lfs`.
#[utoipa::path(
    get, path = "/admin/storage/lfs/status",
    operation_id = "get_admin_storage_lfs_status",
    summary = "Lfs status",
    tag = "Storage",
    responses(
        (status = 200, description = "Success", body = LfsStatus, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
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
#[utoipa::path(
    get, path = "/admin/storage/lfs/repositories",
    operation_id = "get_admin_storage_lfs_repositories",
    summary = "Lfs repositories",
    tag = "Storage",
    params(
        RepositoryUsageQuery,
    ),
    responses(
        (status = 200, description = "Success", body = RepositoryUsagePage, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
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
#[utoipa::path(
    post, path = "/admin/storage/migrations",
    operation_id = "post_admin_storage_migrations",
    summary = "Lfs migrate",
    tag = "Storage",

    responses(
        (status = 200, description = "Success", body = MigrationStarted, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn lfs_migrate(
    state: State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<MigrationRequest>,
) -> Result<(StatusCode, Json<MigrationStarted>), ApiError> {
    migrate(state, headers, jar, LFS, request).await
}

/// Deprecated alias of `GET /admin/storage/domains/lfs/migrations/{id}/events`.
#[utoipa::path(
    get, path = "/admin/storage/progress/{operation_id}",
    operation_id = "get_admin_storage_progress_operation_id",
    summary = "Lfs progress",
    tag = "Storage",
    params(
        ("operation_id" = Uuid, Path),
    ),
    responses(
        (status = 200, description = "Success", body = String, content_type = "text/event-stream"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn lfs_progress(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(operation_id): Path<Uuid>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    require_admin(&state, &headers, &jar, SCOPE_READ).await?;
    migration_events(&state, entry(LFS)?, operation_id).await
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct RegistryStatus {
    active_target_id: Option<Uuid>,
    repository_count: u64,
    #[serde(flatten)]
    usage: RegistryUsage,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct RegistryRepositoryUsageRow {
    repository_id: Uuid,
    repository_name: String,
    owner_name: String,
    owner_type: String,
    #[serde(flatten)]
    usage: RegistryUsage,
}

fn registry_usage(usage: crate::blob_store::RepositoryUsage) -> RegistryUsage {
    let count = |key| usage.details.get(key).copied().unwrap_or(0);
    RegistryUsage {
        object_count: usage.object_count,
        total_bytes: usage.total_bytes,
        blob_count: count("blob_count"),
        manifest_count: count("manifest_count"),
        tag_count: count("tag_count"),
        image_count: count("image_count"),
        upload_count: count("upload_count"),
        upload_bytes: count("upload_bytes"),
    }
}

/// Deprecated alias of `GET /admin/storage/domains/registry`. The registry
/// counters, nested under `details` in the domain API, stay top-level here.
#[utoipa::path(
    get, path = "/admin/storage/registry/status",
    operation_id = "get_admin_storage_registry_status",
    summary = "Registry status",
    tag = "Storage",
    responses(
        (status = 200, description = "Success", body = RegistryStatus, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn registry_status(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<RegistryStatus>, ApiError> {
    require_admin(&state, &headers, &jar, SCOPE_READ).await?;
    let status = status(&state, &Domain::find(&state, REGISTRY).await?).await?;
    Ok(Json(RegistryStatus {
        active_target_id: status.active_target_id,
        repository_count: status.usage.repository_count,
        usage: registry_usage(status.usage.usage),
    }))
}

/// Deprecated alias of `GET /admin/storage/domains/registry/repositories`.
#[utoipa::path(
    get, path = "/admin/storage/registry/repositories",
    operation_id = "get_admin_storage_registry_repositories",
    summary = "Registry repositories",
    tag = "Storage",
    params(
        RepositoryUsageQuery,
    ),
    responses(
        (status = 200, description = "Success", body = RepositoryUsagePage<RegistryRepositoryUsageRow>, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn registry_repositories(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Query(query): Query<RepositoryUsageQuery>,
) -> Result<Json<RepositoryUsagePage<RegistryRepositoryUsageRow>>, ApiError> {
    require_admin(&state, &headers, &jar, SCOPE_READ).await?;
    let domain = Domain::find(&state, REGISTRY).await?;
    let page = repositories(&state, &domain, &query).await?;
    let rows = page
        .repositories
        .into_iter()
        .map(|row: RepositoryUsageRow| RegistryRepositoryUsageRow {
            repository_id: row.repository_id,
            repository_name: row.repository_name,
            owner_name: row.owner_name,
            owner_type: row.owner_type,
            usage: registry_usage(row.usage),
        })
        .collect();
    Ok(Json(RepositoryUsagePage {
        repositories: rows,
        total: page.total,
        limit: page.limit,
        offset: page.offset,
    }))
}

/// Deprecated alias of `POST /admin/storage/domains/registry/migrate`.
#[utoipa::path(
    post, path = "/admin/storage/registry/migrate",
    operation_id = "post_admin_storage_registry_migrate",
    summary = "Registry migrate",
    tag = "Storage",

    responses(
        (status = 200, description = "Success", body = MigrationStarted, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
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
#[utoipa::path(
    get, path = "/admin/storage/registry/migrations/{operation_id}/events",
    operation_id = "get_admin_storage_registry_migrations_operation_id_events",
    summary = "Registry events",
    tag = "Storage",
    params(
        ("operation_id" = Uuid, Path),
    ),
    responses(
        (status = 200, description = "Success", body = String, content_type = "text/event-stream"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
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
#[cfg(test)]
pub(super) fn flatten_details(value: &mut serde_json::Value) {
    let Some(object) = value.as_object_mut() else {
        return;
    };
    if let Some(serde_json::Value::Object(details)) = object.remove("details") {
        object.extend(details);
    }
}

#[cfg(test)]
mod contract_tests {
    use super::*;

    #[test]
    fn typed_registry_counts_preserve_the_legacy_flattened_wire_shape() {
        let usage = crate::blob_store::RepositoryUsage {
            object_count: 11,
            total_bytes: 2048,
            details: [
                ("blob_count", 1),
                ("manifest_count", 2),
                ("tag_count", 3),
                ("image_count", 4),
                ("upload_count", 5),
                ("upload_bytes", 512),
            ]
            .into(),
        };
        let mut old = serde_json::to_value(&usage).unwrap();
        flatten_details(&mut old);
        assert_eq!(serde_json::to_value(registry_usage(usage)).unwrap(), old);
    }
}
