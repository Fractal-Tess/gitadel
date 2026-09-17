use std::{
    collections::HashSet,
    sync::{Arc, RwLock},
};

use anyhow::{Context, Result, ensure};
use chrono::Utc;
use sea_orm::{
    ActiveModelTrait as _, ColumnTrait as _, DatabaseConnection, EntityTrait as _,
    QueryFilter as _, Set, TransactionTrait as _, sea_query::OnConflict,
};
use tokio::sync::{
    Mutex, OwnedMutexGuard, OwnedRwLockWriteGuard, RwLock as AsyncRwLock, RwLockReadGuard,
};
use uuid::Uuid;

use crate::archive::MaintenanceProgressReporter;
use crate::{
    blob_store::{
        BlobDigest, BlobMetadata, FilesystemBlobStore, ObjectPrefix,
        targets::{self, ActiveBlobStore, StorageTargetConfiguration},
    },
    config::{LfsCommand, LfsTargetCommand, S3Settings, Settings},
    database,
    entity::{
        lfs_object, lfs_storage_migration, lfs_storage_state, lfs_storage_target, repository,
    },
};

/// The live LFS target, swapped atomically only after a migration has copied and
/// verified every object. Writer read guards drain before the cutover write guard.
pub(crate) struct LfsStorageManager {
    active: RwLock<ActiveBlobStore>,
    operations: Arc<AsyncRwLock<()>>,
    migrations: Arc<Mutex<()>>,
}

impl LfsStorageManager {
    pub(crate) async fn new(
        database: &DatabaseConnection,
        fallback_path: std::path::PathBuf,
    ) -> Result<Arc<Self>> {
        recover_online_migrations(database).await?;
        Ok(Arc::new(Self {
            active: RwLock::new(targets::load_active(database, fallback_path).await?),
            operations: Arc::new(AsyncRwLock::new(())),
            migrations: Arc::new(Mutex::new(())),
        }))
    }

    pub(crate) fn store(&self) -> Arc<dyn crate::blob_store::BlobStore> {
        self.active
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .store
            .clone()
    }

    pub(crate) fn target_id(&self) -> Option<Uuid> {
        self.active
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .target_id
    }

    pub(crate) async fn lock_operation(&self) -> RwLockReadGuard<'_, ()> {
        self.operations.read().await
    }

    pub(crate) async fn lock_cutover(&self) -> OwnedRwLockWriteGuard<()> {
        self.operations.clone().write_owned().await
    }

    pub(crate) fn try_lock_migration(self: &Arc<Self>) -> Option<OwnedMutexGuard<()>> {
        self.migrations.clone().try_lock_owned().ok()
    }

    pub(crate) fn replace(
        &self,
        store: Arc<dyn crate::blob_store::BlobStore>,
        target_id: Option<Uuid>,
    ) {
        *self
            .active
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            ActiveBlobStore { store, target_id };
    }
}

pub async fn run(command: &LfsCommand, settings: &Settings) -> Result<()> {
    let database = database::connect_and_migrate(&settings.database).await?;
    let result = match command {
        LfsCommand::Target { command } => run_target(command, &database, settings).await,
        LfsCommand::Migrate {
            target_id,
            batch_size,
        } => migrate(&database, settings, *target_id, *batch_size, None).await,
    };
    database.close().await?;
    result
}

async fn run_target(
    command: &LfsTargetCommand,
    database: &DatabaseConnection,
    settings: &Settings,
) -> Result<()> {
    match command {
        LfsTargetCommand::List => {
            println!(
                "{}",
                serde_json::to_string_pretty(
                    // The CLI holds no scan cache, so targets report their
                    // database usage and the volume they sit on.
                    &targets::list(database, &settings.storage.lfs_root, &Default::default())
                        .await?,
                )?
            );
        }
        LfsTargetCommand::AddFilesystem { name, path } => {
            let target = targets::create(
                database,
                name.clone(),
                StorageTargetConfiguration::Filesystem { path: path.clone() },
            )
            .await?;
            println!("{}", serde_json::to_string_pretty(&target)?);
        }
        LfsTargetCommand::AddS3 {
            name,
            endpoint,
            bucket,
            access_key,
            secret_key,
            region,
            prefix,
        } => {
            let target = targets::create(
                database,
                name.clone(),
                StorageTargetConfiguration::S3 {
                    s3: S3Settings {
                        endpoint: endpoint.clone(),
                        bucket: bucket.clone(),
                        access_key: access_key.clone(),
                        secret_key: secret_key.clone(),
                        region: region.clone(),
                        prefix: prefix.clone(),
                    },
                },
            )
            .await?;
            println!("{}", serde_json::to_string_pretty(&target)?);
        }
        LfsTargetCommand::Test { target_id } => {
            let (_, configuration) = load_target(database, *target_id).await?;
            targets::test_configuration(&configuration).await?;
            println!("Storage target {target_id} passed write, stat, read, and delete checks.");
        }
    }
    Ok(())
}

