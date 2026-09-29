//! One storage manager for every [`StorageDomain`]: target selection, online
//! and offline migration, recovery, inventory, and cleanup.

use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{Arc, RwLock},
};

use anyhow::{Context, Result, ensure};
use chrono::Utc;
use sea_orm::{
    ActiveModelTrait as _, ColumnTrait as _, DatabaseConnection, EntityTrait as _,
    QueryFilter as _, Set, TransactionTrait as _, sea_query::Expr,
};
use tokio::sync::{
    Mutex, OwnedMutexGuard, OwnedRwLockWriteGuard, RwLock as AsyncRwLock, RwLockReadGuard,
};
use uuid::Uuid;

use super::{BlobMetadata, BlobStore, ObjectPrefix, StorageDomain, targets};
use crate::entity::{repository, storage_domain_state, storage_migration};

/// Migration states. Every state but the terminal two holds the per-domain
/// active-migration index.
pub(crate) mod state {
    pub const PENDING: &str = "pending";
    pub const COPYING: &str = "copying";
    pub const VERIFYING: &str = "verifying";
    pub const CUTOVER: &str = "cutover";
    pub const COMPLETED: &str = "completed";
    pub const FAILED: &str = "failed";
}

/// Migration phases, shared by every domain.
pub(crate) mod phase {
    pub const SCHEDULED: &str = "scheduled";
    pub const CHECKING_DESTINATION: &str = "checking_destination";
    pub const COPYING: &str = "copying";
    pub const VERIFYING: &str = "verifying";
    pub const CUTOVER: &str = "cutover";
    pub const COMPLETED: &str = "completed";
    pub const FAILED: &str = "failed";
}

/// The store a domain reads and writes, and the target it came from. `None`
/// is the domain's local root.
#[derive(Clone)]
pub struct ActiveStore {
    pub store: Arc<dyn BlobStore>,
    pub target_id: Option<Uuid>,
}

/// The live store of one domain. It is swapped only after a migration has
/// copied and verified every object; writers hold [`Self::lock_operation`],
/// so the cutover write guard waits for them to drain.
pub struct DomainStorage {
    domain: Arc<dyn StorageDomain>,
    local_root: PathBuf,
    active: RwLock<ActiveStore>,
    operations: Arc<AsyncRwLock<()>>,
    migrations: Arc<Mutex<()>>,
}

impl DomainStorage {
    /// Opens the selected store without touching migration records, so an
    /// interrupted offline migration can still be resumed.
    pub async fn load(
        database: &DatabaseConnection,
        domain: Arc<dyn StorageDomain>,
        local_root: PathBuf,
    ) -> Result<Self> {
        let target_id = active_target_id(database, domain.name()).await?;
        let store = match target_id {
            Some(target_id) => open_target(database, target_id).await?,
            None => domain.open_local(local_root.clone()).await?,
        };
        Ok(Self {
            domain,
            local_root,
            active: RwLock::new(ActiveStore { store, target_id }),
            operations: Arc::new(AsyncRwLock::new(())),
            migrations: Arc::new(Mutex::new(())),
        })
    }

    /// Server startup: online migrations cannot survive a restart, so any
    /// that were running are marked failed before the store is opened.
    pub async fn start(
        database: &DatabaseConnection,
        domain: Arc<dyn StorageDomain>,
        local_root: PathBuf,
    ) -> Result<Arc<Self>> {
        recover_interrupted(database, domain.as_ref()).await?;
        Ok(Arc::new(Self::load(database, domain, local_root).await?))
    }

    pub fn domain(&self) -> &dyn StorageDomain {
        self.domain.as_ref()
    }

    /// Where the domain stores objects when no storage target is selected.
    pub fn local_root(&self) -> &std::path::Path {
        &self.local_root
    }

    pub fn active(&self) -> ActiveStore {
        self.active
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub fn store(&self) -> Arc<dyn BlobStore> {
        self.active().store
    }

    pub fn target_id(&self) -> Option<Uuid> {
        self.active
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .target_id
    }

    /// Held by every mutation of the domain's objects.
    pub async fn lock_operation(&self) -> RwLockReadGuard<'_, ()> {
        self.operations.read().await
    }

    /// Blocks mutations while a migration copies the final delta and cuts over.
    pub async fn lock_cutover(&self) -> OwnedRwLockWriteGuard<()> {
        self.operations.clone().write_owned().await
    }

    pub fn try_lock_migration(&self) -> Option<OwnedMutexGuard<()>> {
        self.migrations.clone().try_lock_owned().ok()
    }

