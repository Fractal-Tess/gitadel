//! Fixtures shared by the registry tests: an instance database with
//! repositories, an object-store double, and writers for the file layout
//! earlier versions used.

use std::{
    collections::BTreeMap,
    ops::Range,
    path::{Path, PathBuf},
    sync::Mutex,
};

use anyhow::{Context as _, Result, ensure};
use async_trait::async_trait;
use chrono::Utc;
use sea_orm::{ActiveModelTrait as _, DatabaseConnection, Set};
use sha2::{Digest as _, Sha256};
use tokio::io::AsyncReadExt as _;
use uuid::Uuid;

use crate::{
    blob_store::{
        BlobDigest, BlobMetadata, BlobReader, BlobStore, DigestMismatch, ObjectKey, ObjectPrefix,
        PutOutcome,
    },
    config::DatabaseSettings,
    database,
    entity::repository,
    identity,
};

/// A migrated database in `root` with an administrator.
pub(crate) async fn database(root: &Path) -> (DatabaseConnection, Uuid) {
    let database = database::connect_and_migrate(&DatabaseSettings {
        url: format!("sqlite://{}?mode=rwc", root.join("gitadel.db").display()),
    })
    .await
    .unwrap();
    let user = identity::bootstrap_admin(&database, "registry-test", "correct-horse".to_owned())
        .await
        .unwrap();
    (database, user.id)
}