async fn load_target(
    database: &DatabaseConnection,
    id: Uuid,
) -> Result<(lfs_storage_target::Model, StorageTargetConfiguration)> {
    let target = lfs_storage_target::Entity::find_by_id(id)
        .one(database)
        .await?
        .context("LFS storage target does not exist")?;
    let configuration = serde_json::from_str(&target.configuration)
        .context("stored LFS target configuration is invalid")?;
    Ok((target, configuration))
}

pub(crate) async fn migrate(
    database: &DatabaseConnection,
    settings: &Settings,
    target_id: Uuid,
    batch_size: usize,
    reporter: Option<&MaintenanceProgressReporter>,
) -> Result<()> {
    migrate_inner(
        database,
        settings,
        target_id,
        batch_size,
        Uuid::new_v4(),
        None,
        reporter,
    )
    .await
}

pub(crate) async fn migrate_online(
    database: &DatabaseConnection,
    settings: &Settings,
    target_id: Uuid,
    batch_size: usize,
    operation_id: Uuid,
    manager: Arc<LfsStorageManager>,
    _migration_guard: OwnedMutexGuard<()>,
) -> Result<()> {
    let result = migrate_inner(
        database,
        settings,
        target_id,
        batch_size,
        operation_id,
        Some(manager),
        None,
    )
    .await;
    if let Err(error) = &result {
        mark_failed(database, operation_id, error).await;
    }
    result
}
pub(crate) async fn reserve_online_migration(
    database: &DatabaseConnection,
    settings: &Settings,
    target_id: Uuid,
    operation_id: Uuid,
) -> Result<lfs_storage_migration::Model> {
    let active = targets::load_active(database, settings.storage.lfs_root.clone()).await?;
    let destination_id = (!target_id.is_nil()).then_some(target_id);
    ensure!(
        active.target_id != destination_id,
        "destination is already the active LFS storage target"
    );
    let now = Utc::now();
    Ok(lfs_storage_migration::ActiveModel {
        id: Set(operation_id),
        source_target_id: Set(active.target_id),
        target_id: Set(destination_id),
        state: Set("pending".to_owned()),
        phase: Set("pending".to_owned()),
        last_key: Set(None),
        copied_objects: Set(0),
        copied_bytes: Set(0),
        error: Set(None),
        started_at: Set(now),
        updated_at: Set(now),
        completed_at: Set(None),
    }
    .insert(database)
    .await?)
}
async fn recover_online_migrations(database: &DatabaseConnection) -> Result<()> {
    let migrations = lfs_storage_migration::Entity::update_many()
        .filter(lfs_storage_migration::Column::State.ne("completed"))
        .filter(lfs_storage_migration::Column::State.ne("failed"))
        .col_expr(
            lfs_storage_migration::Column::State,
            sea_orm::sea_query::Expr::value("failed"),
        )
        .col_expr(
            lfs_storage_migration::Column::Phase,
            sea_orm::sea_query::Expr::value("failed"),
        )
        .col_expr(
            lfs_storage_migration::Column::Error,
            sea_orm::sea_query::Expr::value(
                "Gitadel restarted before the online migration completed.",
            ),
        )
        .col_expr(
            lfs_storage_migration::Column::UpdatedAt,
            sea_orm::sea_query::Expr::current_timestamp(),
        )
        .exec(database)
        .await?;
    if migrations.rows_affected > 0 {
        tracing::warn!(
            count = migrations.rows_affected,
            "marked interrupted online LFS migrations as failed"
        );
    }
    Ok(())
}

