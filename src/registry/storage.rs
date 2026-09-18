use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, RwLock},
};

use anyhow::{Context, Result, ensure};
use chrono::Utc;
use sea_orm::{
    ActiveModelTrait as _, ColumnTrait as _, DatabaseConnection, EntityTrait as _,
    QueryFilter as _, Set, TransactionTrait as _,
};
use tokio::sync::{Mutex, OwnedMutexGuard, RwLock as AsyncRwLock};
use uuid::Uuid;

use super::store::{RegistryStore, RegistryUsage};
use crate::{
    blob_store::{
        BlobDigest, BlobMetadata, BlobStore, FilesystemBlobStore, ObjectKey, ObjectPrefix, targets,
    },
    config::StorageSettings,
    entity::{registry_storage_migration, registry_storage_state, repository},
};

pub(crate) struct RegistryStorageManager {
    active: RwLock<Option<ActiveRegistryTarget>>,
    operations: AsyncRwLock<()>,
    migrations: Arc<Mutex<()>>,
}

#[derive(Clone)]
pub(crate) struct ActiveRegistryTarget {
    pub id: Uuid,
    pub store: Arc<dyn BlobStore>,
}

impl RegistryStorageManager {
    pub(crate) async fn new(database: &DatabaseConnection) -> Result<Arc<Self>> {
        recover_migrations(database).await?;
        Ok(Arc::new(Self {
            active: RwLock::new(load_active(database).await?),
            operations: AsyncRwLock::new(()),
            migrations: Arc::new(Mutex::new(())),
        }))
    }

    pub(crate) fn active(&self) -> Option<ActiveRegistryTarget> {
        self.active
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub(crate) fn target_id(&self) -> Option<Uuid> {
        self.active
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .map(|active| active.id)
    }

    pub(crate) fn store(&self) -> Option<Arc<dyn BlobStore>> {
        self.active().map(|active| active.store)
    }

    pub(crate) async fn lock_operation(&self) -> tokio::sync::RwLockReadGuard<'_, ()> {
        self.operations.read().await
    }

    pub(crate) fn try_lock_migration(&self) -> Option<OwnedMutexGuard<()>> {
        self.migrations.clone().try_lock_owned().ok()
    }

    pub(crate) async fn wait_for_migration(&self) {
        let _guard = self.migrations.lock().await;
    }
}

pub(crate) async fn load_active(
    database: &DatabaseConnection,
) -> Result<Option<ActiveRegistryTarget>> {
    let state = registry_storage_state::Entity::find_by_id(1)
        .one(database)
        .await?
        .context("registry storage state is missing")?;
    let Some(id) = state.active_target_id else {
        return Ok(None);
    };
    let (_, configuration) = targets::find(database, id).await?;
    let store = configuration.open().await?;
    targets::verify_ownership(store.as_ref(), id).await?;
    Ok(Some(ActiveRegistryTarget { id, store }))
}

async fn backing_store(
    settings: &StorageSettings,
    active: &Option<ActiveRegistryTarget>,
) -> Result<Arc<dyn BlobStore>> {
    match active {
        Some(active) => Ok(active.store.clone()),
        None => Ok(Arc::new(
            FilesystemBlobStore::new(settings.repository_root.clone()).await?,
        )),
    }
}

/// Canonical registry keys are independent of the selected storage target.
fn key_parts(key: &str) -> Option<(Uuid, &str, &str, bool)> {
    let (repository, rest) = key.strip_prefix("registry/")?.split_once('/')?;
    let storage_key = Uuid::parse_str(repository).ok()?;
    let (image, payload) = rest.split_once('/')?;
    image.parse::<BlobDigest>().ok()?;
    let mut parts = payload.split('/');
    let manifest = match parts.next()? {
        "blobs" => {
            let first = parts.next()?;
            let second = parts.next()?;
            let digest = parts.next()?;
            digest.parse::<BlobDigest>().ok()?;
            if first != &digest[..2] || second != &digest[2..4] {
                return None;
            }
            false
        }
        "manifests" => {
            if parts.next()? != "objects" {
                return None;
            }
            parts.next()?.parse::<BlobDigest>().ok()?;
            true
        }
        _ => return None,
    };
    if parts.next().is_some() {
        return None;
    }
    Some((storage_key, image, payload, manifest))
}

pub(crate) fn registry_object_key(local: &str) -> Option<ObjectKey> {
    let (repository, rest) = local.split_once("/gitadel-registry/images/")?;
    let storage_key = Uuid::parse_str(repository.strip_suffix(".git")?).ok()?;
    let key = ObjectKey::new(format!("registry/{storage_key}/{rest}")).ok()?;
    key_parts(key.as_str())?;
    Some(key)
}

pub(crate) fn local_object_key(key: &ObjectKey) -> Result<ObjectKey> {
    let (storage_key, image, payload, _) =
        key_parts(key.as_str()).context("invalid registry payload key")?;
    ObjectKey::new(format!(
        "{storage_key}.git/gitadel-registry/images/{image}/{payload}"
    ))
}

