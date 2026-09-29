//! Administrator API shared by every storage domain: status, per-repository
//! usage, and online migration between storage targets.
//!
//! Each domain contributes a [`DomainStorage`] and a [`DomainUsage`]
//! provider; everything else, including filtering and pagination, is
//! implemented once here.

use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    convert::Infallible,
    sync::Arc,
    time::Duration,
};

use axum::{
    Json,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::sse::{Event, Sse},
};
use axum_extra::extract::cookie::CookieJar;
use chrono::{DateTime, Utc};
use futures_util::{Stream, stream};
use sea_orm::{
    ColumnTrait as _, DatabaseConnection, EntityTrait as _, QueryFilter as _, QueryOrder as _,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{ApiError, IdentityState, SCOPE_READ, SCOPE_WRITE, require_admin};
use crate::{
    blob_store::{
        DomainStorage, DomainUsage, RepositoryUsage, UsageContext, UsageDetail,
        manager::{self, state},
        targets,
    },
    entity::{namespace, organization, repository, storage_migration},
    registry::usage::RegistryUsageProvider,
    storage::LfsUsage,
};

pub(super) mod legacy;
#[cfg(test)]
mod tests;

const DEFAULT_PAGE_SIZE: u64 = 10;
const MAX_PAGE_SIZE: u64 = 100;
const DEFAULT_BATCH_SIZE: usize = 100;
const PROGRESS_INTERVAL: Duration = Duration::from_millis(500);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DomainKind {
    Lfs,
    Registry,
}

/// A storage domain administrators can inspect and migrate.
pub(crate) struct DomainEntry {
    kind: DomainKind,
    /// Matches [`StorageDomain::name`](crate::blob_store::StorageDomain::name).
    name: &'static str,
    label: &'static str,
    local_label: &'static str,
    /// Kept per domain so audit history stays continuous with the actions
    /// recorded before the shared API existed.
    audit_action: &'static str,
    usage: &'static dyn DomainUsage,
}

static DOMAINS: [DomainEntry; 2] = [
    DomainEntry {
        kind: DomainKind::Lfs,
        name: "lfs",
        label: "Git LFS",
        local_label: "Configured local storage",
        audit_action: "storage.migration.start",
        usage: &LfsUsage,
    },
    DomainEntry {
        kind: DomainKind::Registry,
        name: "registry",
        label: "Container registry",
        local_label: "Repository-backed local storage",
        audit_action: "registry.migration.start",
        usage: &RegistryUsageProvider,
    },
];

/// The catalog entry for `name`; unknown domains are not found.
fn entry(name: &str) -> Result<&'static DomainEntry, ApiError> {
    DOMAINS
        .iter()
        .find(|entry| entry.name == name)
        .ok_or_else(ApiError::not_found)
}

/// A domain together with its live storage.
struct Domain {
    entry: &'static DomainEntry,
    storage: Arc<DomainStorage>,
}

impl Domain {
    async fn open(state: &IdentityState, entry: &'static DomainEntry) -> Result<Self, ApiError> {
        let storage = match entry.kind {
            DomainKind::Lfs => state.lfs_storage().await,
            DomainKind::Registry => state.registry_storage().await,
        }
        .ok_or_else(|| ApiError::internal(format!("{} storage is unavailable", entry.label)))?;
        Ok(Self { entry, storage })
    }

    async fn find(state: &IdentityState, name: &str) -> Result<Self, ApiError> {
        Self::open(state, entry(name)?).await
    }

    async fn usage(
        &self,
        state: &IdentityState,
    ) -> Result<HashMap<Uuid, RepositoryUsage>, ApiError> {
        let settings = state.runtime_settings()?;
        self.entry
            .usage
            .by_repository(UsageContext {
                database: state.database(),
                settings: &settings.storage,
                storage: &self.storage,
            })
            .await
            .map_err(ApiError::internal)
    }
}

#[derive(Debug, Serialize)]
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

#[derive(Debug, Serialize)]
pub struct UsageTotals {
    repository_count: u64,
    #[serde(flatten)]
    usage: RepositoryUsage,
}