async fn migrate_inner(
    database: &DatabaseConnection,
    settings: &Settings,
    target_id: Uuid,
    batch_size: usize,
    operation_id: Uuid,
    manager: Option<Arc<LfsStorageManager>>,
    reporter: Option<&MaintenanceProgressReporter>,
) -> Result<()> {
    ensure!(
        batch_size > 0,
        "migration batch size must be greater than zero"
    );
    let active = targets::load_active(database, settings.storage.lfs_root.clone()).await?;
    let destination_id = (!target_id.is_nil()).then_some(target_id);
    ensure!(
        active.target_id != destination_id,
        "destination is already the active LFS storage target"
    );
    if manager.is_some() {
        set_migration_phase(database, operation_id, "copying", "check").await?;
    }
    if let Some(manager) = &manager {
        ensure!(
            manager.target_id() == active.target_id,
            "the active LFS storage target changed while migration was starting"
        );
    }
    let target: Arc<dyn crate::blob_store::BlobStore> = if let Some(target_id) = destination_id {
        let (_, target_configuration) = load_target(database, target_id).await?;
        target_configuration.open().await?
    } else {
        Arc::new(FilesystemBlobStore::new(settings.storage.lfs_root.clone()).await?)
    };
    if let Some(reporter) = reporter {
        reporter.report(
            crate::archive::MaintenancePhase::CheckingDestination,
            "Checking the LFS destination and ownership marker.",
        );
    }
    if let Some(target_id) = destination_id {
        targets::verify_ownership(target.as_ref(), target_id).await?;
    }

    let mut migration_query = lfs_storage_migration::Entity::find()
        .filter(lfs_storage_migration::Column::State.ne("completed"));
    migration_query = migration_query.filter(lfs_storage_migration::Column::State.ne("failed"));
    let mut migration = match migration_query.one(database).await? {
        Some(existing) => {
            ensure!(
                existing.id == operation_id || manager.is_none(),
                "another LFS storage migration is already active"
            );
            ensure!(
                existing.target_id == destination_id
                    && existing.source_target_id == active.target_id,
                "another LFS storage migration is incomplete"
            );
            existing
        }
        None => {
            let now = Utc::now();
            lfs_storage_migration::ActiveModel {
                id: Set(operation_id),
                source_target_id: Set(active.target_id),
                target_id: Set(destination_id),
                state: Set("copying".to_owned()),
                phase: Set("copy".to_owned()),
                last_key: Set(None),
                copied_objects: Set(0),
                copied_bytes: Set(0),
                error: Set(None),
                started_at: Set(now),
                updated_at: Set(now),
                completed_at: Set(None),
            }
            .insert(database)
            .await?
        }
    };
    let migration_id = migration.id;

    if let Some(reporter) = reporter {
        reporter.report(
            crate::archive::MaintenancePhase::CopyingLfs,
            "Copying Git LFS objects.",
        );
    }
    let result = async {
        let source_store = manager
            .as_ref()
            .map_or_else(|| active.store.clone(), |manager| manager.store());
        let objects = lfs_objects(source_store.as_ref()).await?;
        let total_bytes = objects
            .iter()
            .fold(0_u64, |total, object| total.saturating_add(object.size));
        update_progress(
            u64::try_from(migration.copied_bytes).unwrap_or_default(),
            total_bytes,
            reporter,
        );
        let start_after = migration.last_key.clone();
        let mut since_checkpoint = 0_usize;
        for metadata in objects.iter().filter(|object| {
            start_after
                .as_deref()
                .is_none_or(|key| object.key.as_str() > key)
        }) {
            copy_object(source_store.as_ref(), target.as_ref(), metadata).await?;
            catalog_object(database, metadata, active.target_id).await?;
            migration.last_key = Some(metadata.key.as_str().to_owned());
            migration.copied_objects += 1;
            migration.copied_bytes = migration
                .copied_bytes
                .saturating_add(i64::try_from(metadata.size)?);
            since_checkpoint += 1;
            update_progress(
                u64::try_from(migration.copied_bytes).unwrap_or_default(),
                total_bytes,
                reporter,
            );
            if since_checkpoint >= batch_size {
                migration = save_progress(database, migration, "copying", "copy").await?;
                since_checkpoint = 0;
            }
        }
        migration = save_progress(database, migration, "verifying", "verify").await?;
        verify_objects(target.as_ref(), &objects).await?;
        if let Some(manager) = &manager {
            let initial_keys = objects
                .iter()
                .map(|metadata| metadata.key.as_str())
                .collect::<HashSet<_>>();
            // Block writers only while copying objects created after the snapshot.
            let _operation_guard = manager.lock_cutover().await;
            let source_store = manager.store();
            ensure!(
                manager.target_id() == active.target_id,
                "the active LFS storage target changed during migration"
            );
            let delta = lfs_objects(source_store.as_ref())
                .await?
                .into_iter()
                .filter(|metadata| !initial_keys.contains(metadata.key.as_str()))
                .collect::<Vec<_>>();
            for metadata in &delta {
                copy_object(source_store.as_ref(), target.as_ref(), metadata).await?;
                catalog_object(database, metadata, active.target_id).await?;
                migration.copied_objects += 1;
                migration.copied_bytes = migration
                    .copied_bytes
                    .saturating_add(i64::try_from(metadata.size)?);
            }
            verify_objects(target.as_ref(), &delta).await?;
            migration = save_progress(database, migration, "cutover", "cutover").await?;
            commit_cutover(database, migration, active.target_id, destination_id).await?;
            manager.replace(target.clone(), destination_id);
        } else {
            commit_cutover(database, migration, active.target_id, destination_id).await?;
        }
        if let Some(reporter) = reporter {
            reporter.report(
                crate::archive::MaintenancePhase::WritingMetadata,
                "Selecting the verified LFS target.",
            );
        }
        Ok(())
    }
    .await;

    if let Err(error) = &result {
        mark_failed(database, migration_id, error).await;
    }
    result?;
    println!("LFS storage migration completed; target {target_id} is active.");
    Ok(())
}
fn update_progress(
    processed_bytes: u64,
    total_bytes: u64,
    reporter: Option<&MaintenanceProgressReporter>,
) {
    if let Some(reporter) = reporter {
        reporter.report_progress(
            crate::archive::MaintenancePhase::CopyingLfs,
            "Copying Git LFS objects.",
            processed_bytes,
            total_bytes,
        );
    }
}

