//! Container registry payload storage: the registry [`StorageDomain`], rooted
//! at `storage.registry_root` when no storage target is selected.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::Result;
use async_trait::async_trait;
use sea_orm::{DatabaseConnection, EntityTrait as _};
use uuid::Uuid;

use super::store::{RegistryUsage, usage_by_repository_id};
use crate::{
    blob_store::{
        BlobDigest, BlobStore, DomainStorage, FilesystemBlobStore, ObjectKey, ObjectPrefix,
        StorageDomain,
    },
    config::StorageSettings,
    entity::repository,
};

/// Container registry blobs and manifest bytes, keyed
/// `registry/<storage-key>/{blobs,manifests}/<aa>/<bb>/<digest>` and shared by
/// the images of one repository. Tags and manifest records live in the
/// database, so the payloads are all a storage target has to hold.
///
/// The local root drops the `registry/` prefix, so it holds one directory per
/// repository storage key like the LFS root, plus `uploads/` for resumable
/// upload sessions.
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
        parse_key(key.as_str()).map(|parsed| parsed.storage_key)
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

/// The two kinds of registry payload.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ObjectKind {
    Blob,
    Manifest,
}

impl ObjectKind {
    /// The value stored in `registry_objects.kind`.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Blob => "blob",
            Self::Manifest => "manifest",
        }
    }

    fn directory(self) -> &'static str {
        match self {
            Self::Blob => "blobs",
            Self::Manifest => "manifests",
        }
    }
}

/// The canonical key of a payload with SHA-256 `hex`.
pub(crate) fn object_key(storage_key: Uuid, kind: ObjectKind, hex: &str) -> Result<ObjectKey> {
    let hex = hex.parse::<BlobDigest>()?.to_hex();
    ObjectKey::new(format!(
        "registry/{storage_key}/{}/{}/{}/{hex}",
        kind.directory(),
        &hex[..2],
        &hex[2..4]
    ))
}

/// A recognized registry payload key.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ParsedKey<'a> {
    pub storage_key: Uuid,
    pub kind: ObjectKind,
    pub hex: &'a str,
    /// The image hash of the per-image layout 0.13 used, which the startup
    /// import rewrites to the shared layout.
    pub legacy_image: Option<&'a str>,
}

/// Parses a canonical registry key in the current layout or the per-image
/// layout earlier versions wrote
/// (`registry/<storage-key>/<image-hash>/{blobs/aa/bb/<digest>,manifests/objects/<digest>}`),
/// so leftovers of the old layout are still recognized and cleaned up.
pub(crate) fn parse_key(key: &str) -> Option<ParsedKey<'_>> {
    let (repository, rest) = key.strip_prefix("registry/")?.split_once('/')?;
    let storage_key = Uuid::parse_str(repository).ok()?;
    let parts = rest.split('/').collect::<Vec<_>>();
    let sharded = |kind, first: &str, second: &str, hex: &'_ str| {
        (hex.parse::<BlobDigest>().is_ok() && first == &hex[..2] && second == &hex[2..4])
            .then_some(kind)
    };
    let (kind, hex, legacy_image) = match parts.as_slice() {
        ["blobs", first, second, hex] => {
            (sharded(ObjectKind::Blob, first, second, hex)?, *hex, None)
        }
        ["manifests", first, second, hex] => (
            sharded(ObjectKind::Manifest, first, second, hex)?,
            *hex,
            None,
        ),
        [image, "blobs", first, second, hex] if image.parse::<BlobDigest>().is_ok() => (
            sharded(ObjectKind::Blob, first, second, hex)?,
            *hex,
            Some(*image),
        ),
        [image, "manifests", "objects", hex]
            if image.parse::<BlobDigest>().is_ok() && hex.parse::<BlobDigest>().is_ok() =>
        {
            (ObjectKind::Manifest, *hex, Some(*image))
        }
        _ => return None,
    };
    Some(ParsedKey {
        storage_key,
        kind,
        hex,
        legacy_image,
    })
}