impl UsageTotals {
    /// Sums `usages`, reporting every counter in `fields` even when it is zero.
    fn sum<'a>(
        fields: &[UsageDetail],
        usages: impl IntoIterator<Item = &'a RepositoryUsage>,
    ) -> Self {
        let mut totals = Self {
            repository_count: 0,
            usage: RepositoryUsage {
                details: fields.iter().map(|field| (field.key, 0)).collect(),
                ..RepositoryUsage::default()
            },
        };
        for usage in usages {
            if usage.object_count > 0 {
                totals.repository_count += 1;
            }
            totals.usage += usage;
        }
        totals
    }
}

async fn status(state: &IdentityState, domain: &Domain) -> Result<DomainStatus, ApiError> {
    let usage = domain.usage(state).await?;
    let active_target_id = domain.storage.target_id();
    let active_target_name = match active_target_id {
        Some(id) => Some(
            targets::find(state.database(), id)
                .await
                .map_err(ApiError::internal)?
                .0
                .name,
        ),
        None => None,
    };
    let (active_migration, last_migration) =
        latest_migrations(state.database(), domain.entry).await?;
    Ok(DomainStatus {
        name: domain.entry.name,
        label: domain.entry.label,
        active_target_id,
        active_target_name,
        local_label: domain.entry.local_label,
        local_root: domain.storage.local_root().display().to_string(),
        usage: UsageTotals::sum(domain.entry.usage.details(), usage.values()),
        detail_fields: domain.entry.usage.details(),
        active_migration,
        last_migration,
    })
}

/// The unfinished migration, if any, and the most recent finished one.
async fn latest_migrations(
    database: &DatabaseConnection,
    entry: &'static DomainEntry,
) -> Result<(Option<MigrationView>, Option<MigrationView>), ApiError> {
    let recent = || {
        storage_migration::Entity::find()
            .filter(storage_migration::Column::Domain.eq(entry.name))
            .order_by_desc(storage_migration::Column::StartedAt)
    };
    let terminal = [state::COMPLETED, state::FAILED];
    let active = recent()
        .filter(storage_migration::Column::State.is_not_in(terminal))
        .one(database)
        .await?;
    let last = recent()
        .filter(storage_migration::Column::State.is_in(terminal))
        .one(database)
        .await?;
    Ok((
        active.map(|row| MigrationView::new(entry, row)),
        last.map(|row| MigrationView::new(entry, row)),
    ))
}