    pub async fn wait_for_migration(&self) {
        let _guard = self.migrations.lock().await;
    }

    fn replace(&self, active: ActiveStore) {
        *self
            .active
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = active;
    }

    /// Every object of the domain in the active store.
    pub async fn inventory(&self, database: &DatabaseConnection) -> Result<Vec<BlobMetadata>> {
        inventory(database, self.domain(), self.store().as_ref()).await
    }

    /// Removes a repository's objects from the local root and from every
    /// owned target, so nothing survives in a store that is not selected.
    /// Failures are logged; cleanup is best effort.
    pub async fn delete_repository(&self, database: &DatabaseConnection, storage_key: Uuid) {
        let label = self.domain.label();
        let prefix = match self.domain.repository_prefix(storage_key) {
            Ok(prefix) => prefix,
            Err(error) => {
                tracing::error!(%error, "could not construct the repository {label} prefix");
                return;
            }
        };
        match self.domain.open_local(self.local_root.clone()).await {
            Ok(local) => targets::delete_prefix(local.as_ref(), &prefix, label).await,
            Err(error) => tracing::error!(%error, "could not open local {label} storage"),
        }
        if let Err(error) = targets::delete_prefix_from_targets(database, &prefix, label).await {
            tracing::error!(%error, "could not clean repository {label} objects from every target");
        }
    }

    /// Opens the store a migration writes to; the nil target is the local root.
    async fn open_destination(
        &self,
        database: &DatabaseConnection,
        target_id: Option<Uuid>,
    ) -> Result<Arc<dyn BlobStore>> {
        match target_id {
            Some(target_id) => open_target(database, target_id).await,
            None => self.domain.open_local(self.local_root.clone()).await,
        }
    }

    /// Records a pending online migration to `target_id` (nil selects the
    /// local root). The per-domain unique index rejects a second one.
    pub async fn reserve_migration(
        &self,
        database: &DatabaseConnection,
        target_id: Uuid,
        operation_id: Uuid,
    ) -> Result<storage_migration::Model> {
        let source = active_target_id(database, self.domain.name()).await?;
        let destination = (!target_id.is_nil()).then_some(target_id);
        ensure!(
            source != destination,
            "destination is already the active {} storage target",
            self.domain.label()
        );
        Ok(self
            .new_migration(operation_id, source, destination)
            .insert(database)
            .await?)
    }

    fn new_migration(
        &self,
        id: Uuid,
        source: Option<Uuid>,
        destination: Option<Uuid>,
    ) -> storage_migration::ActiveModel {
        let now = Utc::now();
        storage_migration::ActiveModel {
            id: Set(id),
            domain: Set(self.domain.name().to_owned()),
            source_target_id: Set(source),
            target_id: Set(destination),
            state: Set(state::PENDING.to_owned()),
            phase: Set(phase::SCHEDULED.to_owned()),
            last_key: Set(None),
            copied_objects: Set(0),
            copied_bytes: Set(0),
            total_bytes: Set(None),
            error: Set(None),
            started_at: Set(now),
            updated_at: Set(now),
            completed_at: Set(None),
        }
    }

    /// Runs a reserved migration while Gitadel keeps serving. Writers are
    /// blocked only while the objects created after the first pass are copied
    /// and the target is switched.
    pub async fn migrate_online(
        &self,
        database: &DatabaseConnection,
        operation_id: Uuid,
        batch_size: usize,
        _migration: OwnedMutexGuard<()>,
    ) -> Result<()> {
        let result = async {
            let migration = storage_migration::Entity::find_by_id(operation_id)
                .one(database)
                .await?
                .filter(|migration| migration.domain == self.domain.name())
                .with_context(|| {
                    format!(
                        "online {} migration reservation is missing",
                        self.domain.label()
                    )
                })?;
            self.run_migration(database, migration, batch_size, true)
                .await
        }
        .await;
        if let Err(error) = &result {
            mark_failed(database, operation_id, error).await;
        }
        result
    }