/// Opens the registry storage for a running server, failing any online
/// migration a restart interrupted. Registry data earlier versions left in
/// repository directories is moved to the registry root first, and its
/// file-based metadata is imported into the database.
pub(crate) async fn start(
    database: &DatabaseConnection,
    settings: &StorageSettings,
) -> Result<Arc<DomainStorage>> {
    super::upgrade::relocate(database, settings).await?;
    let local_root = RegistryDomain.local_root(settings);
    let storage = DomainStorage::start(database, Arc::new(RegistryDomain), local_root).await?;
    super::upgrade::import(database, settings, &storage).await?;
    Ok(storage)
}

/// Registry usage per repository. Payload counts and bytes, tags, and images
/// come from the database; staged uploads from `<registry_root>/uploads`.
/// `_storage` is kept for callers; usage no longer lists the active store.
pub(crate) async fn usage_by_repository(
    database: &DatabaseConnection,
    registry_root: &Path,
    _storage: &DomainStorage,
) -> Result<Vec<(repository::Model, RegistryUsage)>> {
    let repositories = repository::Entity::find().all(database).await?;
    let mut usage = usage_by_repository_id(database, registry_root, &repositories).await?;
    Ok(repositories
        .into_iter()
        .map(|repository| {
            let usage = usage.remove(&repository.id).unwrap_or_default();
            (repository, usage)
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest as _, Sha256};

    #[test]
    fn keys_in_both_layouts_are_recognized() {
        let storage_key = Uuid::new_v4();
        let hex = hex::encode(Sha256::digest(b"layer"));
        let image = hex::encode(Sha256::digest(b""));
        let blob = object_key(storage_key, ObjectKind::Blob, &hex).unwrap();
        assert_eq!(
            blob.as_str(),
            format!(
                "registry/{storage_key}/blobs/{}/{}/{hex}",
                &hex[..2],
                &hex[2..4]
            )
        );
        assert_eq!(
            parse_key(blob.as_str()),
            Some(ParsedKey {
                storage_key,
                kind: ObjectKind::Blob,
                hex: &hex,
                legacy_image: None
            })
        );
        let legacy = format!("registry/{storage_key}/{image}/manifests/objects/{hex}");
        assert_eq!(
            parse_key(&legacy).map(|parsed| (parsed.kind, parsed.legacy_image)),
            Some((ObjectKind::Manifest, Some(image.as_str())))
        );
        for invalid in [
            format!("registry/{storage_key}/blobs/00/00/{hex}"),
            format!("registry/{storage_key}/uploads/{hex}"),
            format!("{storage_key}/blobs/{}/{}/{hex}", &hex[..2], &hex[2..4]),
        ] {
            assert_eq!(parse_key(&invalid), None, "{invalid}");
        }
    }

    #[tokio::test]
    async fn local_root_holds_one_directory_per_repository() {
        let root = std::env::temp_dir().join(format!("gitadel-registry-local-{}", Uuid::new_v4()));
        let store = RegistryDomain.open_local(root.clone()).await.unwrap();
        let storage_key = Uuid::new_v4();
        let digest = BlobDigest::from_bytes(Sha256::digest(b"layer").into());
        let hex = digest.to_hex();
        let relative = format!("{storage_key}/blobs/{}/{}/{hex}", &hex[..2], &hex[2..4]);
        let key = object_key(storage_key, ObjectKind::Blob, &hex).unwrap();
        store
            .put_verified(
                &key,
                digest,
                Box::pin(std::io::Cursor::new(b"layer".to_vec())),
            )
            .await
            .unwrap();
        let upload = root.join(format!("uploads/{storage_key}/{}/data", Uuid::new_v4()));
        tokio::fs::create_dir_all(upload.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(upload, b"staged").await.unwrap();

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