pub async fn list_domains(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<Vec<DomainStatus>>, ApiError> {
    require_admin(&state, &headers, &jar, SCOPE_READ).await?;
    let mut statuses = Vec::with_capacity(DOMAINS.len());
    for entry in &DOMAINS {
        statuses.push(status(&state, &Domain::open(&state, entry).await?).await?);
    }
    Ok(Json(statuses))
}

pub async fn domain_status(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(domain): Path<String>,
) -> Result<Json<DomainStatus>, ApiError> {
    require_admin(&state, &headers, &jar, SCOPE_READ).await?;
    let domain = Domain::find(&state, &domain).await?;
    Ok(Json(status(&state, &domain).await?))
}

#[derive(Debug, Default, Deserialize)]
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum UsageSort {
    BytesDesc,
    BytesAsc,
    Name,
}

/// A validated [`RepositoryUsageQuery`] with normalized search terms.
#[derive(Debug)]
struct UsageFilter {
    search: String,
    owner: String,
    owner_type: Option<&'static str>,
    min_bytes: Option<u64>,
    max_bytes: Option<u64>,
    sort: UsageSort,
    limit: u64,
    offset: u64,
}

impl TryFrom<&RepositoryUsageQuery> for UsageFilter {
    type Error = ApiError;

    fn try_from(query: &RepositoryUsageQuery) -> Result<Self, ApiError> {
        let limit = query.limit.unwrap_or(DEFAULT_PAGE_SIZE);
        if !(1..=MAX_PAGE_SIZE).contains(&limit) {
            return Err(ApiError::bad_request(format!(
                "The storage usage page size must be between 1 and {MAX_PAGE_SIZE}."
            )));
        }
        if query
            .min_bytes
            .zip(query.max_bytes)
            .is_some_and(|(minimum, maximum)| minimum > maximum)
        {
            return Err(ApiError::bad_request(
                "The minimum storage usage cannot exceed the maximum.",
            ));
        }
        let owner_type = match query.owner_type.as_deref().unwrap_or("") {
            "" => None,
            "user" => Some("user"),
            "organization" => Some("organization"),
            _ => {
                return Err(ApiError::bad_request(
                    "The owner type must be user or organization.",
                ));
            }
        };
        let sort = match query.sort.as_deref().unwrap_or("bytes_desc") {
            "bytes_desc" => UsageSort::BytesDesc,
            "bytes_asc" => UsageSort::BytesAsc,
            "name" => UsageSort::Name,
            _ => {
                return Err(ApiError::bad_request(
                    "The storage usage sort must be bytes_desc, bytes_asc, or name.",
                ));
            }
        };
        let normalize =
            |value: &Option<String>| value.as_deref().unwrap_or("").trim().to_lowercase();
        Ok(Self {
            search: normalize(&query.search),
            owner: normalize(&query.owner),
            owner_type,
            min_bytes: query.min_bytes,
            max_bytes: query.max_bytes,
            sort,
            limit,
            offset: query.offset.unwrap_or(0),
        })
    }
}

#[derive(Debug, Serialize)]
pub struct RepositoryUsageRow {
    repository_id: Uuid,
    repository_name: String,
    owner_name: String,
    owner_type: String,
    #[serde(flatten)]
    usage: RepositoryUsage,
}

#[derive(Debug, Serialize)]
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
) -> Result<RepositoryUsagePage, ApiError> {
    let filter = UsageFilter::try_from(query)?;
    let mut usage = domain.usage(state).await?;
    usage.retain(|_, usage| usage.object_count > 0);
    let repositories = repository::Entity::find()
        .filter(repository::Column::DeletedAt.is_null())
        .filter(repository::Column::Id.is_in(usage.keys().copied()))
        .all(state.database())
        .await?;
    let owners: HashSet<_> = repositories
        .iter()
        .map(|repository| repository.namespace.clone())
        .collect();
    let namespaces: HashMap<_, _> = namespace::Entity::find()
        .filter(namespace::Column::Slug.is_in(owners))
        .all(state.database())
        .await?
        .into_iter()
        .map(|owner| (owner.slug.clone(), owner))
        .collect();
    let organization_names: HashMap<_, _> = organization::Entity::find()
        .filter(
            organization::Column::Id.is_in(
                namespaces
                    .values()
                    .filter_map(|owner| owner.organization_id),
            ),
        )
        .all(state.database())
        .await?
        .into_iter()
        .map(|organization| (organization.id, organization.display_name.to_lowercase()))
        .collect();

    let mut rows = Vec::new();
    for repository in repositories {
        let Some(usage) = usage.remove(&repository.id) else {
            continue;
        };
        let owner = namespaces
            .get(&repository.namespace)
            .ok_or_else(|| ApiError::internal("Repository namespace is missing."))?;
        let organization_name = owner
            .organization_id
            .and_then(|id| organization_names.get(&id))
            .map(String::as_str);
        if filter.matches(&repository, owner, organization_name, &usage) {
            rows.push(RepositoryUsageRow {
                repository_id: repository.id,
                repository_name: repository.name,
                owner_name: repository.namespace,
                owner_type: owner.kind.clone(),
                usage,
            });
        }
    }
    Ok(filter.page(rows))
}

impl UsageFilter {
    fn matches(
        &self,
        repository: &repository::Model,
        owner: &namespace::Model,
        organization_name: Option<&str>,
        usage: &RepositoryUsage,
    ) -> bool {
        let owner_matches = self.owner.is_empty()
            || owner.slug.to_lowercase().contains(&self.owner)
            || organization_name.is_some_and(|name| name.contains(&self.owner));
        (self.search.is_empty() || repository.name.to_lowercase().contains(&self.search))
            && owner_matches
            && self.owner_type.is_none_or(|kind| owner.kind == kind)
            && self
                .min_bytes
                .is_none_or(|minimum| usage.total_bytes >= minimum)
            && self
                .max_bytes
                .is_none_or(|maximum| usage.total_bytes <= maximum)
    }