/// Inserts a repository owned by the administrator.
pub(crate) async fn repository(
    database: &DatabaseConnection,
    owner: Uuid,
    name: &str,
) -> repository::Model {
    let now = Utc::now();
    repository::ActiveModel {
        id: Set(Uuid::new_v4()),
        namespace: Set("registry-test".to_owned()),
        name: Set(name.to_owned()),
        description: Set(None),
        website_url: Set(None),
        visibility: Set("private".to_owned()),
        object_format: Set("sha1".to_owned()),
        mirrored: Set(false),
        default_branch: Set(Some("main".to_owned())),
        issue_counter: Set(0),
        storage_key: Set(Uuid::new_v4()),
        created_by: Set(owner),
        archived_at: Set(None),
        deleted_at: Set(None),
        icon_updated_at: Set(None),
        icon_source: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(database)
    .await
    .unwrap()
}

/// An in-memory object store with S3's capabilities: no hard links and no
/// file moves, so duplicates and file puts copy through the default methods.
#[derive(Default)]
pub(crate) struct MemoryBlobStore {
    objects: Mutex<BTreeMap<String, Vec<u8>>>,
}

impl MemoryBlobStore {
    fn get(&self, key: &ObjectKey) -> Option<Vec<u8>> {
        self.objects
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(key.as_str())
            .cloned()
    }
}

fn metadata(key: &ObjectKey, bytes: &[u8]) -> BlobMetadata {
    BlobMetadata {
        key: key.clone(),
        size: bytes.len() as u64,
        digest: BlobDigest::from_bytes(Sha256::digest(bytes).into()),
    }
}

#[async_trait]
impl BlobStore for MemoryBlobStore {
    async fn stat(&self, key: &ObjectKey) -> Result<Option<BlobMetadata>> {
        Ok(self.get(key).map(|bytes| metadata(key, &bytes)))
    }

    async fn read(&self, key: &ObjectKey) -> Result<BlobReader> {
        let bytes = self
            .get(key)
            .with_context(|| format!("blob {key} does not exist"))?;
        Ok(Box::pin(std::io::Cursor::new(bytes)))
    }

    async fn read_range(&self, key: &ObjectKey, range: Range<u64>) -> Result<BlobReader> {
        let bytes = self
            .get(key)
            .with_context(|| format!("blob {key} does not exist"))?;
        ensure!(
            range.start < range.end && range.end <= bytes.len() as u64,
            "blob byte range is invalid"
        );
        Ok(Box::pin(std::io::Cursor::new(
            bytes[range.start as usize..range.end as usize].to_vec(),
        )))
    }

    async fn put_verified(
        &self,
        key: &ObjectKey,
        expected: BlobDigest,
        mut body: BlobReader,
    ) -> Result<PutOutcome> {
        let mut bytes = Vec::new();
        body.read_to_end(&mut bytes).await?;
        ensure!(
            Sha256::digest(&bytes).as_slice() == expected.as_bytes(),
            DigestMismatch
        );
        let size = bytes.len() as u64;
        let created = self
            .objects
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(key.as_str().to_owned(), bytes)
            .is_none();
        Ok(PutOutcome { size, created })
    }

    async fn list(&self, prefix: &ObjectPrefix) -> Result<Vec<BlobMetadata>> {
        let objects = self
            .objects
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        objects
            .iter()
            .filter(|(key, _)| {
                prefix.as_str().is_empty()
                    || key
                        .strip_prefix(prefix.as_str())
                        .is_some_and(|rest| rest.starts_with('/'))
            })
            .map(|(key, bytes)| Ok(metadata(&ObjectKey::new(key.clone())?, bytes)))
            .collect()
    }

    async fn delete(&self, key: &ObjectKey) -> Result<()> {
        self.objects
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(key.as_str());
        Ok(())
    }
}

pub(crate) const CONFIG: &[u8] = b"{}";
pub(crate) const LAYER: &[u8] = b"legacy layer";
const OCI_MANIFEST: &str = "application/vnd.oci.image.manifest.v1+json";

pub(crate) fn digest(bytes: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
}

/// An image in the per-image file layout of 0.13: one config blob, one
/// layer, a manifest tagged `latest`, and a stale upload session.
pub(crate) struct LegacyImage {
    pub manifest: Vec<u8>,
    pub manifest_digest: String,
}

/// Writes an image into `image_dir` (`.../<image-hash>` for `suffix`), laid out
/// as 0.13 did. Payloads go into `image_dir` itself when `payloads` is `None`,
/// which is the local layout, or into `payloads` under their per-image keys.
pub(crate) async fn write_legacy_image(
    image_dir: &Path,
    storage_key: Uuid,
    suffix: &str,
    payloads: Option<&dyn BlobStore>,
) -> LegacyImage {
    let image_hash = hex::encode(Sha256::digest(suffix.as_bytes()));
    assert_eq!(
        image_dir.file_name().and_then(|name| name.to_str()),
        Some(image_hash.as_str())
    );
    let manifest = serde_json::to_vec(&serde_json::json!({
        "schemaVersion": 2,
        "mediaType": OCI_MANIFEST,
        "config": {"mediaType": "application/vnd.oci.image.config.v1+json", "digest": digest(CONFIG), "size": CONFIG.len()},
        "layers": [{"mediaType": "application/vnd.oci.image.layer.v1.tar", "digest": digest(LAYER), "size": LAYER.len()}],
        "annotations": {"org.opencontainers.image.title": "legacy"}
    }))
    .unwrap();
    let manifest_digest = digest(&manifest);
    let write = |relative: String, bytes: Vec<u8>| {
        let path = image_dir.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    };
    write("suffix".to_owned(), suffix.as_bytes().to_vec());
    let mut payload_files = Vec::new();
    for blob in [CONFIG, LAYER] {
        let hex = hex::encode(Sha256::digest(blob));
        payload_files.push((
            format!("blobs/{}/{}/{hex}", &hex[..2], &hex[2..4]),
            blob.to_vec(),
        ));
    }
    let manifest_hex = &manifest_digest["sha256:".len()..];
    payload_files.push((
        format!("manifests/objects/{manifest_hex}"),
        manifest.clone(),
    ));
    for (relative, bytes) in payload_files {
        match payloads {
            None => write(relative, bytes),
            Some(store) => {
                let key = ObjectKey::new(format!("registry/{storage_key}/{image_hash}/{relative}"))
                    .unwrap();
                let expected = BlobDigest::from_bytes(Sha256::digest(&bytes).into());
                store
                    .put_verified(&key, expected, Box::pin(std::io::Cursor::new(bytes)))
                    .await
                    .unwrap();
            }
        }
    }
    write(
        format!("manifests/objects/{manifest_hex}.meta"),
        serde_json::to_vec(&serde_json::json!({"media_type": OCI_MANIFEST})).unwrap(),
    );
    for (reference, updated_at) in [
        (manifest_digest.as_str(), Some("2026-09-01T10:00:00+00:00")),
        ("latest", Some("2026-09-02T10:00:00+00:00")),
        ("undated", None),
    ] {
        write(
            format!("refs/{}.json", hex::encode(Sha256::digest(reference))),
            serde_json::to_vec(&serde_json::json!({
                "reference": reference, "digest": manifest_digest, "updated_at": updated_at
            }))
            .unwrap(),
        );
    }
    write(
        format!("uploads/{}/data", Uuid::new_v4()),
        b"unfinished".to_vec(),
    );
    LegacyImage {
        manifest,
        manifest_digest,
    }
}

/// `<root>/<storage-key>/<image-hash>`, the image directory under a registry
/// root, or under `<storage-key>.git/gitadel-registry/images` for 0.13.
pub(crate) fn legacy_image_dir(images: &Path, suffix: &str) -> PathBuf {
    images.join(hex::encode(Sha256::digest(suffix.as_bytes())))
}