async fn verify_objects(
    target: &dyn crate::blob_store::BlobStore,
    objects: &[BlobMetadata],
) -> Result<()> {
    for metadata in objects {
        let copied = target
            .stat(&metadata.key)
            .await?
            .context("copied LFS object is missing from destination")?;
        ensure!(
            copied.size == metadata.size,
            "copied LFS object size mismatch"
        );
        ensure!(
            copied.digest == metadata.digest,
            "copied LFS object digest mismatch"
        );
    }
    Ok(())
}

pub(crate) async fn mark_failed(
    database: &DatabaseConnection,
    migration_id: Uuid,
    error: &anyhow::Error,
) {
    if let Ok(Some(failed)) = lfs_storage_migration::Entity::find_by_id(migration_id)
        .one(database)
        .await
    {
        let mut failed: lfs_storage_migration::ActiveModel = failed.into();
        failed.state = Set("failed".to_owned());
        failed.phase = Set("failed".to_owned());
        failed.error = Set(Some(format!("{error:#}")));
        failed.updated_at = Set(Utc::now());
        let _ = failed.update(database).await;
    }
}
async fn set_migration_phase(
    database: &DatabaseConnection,
    migration_id: Uuid,
    state: &str,
    phase: &str,
) -> Result<()> {
    let migration = lfs_storage_migration::Entity::find_by_id(migration_id)
        .one(database)
        .await?
        .context("online LFS migration reservation is missing")?;
    save_progress(database, migration, state, phase).await?;
    Ok(())
}

async fn lfs_objects(store: &dyn crate::blob_store::BlobStore) -> Result<Vec<BlobMetadata>> {
    let mut objects = store.list(&ObjectPrefix::new("")?).await?;
    objects.retain(|object| {
        let parts = object.key.as_str().split('/').collect::<Vec<_>>();
        parts.len() == 4
            && Uuid::parse_str(parts[0]).is_ok()
            && parts[1].len() == 2
            && parts[2].len() == 2
            && parts[3].parse::<BlobDigest>().is_ok()
    });
    objects.sort_by(|left, right| left.key.as_str().cmp(right.key.as_str()));
    Ok(objects)
}

