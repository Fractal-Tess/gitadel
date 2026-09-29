//! Container registry payload storage: the registry [`StorageDomain`] and the
//! local store that keeps payloads inside repository directories.

use std::{
    collections::HashMap,
    ops::Range,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context, Result};
use async_trait::async_trait;
use sea_orm::{DatabaseConnection, EntityTrait as _};
use uuid::Uuid;

use super::store::{RegistryStore, RegistryUsage};
use crate::{
    blob_store::{
        BlobDigest, BlobMetadata, BlobReader, BlobStore, DomainStorage, FilesystemBlobStore,
        ObjectKey, ObjectPrefix, PutOutcome, StorageDomain,
    },
    config::StorageSettings,
    entity::repository,
};

/// Container registry blobs and manifests, keyed
/// `registry/<storage-key>/<image-hash>/{blobs/aa/bb/<digest>,manifests/objects/<digest>}`.
/// Tags, manifest media types, and uploads stay in the repository directory.
pub(crate) struct RegistryDomain;

#[async_trait]
impl StorageDomain for RegistryDomain {
    fn name(&self) -> &'static str {
        "registry"
    }

    fn label(&self) -> &'static str {
        "registry"
    }

    fn key_prefix(&self) -> &'static str {
        "registry"
    }

    fn repository_storage_key(&self, key: &ObjectKey) -> Option<Uuid> {
        key_parts(key.as_str()).map(|(storage_key, ..)| storage_key)
    }

    fn repository_prefix(&self, storage_key: Uuid) -> Result<ObjectPrefix> {
        ObjectPrefix::new(format!("registry/{storage_key}"))
    }

    fn local_root(&self, settings: &StorageSettings) -> PathBuf {
        settings.repository_root.clone()
    }

    async fn open_local(&self, root: PathBuf) -> Result<Arc<dyn BlobStore>> {
        Ok(Arc::new(RegistryLocalStore::new(root).await?))
    }
}

