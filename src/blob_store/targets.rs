use std::{path::PathBuf, sync::Arc};

use anyhow::{Context, Result, bail, ensure};
use chrono::Utc;
use sea_orm::{
    ActiveModelTrait as _, ConnectionTrait as _, DatabaseConnection, EntityTrait as _, Set,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use tokio::io::AsyncReadExt as _;
use uuid::Uuid;

use crate::{
    config::S3Settings,
    entity::{lfs_storage_state, lfs_storage_target},
};

use super::{BlobDigest, BlobStore, FilesystemBlobStore, ObjectKey, ObjectPrefix, S3BlobStore};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum StorageTargetConfiguration {
    Filesystem { path: PathBuf },
    S3 { s3: S3Settings },
}

impl StorageTargetConfiguration {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Filesystem { .. } => "filesystem",
            Self::S3 { .. } => "s3",
        }
    }

    pub async fn open(&self) -> Result<Arc<dyn BlobStore>> {
        match self {
            Self::Filesystem { path } => {
                Ok(Arc::new(FilesystemBlobStore::new(path.clone()).await?))
            }
            Self::S3 { s3 } => Ok(Arc::new(S3BlobStore::new(s3)?)),
        }
    }

    pub fn redacted(&self) -> serde_json::Value {
        match self {
            Self::Filesystem { path } => serde_json::json!({
                "kind": "filesystem",
                "path": path,
            }),
            Self::S3 { s3 } => serde_json::json!({
                "kind": "s3",
                "s3": {
                    "endpoint": s3.endpoint,
                    "bucket": s3.bucket,
                    "access_key": if s3.access_key.is_empty() { "" } else { "configured" },
                    "secret_key": if s3.secret_key.is_empty() { "" } else { "configured" },
                    "region": s3.region,
                    "prefix": s3.prefix,
                }
            }),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct StorageTargetView {
    pub id: Uuid,
    pub name: String,
    pub kind: String,
    pub configuration: serde_json::Value,
    pub active: bool,
    pub managed_by_config: bool,
    pub capacity: Option<StorageTargetCapacity>,
    pub usage: StorageTargetUsage,
}

/// What the underlying volume holds, for targets that sit on one. Object stores
/// publish no capacity, so S3 targets leave this empty rather than inventing a
/// ceiling.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct StorageTargetCapacity {
    pub total_bytes: u64,
    pub available_bytes: u64,
}

#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct StorageTargetUsage {
    /// LFS bytes the database attributes to this target. Free to read on every
    /// request, but it only counts objects Gitadel has a record of.
    pub lfs_object_count: u64,
    pub lfs_bytes: u64,
    /// Everything actually stored at the destination, from the last scan. Costs
    /// a full listing, so it is only ever filled in on request.
    pub measured: Option<MeasuredUsage>,
}

#[derive(Clone, Copy, Debug, Serialize)]
pub struct MeasuredUsage {
    pub object_count: u64,
    pub total_bytes: u64,
    pub measured_at: chrono::DateTime<Utc>,
}

/// `fs2` reports on the mount a path belongs to, so two targets on one disk
/// report identical figures, and a path that does not exist yet reports none.
fn filesystem_capacity(path: &std::path::Path) -> Option<StorageTargetCapacity> {
    Some(StorageTargetCapacity {
        total_bytes: fs2::total_space(path).ok()?,
        available_bytes: fs2::available_space(path).ok()?,
    })
}

/// LFS bytes per target in one pass. Objects written before any target existed
/// carry no target id and belong to the configured local path, which `list`
/// surfaces under the nil id.
async fn lfs_usage_by_target(
    database: &DatabaseConnection,
) -> Result<std::collections::HashMap<Uuid, (u64, u64)>> {
    let rows = database
        .query_all_raw(sea_orm::Statement::from_string(
            database.get_database_backend(),
            "SELECT storage_target_id, COUNT(*) AS object_count, COALESCE(SUM(size), 0) AS total_bytes \
             FROM lfs_objects GROUP BY storage_target_id",
        ))
        .await?;
    let mut usage = std::collections::HashMap::with_capacity(rows.len());
    for row in rows {
        let target_id = row
            .try_get::<Option<Uuid>>("", "storage_target_id")?
            .unwrap_or_else(Uuid::nil);
        let object_count = row.try_get::<i64>("", "object_count")?;
        let total_bytes = row.try_get::<i64>("", "total_bytes")?;
        usage.insert(
            target_id,
            (
                u64::try_from(object_count).unwrap_or(0),
                u64::try_from(total_bytes).unwrap_or(0),
            ),
        );
    }
    Ok(usage)
}

