//! Administrator API shared by every storage domain: status, per-repository
//! usage, and online migration between storage targets.
//!
//! Each domain contributes a [`DomainStorage`] and a [`DomainUsage`]
//! provider; everything else, including filtering and pagination, is
//! implemented once here.

use std::{borrow::Cow, convert::Infallible, time::Duration};

use axum::{
    Json,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::sse::{Event, Sse},
};
use axum_extra::extract::cookie::CookieJar;
use chrono::{DateTime, Utc};
use futures_util::{Stream, stream};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{ApiError, IdentityState, SCOPE_READ, SCOPE_WRITE, require_admin_principal};
use crate::blob_store::{RepositoryUsage, UsageDetail, manager::state};

pub(crate) mod legacy;
pub(crate) mod overview;
#[cfg(test)]
mod tests;

const DEFAULT_BATCH_SIZE: usize = 100;
const PROGRESS_INTERVAL: Duration = Duration::from_millis(500);

/// The catalog entry for `name`; unknown domains are not found.
fn entry(name: &str) -> Result<&'static DomainEntry, ApiError> {
    Ok(storage_domains::entry(name)?)
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct DomainStatus {
    name: &'static str,
    label: &'static str,
    /// `None` while the domain uses its local root.
    active_target_id: Option<Uuid>,
    active_target_name: Option<String>,
    local_label: &'static str,
    local_root: String,
    usage: UsageTotals,
    detail_fields: &'static [UsageDetail],
    active_migration: Option<MigrationView>,
    last_migration: Option<MigrationView>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct UsageTotals {
    repository_count: u64,
    #[serde(flatten)]
    usage: RepositoryUsage,
}

async fn status(
    state: &IdentityState,
    domain: &Domain,
    actor: &gitadel_identity::Principal,
) -> Result<DomainStatus, ApiError> {
    Ok(
        storage_domains::status(&state.storage_service(), &domain.0, actor)
            .await?
            .into(),
    )
}

#[utoipa::path(
    get, path = "/admin/storage/domains",
    operation_id = "get_admin_storage_domains",
    summary = "List domains",
    tag = "Storage",
    responses(
        (status = 200, description = "Success", body = Vec<DomainStatus>, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn list_domains(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<Vec<DomainStatus>>, ApiError> {
    let actor = require_admin_principal(&state, &headers, &jar, SCOPE_READ).await?;
    let mut statuses = Vec::with_capacity(DOMAINS.len());
    for entry in &DOMAINS {
        statuses.push(status(&state, &Domain::open(&state, entry).await?, &actor).await?);
    }
    Ok(Json(statuses))
}

#[utoipa::path(
    get, path = "/admin/storage/domains/{domain}",
    operation_id = "get_admin_storage_domains_domain",
    summary = "Domain status",
    tag = "Storage",
    params(
        ("domain" = String, Path),
    ),
    responses(
        (status = 200, description = "Success", body = DomainStatus, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn domain_status(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(domain): Path<String>,
) -> Result<Json<DomainStatus>, ApiError> {
    let actor = require_admin_principal(&state, &headers, &jar, SCOPE_READ).await?;
    let domain = Domain::find(&state, &domain).await?;
    Ok(Json(status(&state, &domain, &actor).await?))
}

#[derive(Debug, Default, Deserialize, utoipa::ToSchema, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct RepositoryUsageQuery {
    pub search: Option<String>,
    pub owner: Option<String>,
    pub owner_type: Option<String>,
    pub min_bytes: Option<u64>,
    pub max_bytes: Option<u64>,
    pub sort: Option<String>,
    pub limit: Option<u64>,
    pub offset: Option<u64>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct RepositoryUsageRow {
    repository_id: Uuid,
    repository_name: String,
    owner_name: String,
    owner_type: String,
    #[serde(flatten)]
    usage: RepositoryUsage,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct RepositoryUsagePage<Row = RepositoryUsageRow> {
    repositories: Vec<Row>,
    total: u64,
    limit: u64,
    offset: u64,
}

async fn repositories(
    state: &IdentityState,
    domain: &Domain,
    query: &RepositoryUsageQuery,
    actor: &gitadel_identity::Principal,
) -> Result<RepositoryUsagePage, ApiError> {
    Ok(
        storage_domains::repositories(&state.storage_service(), &domain.0, &query.into(), actor)
            .await?
            .into(),
    )
}

#[utoipa::path(
    get, path = "/admin/storage/domains/{domain}/repositories",
    operation_id = "get_admin_storage_domains_domain_repositories",
    summary = "Domain repositories",
    tag = "Storage",
    params(
        ("domain" = String, Path),
        RepositoryUsageQuery,
    ),
    responses(
        (status = 200, description = "Success", body = RepositoryUsagePage, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn domain_repositories(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(domain): Path<String>,
    Query(query): Query<RepositoryUsageQuery>,
) -> Result<Json<RepositoryUsagePage>, ApiError> {
    let actor = require_admin_principal(&state, &headers, &jar, SCOPE_READ).await?;
    let domain = Domain::find(&state, &domain).await?;
    Ok(Json(repositories(&state, &domain, &query, &actor).await?))
}

/// Selects the destination of a migration. `local` selects the domain's
/// local root; so does the nil `target_id`, which older clients send.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct MigrationRequest {
    target_id: Option<Uuid>,
    #[serde(default)]
    local: bool,
    #[serde(default = "default_batch_size")]
    batch_size: usize,
}

#[cfg(test)]
impl MigrationRequest {
    fn destination(&self) -> Result<Uuid, ApiError> {
        Ok(storage_domains::MigrationRequest::from(self).destination()?)
    }
}

fn default_batch_size() -> usize {
    DEFAULT_BATCH_SIZE
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct MigrationStarted {
    operation_id: Uuid,
    domain: &'static str,
    /// Nil when the destination is the local root.
    target_id: Uuid,
    local: bool,
    message: String,
}

async fn start_migration(
    state: &IdentityState,
    domain: &Domain,
    actor: &gitadel_identity::Principal,
    request: &MigrationRequest,
) -> Result<MigrationStarted, ApiError> {
    Ok(storage_domains::start_migration(
        &state.storage_service(),
        &domain.0,
        actor,
        &request.into(),
    )
    .await?
    .into())
}

#[utoipa::path(
    post, path = "/admin/storage/domains/{domain}/migrate",
    operation_id = "post_admin_storage_domains_domain_migrate",
    summary = "Migrate domain",
    tag = "Storage",

    params(
        ("domain" = String, Path),
    ),
    responses(
        (status = 202, description = "Success", body = MigrationStarted, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn migrate_domain(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(domain): Path<String>,
    Json(request): Json<MigrationRequest>,
) -> Result<(StatusCode, Json<MigrationStarted>), ApiError> {
    let actor = require_admin_principal(&state, &headers, &jar, SCOPE_WRITE).await?;
    let domain = Domain::find(&state, &domain).await?;
    let started = start_migration(&state, &domain, &actor, &request).await?;
    Ok((StatusCode::ACCEPTED, Json(started)))
}

/// One migration's progress, as returned by the progress endpoint and sent by
/// its event stream.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct MigrationView {
    operation_id: Uuid,
    domain: &'static str,
    operation: String,
    source_target_id: Option<Uuid>,
    target_id: Option<Uuid>,
    state: String,
    phase: Cow<'static, str>,
    message: String,
    key: String,
    copied_objects: u64,
    processed_bytes: u64,
    total_bytes: Option<u64>,
    error: Option<String>,
    started_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    completed_at: Option<DateTime<Utc>>,
}

async fn find_migration(
    state: &IdentityState,
    entry: &'static DomainEntry,
    operation_id: Uuid,
    actor: &gitadel_identity::Principal,
) -> Result<MigrationView, ApiError> {
    Ok(
        storage_domains::migration(&state.storage_service(), actor, entry, operation_id)
            .await?
            .into(),
    )
}

#[utoipa::path(
    get, path = "/admin/storage/domains/{domain}/migrations/{operation_id}",
    operation_id = "get_admin_storage_domains_domain_migrations_operation_id",
    summary = "Domain migration",
    tag = "Storage",
    params(
        ("domain" = String, Path),
        ("operation_id" = Uuid, Path),
    ),
    responses(
        (status = 200, description = "Success", body = MigrationView, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn domain_migration(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((domain, operation_id)): Path<(String, Uuid)>,
) -> Result<Json<MigrationView>, ApiError> {
    let actor = require_admin_principal(&state, &headers, &jar, SCOPE_READ).await?;
    Ok(Json(
        find_migration(&state, entry(&domain)?, operation_id, &actor).await?,
    ))
}

#[utoipa::path(
    get, path = "/admin/storage/domains/{domain}/migrations/{operation_id}/events",
    operation_id = "get_admin_storage_domains_domain_migrations_operation_id_events",
    summary = "Domain migration events",
    tag = "Storage",
    params(
        ("domain" = String, Path),
        ("operation_id" = Uuid, Path),
    ),
    responses(
        (status = 200, description = "Success", body = String, content_type = "text/event-stream"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn domain_migration_events(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((domain, operation_id)): Path<(String, Uuid)>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let actor = require_admin_principal(&state, &headers, &jar, SCOPE_READ).await?;
    migration_events(&state, entry(&domain)?, operation_id, actor).await
}

/// Streams a migration's progress until it completes or fails.
async fn migration_events(
    state: &IdentityState,
    entry: &'static DomainEntry,
    operation_id: Uuid,
    actor: gitadel_identity::Principal,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>> + use<>>, ApiError> {
    find_migration(state, entry, operation_id, &actor).await?;
    let events = stream::unfold(
        (state.clone(), actor, false),
        move |(state_handle, actor, done)| async move {
            if done {
                return None;
            }
            let (payload, terminal) = match find_migration(
                &state_handle,
                entry,
                operation_id,
                &actor,
            )
            .await
            {
                Ok(view) => {
                    let terminal = view.is_terminal();
                    (serde_json::to_value(view).unwrap_or_default(), terminal)
                }
                Err(error) => {
                    tracing::error!(?error, %operation_id, "could not read storage migration progress");
                    (
                        serde_json::json!({
                            "operation_id": operation_id,
                            "domain": entry.name,
                            "operation": format!("{}_migrate", entry.name),
                            "state": state::FAILED,
                            "phase": "failed",
                            "message": format!("Could not read {} migration progress.", entry.label),
                            "key": "",
                            "processed_bytes": null,
                            "total_bytes": null,
                        }),
                        true,
                    )
                }
            };
            let event = Event::default().data(payload.to_string());
            if !terminal {
                tokio::time::sleep(PROGRESS_INTERVAL).await;
            }
            Some((Ok(event), (state_handle, actor, terminal)))
        },
    );
    Ok(Sse::new(events))
}

use gitadel_storage::domains as storage_domains;
use storage_domains::{DOMAINS, DomainEntry};
struct Domain(storage_domains::Domain);
impl std::ops::Deref for Domain {
    type Target = storage_domains::Domain;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl Domain {
    async fn open(state: &IdentityState, entry: &'static DomainEntry) -> Result<Self, ApiError> {
        Ok(Self(
            storage_domains::Domain::open(&state.storage_service(), entry).await?,
        ))
    }
    async fn find(state: &IdentityState, name: &str) -> Result<Self, ApiError> {
        Ok(Self(
            storage_domains::Domain::find(&state.storage_service(), name).await?,
        ))
    }
}
impl MigrationView {
    fn is_terminal(&self) -> bool {
        matches!(self.state.as_str(), state::COMPLETED | state::FAILED)
    }
}
impl From<storage_domains::DomainStatus> for DomainStatus {
    fn from(v: storage_domains::DomainStatus) -> Self {
        Self {
            name: v.name,
            label: v.label,
            active_target_id: v.active_target_id,
            active_target_name: v.active_target_name,
            local_label: v.local_label,
            local_root: v.local_root,
            usage: v.usage.into(),
            detail_fields: v.detail_fields,
            active_migration: v.active_migration.map(Into::into),
            last_migration: v.last_migration.map(Into::into),
        }
    }
}

impl From<storage_domains::UsageTotals> for UsageTotals {
    fn from(v: storage_domains::UsageTotals) -> Self {
        Self {
            repository_count: v.repository_count,
            usage: v.usage,
        }
    }
}
impl From<&RepositoryUsageQuery> for storage_domains::RepositoryUsageQuery {
    fn from(v: &RepositoryUsageQuery) -> Self {
        Self {
            search: v.search.clone(),
            owner: v.owner.clone(),
            owner_type: v.owner_type.clone(),
            min_bytes: v.min_bytes,
            max_bytes: v.max_bytes,
            sort: v.sort.clone(),
            limit: v.limit,
            offset: v.offset,
        }
    }
}
impl From<storage_domains::RepositoryUsageRow> for RepositoryUsageRow {
    fn from(v: storage_domains::RepositoryUsageRow) -> Self {
        Self {
            repository_id: v.repository_id,
            repository_name: v.repository_name,
            owner_name: v.owner_name,
            owner_type: v.owner_type,
            usage: v.usage,
        }
    }
}
impl From<storage_domains::RepositoryUsagePage> for RepositoryUsagePage {
    fn from(v: storage_domains::RepositoryUsagePage) -> Self {
        Self {
            repositories: v.repositories.into_iter().map(Into::into).collect(),
            total: v.total,
            limit: v.limit,
            offset: v.offset,
        }
    }
}
impl From<&MigrationRequest> for storage_domains::MigrationRequest {
    fn from(v: &MigrationRequest) -> Self {
        Self {
            target_id: v.target_id,
            local: v.local,
            batch_size: v.batch_size,
        }
    }
}
impl From<storage_domains::MigrationStarted> for MigrationStarted {
    fn from(v: storage_domains::MigrationStarted) -> Self {
        Self {
            operation_id: v.operation_id,
            domain: v.domain,
            target_id: v.target_id,
            local: v.local,
            message: v.message,
        }
    }
}
impl From<storage_domains::MigrationView> for MigrationView {
    fn from(v: storage_domains::MigrationView) -> Self {
        Self {
            operation_id: v.operation_id,
            domain: v.domain,
            operation: v.operation,
            source_target_id: v.source_target_id,
            target_id: v.target_id,
            state: v.state,
            phase: v.phase,
            message: v.message,
            key: v.key,
            copied_objects: v.copied_objects,
            processed_bytes: v.processed_bytes,
            total_bytes: v.total_bytes,
            error: v.error,
            started_at: v.started_at,
            updated_at: v.updated_at,
            completed_at: v.completed_at,
        }
    }
}