fn stored_key(key: &ObjectKey, external: bool) -> Result<ObjectKey> {
    if external {
        Ok(key.clone())
    } else {
        local_object_key(key)
    }
}

pub(crate) async fn inventory(
    store: &dyn BlobStore,
    external: bool,
    repositories: &[repository::Model],
) -> Result<Vec<BlobMetadata>> {
    let mut objects = Vec::new();
    if external {
        let owners: HashSet<_> = repositories
            .iter()
            .map(|repository| repository.storage_key)
            .collect();
        for object in store.list(&ObjectPrefix::new("registry")?).await? {
            if key_parts(object.key.as_str())
                .is_some_and(|(owner, _, _, _)| owners.contains(&owner))
            {
                objects.push(object);
            }
        }
    } else {
        for repository in repositories {
            let prefix = ObjectPrefix::new(format!(
                "{}.git/gitadel-registry/images",
                repository.storage_key
            ))?;
            for mut object in store.list(&prefix).await? {
                if let Some(key) = registry_object_key(object.key.as_str()) {
                    object.key = key;
                    objects.push(object);
                }
            }
        }
    }
    objects.sort_by(|left, right| left.key.as_str().cmp(right.key.as_str()));
    Ok(objects)
}

pub(crate) async fn usage_by_repository(
    database: &DatabaseConnection,
    settings: &StorageSettings,
    active: &Option<ActiveRegistryTarget>,
) -> Result<Vec<(repository::Model, RegistryUsage)>> {
    let repositories = repository::Entity::find().all(database).await?;
    let store = backing_store(settings, active).await?;
    let objects = inventory(store.as_ref(), active.is_some(), &repositories).await?;
    let mut payload_usage = HashMap::<Uuid, RegistryUsage>::new();
    for object in objects {
        let (owner, _, _, manifest) =
            key_parts(object.key.as_str()).context("invalid inventoried registry key")?;
        let usage = payload_usage.entry(owner).or_default();
        usage.object_count += 1;
        usage.total_bytes = usage
            .total_bytes
            .checked_add(object.size)
            .context("registry usage overflow")?;
        if manifest {
            usage.manifest_count += 1;
        } else {
            usage.blob_count += 1;
        }
    }
    let registry = RegistryStore::new();
    let mut result = Vec::with_capacity(repositories.len());
    for repository in repositories {
        let path = settings
            .repository_root
            .join(format!("{}.git", repository.storage_key));
        let mut usage = registry.metadata_usage(&path).await?;
        usage += payload_usage
            .remove(&repository.storage_key)
            .unwrap_or_default();
        result.push((repository, usage));
    }
    Ok(result)
}

pub(crate) async fn reserve_migration(
    database: &DatabaseConnection,
    target_id: Uuid,
    operation_id: Uuid,
    source_target_id: Option<Uuid>,
) -> Result<()> {
    let destination = (!target_id.is_nil()).then_some(target_id);
    ensure!(
        source_target_id != destination,
        "destination is already active"
    );
    let now = Utc::now();
    registry_storage_migration::ActiveModel {
        id: Set(operation_id),
        source_target_id: Set(source_target_id),
        target_id: Set(destination),
        state: Set("pending".to_owned()),
        phase: Set("scheduled".to_owned()),
        last_key: Set(None),
        copied_objects: Set(0),
        copied_bytes: Set(0),
        total_bytes: Set(None),
        error: Set(None),
        started_at: Set(now),
        updated_at: Set(now),
        completed_at: Set(None),
    }
    .insert(database)
    .await?;
    Ok(())
}

async fn save_progress(
    database: &DatabaseConnection,
    migration: &registry_storage_migration::Model,
) -> Result<()> {
    registry_storage_migration::ActiveModel {
        id: Set(migration.id),
        state: Set(migration.state.clone()),
        phase: Set(migration.phase.clone()),
        last_key: Set(migration.last_key.clone()),
        copied_objects: Set(migration.copied_objects),
        copied_bytes: Set(migration.copied_bytes),
        total_bytes: Set(migration.total_bytes),
        updated_at: Set(Utc::now()),
        ..Default::default()
    }
    .update(database)
    .await?;
    Ok(())
}

pub(crate) async fn mark_failed(
    database: &DatabaseConnection,
    operation_id: Uuid,
    error: &anyhow::Error,
) {
    let update = registry_storage_migration::ActiveModel {
        id: Set(operation_id),
        state: Set("failed".to_owned()),
        phase: Set("failed".to_owned()),
        error: Set(Some(format!("{error:#}"))),
        updated_at: Set(Utc::now()),
        completed_at: Set(Some(Utc::now())),
        ..Default::default()
    }
    .update(database)
    .await;
    if let Err(update_error) = update {
        tracing::error!(%update_error, %operation_id, "could not record registry migration failure");
    }
}

pub(crate) async fn migrate_online(
    database: &DatabaseConnection,
    settings: &StorageSettings,
    operation_id: Uuid,
    batch_size: usize,
    manager: Arc<RegistryStorageManager>,
    _migration_guard: OwnedMutexGuard<()>,
) -> Result<()> {
    let result = migrate(database, settings, operation_id, batch_size, &manager).await;
    if let Err(error) = &result {
        mark_failed(database, operation_id, error).await;
    }
    result
}