    /// Migrates while Gitadel is stopped. An interrupted offline migration to
    /// the same destination resumes after the last copied key.
    pub async fn migrate_offline(
        &self,
        database: &DatabaseConnection,
        target_id: Uuid,
        batch_size: usize,
    ) -> Result<()> {
        let label = self.domain.label();
        let source = active_target_id(database, self.domain.name()).await?;
        let destination = (!target_id.is_nil()).then_some(target_id);
        ensure!(
            source != destination,
            "destination is already the active {label} storage target"
        );
        let migration = match unfinished(self.domain.name()).one(database).await? {
            Some(existing) => {
                ensure!(
                    existing.target_id == destination && existing.source_target_id == source,
                    "another {label} storage migration is incomplete"
                );
                existing
            }
            None => {
                self.new_migration(Uuid::new_v4(), source, destination)
                    .insert(database)
                    .await?
            }
        };
        let operation_id = migration.id;
        let result = self
            .run_migration(database, migration, batch_size, false)
            .await;
        if let Err(error) = &result {
            mark_failed(database, operation_id, error).await;
        }
        result
    }

    async fn run_migration(
        &self,
        database: &DatabaseConnection,
        mut migration: storage_migration::Model,
        batch_size: usize,
        online: bool,
    ) -> Result<()> {
        let label = self.domain.label();
        ensure!(
            batch_size > 0,
            "migration batch size must be greater than zero"
        );
        let source_id = migration.source_target_id;
        let destination_id = migration.target_id;
        ensure!(
            source_id != destination_id,
            "destination is already the active {label} storage target"
        );
        migration = save_progress(
            database,
            migration,
            state::PENDING,
            phase::CHECKING_DESTINATION,
        )
        .await?;
        ensure!(
            self.target_id() == source_id,
            "the active {label} storage target changed while migration was starting"
        );
        let destination = self.open_destination(database, destination_id).await?;

        let source = self.store();
        let snapshot = inventory(database, self.domain(), source.as_ref()).await?;
        let total_bytes = snapshot
            .iter()
            .try_fold(0_u64, |total, object| total.checked_add(object.size))
            .with_context(|| format!("{label} migration size overflow"))?;
        migration.total_bytes = Some(i64::try_from(total_bytes)?);
        migration = save_progress(database, migration, state::COPYING, phase::COPYING).await?;

        let start_after = migration.last_key.clone();
        let mut since_checkpoint = 0_usize;
        for object in snapshot.iter().filter(|object| {
            start_after
                .as_deref()
                .is_none_or(|key| object.key.as_str() > key)
        }) {
            if !self
                .copy_and_verify(source.as_ref(), destination.as_ref(), object)
                .await?
            {
                continue;
            }
            self.domain
                .object_copied(database, object, source_id)
                .await?;
            record_copy(&mut migration, object)?;
            since_checkpoint += 1;
            if since_checkpoint >= batch_size {
                migration =
                    save_progress(database, migration, state::COPYING, phase::COPYING).await?;
                since_checkpoint = 0;
            }
        }
        migration = save_progress(database, migration, state::VERIFYING, phase::VERIFYING).await?;

        // Only objects created after the snapshot are copied while writers wait.
        let _cutover = if online {
            Some(self.lock_cutover().await)
        } else {
            None
        };
        ensure!(
            self.target_id() == source_id,
            "the active {label} storage target changed during migration"
        );
        let copied = snapshot
            .iter()
            .map(|object| object.key.as_str())
            .collect::<HashSet<_>>();
        let current = inventory(database, self.domain(), source.as_ref()).await?;
        for object in current
            .iter()
            .filter(|object| !copied.contains(object.key.as_str()))
        {
            if self
                .copy_and_verify(source.as_ref(), destination.as_ref(), object)
                .await?
            {
                self.domain
                    .object_copied(database, object, source_id)
                    .await?;
                record_copy(&mut migration, object)?;
            }
        }
        self.reconcile_destination(database, destination.as_ref(), &current)
            .await?;
        migration = save_progress(database, migration, state::CUTOVER, phase::CUTOVER).await?;
        self.commit_cutover(database, migration).await?;
        if online {
            self.replace(ActiveStore {
                store: destination,
                target_id: destination_id,
            });
        }
        Ok(())
    }

    /// Copies one object and reads it back through the destination's
    /// digest-verifying reader. Returns `false` when the object was deleted
    /// from the source after the inventory listed it.
    async fn copy_and_verify(
        &self,
        source: &dyn BlobStore,
        destination: &dyn BlobStore,
        object: &BlobMetadata,
    ) -> Result<bool> {
        let label = self.domain.label();
        match copy_object(source, destination, object, label).await {
            Ok(()) => {}
            Err(error) => {
                if source.stat(&object.key).await?.is_none() {
                    return Ok(false);
                }
                return Err(error);
            }
        }
        let mut reader = destination
            .read(&object.key)
            .await
            .with_context(|| format!("could not read back copied {label} object {}", object.key))?;
        let size = tokio::io::copy(&mut reader, &mut tokio::io::sink())
            .await
            .with_context(|| format!("copied {label} object {} failed verification", object.key))?;
        ensure!(
            size == object.size,
            "copied {label} object {} has {size} bytes, expected {}",
            object.key,
            object.size
        );
        Ok(true)
    }

