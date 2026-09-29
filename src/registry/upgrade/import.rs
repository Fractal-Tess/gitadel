//! Imports the file-based registry metadata of 0.13 (tags, manifest media
//! types, image names) into the database, and moves payloads from the
//! per-image key layout to the layout the images of a repository share.

use std::{
    collections::{HashMap, HashSet},
    io::ErrorKind,
    path::{Path, PathBuf},
};

use anyhow::{Context as _, Result};
use chrono::{DateTime, Utc};
use sea_orm::{DatabaseConnection, EntityTrait as _, TransactionTrait as _};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use tokio::{fs, io::AsyncReadExt as _};
use uuid::Uuid;

use super::{completed, mark_completed};
use crate::{
    blob_store::{BlobStore, DomainStorage, ObjectKey, ObjectPrefix},
    config::StorageSettings,
    entity::repository,
    registry::{
        storage::{ObjectKind, object_key, parse_key},
        store::{
            MAX_MANIFEST_BYTES, ManifestFacts, ManifestRecord, ensure_image, insert_object_row,
            link_blob, normalize_digest, read_bounded_file, read_small_file, record_manifest,
            record_tag, supported_manifest_media_type, valid_suffix, valid_tag,
        },
    },
};

/// Imports file-based registry metadata into the database.
pub(crate) const IMPORT: &str = "import_registry_metadata";

#[derive(Deserialize)]
struct ManifestMeta {
    media_type: String,
}

#[derive(Deserialize)]
struct ReferenceMeta {
    reference: String,
    digest: String,
    #[serde(default)]
    updated_at: Option<String>,
}

/// What importing one image did.
#[derive(Debug, Default)]
struct ImageImport {
    blobs: usize,
    manifests: usize,
    tags: usize,
    dropped_uploads: usize,
}

/// Imports every image directory `<registry_root>/<storage-key>/<image-hash>/`
/// left by 0.13 (or moved there by [`super::relocate`]).
///
/// Per image, payloads are first copied to their shared keys in the active
/// store, then the metadata is written in one transaction, and only then are
/// the old payload keys and the image directory removed. Every step tolerates
/// having run before, so an interrupted import resumes on the next start.
pub(crate) async fn import(
    database: &DatabaseConnection,
    settings: &StorageSettings,
    storage: &DomainStorage,
) -> Result<()> {
    if completed(database, IMPORT).await? {
        return Ok(());
    }
    let store = storage.store();
    let repositories = repository::Entity::find()
        .all(database)
        .await?
        .into_iter()
        .map(|repository| (repository.storage_key, repository))
        .collect::<HashMap<_, _>>();
    for (storage_key, directory) in
        subdirectories(&settings.registry_root, |name| Uuid::parse_str(name).ok()).await?
    {
        let images = subdirectories(&directory, |name| {
            (name.len() == 64 && name.bytes().all(|byte| byte.is_ascii_hexdigit()))
                .then(|| name.to_owned())
        })
        .await?;
        if images.is_empty() {
            continue;
        }
        let Some(repository) = repositories.get(&storage_key) else {
            tracing::warn!(
                %storage_key,
                directory = %directory.display(),
                "left registry data of an unknown repository in place"
            );
            continue;
        };
        for (image_hash, image_dir) in images {
            let imported = import_image(
                database,
                store.as_ref(),
                repository,
                &image_hash,
                &image_dir,
            )
            .await
            .with_context(|| format!("could not import registry image {}", image_dir.display()))?;
            if let Some(imported) = imported {
                tracing::info!(
                    repository_id = %repository.id,
                    image = %image_hash,
                    blobs = imported.blobs,
                    manifests = imported.manifests,
                    tags = imported.tags,
                    dropped_uploads = imported.dropped_uploads,
                    "imported container registry metadata into the database"
                );
            }
        }
    }
    mark_completed(database, IMPORT).await
}

/// Subdirectories of `root` whose names `parse` accepts.
async fn subdirectories<T>(
    root: &Path,
    parse: impl Fn(&str) -> Option<T>,
) -> Result<Vec<(T, PathBuf)>> {
    let mut entries = match fs::read_dir(root).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut result = Vec::new();
    while let Some(entry) = entries.next_entry().await? {
        if !entry.file_type().await?.is_dir() {
            continue;
        }
        if let Some(value) = entry.file_name().to_str().and_then(&parse) {
            result.push((value, entry.path()));
        }
    }
    Ok(result)
}