    fn page(&self, mut rows: Vec<RepositoryUsageRow>) -> RepositoryUsagePage {
        rows.sort_by(|left, right| {
            let bytes = match self.sort {
                UsageSort::BytesDesc => right.usage.total_bytes.cmp(&left.usage.total_bytes),
                UsageSort::BytesAsc => left.usage.total_bytes.cmp(&right.usage.total_bytes),
                UsageSort::Name => std::cmp::Ordering::Equal,
            };
            bytes
                .then_with(|| left.repository_name.cmp(&right.repository_name))
                .then_with(|| left.owner_name.cmp(&right.owner_name))
                .then_with(|| left.repository_id.cmp(&right.repository_id))
        });
        let total = rows.len() as u64;
        let repositories = rows
            .into_iter()
            .skip(usize::try_from(self.offset).unwrap_or(usize::MAX))
            .take(usize::try_from(self.limit).unwrap_or(usize::MAX))
            .collect();
        RepositoryUsagePage {
            repositories,
            total,
            limit: self.limit,
            offset: self.offset,
        }
    }
}

pub async fn domain_repositories(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(domain): Path<String>,
    Query(query): Query<RepositoryUsageQuery>,
) -> Result<Json<RepositoryUsagePage>, ApiError> {
    require_admin(&state, &headers, &jar, SCOPE_READ).await?;
    let domain = Domain::find(&state, &domain).await?;
    Ok(Json(repositories(&state, &domain, &query).await?))
}

/// Selects the destination of a migration. `local` selects the domain's
/// local root; so does the nil `target_id`, which older clients send.
#[derive(Debug, Deserialize)]
pub struct MigrationRequest {
    target_id: Option<Uuid>,
    #[serde(default)]
    local: bool,
    #[serde(default = "default_batch_size")]
    batch_size: usize,
}

fn default_batch_size() -> usize {
    DEFAULT_BATCH_SIZE
}

impl MigrationRequest {
    /// The destination target id, nil for the local root.
    fn destination(&self) -> Result<Uuid, ApiError> {
        match (self.target_id, self.local) {
            (Some(target_id), false) => Ok(target_id),
            (None, true) => Ok(Uuid::nil()),
            (Some(target_id), true) if target_id.is_nil() => Ok(target_id),
            (Some(_), true) => Err(ApiError::bad_request(
                "Choose either a storage target or local storage, not both.",
            )),
            (None, false) => Err(ApiError::bad_request(
                "Choose a storage target or local storage.",
            )),
        }
    }
}

#[derive(Debug, Serialize)]
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
    actor_id: Uuid,
    request: &MigrationRequest,
) -> Result<MigrationStarted, ApiError> {
    let label = domain.entry.label;
    let target_id = request.destination()?;
    if request.batch_size == 0 {
        return Err(ApiError::bad_request(
            "The migration batch size must be greater than zero.",
        ));
    }
    if !target_id.is_nil() {
        targets::find(state.database(), target_id)
            .await
            .map_err(|_| ApiError::bad_request("The storage target does not exist."))?;
    }
    let migration_guard = domain.storage.try_lock_migration().ok_or_else(|| {
        ApiError::conflict(format!(
            "Another {label} storage migration is already in progress."
        ))
    })?;
    let operation_id = Uuid::new_v4();
    domain
        .storage
        .reserve_migration(state.database(), target_id, operation_id)
        .await
        .map_err(|error| ApiError::bad_request(format!("Could not start migration: {error:#}")))?;
    if let Err(error) = state
        .audit(
            Some(actor_id),
            domain.entry.audit_action,
            Some(target_id.to_string()),
        )
        .await
    {
        manager::mark_failed(state.database(), operation_id, &anyhow::anyhow!("{error}")).await;
        return Err(error);
    }
    let storage = domain.storage.clone();
    let database = state.database().clone();
    let batch_size = request.batch_size;
    tokio::spawn(async move {
        if let Err(error) = storage
            .migrate_online(&database, operation_id, batch_size, migration_guard)
            .await
        {
            tracing::error!(%error, %operation_id, domain = label, "online storage migration failed");
        }
    });
    Ok(MigrationStarted {
        operation_id,
        domain: domain.entry.name,
        target_id,
        local: target_id.is_nil(),
        message: format!("{label} storage migration started without restarting Gitadel."),
    })
}

