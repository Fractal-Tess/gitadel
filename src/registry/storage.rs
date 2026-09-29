//! Container registry payload storage: the registry [`StorageDomain`], rooted
//! at `storage.registry_root` when no storage target is selected.

use std::{
    collections::HashMap,
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
        BlobDigest, BlobStore, DomainStorage, FilesystemBlobStore, ObjectKey, ObjectPrefix,
        StorageDomain,
    },
    config::StorageSettings,
    entity::repository,
};

/// Container registry blobs and manifests, keyed
/// `registry/<storage-key>/<image-hash>/{blobs/aa/bb/<digest>,manifests/objects/<digest>}`.
/// The local root drops the `registry/` prefix, so it holds one directory per
/// repository storage key, like the LFS root. Tags, manifest media types, and
/// uploads stay beside the payloads in the local root.
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
        settings.registry_root.clone()
    }

    async fn open_local(&self, root: PathBuf) -> Result<Arc<dyn BlobStore>> {
        Ok(Arc::new(
            FilesystemBlobStore::with_namespace(root, self.key_prefix()).await?,
        ))
    }
}

/// Opens the registry storage for a running server, failing any online
/// migration a restart interrupted. Registry data left in repository
/// directories by earlier versions is moved to the registry root first.
pub(crate) async fn start(
    database: &DatabaseConnection,
    settings: &StorageSettings,
) -> Result<Arc<DomainStorage>> {
    super::upgrade::relocate(database, settings).await?;
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

/// Registry usage per repository: payloads from the active store, and tags,
/// images, and uploads from each repository directory under `registry_root`.
pub(crate) async fn usage_by_repository(
    database: &DatabaseConnection,
    registry_root: &Path,
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
        let path = registry_root.join(repository.storage_key.to_string());
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

    #[tokio::test]
    async fn local_root_holds_one_directory_per_repository() {
        let root = std::env::temp_dir().join(format!("gitadel-registry-local-{}", Uuid::new_v4()));
        let store = RegistryDomain.open_local(root.clone()).await.unwrap();
        let storage_key = Uuid::new_v4();
        let image = hex::encode(Sha256::digest(b""));
        let digest = BlobDigest::from_bytes(Sha256::digest(b"layer").into());
        let hex = digest.to_hex();
        let relative = format!(
            "{storage_key}/{image}/blobs/{}/{}/{hex}",
            &hex[..2],
            &hex[2..4]
        );
        let key = ObjectKey::new(format!("registry/{relative}")).unwrap();
        store
            .put_verified(
                &key,
                digest,
                Box::pin(std::io::Cursor::new(b"layer".to_vec())),
            )
            .await
            .unwrap();
        tokio::fs::write(root.join(format!("{storage_key}/{image}/suffix")), b"")
            .await
            .unwrap();

        assert!(root.join(&relative).is_file());
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
        assert_eq!(
            RegistryDomain.repository_storage_key(&key),
            Some(storage_key)
        );
        store.delete(&key).await.unwrap();
        assert!(store.stat(&key).await.unwrap().is_none());
        tokio::fs::remove_dir_all(root).await.unwrap();
    }
}