/// Imports one image, or returns `None` when the directory is not a valid
/// image, which 0.13 ignored as well and which is left in place.
async fn import_image(
    database: &DatabaseConnection,
    store: &dyn BlobStore,
    repository: &repository::Model,
    image_hash: &str,
    image_dir: &Path,
) -> Result<Option<ImageImport>> {
    let suffix = match read_small_file(&image_dir.join("suffix")).await {
        Ok(suffix) => suffix,
        Err(error) => {
            tracing::warn!(%error, directory = %image_dir.display(), "left a registry image without a readable name in place");
            return Ok(None);
        }
    };
    if !valid_suffix(&suffix) || hex::encode(Sha256::digest(suffix.as_bytes())) != image_hash {
        tracing::warn!(directory = %image_dir.display(), "left a registry image with an invalid name in place");
        return Ok(None);
    }
    let storage_key = repository.storage_key;

    // Copy payloads to their shared keys; the old keys stay until the
    // metadata is committed.
    let legacy = store
        .list(&ObjectPrefix::new(format!(
            "registry/{storage_key}/{image_hash}"
        ))?)
        .await?
        .into_iter()
        .filter(|object| {
            parse_key(object.key.as_str())
                .is_some_and(|parsed| parsed.legacy_image == Some(image_hash))
        })
        .collect::<Vec<_>>();
    let mut blobs = Vec::new();
    let mut manifests = HashMap::new();
    for object in &legacy {
        let parsed = parse_key(object.key.as_str()).context("legacy key disappeared")?;
        let key = object_key(storage_key, parsed.kind, parsed.hex)?;
        if store.stat(&key).await?.is_none() {
            store.duplicate(&object.key, &key).await?;
        }
        let digest = format!("sha256:{}", parsed.hex);
        match parsed.kind {
            ObjectKind::Blob => blobs.push((digest, object.size)),
            ObjectKind::Manifest => {
                manifests.insert(digest, (key, object.size));
            }
        }
    }

    // A manifest counted only once its digest reference was written.
    let references = read_references(&image_dir.join("refs")).await?;
    let committed = references
        .iter()
        .filter(|(reference, _, _)| reference.starts_with("sha256:"))
        .map(|(reference, _, updated_at)| (reference.clone(), *updated_at))
        .collect::<HashMap<_, _>>();
    let mut records = Vec::new();
    for (digest, (key, size)) in &manifests {
        let Some(pushed_at) = committed.get(digest) else {
            continue;
        };
        let meta_path = image_dir
            .join("manifests/objects")
            .join(format!("{}.meta", &digest["sha256:".len()..]));
        let Some(media_type) = read_bounded_file(&meta_path, 1024)
            .await
            .ok()
            .and_then(|bytes| serde_json::from_slice::<ManifestMeta>(&bytes).ok())
            .map(|meta| meta.media_type)
            .filter(|media_type| supported_manifest_media_type(media_type))
        else {
            continue;
        };
        let Some(value) = read_manifest(store, key).await? else {
            continue;
        };
        let facts = ManifestFacts::parse(&value, &media_type);
        records.push((digest.clone(), media_type, *size, facts, *pushed_at));
    }
    let manifest_digests = records
        .iter()
        .map(|(digest, ..)| digest.as_str())
        .collect::<HashSet<_>>();
    let tags = references
        .iter()
        .filter(|(reference, digest, _)| {
            !reference.starts_with("sha256:") && manifest_digests.contains(digest.as_str())
        })
        .collect::<Vec<_>>();

    let now = Utc::now();
    let transaction = database.begin().await?;
    let image_id = ensure_image(&transaction, repository.id, &suffix).await?;
    for (digest, size) in &blobs {
        insert_object_row(
            &transaction,
            repository.id,
            ObjectKind::Blob,
            digest,
            *size,
            now,
        )
        .await?;
        link_blob(&transaction, image_id, digest, now).await?;
    }
    for (digest, media_type, size, facts, pushed_at) in &records {
        insert_object_row(
            &transaction,
            repository.id,
            ObjectKind::Manifest,
            digest,
            *size,
            now,
        )
        .await?;
        record_manifest(
            &transaction,
            ManifestRecord {
                image_id,
                digest,
                media_type,
                size: *size,
                facts,
                created_at: pushed_at.unwrap_or(now),
                pushed_at: *pushed_at,
            },
        )
        .await?;
    }
    for (tag, digest, updated_at) in &tags {
        record_tag(&transaction, image_id, tag, digest, *updated_at).await?;
    }
    transaction.commit().await?;

    for object in &legacy {
        store.delete(&object.key).await?;
    }
    let dropped_uploads = subdirectories(&image_dir.join("uploads"), |name| {
        Uuid::parse_str(name).ok()
    })
    .await?
    .len();
    fs::remove_dir_all(image_dir).await?;
    Ok(Some(ImageImport {
        blobs: blobs.len(),
        manifests: records.len(),
        tags: tags.len(),
        dropped_uploads,
    }))
}