async fn migrate(
    database: &DatabaseConnection,
    settings: &StorageSettings,
    operation_id: Uuid,
    batch_size: usize,
    manager: &RegistryStorageManager,
) -> Result<()> {
    ensure!(
        batch_size > 0,
        "migration batch size must be greater than zero"
    );
    let mut migration = registry_storage_migration::Entity::find_by_id(operation_id)
        .one(database)
        .await?
        .context("registry migration is missing")?;
    migration.phase = "checking_destination".to_owned();
    save_progress(database, &migration).await?;
    let destination = if let Some(id) = migration.target_id {
        let (_, configuration) = targets::find(database, id).await?;
        let store = configuration.open().await?;
        targets::verify_ownership(store.as_ref(), id).await?;
        Some(ActiveRegistryTarget { id, store })
    } else {
        None
    };

    // Freeze mutations, not pulls. Source payloads stay intact throughout copying and cutover.
    let _operations = manager.operations.write().await;
    let source = manager.active();
    ensure!(
        source.as_ref().map(|source| source.id) == migration.source_target_id,
        "registry source changed before migration"
    );
    let source_store = backing_store(settings, &source).await?;
    let destination_store = backing_store(settings, &destination).await?;
    let repositories = repository::Entity::find().all(database).await?;
    let objects = inventory(source_store.as_ref(), source.is_some(), &repositories).await?;
    let previous = inventory(
        destination_store.as_ref(),
        destination.is_some(),
        &repositories,
    )
    .await?;
    let total = objects
        .iter()
        .try_fold(0_u64, |total, object| total.checked_add(object.size))
        .context("registry migration size overflow")?;
    migration.total_bytes = Some(i64::try_from(total).context("registry migration is too large")?);
    migration.state = "copying".to_owned();
    migration.phase = "copying_registry".to_owned();
    save_progress(database, &migration).await?;
    for batch in objects.chunks(batch_size) {
        for object in batch {
            let source_key = stored_key(&object.key, source.is_some())?;
            let destination_key = stored_key(&object.key, destination.is_some())?;
            let reader = source_store.read(&source_key).await?;
            destination_store
                .put_verified(&destination_key, object.digest, reader)
                .await?;
            let mut verification = destination_store.read(&destination_key).await?;
            let size = tokio::io::copy(&mut verification, &mut tokio::io::sink()).await?;
            ensure!(
                size == object.size,
                "registry destination size mismatch for {}",
                object.key
            );
            migration.copied_objects += 1;
            migration.copied_bytes += i64::try_from(size)?;
            migration.last_key = Some(object.key.to_string());
        }
        save_progress(database, &migration).await?;
    }
    migration.state = "cutover".to_owned();
    migration.phase = "writing_metadata".to_owned();
    save_progress(database, &migration).await?;
    let present: HashSet<_> = objects.iter().map(|object| &object.key).collect();
    for object in previous {
        if !present.contains(&object.key) {
            destination_store
                .delete(&stored_key(&object.key, destination.is_some())?)
                .await?;
        }
    }

    let transaction = database.begin().await?;
    let current = registry_storage_state::Entity::find_by_id(1)
        .one(&transaction)
        .await?
        .context("registry storage state is missing")?;
    ensure!(
        current.active_target_id == migration.source_target_id,
        "registry source changed during migration"
    );
    let now = Utc::now();
    registry_storage_state::ActiveModel {
        id: Set(1),
        active_target_id: Set(destination.as_ref().map(|target| target.id)),
        updated_at: Set(now),
    }
    .update(&transaction)
    .await?;
    registry_storage_migration::ActiveModel {
        id: Set(operation_id),
        state: Set("completed".to_owned()),
        phase: Set("completed".to_owned()),
        updated_at: Set(now),
        completed_at: Set(Some(now)),
        ..Default::default()
    }
    .update(&transaction)
    .await?;
    transaction.commit().await?;
    *manager
        .active
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = destination;
    Ok(())
}

async fn recover_migrations(database: &DatabaseConnection) -> Result<()> {
    registry_storage_migration::Entity::update_many()
        .filter(registry_storage_migration::Column::State.ne("completed"))
        .filter(registry_storage_migration::Column::State.ne("failed"))
        .col_expr(registry_storage_migration::Column::State, sea_orm::sea_query::Expr::value("failed"))
        .col_expr(registry_storage_migration::Column::Phase, sea_orm::sea_query::Expr::value("failed"))
        .col_expr(registry_storage_migration::Column::Error, sea_orm::sea_query::Expr::value(
            "Gitadel restarted before the registry migration completed. The previous target remains selected."))
        .col_expr(registry_storage_migration::Column::UpdatedAt, sea_orm::sea_query::Expr::current_timestamp())
        .col_expr(registry_storage_migration::Column::CompletedAt, sea_orm::sea_query::Expr::current_timestamp())
        .exec(database).await?;
    Ok(())
}