async fn copy_object(
    source: &dyn crate::blob_store::BlobStore,
    target: &dyn crate::blob_store::BlobStore,
    metadata: &BlobMetadata,
) -> Result<()> {
    let reader = source.read(&metadata.key).await?;
    let outcome = target
        .put_verified(&metadata.key, metadata.digest, reader)
        .await?;
    ensure!(
        outcome.size == metadata.size,
        "copied LFS object size mismatch"
    );
    Ok(())
}

async fn catalog_object(
    database: &DatabaseConnection,
    metadata: &BlobMetadata,
    target_id: Option<Uuid>,
) -> Result<()> {
    let storage_key = metadata
        .key
        .as_str()
        .split('/')
        .next()
        .context("LFS object key has no repository storage key")?
        .parse::<Uuid>()?;
    let repository_id = repository::Entity::find()
        .filter(repository::Column::StorageKey.eq(storage_key))
        .one(database)
        .await?
        .context("LFS object belongs to an unknown repository storage key")?
        .id;
    let oid = metadata
        .key
        .as_str()
        .rsplit('/')
        .next()
        .context("LFS object key has no digest")?;
    lfs_object::Entity::insert(lfs_object::ActiveModel {
        repository_id: Set(repository_id),
        oid: Set(oid.to_owned()),
        size: Set(i64::try_from(metadata.size)?),
        storage_target_id: Set(target_id),
        created_at: Set(Utc::now()),
    })
    .on_conflict(
        OnConflict::columns([lfs_object::Column::RepositoryId, lfs_object::Column::Oid])
            .update_columns([lfs_object::Column::Size])
            .to_owned(),
    )
    .exec(database)
    .await?;
    Ok(())
}

