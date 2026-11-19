//! Shared storage-domain status, usage filtering and migration operations.
use crate::{
    blob_store::{
        DomainStorage, DomainUsage, RepositoryUsage, UsageContext, UsageDetail,
        manager::{self, state},
        targets,
    },
    entity::{namespace, organization, repository, storage_migration},
    lfs::LfsUsage,
    service::StorageService,
};
use chrono::{DateTime, Utc};
use gitadel_identity::IdentityError as ServiceError;
use sea_orm::{
    ColumnTrait as _, DatabaseConnection, EntityTrait as _, QueryFilter as _, QueryOrder as _,
};
use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    sync::Arc,
};
use uuid::Uuid;
const DEFAULT_PAGE_SIZE: u64 = 10;
const MAX_PAGE_SIZE: u64 = 100;
const DEFAULT_BATCH_SIZE: usize = 100;
pub struct DomainStatus {
    pub name: &'static str,
    pub label: &'static str,
    /// `None` while the domain uses its local root.
    pub active_target_id: Option<Uuid>,
    pub active_target_name: Option<String>,
    pub local_label: &'static str,
    pub local_root: String,
    pub usage: UsageTotals,
    pub detail_fields: &'static [UsageDetail],
    pub active_migration: Option<MigrationView>,
    pub last_migration: Option<MigrationView>,
}
pub struct UsageTotals {
    pub repository_count: u64,
    pub usage: RepositoryUsage,
}
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
pub struct RepositoryUsageRow {
    pub repository_id: Uuid,
    pub repository_name: String,
    pub owner_name: String,
    pub owner_type: String,
    pub usage: RepositoryUsage,
}
pub struct RepositoryUsagePage<Row = RepositoryUsageRow> {
    pub repositories: Vec<Row>,
    pub total: u64,
    pub limit: u64,
    pub offset: u64,
}
pub struct MigrationRequest {
    pub target_id: Option<Uuid>,
    pub local: bool,
    pub batch_size: usize,
}
pub struct MigrationStarted {
    pub operation_id: Uuid,
    pub domain: &'static str,
    /// Nil when the destination is the local root.
    pub target_id: Uuid,
    pub local: bool,
    pub message: String,
}
pub struct MigrationView {
    pub operation_id: Uuid,
    pub domain: &'static str,
    pub operation: String,
    pub source_target_id: Option<Uuid>,
    pub target_id: Option<Uuid>,
    pub state: String,
    pub phase: Cow<'static, str>,
    pub message: String,
    pub key: String,
    pub copied_objects: u64,
    pub processed_bytes: u64,
    pub total_bytes: Option<u64>,
    pub error: Option<String>,
    pub started_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DomainKind {
    Lfs,
    Registry,
}
pub struct DomainEntry {
    kind: DomainKind,
    /// Matches [`StorageDomain::name`](crate::blob_store::StorageDomain::name).
    pub name: &'static str,
    pub label: &'static str,
    local_label: &'static str,
    /// Kept per domain so audit history stays continuous with the actions
    /// recorded before the shared API existed.
    audit_action: &'static str,
}
pub struct Domain {
    pub entry: &'static DomainEntry,
    pub storage: Arc<DomainStorage>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UsageSort {
    BytesDesc,
    BytesAsc,
    Name,
}
pub struct UsageFilter {
    search: String,
    owner: String,
    owner_type: Option<&'static str>,
    min_bytes: Option<u64>,
    max_bytes: Option<u64>,
    sort: UsageSort,
    limit: u64,
    offset: u64,
}
pub static DOMAINS: [DomainEntry; 2] = [
    DomainEntry {
        kind: DomainKind::Lfs,
        name: "lfs",
        label: "Git LFS",
        local_label: "Configured local storage",
        audit_action: "storage.migration.start",
    },
    DomainEntry {
        kind: DomainKind::Registry,
        name: "registry",
        label: "Container registry",
        local_label: "Configured local storage",
        audit_action: "registry.migration.start",
    },
];
impl DomainEntry {
    fn usage<'a>(&self, state: &'a StorageService<'_>) -> &'a dyn DomainUsage {
        match self.kind {
            DomainKind::Lfs => &LfsUsage,
            DomainKind::Registry => state.registry_usage(),
        }
    }
}
impl Domain {
    pub async fn open(
        state: &StorageService<'_>,
        entry: &'static DomainEntry,
    ) -> Result<Self, ServiceError> {
        let storage = match entry.kind {
            DomainKind::Lfs => state.lfs_storage().await,
            DomainKind::Registry => state.registry_storage().await,
        }
        .ok_or_else(|| ServiceError::internal(format!("{} storage is unavailable", entry.label)))?;
        Ok(Self { entry, storage })
    }

    pub async fn find(state: &StorageService<'_>, name: &str) -> Result<Self, ServiceError> {
        Self::open(state, entry(name)?).await
    }