pub async fn migrate_domain(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(domain): Path<String>,
    Json(request): Json<MigrationRequest>,
) -> Result<(StatusCode, Json<MigrationStarted>), ApiError> {
    let actor = require_admin(&state, &headers, &jar, SCOPE_WRITE).await?;
    let domain = Domain::find(&state, &domain).await?;
    let started = start_migration(&state, &domain, actor.user.id, &request).await?;
    Ok((StatusCode::ACCEPTED, Json(started)))
}

/// One migration's progress, as returned by the progress endpoint and sent by
/// its event stream.
#[derive(Debug, Serialize)]
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

impl MigrationView {
    fn new(entry: &'static DomainEntry, row: storage_migration::Model) -> Self {
        let phase = manager::progress_phase(&row);
        let message = progress_message(entry.label, &phase, row.error.as_deref());
        Self {
            operation_id: row.id,
            domain: entry.name,
            operation: format!("{}_migrate", entry.name),
            source_target_id: row.source_target_id,
            target_id: row.target_id,
            state: row.state,
            phase,
            message,
            key: row.last_key.unwrap_or_default(),
            copied_objects: u64::try_from(row.copied_objects).unwrap_or(0),
            processed_bytes: u64::try_from(row.copied_bytes).unwrap_or(0),
            total_bytes: row.total_bytes.and_then(|bytes| u64::try_from(bytes).ok()),
            error: row.error,
            started_at: row.started_at,
            updated_at: row.updated_at,
            completed_at: row.completed_at,
        }
    }

    fn is_terminal(&self) -> bool {
        matches!(self.state.as_str(), state::COMPLETED | state::FAILED)
    }
}

fn progress_message(label: &str, phase: &str, error: Option<&str>) -> String {
    match phase {
        "scheduled" => format!("{label} storage migration is starting."),
        "checking_destination" => "Checking the destination storage target.".to_owned(),
        "writing_metadata" => format!("Selecting the verified {label} storage target."),
        "completed" => format!("{label} storage migration completed."),
        "failed" => error.map_or_else(
            || format!("{label} storage migration failed."),
            str::to_owned,
        ),
        _ => format!("Copying and verifying {label} objects."),
    }
}

async fn find_migration(
    database: &DatabaseConnection,
    entry: &'static DomainEntry,
    operation_id: Uuid,
) -> Result<MigrationView, ApiError> {
    storage_migration::Entity::find_by_id(operation_id)
        .one(database)
        .await?
        .filter(|migration| migration.domain == entry.name)
        .map(|migration| MigrationView::new(entry, migration))
        .ok_or_else(ApiError::not_found)
}

pub async fn domain_migration(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((domain, operation_id)): Path<(String, Uuid)>,
) -> Result<Json<MigrationView>, ApiError> {
    require_admin(&state, &headers, &jar, SCOPE_READ).await?;
    Ok(Json(
        find_migration(state.database(), entry(&domain)?, operation_id).await?,
    ))
}

pub async fn domain_migration_events(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((domain, operation_id)): Path<(String, Uuid)>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    require_admin(&state, &headers, &jar, SCOPE_READ).await?;
    migration_events(&state, entry(&domain)?, operation_id).await
}

/// Streams a migration's progress until it completes or fails.
async fn migration_events(
    state: &IdentityState,
    entry: &'static DomainEntry,
    operation_id: Uuid,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>> + use<>>, ApiError> {
    find_migration(state.database(), entry, operation_id).await?;
    let events = stream::unfold(
        (state.database().clone(), false),
        move |(database, done)| async move {
            if done {
                return None;
            }
            let (payload, terminal) = match find_migration(&database, entry, operation_id).await {
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
            Some((Ok(event), (database, terminal)))
        },
    );
    Ok(Sse::new(events))
}