async fn save_progress(
    database: &DatabaseConnection,
    migration: lfs_storage_migration::Model,
    state: &str,
    phase: &str,
) -> Result<lfs_storage_migration::Model> {
    Ok(lfs_storage_migration::ActiveModel {
        id: sea_orm::Unchanged(migration.id),
        copied_objects: Set(migration.copied_objects),
        copied_bytes: Set(migration.copied_bytes),
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

async fn commit_cutover(
    database: &DatabaseConnection,
    migration: lfs_storage_migration::Model,
    source_id: Option<Uuid>,
    target_id: Option<Uuid>,
) -> Result<()> {
    let transaction = database.begin().await?;
    let current_state = lfs_storage_state::Entity::find_by_id(1)
        .one(&transaction)
        .await?
        .context("LFS storage state is missing")?;
    ensure!(
        current_state.active_target_id == source_id,
        "the active LFS storage target changed before cutover"
    );
    let mut objects = lfs_object::Entity::update_many();
    objects = objects.col_expr(
        lfs_object::Column::StorageTargetId,
        sea_orm::sea_query::Expr::value(target_id),
    );
    objects = match source_id {
        Some(source_id) => objects.filter(lfs_object::Column::StorageTargetId.eq(source_id)),
        None => objects.filter(lfs_object::Column::StorageTargetId.is_null()),
    };
    objects.exec(&transaction).await?;

    let mut state: lfs_storage_state::ActiveModel = lfs_storage_state::Entity::find_by_id(1)
        .one(&transaction)
        .await?
        .context("LFS storage state is missing")?
        .into();
    state.active_target_id = Set(target_id);
    state.updated_at = Set(Utc::now());
    state.update(&transaction).await?;

    let mut completed: lfs_storage_migration::ActiveModel = migration.into();
    completed.state = Set("completed".to_owned());
    completed.phase = Set("complete".to_owned());
    completed.error = Set(None);
    completed.updated_at = Set(Utc::now());
    completed.completed_at = Set(Some(Utc::now()));
    completed.update(&transaction).await?;
    transaction.commit().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        blob_store::{BlobStore as _, lfs_object_key},
        identity,
    };
    use sha2::{Digest as _, Sha256};

    #[tokio::test]
    async fn migration_copies_verifies_cuts_over_and_can_return_local() {
        let root =
            std::env::temp_dir().join(format!("gitadel-storage-migration-{}", Uuid::new_v4()));
        let mut settings = Settings::default();
        settings.database.url = format!("sqlite://{}?mode=rwc", root.join("gitadel.db").display());
        settings.storage.repository_root = root.join("repositories");
        settings.storage.lfs_root = root.join("lfs");
        settings.storage.actions_artifact_root = root.join("actions");
        tokio::fs::create_dir_all(&root).await.unwrap();
        let database = database::connect_and_migrate(&settings.database)
            .await
            .unwrap();
        let user = identity::bootstrap_admin(
            &database,
            "storage-test",
            "correct-horse-battery".to_owned(),
        )
        .await
        .unwrap();
        let repository_id = Uuid::new_v4();
        let storage_key = Uuid::new_v4();
        let now = Utc::now();
        repository::ActiveModel {
            id: Set(repository_id),
            namespace: Set(user.username.clone()),
            name: Set("repository".to_owned()),
            description: Set(None),
            website_url: Set(None),
            visibility: Set("private".to_owned()),
            object_format: Set("sha1".to_owned()),
            mirrored: Set(false),
            default_branch: Set(Some("main".to_owned())),
            issue_counter: Set(0),
            storage_key: Set(storage_key),
            created_by: Set(user.id),
            archived_at: Set(None),
            deleted_at: Set(None),
            icon_updated_at: Set(None),
            icon_source: Set(None),
            created_at: Set(now),
            updated_at: Set(now),
        }
        .insert(&database)
        .await
        .unwrap();

        let payload = b"migration payload";
        let digest = BlobDigest::from_bytes(Sha256::digest(payload).into());
        let key = lfs_object_key(storage_key, &digest.to_hex()).unwrap();
        let source = FilesystemBlobStore::new(settings.storage.lfs_root.clone())
            .await
            .unwrap();
        source
            .put_verified(
                &key,
                digest,
                Box::pin(std::io::Cursor::new(payload.as_slice())),
            )
            .await
            .unwrap();

        let target_path = root.join("secondary-lfs");
        let target = targets::create(
            &database,
            "Secondary".to_owned(),
            StorageTargetConfiguration::Filesystem {
                path: target_path.clone(),
            },
        )
        .await
        .unwrap();
        migrate(&database, &settings, target.id, 1, None)
            .await
            .unwrap();
        assert_eq!(
            lfs_storage_state::Entity::find_by_id(1)
                .one(&database)
                .await
                .unwrap()
                .unwrap()
                .active_target_id,
            Some(target.id)
        );
        let progress = lfs_storage_migration::Entity::find()
            .one(&database)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            (
                progress.copied_objects,
                progress.copied_bytes,
                progress.last_key.as_deref()
            ),
            (1, payload.len() as i64, Some(key.as_str()))
        );

        // Reported usage follows the objects: cutover reattributes the bytes to
        // the destination, and the configured local path goes back to nothing.
        let views = targets::list(&database, &settings.storage.lfs_root, &Default::default())
            .await
            .unwrap();
        let usage_of = |id: Uuid| views.iter().find(|view| view.id == id).unwrap().usage;
        assert_eq!(
            (
                usage_of(target.id).lfs_object_count,
                usage_of(target.id).lfs_bytes
            ),
            (1, payload.len() as u64)
        );
        assert_eq!(usage_of(Uuid::nil()).lfs_bytes, 0);
        assert!(
            views.iter().all(|view| view
                .capacity
                .is_some_and(|capacity| capacity.total_bytes > 0)),
            "filesystem targets should report the volume they sit on"
        );

        // Scanning reads the destination itself, and leaves out the ownership
        // marker Gitadel wrote when it claimed the target.
        let measured = targets::measure(&StorageTargetConfiguration::Filesystem {
            path: target_path.clone(),
        })
        .await
        .unwrap();
        assert_eq!(
            (measured.object_count, measured.total_bytes),
            (1, payload.len() as u64)
        );

        let destination = FilesystemBlobStore::new(target_path).await.unwrap();
        assert_eq!(
            destination.stat(&key).await.unwrap().unwrap().size,
            payload.len() as u64
        );
        assert!(source.stat(&key).await.unwrap().is_some());

        migrate(&database, &settings, Uuid::nil(), 1, None)
            .await
            .unwrap();
        assert_eq!(
            lfs_storage_state::Entity::find_by_id(1)
                .one(&database)
                .await
                .unwrap()
                .unwrap()
                .active_target_id,
            None
        );
        targets::delete(&database, target.id).await.unwrap();
        assert!(
            lfs_storage_target::Entity::find_by_id(target.id)
                .one(&database)
                .await
                .unwrap()
                .is_none()
        );
        database.close().await.unwrap();
        tokio::fs::remove_dir_all(root).await.unwrap();
    }
}