/// Sums everything the destination actually holds. This is the only figure that
/// catches objects the database has no record of, and the only usable one for
/// S3, where the bucket reports no capacity to measure against.
pub async fn measure(configuration: &StorageTargetConfiguration) -> Result<MeasuredUsage> {
    let store = configuration.open().await?;
    let mut object_count = 0;
    let mut total_bytes = 0;
    for object in store.list(&ObjectPrefix::new("")?).await? {
        // The ownership marker is Gitadel's bookkeeping, not stored content.
        if object.key.as_str().starts_with(".gitadel/") {
            continue;
        }
        object_count += 1;
        total_bytes += object.size;
    }
    Ok(MeasuredUsage {
        object_count,
        total_bytes,
        measured_at: Utc::now(),
    })
}

pub struct ActiveBlobStore {
    pub store: Arc<dyn BlobStore>,
    pub target_id: Option<Uuid>,
}

pub async fn load_active(
    database: &DatabaseConnection,
    fallback_path: PathBuf,
) -> Result<ActiveBlobStore> {
    let state = lfs_storage_state::Entity::find_by_id(1)
        .one(database)
        .await?
        .context("LFS storage state is missing")?;
    let Some(target_id) = state.active_target_id else {
        return Ok(ActiveBlobStore {
            store: Arc::new(FilesystemBlobStore::new(fallback_path).await?),
            target_id: None,
        });
    };
    let target = lfs_storage_target::Entity::find_by_id(target_id)
        .one(database)
        .await?
        .context("active LFS storage target is missing")?;
    let configuration: StorageTargetConfiguration = serde_json::from_str(&target.configuration)
        .context("active LFS storage target configuration is invalid")?;
    let store = configuration.open().await?;
    verify_ownership(store.as_ref(), target_id).await?;
    Ok(ActiveBlobStore {
        store,
        target_id: Some(target_id),
    })
}

pub async fn list(
    database: &DatabaseConnection,
    fallback_path: &std::path::Path,
    measured: &std::collections::HashMap<Uuid, MeasuredUsage>,
) -> Result<Vec<StorageTargetView>> {
    let active = lfs_storage_state::Entity::find_by_id(1)
        .one(database)
        .await?
        .context("LFS storage state is missing")?
        .active_target_id;
    let targets = lfs_storage_target::Entity::find().all(database).await?;
    let lfs_usage = lfs_usage_by_target(database).await?;
    let usage_of = |id: Uuid| {
        let (lfs_object_count, lfs_bytes) = lfs_usage.get(&id).copied().unwrap_or((0, 0));
        StorageTargetUsage {
            lfs_object_count,
            lfs_bytes,
            measured: measured.get(&id).copied(),
        }
    };
    let mut views = Vec::with_capacity(targets.len() + 1);
    views.push(StorageTargetView {
        id: Uuid::nil(),
        name: "Configured local storage".to_owned(),
        kind: "filesystem".to_owned(),
        configuration: serde_json::json!({
            "kind": "filesystem",
            "path": fallback_path,
        }),
        active: active.is_none(),
        managed_by_config: true,
        capacity: filesystem_capacity(fallback_path),
        usage: usage_of(Uuid::nil()),
    });
    for target in targets {
        let configuration: StorageTargetConfiguration = serde_json::from_str(&target.configuration)
            .context("stored LFS target configuration is invalid")?;
        let capacity = match &configuration {
            StorageTargetConfiguration::Filesystem { path } => filesystem_capacity(path),
            StorageTargetConfiguration::S3 { .. } => None,
        };
        views.push(StorageTargetView {
            id: target.id,
            name: target.name,
            kind: target.kind,
            configuration: configuration.redacted(),
            active: active == Some(target.id),
            managed_by_config: false,
            capacity,
            usage: usage_of(target.id),
        });
    }
    Ok(views)
}