/// Valid references of `refs/`: `(reference, digest, updated_at)`, with
/// digests normalized.
async fn read_references(refs: &Path) -> Result<Vec<(String, String, Option<DateTime<Utc>>)>> {
    let mut entries = match fs::read_dir(refs).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut result = Vec::new();
    while let Some(entry) = entries.next_entry().await? {
        if !entry.file_type().await?.is_file() {
            continue;
        }
        let Some(reference) = read_bounded_file(&entry.path(), 4096)
            .await
            .ok()
            .and_then(|bytes| serde_json::from_slice::<ReferenceMeta>(&bytes).ok())
        else {
            continue;
        };
        let Ok(digest) = normalize_digest(&reference.digest) else {
            continue;
        };
        let name = if reference.reference.starts_with("sha256:") {
            match normalize_digest(&reference.reference) {
                Ok(name) if name == digest => name,
                _ => continue,
            }
        } else if valid_tag(&reference.reference) {
            reference.reference
        } else {
            continue;
        };
        let updated_at = reference
            .updated_at
            .as_deref()
            .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
            .map(|value| value.with_timezone(&Utc));
        result.push((name, digest, updated_at));
    }
    Ok(result)
}

/// A manifest payload parsed as JSON, or `None` when it is unreadable.
async fn read_manifest(store: &dyn BlobStore, key: &ObjectKey) -> Result<Option<Value>> {
    let mut reader = store.read(key).await?.take(MAX_MANIFEST_BYTES as u64 + 1);
    let mut bytes = Vec::new();
    if reader.read_to_end(&mut bytes).await.is_err() || bytes.len() > MAX_MANIFEST_BYTES {
        tracing::warn!(%key, "skipped an unreadable legacy registry manifest");
        return Ok(None);
    }
    Ok(serde_json::from_slice(&bytes).ok())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use sea_orm::{ColumnTrait as _, ConnectionTrait as _, QueryFilter as _, sea_query::Expr};

    use super::*;
    use crate::{
        blob_store::{
            StorageDomain as _,
            targets::{self, StorageTargetConfiguration},
        },
        entity::storage_domain_state,
        registry::{
            storage::RegistryDomain,
            store::RegistryStore,
            test_support::{self, LAYER, LegacyImage},
        },
    };

    struct Fixture {
        root: PathBuf,
        settings: StorageSettings,
        database: DatabaseConnection,
        repository: repository::Model,
    }

    impl Fixture {
        async fn new() -> Self {
            let root =
                std::env::temp_dir().join(format!("gitadel-registry-import-{}", Uuid::new_v4()));
            fs::create_dir_all(&root).await.unwrap();
            let (database, owner) = test_support::database(&root).await;
            let repository = test_support::repository(&database, owner, "legacy").await;
            let settings = StorageSettings {
                repository_root: root.join("repositories"),
                lfs_root: root.join("lfs"),
                registry_root: root.join("registry"),
                actions_artifact_root: root.join("actions-artifacts"),
            };
            Self {
                root,
                settings,
                database,
                repository,
            }
        }

        fn image_dir(&self, suffix: &str) -> PathBuf {
            test_support::legacy_image_dir(
                &self
                    .settings
                    .registry_root
                    .join(self.repository.storage_key.to_string()),
                suffix,
            )
        }

        /// Selects a filesystem storage target for the registry.
        async fn select_target(&self) {
            let target = targets::create(
                &self.database,
                "Registry target".to_owned(),
                StorageTargetConfiguration::Filesystem {
                    path: self.root.join("target"),
                },
            )
            .await
            .unwrap();
            storage_domain_state::Entity::update_many()
                .col_expr(
                    storage_domain_state::Column::ActiveTargetId,
                    Expr::value(Some(target.id)),
                )
                .filter(storage_domain_state::Column::Domain.eq("registry"))
                .exec(&self.database)
                .await
                .unwrap();
        }

        async fn storage(&self) -> Arc<DomainStorage> {
            Arc::new(
                DomainStorage::load(
                    &self.database,
                    Arc::new(RegistryDomain),
                    RegistryDomain.local_root(&self.settings),
                )
                .await
                .unwrap(),
            )
        }

        async fn assert_imported(
            &self,
            storage: &DomainStorage,
            suffix: &str,
            legacy: &LegacyImage,
        ) {
            let registry = RegistryStore::new(self.database.clone(), &self.settings.registry_root);
            let image = registry.image(&self.repository, suffix, storage.store());
            let manifest = image.get_manifest("latest").await.unwrap().unwrap();
            assert_eq!(
                (manifest.digest.as_str(), manifest.bytes.as_slice()),
                (legacy.manifest_digest.as_str(), legacy.manifest.as_slice())
            );
            assert_eq!(
                image.tags_page("", 10).await.unwrap(),
                ["latest", "undated"]
            );
            let (mut reader, _) = image
                .blob_reader(&test_support::digest(LAYER), None)
                .await
                .unwrap()
                .unwrap();
            let mut layer = Vec::new();
            reader.read_to_end(&mut layer).await.unwrap();
            assert_eq!(layer, LAYER);

            let images = registry.browse_images(self.repository.id).await.unwrap();
            assert_eq!(images.len(), 1);
            assert_eq!(
                images[0]
                    .references
                    .iter()
                    .map(|reference| (reference.tag.as_deref(), reference.updated_at.is_some()))
                    .collect::<Vec<_>>(),
                [(Some("latest"), true), (Some("undated"), false)]
            );
            assert_eq!(
                images[0].updated_at.as_deref(),
                Some("2026-09-02T10:00:00+00:00")
            );

            // Only the shared layout remains, and the metadata files are gone.
            let keys = storage
                .store()
                .list(&ObjectPrefix::new("registry").unwrap())
                .await
                .unwrap();
            assert_eq!(keys.len(), 3);
            assert!(keys.iter().all(|object| {
                parse_key(object.key.as_str()).is_some_and(|parsed| parsed.legacy_image.is_none())
            }));
            assert!(!self.image_dir(suffix).exists());
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[tokio::test]
    async fn local_metadata_files_are_imported_once() {
        let fixture = Fixture::new().await;
        let legacy = test_support::write_legacy_image(
            &fixture.image_dir(""),
            fixture.repository.storage_key,
            "",
            None,
        )
        .await;
        let storage = fixture.storage().await;
        import(&fixture.database, &fixture.settings, &storage)
            .await
            .unwrap();
        fixture.assert_imported(&storage, "", &legacy).await;
        assert!(completed(&fixture.database, IMPORT).await.unwrap());
    }

    #[tokio::test]
    async fn payloads_on_a_storage_target_are_rekeyed() {
        let fixture = Fixture::new().await;
        fixture.select_target().await;
        let storage = fixture.storage().await;
        assert!(storage.target_id().is_some());
        let legacy = test_support::write_legacy_image(
            &fixture.image_dir("worker"),
            fixture.repository.storage_key,
            "worker",
            Some(storage.store().as_ref()),
        )
        .await;
        import(&fixture.database, &fixture.settings, &storage)
            .await
            .unwrap();
        fixture.assert_imported(&storage, "worker", &legacy).await;
    }

    #[tokio::test]
    async fn interrupted_import_resumes_after_old_payloads_are_gone() {
        let fixture = Fixture::new().await;
        fixture.select_target().await;
        let storage = fixture.storage().await;
        let image_dir = fixture.image_dir("");
        let legacy = test_support::write_legacy_image(
            &image_dir,
            fixture.repository.storage_key,
            "",
            Some(storage.store().as_ref()),
        )
        .await;
        let saved = fixture.root.join("saved-metadata");
        copy_directory(&image_dir, &saved);
        import(&fixture.database, &fixture.settings, &storage)
            .await
            .unwrap();

        // The first run stopped after deleting the old payload keys but
        // before removing the metadata directory and recording completion.
        copy_directory(&saved, &image_dir);
        fixture
            .database
            .execute_unprepared("DELETE FROM registry_upgrades")
            .await
            .unwrap();
        import(&fixture.database, &fixture.settings, &storage)
            .await
            .unwrap();
        fixture.assert_imported(&storage, "", &legacy).await;
    }

    fn copy_directory(source: &Path, destination: &Path) {
        for entry in walkdir::WalkDir::new(source) {
            let entry = entry.unwrap();
            let target = destination.join(entry.path().strip_prefix(source).unwrap());
            if entry.file_type().is_dir() {
                std::fs::create_dir_all(target).unwrap();
            } else {
                std::fs::copy(entry.path(), target).unwrap();
            }
        }
    }
}