    pub async fn usage(
        &self,
        state: &StorageService<'_>,
    ) -> Result<HashMap<Uuid, RepositoryUsage>, ServiceError> {
        let settings = state.runtime_settings()?;
        self.entry
            .usage(state)
            .by_repository(UsageContext {
                database: state.database(),
                settings: &settings.storage,
                storage: &self.storage,
            })
            .await
            .map_err(ServiceError::internal)
    }
}
impl UsageTotals {
    /// Sums `usages`, reporting every counter in `fields` even when it is zero.
    pub fn sum<'a>(
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
impl UsageFilter {
    pub fn matches(
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

    pub fn page(&self, mut rows: Vec<RepositoryUsageRow>) -> RepositoryUsagePage {
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
impl TryFrom<&RepositoryUsageQuery> for UsageFilter {
    type Error = ServiceError;

    fn try_from(query: &RepositoryUsageQuery) -> Result<Self, ServiceError> {
        let limit = query.limit.unwrap_or(DEFAULT_PAGE_SIZE);
        if !(1..=MAX_PAGE_SIZE).contains(&limit) {
            return Err(ServiceError::bad_request(format!(
                "The storage usage page size must be between 1 and {MAX_PAGE_SIZE}."
            )));
        }
        if query
            .min_bytes
            .zip(query.max_bytes)
            .is_some_and(|(minimum, maximum)| minimum > maximum)
        {
            return Err(ServiceError::bad_request(
                "The minimum storage usage cannot exceed the maximum.",
            ));
        }
        let owner_type = match query.owner_type.as_deref().unwrap_or("") {
            "" => None,
            "user" => Some("user"),
            "organization" => Some("organization"),
            _ => {
                return Err(ServiceError::bad_request(
                    "The owner type must be user or organization.",
                ));
            }
        };
        let sort = match query.sort.as_deref().unwrap_or("bytes_desc") {
            "bytes_desc" => UsageSort::BytesDesc,
            "bytes_asc" => UsageSort::BytesAsc,
            "name" => UsageSort::Name,
            _ => {
                return Err(ServiceError::bad_request(
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
impl MigrationRequest {
    /// The destination target id, nil for the local root.
    pub fn destination(&self) -> Result<Uuid, ServiceError> {
        match (self.target_id, self.local) {
            (Some(target_id), false) => Ok(target_id),
            (None, true) => Ok(Uuid::nil()),
            (Some(target_id), true) if target_id.is_nil() => Ok(target_id),
            (Some(_), true) => Err(ServiceError::bad_request(
                "Choose either a storage target or local storage, not both.",
            )),
            (None, false) => Err(ServiceError::bad_request(
                "Choose a storage target or local storage.",
            )),
        }
    }
}
impl MigrationView {
    pub fn new(entry: &'static DomainEntry, row: storage_migration::Model) -> Self {
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

    pub fn is_terminal(&self) -> bool {
        matches!(self.state.as_str(), state::COMPLETED | state::FAILED)
    }
}
pub fn entry(name: &str) -> Result<&'static DomainEntry, ServiceError> {
    DOMAINS
        .iter()
        .find(|entry| entry.name == name)
        .ok_or_else(ServiceError::not_found)
}
pub async fn status(
    state: &StorageService<'_>,
    domain: &Domain,
    actor: &gitadel_identity::Principal,
) -> Result<DomainStatus, ServiceError> {
    state
        .require_admin(actor, gitadel_identity::SCOPE_READ)
        .await?;
    let usage = domain.usage(state).await?;
    let active_target_id = domain.storage.target_id();
    let active_target_name = match active_target_id {
        Some(id) => Some(
            targets::find(state.database(), id)
                .await
                .map_err(ServiceError::internal)?
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
        usage: UsageTotals::sum(domain.entry.usage(state).details(), usage.values()),
        detail_fields: domain.entry.usage(state).details(),
        active_migration,
        last_migration,
    })
}
pub async fn latest_migrations(
    database: &DatabaseConnection,
    entry: &'static DomainEntry,
) -> Result<(Option<MigrationView>, Option<MigrationView>), ServiceError> {
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
pub async fn repositories(
    state: &StorageService<'_>,
    domain: &Domain,
    query: &RepositoryUsageQuery,
    actor: &gitadel_identity::Principal,
) -> Result<RepositoryUsagePage, ServiceError> {
    state
        .require_admin(actor, gitadel_identity::SCOPE_READ)
        .await?;
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
            .ok_or_else(|| ServiceError::internal("Repository namespace is missing."))?;
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
pub fn default_batch_size() -> usize {
    DEFAULT_BATCH_SIZE
}
pub async fn start_migration(
    state: &StorageService<'_>,
    domain: &Domain,
    actor: &gitadel_identity::Principal,
    request: &MigrationRequest,
) -> Result<MigrationStarted, ServiceError> {
    let actor_id = state
        .require_admin(actor, gitadel_identity::SCOPE_WRITE)
        .await?
        .user
        .id;
    let label = domain.entry.label;
    let target_id = request.destination()?;
    if request.batch_size == 0 {
        return Err(ServiceError::bad_request(
            "The migration batch size must be greater than zero.",
        ));
    }
    if !target_id.is_nil() {
        targets::find(state.database(), target_id)
            .await
            .map_err(|_| ServiceError::bad_request("The storage target does not exist."))?;
    }
    let migration_guard = domain.storage.try_lock_migration().ok_or_else(|| {
        ServiceError::conflict(format!(
            "Another {label} storage migration is already in progress."
        ))
    })?;
    let operation_id = Uuid::new_v4();
    domain
        .storage
        .reserve_migration(state.database(), target_id, operation_id)
        .await
        .map_err(|error| {
            ServiceError::bad_request(format!("Could not start migration: {error:#}"))
        })?;
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
pub fn progress_message(label: &str, phase: &str, error: Option<&str>) -> String {
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
pub(crate) async fn find_migration(
    database: &DatabaseConnection,
    entry: &'static DomainEntry,
    operation_id: Uuid,
) -> Result<MigrationView, ServiceError> {
    storage_migration::Entity::find_by_id(operation_id)
        .one(database)
        .await?
        .filter(|migration| migration.domain == entry.name)
        .map(|migration| MigrationView::new(entry, migration))
        .ok_or_else(ServiceError::not_found)
}

pub async fn migration(
    state: &StorageService<'_>,
    actor: &gitadel_identity::Principal,
    entry: &'static DomainEntry,
    id: Uuid,
) -> Result<MigrationView, ServiceError> {
    state
        .require_admin(actor, gitadel_identity::SCOPE_READ)
        .await?;
    find_migration(state.database(), entry, id).await
}
