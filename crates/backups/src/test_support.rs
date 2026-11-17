//! Data-only fixtures for legacy archive recovery.
use crate::{
    blob_store::{BlobDigest, BlobStore, ObjectKey},
    config::DatabaseSettings,
    database,
    entity::{repository, user},
};
use chrono::Utc;
use sea_orm::{ActiveModelTrait as _, DatabaseConnection, Set};
use sha2::{Digest as _, Sha256};
use std::path::{Path, PathBuf};
use uuid::Uuid;

pub(crate) async fn database(root: &Path) -> (DatabaseConnection, Uuid) {
    let database = database::connect_and_migrate(&DatabaseSettings {
        url: format!("sqlite://{}?mode=rwc", root.join("gitadel.db").display()),
    })
    .await
    .unwrap();
    let now = Utc::now();
    let user = user::ActiveModel {
        id: Set(Uuid::new_v4()),
        username: Set("registry-test".to_owned()),
        password_hash: Set("fixture-password-hash".to_owned()),
        is_admin: Set(true),
        default_repository_visibility: Set("private".to_owned()),
        theme_preference: Set("system".to_owned()),
        motion_preference: Set("system".to_owned()),
        disabled_at: Set(None),
        avatar_updated_at: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(&database)
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
    LegacyImage { manifest }
}

/// `<root>/<storage-key>/<image-hash>`, the image directory under a registry
/// root, or under `<storage-key>.git/gitadel-registry/images` for 0.13.
pub(crate) fn legacy_image_dir(images: &Path, suffix: &str) -> PathBuf {
    images.join(hex::encode(Sha256::digest(suffix.as_bytes())))
}