    /// Makes the destination hold exactly the source's objects: every source
    /// object must be present at its size, and objects left behind by an
    /// earlier residence are removed so they cannot reappear.
    async fn reconcile_destination(
        &self,
        database: &DatabaseConnection,
        destination: &dyn BlobStore,
        source: &[BlobMetadata],
    ) -> Result<()> {
        let label = self.domain.label();
        let held = inventory(database, self.domain(), destination)
            .await?
            .into_iter()
            .map(|object| (object.key.as_str().to_owned(), object))
            .collect::<HashMap<_, _>>();
        for object in source {
            let copied = held
                .get(object.key.as_str())
                .with_context(|| format!("copied {label} object {} is missing", object.key))?;
            ensure!(
                copied.size == object.size && copied.digest == object.digest,
                "copied {label} object {} does not match its source",
                object.key
            );
        }
        let wanted = source
            .iter()
            .map(|object| object.key.as_str())
            .collect::<HashSet<_>>();
        for (key, object) in &held {
            if !wanted.contains(key.as_str()) {
                destination.delete(&object.key).await?;
            }
        }
        Ok(())
    }

    async fn commit_cutover(
        &self,
        database: &DatabaseConnection,
        migration: storage_migration::Model,
    ) -> Result<()> {
        let label = self.domain.label();
        let transaction = database.begin().await?;
        let current = storage_domain_state::Entity::find_by_id(self.domain.name())
            .one(&transaction)
            .await?
            .with_context(|| format!("{label} storage state is missing"))?;
        ensure!(
            current.active_target_id == migration.source_target_id,
            "the active {label} storage target changed before cutover"
        );
        let now = Utc::now();
        storage_domain_state::ActiveModel {
            domain: sea_orm::Unchanged(current.domain),
            active_target_id: Set(migration.target_id),
            updated_at: Set(now),
        }
        .update(&transaction)
        .await?;
        self.domain
            .cutover(
                &transaction,
                migration.source_target_id,
                migration.target_id,
            )
            .await?;
        let mut completed: storage_migration::ActiveModel = migration.into();
        completed.state = Set(state::COMPLETED.to_owned());
        completed.phase = Set(phase::COMPLETED.to_owned());
        completed.error = Set(None);
        completed.updated_at = Set(now);
        completed.completed_at = Set(Some(now));
        completed.update(&transaction).await?;
        transaction.commit().await?;
        Ok(())
    }
}

/// Every object of `domain` in `store` that belongs to a known repository,
/// sorted by key.
pub async fn inventory(
    database: &DatabaseConnection,
    domain: &dyn StorageDomain,
    store: &dyn BlobStore,
) -> Result<Vec<BlobMetadata>> {
    let owners = repository::Entity::find()
        .all(database)
        .await?
        .into_iter()
        .map(|repository| repository.storage_key)
        .collect::<HashSet<_>>();
    let mut objects = store.list(&ObjectPrefix::new(domain.key_prefix())?).await?;
    objects.retain(|object| {
        domain
            .repository_storage_key(&object.key)
            .is_some_and(|owner| owners.contains(&owner))
    });
    objects.sort_by(|left, right| left.key.as_str().cmp(right.key.as_str()));
    Ok(objects)
}

/// Streams one object into `destination`, which verifies its SHA-256.
pub async fn copy_object(
    source: &dyn BlobStore,
    destination: &dyn BlobStore,
    object: &BlobMetadata,
    label: &str,
) -> Result<()> {
    let reader = source
        .read(&object.key)
        .await
        .with_context(|| format!("could not read {label} object {}", object.key))?;
    let outcome = destination
        .put_verified(&object.key, object.digest, reader)
        .await
        .with_context(|| format!("could not copy {label} object {}", object.key))?;
    ensure!(
        outcome.size == object.size,
        "copied {label} object {} size mismatch",
        object.key
    );
    Ok(())
}

/// The target a domain has selected, or `None` for its local root.
pub async fn active_target_id(
    database: &impl sea_orm::ConnectionTrait,
    domain: &str,
) -> Result<Option<Uuid>> {
    Ok(storage_domain_state::Entity::find_by_id(domain)
        .one(database)
        .await?
        .with_context(|| format!("{domain} storage state is missing"))?
        .active_target_id)
}