/// Opens the registry storage for a running server, failing any online
/// migration a restart interrupted.
pub(crate) async fn start(
    database: &DatabaseConnection,
    settings: &StorageSettings,
) -> Result<Arc<DomainStorage>> {
    let local_root = RegistryDomain.local_root(settings);
    DomainStorage::start(database, Arc::new(RegistryDomain), local_root).await
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

/// The canonical key of a payload path relative to the repository root, or
/// `None` for registry metadata and every other file.
pub(crate) fn registry_object_key(local: &str) -> Option<ObjectKey> {
    let (repository, rest) = local.split_once("/gitadel-registry/images/")?;
    let storage_key = Uuid::parse_str(repository.strip_suffix(".git")?).ok()?;
    let key = ObjectKey::new(format!("registry/{storage_key}/{rest}")).ok()?;
    key_parts(key.as_str())?;
    Some(key)
}

/// The path of a canonical key relative to the repository root.
pub(crate) fn local_object_key(key: &ObjectKey) -> Result<ObjectKey> {
    let (storage_key, image, payload, _) =
        key_parts(key.as_str()).context("invalid registry payload key")?;
    ObjectKey::new(format!(
        "{storage_key}.git/gitadel-registry/images/{image}/{payload}"
    ))
}

/// Presents payloads stored inside `<root>/<storage-key>.git/gitadel-registry`
/// under their canonical keys, so the local root behaves like any other
/// store.
pub(crate) struct RegistryLocalStore {
    root: PathBuf,
    files: FilesystemBlobStore,
}

impl RegistryLocalStore {
    pub(crate) async fn new(root: PathBuf) -> Result<Self> {
        Ok(Self {
            files: FilesystemBlobStore::new(root.clone()).await?,
            root,
        })
    }

    /// Repository directories under the root, by storage key.
    async fn repositories(&self) -> Result<Vec<Uuid>> {
        let mut result = Vec::new();
        let mut entries = match tokio::fs::read_dir(&self.root).await {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(result),
            Err(error) => return Err(error.into()),
        };
        while let Some(entry) = entries.next_entry().await? {
            if !entry.file_type().await?.is_dir() {
                continue;
            }
            if let Some(storage_key) = entry
                .file_name()
                .to_str()
                .and_then(|name| name.strip_suffix(".git"))
                .and_then(|name| Uuid::parse_str(name).ok())
            {
                result.push(storage_key);
            }
        }
        Ok(result)
    }

    /// The local directories a canonical listing prefix covers.
    async fn local_prefixes(&self, prefix: &ObjectPrefix) -> Result<Vec<String>> {
        let images = |storage_key: Uuid| format!("{storage_key}.git/gitadel-registry/images");
        let value = prefix.as_str();
        if value.is_empty() || value == "registry" {
            return Ok(self.repositories().await?.into_iter().map(images).collect());
        }
        let Some(rest) = value.strip_prefix("registry/") else {
            return Ok(Vec::new());
        };
        let (repository, tail) = match rest.split_once('/') {
            Some((repository, tail)) => (repository, Some(tail)),
            None => (rest, None),
        };
        let Ok(storage_key) = Uuid::parse_str(repository) else {
            return Ok(Vec::new());
        };
        Ok(vec![match tail {
            Some(tail) => format!("{}/{tail}", images(storage_key)),
            None => images(storage_key),
        }])
    }
}

#[async_trait]
impl BlobStore for RegistryLocalStore {
    async fn stat(&self, key: &ObjectKey) -> Result<Option<BlobMetadata>> {
        Ok(self
            .files
            .stat(&local_object_key(key)?)
            .await?
            .map(|metadata| BlobMetadata {
                key: key.clone(),
                ..metadata
            }))
    }

    async fn read(&self, key: &ObjectKey) -> Result<BlobReader> {
        self.files.read(&local_object_key(key)?).await
    }

    async fn read_range(&self, key: &ObjectKey, range: Range<u64>) -> Result<BlobReader> {
        self.files.read_range(&local_object_key(key)?, range).await
    }

    async fn put_verified(
        &self,
        key: &ObjectKey,
        expected: BlobDigest,
        body: BlobReader,
    ) -> Result<PutOutcome> {
        self.files
            .put_verified(&local_object_key(key)?, expected, body)
            .await
    }

    async fn list(&self, prefix: &ObjectPrefix) -> Result<Vec<BlobMetadata>> {
        let mut objects = Vec::new();
        for local in self.local_prefixes(prefix).await? {
            for mut object in self.files.list(&ObjectPrefix::new(local)?).await? {
                if let Some(key) = registry_object_key(object.key.as_str()) {
                    object.key = key;
                    objects.push(object);
                }
            }
        }
        objects.sort_by(|left, right| left.key.as_str().cmp(right.key.as_str()));
        Ok(objects)
    }

    async fn delete(&self, key: &ObjectKey) -> Result<()> {
        self.files.delete(&local_object_key(key)?).await
    }

    async fn put_file(
        &self,
        key: &ObjectKey,
        expected: BlobDigest,
        path: &Path,
    ) -> Result<PutOutcome> {
        self.files
            .put_file(&local_object_key(key)?, expected, path)
            .await
    }

    async fn duplicate(&self, source: &ObjectKey, destination: &ObjectKey) -> Result<PutOutcome> {
        self.files
            .duplicate(&local_object_key(source)?, &local_object_key(destination)?)
            .await
    }
}

/// Registry usage per repository: payloads from the active store, and tags,
/// images, and uploads from each repository directory under `repository_root`.
pub(crate) async fn usage_by_repository(
    database: &DatabaseConnection,
    repository_root: &Path,
    storage: &DomainStorage,
) -> Result<Vec<(repository::Model, RegistryUsage)>> {
    let repositories = repository::Entity::find().all(database).await?;
    let mut payload_usage = HashMap::<Uuid, RegistryUsage>::new();
    for object in storage.inventory(database).await? {
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
        let path = repository_root.join(format!("{}.git", repository.storage_key));
        let mut usage = registry.metadata_usage(&path).await?;
        usage += payload_usage
            .remove(&repository.storage_key)
            .unwrap_or_default();
        result.push((repository, usage));
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest as _, Sha256};

    fn blob_key(storage_key: Uuid, payload: &[u8]) -> (ObjectKey, BlobDigest) {
        let image = hex::encode(Sha256::digest(b""));
        let digest = BlobDigest::from_bytes(Sha256::digest(payload).into());
        let hex = digest.to_hex();
        let key = ObjectKey::new(format!(
            "registry/{storage_key}/{image}/blobs/{}/{}/{hex}",
            &hex[..2],
            &hex[2..4]
        ))
        .unwrap();
        (key, digest)
    }

    #[tokio::test]
    async fn local_store_keeps_payloads_inside_repository_directories() {
        let root = std::env::temp_dir().join(format!("gitadel-registry-local-{}", Uuid::new_v4()));
        let store = RegistryLocalStore::new(root.clone()).await.unwrap();
        let storage_key = Uuid::new_v4();
        let (key, digest) = blob_key(storage_key, b"layer");
        store
            .put_verified(
                &key,
                digest,
                Box::pin(std::io::Cursor::new(b"layer".to_vec())),
            )
            .await
            .unwrap();
        tokio::fs::write(
            root.join(format!("{storage_key}.git/gitadel-registry/images/suffix")),
            b"",
        )
        .await
        .unwrap();

        assert!(
            root.join(local_object_key(&key).unwrap().as_str())
                .is_file()
        );
        for prefix in [
            String::new(),
            "registry".to_owned(),
            format!("registry/{storage_key}"),
        ] {
            let listed = store
                .list(&ObjectPrefix::new(prefix).unwrap())
                .await
                .unwrap();
            assert_eq!(
                listed.iter().map(|object| &object.key).collect::<Vec<_>>(),
                [&key]
            );
        }
        assert_eq!(store.stat(&key).await.unwrap().unwrap().key, key);
        store.delete(&key).await.unwrap();
        assert!(store.stat(&key).await.unwrap().is_none());
        tokio::fs::remove_dir_all(root).await.unwrap();
    }
}
