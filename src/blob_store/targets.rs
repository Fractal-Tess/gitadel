use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context, Result, bail, ensure};
use chrono::Utc;
use sea_orm::{
    ActiveModelTrait as _, ColumnTrait as _, Condition, ConnectionTrait as _, DatabaseConnection,
    EntityTrait as _, QueryFilter as _, Set,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use tokio::io::AsyncReadExt as _;
use uuid::Uuid;

use crate::{
    config::S3Settings,
    entity::{storage_domain_state, storage_migration, storage_target},
    registry::storage::RegistryDomain,
};

use super::{
    BlobDigest, BlobStore, FilesystemBlobStore, ObjectKey, ObjectPrefix, S3BlobStore,
    manager::{self, active_target_id},
};

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
    pub registry_active: bool,
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
    pub lfs_object_count: u64,
    pub lfs_bytes: u64,
    pub registry_object_count: u64,
    pub registry_bytes: u64,
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

pub async fn list(
    database: &DatabaseConnection,
    fallback_path: &std::path::Path,
    measured: &std::collections::HashMap<Uuid, MeasuredUsage>,
) -> Result<Vec<StorageTargetView>> {
    let active = active_target_id(database, "lfs").await?;
    let registry_active = active_target_id(database, "registry").await?;
    let targets = storage_target::Entity::find().all(database).await?;
    let lfs_usage = lfs_usage_by_target(database).await?;
    let mut registry_object_count = 0;
    let mut registry_bytes = 0_u64;
    if let Some(id) = registry_active {
        let (_, configuration) = find(database, id).await?;
        let store = configuration.open().await?;
        verify_ownership(store.as_ref(), id).await?;
        let objects = manager::inventory(database, &RegistryDomain, store.as_ref()).await?;
        registry_object_count = objects.len() as u64;
        for object in objects {
            registry_bytes = registry_bytes
                .checked_add(object.size)
                .context("registry usage overflow")?;
        }
    }
    let usage_of = |id: Uuid| {
        let (lfs_object_count, lfs_bytes) = lfs_usage.get(&id).copied().unwrap_or((0, 0));
        StorageTargetUsage {
            lfs_object_count,
            lfs_bytes,
            registry_object_count: if registry_active == Some(id) {
                registry_object_count
            } else {
                0
            },
            registry_bytes: if registry_active == Some(id) {
                registry_bytes
            } else {
                0
            },
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
        registry_active: false,
        managed_by_config: true,
        capacity: filesystem_capacity(fallback_path),
        usage: usage_of(Uuid::nil()),
    });
    for target in targets {
        let configuration: StorageTargetConfiguration = serde_json::from_str(&target.configuration)
            .context("stored storage target configuration is invalid")?;
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
            registry_active: registry_active == Some(target.id),
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
) -> Result<(storage_target::Model, StorageTargetConfiguration)> {
    let target = storage_target::Entity::find_by_id(id)
        .one(database)
        .await?
        .context("storage target does not exist")?;
    let configuration = serde_json::from_str(&target.configuration)
        .context("stored storage target configuration is invalid")?;
    Ok((target, configuration))
}

/// Whether `path` is missing or an empty directory. The blob store's own
/// listing only sees digest-named objects, so a directory holding unrelated
/// files would otherwise be claimed and marked as Gitadel's.
async fn directory_is_empty(path: &Path) -> Result<bool> {
    let mut entries = match tokio::fs::read_dir(path).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(true),
        Err(error) => {
            return Err(error).with_context(|| format!("could not read {}", path.display()));
        }
    };
    Ok(entries
        .next_entry()
        .await
        .with_context(|| format!("could not read {}", path.display()))?
        .is_none())
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
    if let StorageTargetConfiguration::Filesystem { path } = &configuration
        && !directory_is_empty(path).await?
    {
        bail!("storage target must be empty before Gitadel claims it");
    }
    let store = configuration.open().await?;
    if !store.list(&ObjectPrefix::new("")?).await?.is_empty() {
        bail!("storage target must be empty before Gitadel claims it");
    }
    test_store(store.as_ref()).await?;
    write_ownership_marker(store.as_ref(), id).await?;
    let now = Utc::now();
    let model = storage_target::ActiveModel {
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
        registry_active: false,
        managed_by_config: false,
        capacity,
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

/// Deletes `prefix` from every owned storage target. Local roots belong to
/// their domain, which cleans them itself.
pub async fn delete_prefix_from_targets(
    database: &DatabaseConnection,
    prefix: &ObjectPrefix,
    label: &str,
) -> Result<()> {
    for target in storage_target::Entity::find().all(database).await? {
        let configuration: StorageTargetConfiguration = match serde_json::from_str(
            &target.configuration,
        ) {
            Ok(configuration) => configuration,
            Err(error) => {
                tracing::error!(%error, target_id = %target.id, "could not decode storage target during {label} cleanup");
                continue;
            }
        };
        let store = match configuration.open().await {
            Ok(store) => store,
            Err(error) => {
                tracing::error!(%error, target_id = %target.id, "could not open storage target during {label} cleanup");
                continue;
            }
        };
        if let Err(error) = verify_ownership(store.as_ref(), target.id).await {
            tracing::error!(%error, target_id = %target.id, "refused to clean an unowned storage target");
            continue;
        }
        delete_prefix(store.as_ref(), prefix, label).await;
    }
    Ok(())
}

pub(super) async fn delete_prefix(store: &dyn BlobStore, prefix: &ObjectPrefix, label: &str) {
    let objects = match store.list(prefix).await {
        Ok(objects) => objects,
        Err(error) => {
            tracing::error!(%error, "could not list repository {label} objects for cleanup");
            return;
        }
    };
    for object in objects {
        if let Err(error) = store.delete(&object.key).await {
            tracing::error!(%error, key = %object.key, "could not clean up repository {label} object");
        }
    }
}

fn ownership_marker(target_id: Uuid) -> Result<(ObjectKey, Vec<u8>)> {
    // The marker predates shared targets; its wording is part of the on-disk format.
    let payload = format!("gitadel-lfs-target:{target_id}").into_bytes();
    let digest = BlobDigest::from_bytes(Sha256::digest(&payload).into());
    let key = ObjectKey::new(format!(".gitadel/target/{target_id}/{}", digest.to_hex()))?;
    Ok((key, payload))
}

/// Display name of a persisted domain identifier.
fn domain_label(domain: &str) -> &str {
    match domain {
        "lfs" => "LFS",
        other => other,
    }
}

pub async fn delete(database: &DatabaseConnection, target_id: Uuid) -> Result<()> {
    if let Some(state) = storage_domain_state::Entity::find()
        .filter(storage_domain_state::Column::ActiveTargetId.eq(target_id))
        .one(database)
        .await?
    {
        bail!(
            "the active {} storage target cannot be deleted",
            domain_label(&state.domain)
        );
    }
    let migration = storage_migration::Entity::find()
        .filter(storage_migration::Column::State.ne(manager::state::COMPLETED))
        .filter(storage_migration::Column::State.ne(manager::state::FAILED))
        .filter(
            Condition::any()
                .add(storage_migration::Column::SourceTargetId.eq(target_id))
                .add(storage_migration::Column::TargetId.eq(target_id)),
        )
        .one(database)
        .await?;
    if let Some(migration) = migration {
        bail!(
            "a target used by an in-progress {} migration cannot be deleted",
            domain_label(&migration.domain)
        );
    }
    let result = storage_target::Entity::delete_by_id(target_id)
        .exec(database)
        .await?;
    ensure!(result.rows_affected == 1, "storage target does not exist");
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, path::PathBuf};

    use chrono::Utc;
    use sea_orm::{
        ActiveModelTrait as _, DatabaseConnection, EntityTrait as _, IntoActiveModel as _, Set,
    };
    use sha2::{Digest as _, Sha256};
    use uuid::Uuid;

    use super::{
        MeasuredUsage, StorageTargetConfiguration, create, delete, delete_prefix_from_targets,
        domain_label, find, list, measure, ownership_marker, test_configuration, test_store,
        verify_ownership, write_ownership_marker,
    };
    use crate::{
        actions::test_support,
        blob_store::{
            BlobDigest, BlobStore, FilesystemBlobStore, ObjectKey, ObjectPrefix, manager,
        },
        config::S3Settings,
        entity::{lfs_object, storage_domain_state, storage_migration},
    };

    struct Scratch {
        root: PathBuf,
    }

    impl Scratch {
        fn new() -> Self {
            let root =
                std::env::temp_dir().join(format!("gitadel-storage-targets-{}", Uuid::new_v4()));
            std::fs::create_dir_all(&root).unwrap();
            Self { root }
        }

        /// A fresh, empty directory under the scratch root.
        fn directory(&self, name: &str) -> PathBuf {
            let path = self.root.join(name);
            std::fs::create_dir_all(&path).unwrap();
            path
        }

        fn filesystem(&self, name: &str) -> StorageTargetConfiguration {
            StorageTargetConfiguration::Filesystem {
                path: self.directory(name),
            }
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn s3(access_key: &str, secret_key: &str) -> StorageTargetConfiguration {
        StorageTargetConfiguration::S3 {
            s3: S3Settings {
                endpoint: url::Url::parse("https://s3.example.test").unwrap(),
                bucket: "gitadel".to_owned(),
                access_key: access_key.to_owned(),
                secret_key: secret_key.to_owned(),
                region: "eu-central-1".to_owned(),
                prefix: "lfs".to_owned(),
            },
        }
    }

    /// Stores `content` under `directory`, named by its digest as every
    /// Gitadel object is, and returns the key.
    async fn put(store: &dyn BlobStore, directory: &str, content: &[u8]) -> String {
        let digest = BlobDigest::from_bytes(Sha256::digest(content).into());
        let key = format!("{directory}/{}", digest.to_hex());
        store
            .put_verified(
                &ObjectKey::new(key.clone()).unwrap(),
                digest,
                Box::pin(std::io::Cursor::new(content.to_vec())),
            )
            .await
            .unwrap();
        key
    }

    async fn keys(store: &dyn BlobStore, prefix: &str) -> Vec<String> {
        let mut keys = store
            .list(&ObjectPrefix::new(prefix).unwrap())
            .await
            .unwrap()
            .into_iter()
            .map(|object| object.key.as_str().to_owned())
            .collect::<Vec<_>>();
        keys.sort();
        keys
    }

    async fn activate(database: &DatabaseConnection, domain: &str, target: Option<Uuid>) {
        let mut state = storage_domain_state::Entity::find_by_id(domain)
            .one(database)
            .await
            .unwrap()
            .unwrap()
            .into_active_model();
        state.active_target_id = Set(target);
        state.updated_at = Set(Utc::now());
        state.update(database).await.unwrap();
    }

    #[test]
    fn configurations_round_trip_and_redact_credentials() {
        let filesystem = StorageTargetConfiguration::Filesystem {
            path: PathBuf::from("/var/lib/gitadel/lfs"),
        };
        assert_eq!(filesystem.kind(), "filesystem");
        assert_eq!(
            filesystem.redacted(),
            serde_json::json!({ "kind": "filesystem", "path": "/var/lib/gitadel/lfs" })
        );
        let encoded = serde_json::to_string(&filesystem).unwrap();
        assert_eq!(
            encoded,
            r#"{"kind":"filesystem","path":"/var/lib/gitadel/lfs"}"#
        );
        let decoded: StorageTargetConfiguration = serde_json::from_str(&encoded).unwrap();
        assert!(matches!(
            decoded,
            StorageTargetConfiguration::Filesystem { path } if path == std::path::Path::new("/var/lib/gitadel/lfs")
        ));

        let configured = s3("AKIA", "hunter2");
        assert_eq!(configured.kind(), "s3");
        let redacted = configured.redacted();
        assert_eq!(redacted["kind"], "s3");
        assert_eq!(redacted["s3"]["endpoint"], "https://s3.example.test/");
        assert_eq!(redacted["s3"]["bucket"], "gitadel");
        assert_eq!(redacted["s3"]["access_key"], "configured");
        assert_eq!(redacted["s3"]["secret_key"], "configured");
        assert_eq!(redacted["s3"]["region"], "eu-central-1");
        assert_eq!(redacted["s3"]["prefix"], "lfs");
        let shown = redacted.to_string();
        assert!(
            !shown.contains("hunter2") && !shown.contains("AKIA"),
            "{shown}"
        );
        // The stored form keeps the credentials so the target can be opened.
        let stored = serde_json::to_string(&configured).unwrap();
        assert!(stored.contains("hunter2"));
        let decoded: StorageTargetConfiguration = serde_json::from_str(&stored).unwrap();
        assert!(
            matches!(decoded, StorageTargetConfiguration::S3 { s3 } if s3.secret_key == "hunter2")
        );

        let empty = s3("", "").redacted();
        assert_eq!(empty["s3"]["access_key"], "");
        assert_eq!(empty["s3"]["secret_key"], "");

        assert!(serde_json::from_str::<StorageTargetConfiguration>(r#"{"kind":"ftp"}"#).is_err());
    }

    #[test]
    fn ownership_markers_are_fixed_per_target() {
        let id = Uuid::new_v4();
        let (key, payload) = ownership_marker(id).unwrap();
        assert_eq!(payload, format!("gitadel-lfs-target:{id}").into_bytes());
        let digest = BlobDigest::from_bytes(Sha256::digest(&payload).into()).to_hex();
        assert_eq!(key.as_str(), format!(".gitadel/target/{id}/{digest}"));
        assert_eq!(ownership_marker(id).unwrap(), (key, payload));
        assert_ne!(ownership_marker(Uuid::new_v4()).unwrap().0.as_str(), {
            let (key, _) = ownership_marker(id).unwrap();
            key.as_str().to_owned()
        });

        assert_eq!(domain_label("lfs"), "LFS");
        assert_eq!(domain_label("registry"), "registry");
    }

    #[tokio::test]
    async fn the_storage_test_leaves_nothing_behind() {
        let scratch = Scratch::new();
        let store = FilesystemBlobStore::new(scratch.directory("probe"))
            .await
            .unwrap();
        let kept = put(&store, "kept", b"kept").await;

        test_store(&store).await.unwrap();
        test_configuration(&scratch.filesystem("probe"))
            .await
            .unwrap();

        assert_eq!(keys(&store, "").await, [kept]);
    }

    #[tokio::test]
    async fn create_claims_only_a_named_empty_destination() {
        let scratch = Scratch::new();
        let database = test_support::database().await;

        let error = create(&database, "   ".to_owned(), scratch.filesystem("unnamed"))
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), "storage target name cannot be empty");

        let occupied = scratch.directory("occupied");
        let foreign = FilesystemBlobStore::new(occupied.clone()).await.unwrap();
        put(&foreign, "existing", b"not ours").await;
        let error = create(
            &database,
            "Occupied".to_owned(),
            StorageTargetConfiguration::Filesystem {
                path: occupied.clone(),
            },
        )
        .await
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            "storage target must be empty before Gitadel claims it"
        );
        assert_eq!(
            std::fs::read_dir(&occupied).unwrap().count(),
            1,
            "a refused destination is not written to"
        );
        assert!(find(&database, Uuid::new_v4()).await.is_err());

        let view = create(
            &database,
            "  Vault  ".to_owned(),
            scratch.filesystem("vault"),
        )
        .await
        .unwrap();
        assert_eq!(view.name, "Vault");
        assert_eq!(view.kind, "filesystem");
        assert!(!view.active && !view.registry_active && !view.managed_by_config);
        assert!(view.capacity.is_some());
        assert_eq!(view.usage.lfs_object_count, 0);
        assert_eq!(
            view.configuration["path"],
            scratch.root.join("vault").to_str().unwrap()
        );

        let (model, configuration) = find(&database, view.id).await.unwrap();
        assert_eq!(model.name, "Vault");
        let store = configuration.open().await.unwrap();
        verify_ownership(store.as_ref(), view.id).await.unwrap();
        let error = verify_ownership(store.as_ref(), Uuid::new_v4())
            .await
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "storage target ownership marker is missing"
        );
        // The marker is the only thing a fresh target holds.
        let (marker, _) = ownership_marker(view.id).unwrap();
        assert_eq!(keys(store.as_ref(), "").await, [marker.as_str()]);
    }

    #[tokio::test]
    async fn delete_refuses_active_and_migrating_targets() {
        let scratch = Scratch::new();
        let database = test_support::database().await;
        let first = create(&database, "First".to_owned(), scratch.filesystem("first"))
            .await
            .unwrap()
            .id;
        let second = create(&database, "Second".to_owned(), scratch.filesystem("second"))
            .await
            .unwrap()
            .id;

        activate(&database, "lfs", Some(first)).await;
        activate(&database, "registry", Some(second)).await;
        assert_eq!(
            delete(&database, first).await.unwrap_err().to_string(),
            "the active LFS storage target cannot be deleted"
        );
        assert_eq!(
            delete(&database, second).await.unwrap_err().to_string(),
            "the active registry storage target cannot be deleted"
        );
        activate(&database, "lfs", None).await;
        activate(&database, "registry", None).await;

        let now = Utc::now();
        let migration = storage_migration::ActiveModel {
            id: Set(Uuid::new_v4()),
            domain: Set("lfs".to_owned()),
            source_target_id: Set(Some(first)),
            target_id: Set(Some(second)),
            state: Set(manager::state::COPYING.to_owned()),
            phase: Set(manager::phase::SCHEDULED.to_owned()),
            last_key: Set(None),
            copied_objects: Set(0),
            copied_bytes: Set(0),
            total_bytes: Set(None),
            error: Set(None),
            started_at: Set(now),
            updated_at: Set(now),
            completed_at: Set(None),
        }
        .insert(&database)
        .await
        .unwrap();
        for target in [first, second] {
            assert_eq!(
                delete(&database, target).await.unwrap_err().to_string(),
                "a target used by an in-progress LFS migration cannot be deleted"
            );
        }
        for finished in [manager::state::FAILED, manager::state::COMPLETED] {
            let mut active = migration.clone().into_active_model();
            active.state = Set(finished.to_owned());
            active.update(&database).await.unwrap();
        }

        delete(&database, first).await.unwrap();
        assert!(find(&database, first).await.is_err());
        assert_eq!(
            delete(&database, first).await.unwrap_err().to_string(),
            "storage target does not exist"
        );
        assert!(find(&database, second).await.is_ok());
    }

    #[tokio::test]
    async fn list_reports_local_storage_first_with_usage_per_target() {
        let scratch = Scratch::new();
        let database = test_support::database().await;
        let owner = test_support::owner(&database, "alice").await;
        let repository = test_support::repository(&database, &owner, "project").await;
        let target = create(&database, "Vault".to_owned(), scratch.filesystem("vault"))
            .await
            .unwrap()
            .id;
        for (oid, size, target_id) in [
            ("a".repeat(64), 10, Some(target)),
            ("b".repeat(64), 20, Some(target)),
            ("c".repeat(64), 5, None),
        ] {
            lfs_object::ActiveModel {
                repository_id: Set(repository.id),
                oid: Set(oid),
                size: Set(size),
                storage_target_id: Set(target_id),
                created_at: Set(Utc::now()),
            }
            .insert(&database)
            .await
            .unwrap();
        }
        let measured = MeasuredUsage {
            object_count: 3,
            total_bytes: 31,
            measured_at: Utc::now(),
        };
        let local = scratch.directory("local");

        let views = list(&database, &local, &HashMap::from([(target, measured)]))
            .await
            .unwrap();

        assert_eq!(views.len(), 2);
        let fallback = &views[0];
        assert_eq!(fallback.id, Uuid::nil());
        assert_eq!(fallback.name, "Configured local storage");
        assert!(fallback.active && fallback.managed_by_config && !fallback.registry_active);
        assert_eq!(fallback.configuration["path"], local.to_str().unwrap());
        assert!(fallback.capacity.is_some());
        assert_eq!(fallback.usage.lfs_object_count, 1);
        assert_eq!(fallback.usage.lfs_bytes, 5);
        assert!(fallback.usage.measured.is_none());
        let vault = &views[1];
        assert_eq!(vault.id, target);
        assert!(!vault.active && !vault.managed_by_config);
        assert_eq!(vault.usage.lfs_object_count, 2);
        assert_eq!(vault.usage.lfs_bytes, 30);
        assert_eq!(
            vault.usage.measured.map(|usage| usage.total_bytes),
            Some(31)
        );

        activate(&database, "lfs", Some(target)).await;
        let views = list(&database, &local, &HashMap::new()).await.unwrap();
        assert!(!views[0].active);
        assert!(views[1].active);
        assert!(views[1].usage.measured.is_none());
    }

    #[tokio::test]
    async fn measure_counts_content_but_not_bookkeeping() {
        let scratch = Scratch::new();
        let configuration = scratch.filesystem("measured");
        let store = configuration.open().await.unwrap();
        put(store.as_ref(), "repo/ab/cd", b"four").await;
        put(store.as_ref(), "repo/ef/gh", b"eleven bytes").await;
        write_ownership_marker(store.as_ref(), Uuid::new_v4())
            .await
            .unwrap();

        let usage = measure(&configuration).await.unwrap();

        assert_eq!(usage.object_count, 2);
        assert_eq!(usage.total_bytes, 16);
    }

    #[tokio::test]
    async fn prefix_cleanup_touches_only_owned_targets() {
        let scratch = Scratch::new();
        let database = test_support::database().await;
        let owned = create(&database, "Owned".to_owned(), scratch.filesystem("owned"))
            .await
            .unwrap()
            .id;
        let hijacked = create(
            &database,
            "Hijacked".to_owned(),
            scratch.filesystem("hijacked"),
        )
        .await
        .unwrap()
        .id;
        let owned_store = find(&database, owned)
            .await
            .unwrap()
            .1
            .open()
            .await
            .unwrap();
        let hijacked_store = find(&database, hijacked)
            .await
            .unwrap()
            .1
            .open()
            .await
            .unwrap();
        let mut cleaned = String::new();
        let mut kept = String::new();
        for store in [owned_store.as_ref(), hijacked_store.as_ref()] {
            cleaned = put(store, "repo", b"content").await;
            kept = put(store, "other", b"content").await;
        }
        // The second destination now belongs to somebody else.
        let (marker, _) = ownership_marker(hijacked).unwrap();
        hijacked_store.delete(&marker).await.unwrap();

        delete_prefix_from_targets(&database, &ObjectPrefix::new("repo").unwrap(), "LFS")
            .await
            .unwrap();

        assert_eq!(
            keys(owned_store.as_ref(), "repo").await,
            Vec::<String>::new()
        );
        assert_eq!(keys(owned_store.as_ref(), "other").await, [kept]);
        assert_eq!(keys(hijacked_store.as_ref(), "repo").await, [cleaned]);
    }

    #[tokio::test]
    async fn create_refuses_a_destination_holding_foreign_files() {
        let scratch = Scratch::new();
        let database = test_support::database().await;
        let occupied = scratch.directory("occupied");
        std::fs::write(occupied.join("stray.bin"), b"not ours").unwrap();

        let result = create(
            &database,
            "Occupied".to_owned(),
            StorageTargetConfiguration::Filesystem {
                path: occupied.clone(),
            },
        )
        .await;

        assert_eq!(
            result.err().map(|error| error.to_string()),
            Some("storage target must be empty before Gitadel claims it".to_owned())
        );
        assert_eq!(std::fs::read_dir(&occupied).unwrap().count(), 1);
    }
}