async fn open_target(database: &DatabaseConnection, target_id: Uuid) -> Result<Arc<dyn BlobStore>> {
    let (_, configuration) = targets::find(database, target_id).await?;
    let store = configuration.open().await?;
    targets::verify_ownership(store.as_ref(), target_id).await?;
    Ok(store)
}

fn unfinished(domain: &str) -> sea_orm::Select<storage_migration::Entity> {
    storage_migration::Entity::find()
        .filter(storage_migration::Column::Domain.eq(domain))
        .filter(storage_migration::Column::State.ne(state::COMPLETED))
        .filter(storage_migration::Column::State.ne(state::FAILED))
}

/// Online migrations cannot resume after a restart; the previous target stays
/// selected because cutover commits atomically.
pub async fn recover_interrupted(
    database: &DatabaseConnection,
    domain: &dyn StorageDomain,
) -> Result<()> {
    let result = storage_migration::Entity::update_many()
        .filter(storage_migration::Column::Domain.eq(domain.name()))
        .filter(storage_migration::Column::State.ne(state::COMPLETED))
        .filter(storage_migration::Column::State.ne(state::FAILED))
        .col_expr(storage_migration::Column::State, Expr::value(state::FAILED))
        .col_expr(storage_migration::Column::Phase, Expr::value(phase::FAILED))
        .col_expr(
            storage_migration::Column::Error,
            Expr::value(format!(
                "Gitadel restarted before the {} storage migration completed. The previous target remains selected.",
                domain.label()
            )),
        )
        .col_expr(
            storage_migration::Column::UpdatedAt,
            Expr::current_timestamp(),
        )
        .col_expr(
            storage_migration::Column::CompletedAt,
            Expr::current_timestamp(),
        )
        .exec(database)
        .await?;
    if result.rows_affected > 0 {
        tracing::warn!(
            count = result.rows_affected,
            domain = domain.name(),
            "marked interrupted storage migrations as failed"
        );
    }
    Ok(())
}

/// Records a failed migration. Errors are logged because the caller is
/// already reporting the original failure.
pub async fn mark_failed(database: &DatabaseConnection, operation_id: Uuid, error: &anyhow::Error) {
    let now = Utc::now();
    let update = storage_migration::ActiveModel {
        id: sea_orm::Unchanged(operation_id),
        state: Set(state::FAILED.to_owned()),
        phase: Set(phase::FAILED.to_owned()),
        error: Set(Some(format!("{error:#}"))),
        updated_at: Set(now),
        completed_at: Set(Some(now)),
        ..Default::default()
    }
    .update(database)
    .await;
    if let Err(update_error) = update {
        tracing::error!(%update_error, %operation_id, "could not record storage migration failure");
    }
}

/// The phase reported to progress streams, which name the copy phase after
/// the domain (`copying_lfs`, `copying_registry`).
pub fn progress_phase(migration: &storage_migration::Model) -> Cow<'static, str> {
    match migration.state.as_str() {
        state::COMPLETED => return Cow::Borrowed(phase::COMPLETED),
        state::FAILED => return Cow::Borrowed(phase::FAILED),
        _ => {}
    }
    match migration.phase.as_str() {
        phase::CHECKING_DESTINATION => Cow::Borrowed(phase::CHECKING_DESTINATION),
        phase::COPYING | phase::VERIFYING => Cow::Owned(format!("copying_{}", migration.domain)),
        phase::CUTOVER => Cow::Borrowed("writing_metadata"),
        _ => Cow::Borrowed(phase::SCHEDULED),
    }
}

fn record_copy(migration: &mut storage_migration::Model, object: &BlobMetadata) -> Result<()> {
    migration.last_key = Some(object.key.as_str().to_owned());
    migration.copied_objects += 1;
    migration.copied_bytes = migration
        .copied_bytes
        .saturating_add(i64::try_from(object.size)?);
    Ok(())
}

async fn save_progress(
    database: &DatabaseConnection,
    migration: storage_migration::Model,
    state: &str,
    phase: &str,
) -> Result<storage_migration::Model> {
    Ok(storage_migration::ActiveModel {
        id: sea_orm::Unchanged(migration.id),
        copied_objects: Set(migration.copied_objects),
        copied_bytes: Set(migration.copied_bytes),
        total_bytes: Set(migration.total_bytes),
        last_key: Set(migration.last_key),
        state: Set(state.to_owned()),
        phase: Set(phase.to_owned()),
        error: Set(None),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .update(database)
    .await?)
}

#[cfg(test)]
mod tests;