pub async fn find(
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

pub async fn create(
    database: &DatabaseConnection,
    name: String,
    configuration: StorageTargetConfiguration,
) -> Result<StorageTargetView> {
    if name.trim().is_empty() {
        bail!("storage target name cannot be empty");
    }
    let id = Uuid::new_v4();
    let store = configuration.open().await?;
    if !store.list(&ObjectPrefix::new("")?).await?.is_empty() {
        bail!("storage target must be empty before Gitadel claims it");
    }
    test_store(store.as_ref()).await?;
    write_ownership_marker(store.as_ref(), id).await?;
    let now = Utc::now();
    let model = lfs_storage_target::ActiveModel {
        id: Set(id),
        name: Set(name.trim().to_owned()),
        kind: Set(configuration.kind().to_owned()),
        configuration: Set(serde_json::to_string(&configuration)?),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(database)
    .await;
    let model = match model {
        Ok(model) => model,
        Err(error) => {
            if let Ok((key, _)) = ownership_marker(id) {
                let _ = store.delete(&key).await;
            }
            return Err(error.into());
        }
    };
    let capacity = match &configuration {
        StorageTargetConfiguration::Filesystem { path } => filesystem_capacity(path),
        StorageTargetConfiguration::S3 { .. } => None,
    };
    Ok(StorageTargetView {
        id,
        name: model.name,
        kind: model.kind,
        configuration: configuration.redacted(),
        active: false,
        managed_by_config: false,
        capacity,
        // A target has to be empty before Gitadel claims it, so there is nothing
        // to report and nothing to scan for.
        usage: StorageTargetUsage::default(),
    })
}

pub async fn test_configuration(configuration: &StorageTargetConfiguration) -> Result<()> {
    let store = configuration.open().await?;
    test_store(store.as_ref()).await
}

async fn test_store(store: &dyn BlobStore) -> Result<()> {
    let payload = format!("gitadel-storage-test-{}", Uuid::new_v4());
    let digest = BlobDigest::from_bytes(Sha256::digest(payload.as_bytes()).into());
    let key = ObjectKey::new(format!(
        ".gitadel/test/{}/{}",
        Uuid::new_v4(),
        digest.to_hex()
    ))?;
    store
        .put_verified(
            &key,
            digest,
            Box::pin(std::io::Cursor::new(payload.as_bytes().to_vec())),
        )
        .await?;
    let result = async {
        let metadata = store
            .stat(&key)
            .await?
            .context("storage test object is missing")?;
        if metadata.size != payload.len() as u64 {
            bail!("storage test object size mismatch");
        }
        let mut reader = store.read(&key).await?;
        let mut actual = Vec::new();
        reader.read_to_end(&mut actual).await?;
        if actual != payload.as_bytes() {
            bail!("storage test object content mismatch");
        }
        Ok(())
    }
    .await;
    let delete_result = store.delete(&key).await;
    result.and(delete_result)
}

pub async fn verify_ownership(store: &dyn BlobStore, target_id: Uuid) -> Result<()> {
    let (key, expected) = ownership_marker(target_id)?;
    let mut reader = store
        .read(&key)
        .await
        .context("storage target ownership marker is missing")?;
    let mut actual = Vec::with_capacity(expected.len());
    reader.read_to_end(&mut actual).await?;
    ensure!(
        actual == expected,
        "storage target ownership marker does not match"
    );
    Ok(())
}

async fn write_ownership_marker(store: &dyn BlobStore, target_id: Uuid) -> Result<()> {
    let (key, payload) = ownership_marker(target_id)?;
    let digest = BlobDigest::from_bytes(Sha256::digest(&payload).into());
    store
        .put_verified(&key, digest, Box::pin(std::io::Cursor::new(payload)))
        .await?;
    Ok(())
}
pub async fn delete_prefix_from_all(
    database: &DatabaseConnection,
    fallback_path: PathBuf,
    prefix: &ObjectPrefix,
) -> Result<()> {
    let local = FilesystemBlobStore::new(fallback_path).await?;
    delete_prefix(&local, prefix).await;
    for target in lfs_storage_target::Entity::find().all(database).await? {
        let configuration: StorageTargetConfiguration = match serde_json::from_str(
            &target.configuration,
        ) {
            Ok(configuration) => configuration,
            Err(error) => {
                tracing::error!(%error, target_id = %target.id, "could not decode LFS target during cleanup");
                continue;
            }
        };
        let store = match configuration.open().await {
            Ok(store) => store,
            Err(error) => {
                tracing::error!(%error, target_id = %target.id, "could not open LFS target during cleanup");
                continue;
            }
        };
        if let Err(error) = verify_ownership(store.as_ref(), target.id).await {
            tracing::error!(%error, target_id = %target.id, "refused to clean an unowned LFS target");
            continue;
        }
        delete_prefix(store.as_ref(), prefix).await;
    }
    Ok(())
}

async fn delete_prefix(store: &dyn BlobStore, prefix: &ObjectPrefix) {
    let objects = match store.list(prefix).await {
        Ok(objects) => objects,
        Err(error) => {
            tracing::error!(%error, "could not list repository LFS objects for cleanup");
            return;
        }
    };
    for object in objects {
        if let Err(error) = store.delete(&object.key).await {
            tracing::error!(%error, key = %object.key, "could not clean up repository LFS object");
        }
    }
}

fn ownership_marker(target_id: Uuid) -> Result<(ObjectKey, Vec<u8>)> {
    let payload = format!("gitadel-lfs-target:{target_id}").into_bytes();
    let digest = BlobDigest::from_bytes(Sha256::digest(&payload).into());
    let key = ObjectKey::new(format!(".gitadel/target/{target_id}/{}", digest.to_hex()))?;
    Ok((key, payload))
}

pub async fn delete(database: &DatabaseConnection, target_id: Uuid) -> Result<()> {
    let active = lfs_storage_state::Entity::find_by_id(1)
        .one(database)
        .await?
        .context("LFS storage state is missing")?;
    if active.active_target_id == Some(target_id) {
        bail!("the active LFS storage target cannot be deleted");
    }
    let result = lfs_storage_target::Entity::delete_by_id(target_id)
        .exec(database)
        .await?;
    ensure!(
        result.rows_affected == 1,
        "LFS storage target does not exist"
    );
    Ok(())
}
