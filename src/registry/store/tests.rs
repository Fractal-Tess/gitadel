//! Every scenario runs against the local registry root and an object-store
//! double without hard links or file moves, as a storage target would be.

use axum::body::Bytes;

use super::*;
use crate::{
    blob_store::{ObjectPrefix, StorageDomain as _},
    registry::{
        storage::RegistryDomain,
        test_support::{self, MemoryBlobStore},
    },
};

#[derive(Clone, Copy, Debug)]
enum Backend {
    Local,
    Object,
}

const BACKENDS: [Backend; 2] = [Backend::Local, Backend::Object];

struct Fixture {
    root: PathBuf,
    database: DatabaseConnection,
    owner: Uuid,
    repository: repository::Model,
    registry: RegistryStore,
    store: Arc<dyn BlobStore>,
    image: ImageStore,
    uploader: Uuid,
}

impl Fixture {
    async fn new(backend: Backend) -> Self {
        let root = std::env::temp_dir().join(format!("gitadel-registry-test-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).await.unwrap();
        let (database, owner) = test_support::database(&root).await;
        let repository = test_support::repository(&database, owner, "images").await;
        let registry_root = root.join("registry");
        let store: Arc<dyn BlobStore> = match backend {
            Backend::Local => RegistryDomain
                .open_local(registry_root.clone())
                .await
                .unwrap(),
            Backend::Object => Arc::new(MemoryBlobStore::default()),
        };
        let registry = RegistryStore::new(database.clone(), &registry_root);
        let image = registry.image(&repository, "", store.clone());
        Self {
            root,
            database,
            owner,
            repository,
            registry,
            store,
            image,
            uploader: Uuid::new_v4(),
        }
    }

    /// A fresh handle on an image, as a later request would get.
    fn image(&self, repository: &repository::Model, suffix: &str) -> ImageStore {
        self.registry.image(repository, suffix, self.store.clone())
    }

    async fn blob(&self, image: &ImageStore, bytes: &'static [u8]) -> String {
        let digest = test_support::digest(bytes);
        let id = image.start_upload(self.uploader).await.unwrap();
        image
            .append_upload(id, self.uploader, Body::from(bytes), None)
            .await
            .unwrap();
        image
            .finish_upload(id, self.uploader, &digest)
            .await
            .unwrap();
        digest
    }

    /// Pushes an image manifest with `config` and `layers` under `reference`.
    async fn manifest(
        &self,
        image: &ImageStore,
        reference: &str,
        config: &'static [u8],
        layers: &[&'static [u8]],
    ) -> StoredManifest {
        let config_digest = self.blob(image, config).await;
        let mut descriptors = Vec::new();
        for layer in layers {
            descriptors.push(serde_json::json!({
                "mediaType": "application/vnd.oci.image.layer.v1.tar",
                "digest": self.blob(image, layer).await,
                "size": layer.len(),
            }));
        }
        let bytes = serde_json::to_vec(&serde_json::json!({
            "schemaVersion": 2, "mediaType": OCI_IMAGE_MANIFEST_MEDIA_TYPE,
            "config": {"mediaType": "application/vnd.oci.image.config.v1+json", "digest": config_digest, "size": config.len()},
            "layers": descriptors,
        }))
        .unwrap();
        image
            .put_manifest(reference, OCI_IMAGE_MANIFEST_MEDIA_TYPE, &bytes)
            .await
            .unwrap()
    }

    async fn stored(&self, repository: &repository::Model, kind: ObjectKind, digest: &str) -> bool {
        let key = object_key(repository.storage_key, kind, &digest["sha256:".len()..]).unwrap();
        self.store.stat(&key).await.unwrap().is_some()
    }

    async fn read_blob(&self, image: &ImageStore, digest: &str) -> Option<Vec<u8>> {
        let (mut reader, _) = image.blob_reader(digest, None).await.unwrap()?;
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await.unwrap();
        Some(bytes)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn underscore_prefixed_tags_are_valid() {
    assert!(valid_tag("_test"));
}

#[tokio::test]
async fn cancelled_chunks_resume_without_blocking_other_uploads() {
    for backend in BACKENDS {
        let fixture = Fixture::new(backend).await;
        let owner = fixture.uploader;
        let id = fixture.image.start_upload(owner).await.unwrap();
        fixture
            .image
            .append_upload(id, owner, Body::from("head"), None)
            .await
            .unwrap();
        let (sent, received) = tokio::sync::oneshot::channel();
        let stream = futures_util::stream::once(async {
            Ok::<_, std::io::Error>(Bytes::from_static(b"uncommitted"))
        })
        .chain(futures_util::stream::once(async move {
            let _ = sent.send(());
            std::future::pending::<Result<Bytes, std::io::Error>>().await
        }));
        let image = fixture.image.clone();
        let pending = tokio::spawn(async move {
            image
                .append_upload(id, owner, Body::from_stream(stream), None)
                .await
        });
        received.await.unwrap();
        let other = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            fixture.image.start_upload(owner),
        )
        .await
        .expect("a stalled client must not block another upload")
        .unwrap();
        fixture.image.cancel_upload(other, owner).await.unwrap();
        pending.abort();
        let _ = pending.await;
        let reopened = fixture.image(&fixture.repository, "");
        assert_eq!(reopened.upload_status(id, owner).await.unwrap(), Some(4));
        reopened
            .append_upload(id, owner, Body::from("tail"), Some((4, 7)))
            .await
            .unwrap();
        let digest = test_support::digest(b"headtail");
        reopened.finish_upload(id, owner, &digest).await.unwrap();
        assert_eq!(
            fixture.read_blob(&reopened, &digest).await.as_deref(),
            Some(&b"headtail"[..]),
            "{backend:?}"
        );
    }
}

#[tokio::test]
async fn upload_sessions_belong_to_their_owner_and_image() {
    for backend in BACKENDS {
        let fixture = Fixture::new(backend).await;
        let id = fixture.image.start_upload(fixture.uploader).await.unwrap();
        let stranger = Uuid::new_v4();
        assert!(matches!(
            fixture.image.upload_status(id, stranger).await,
            Err(StoreError::UploadUnknown)
        ));
        assert!(matches!(
            fixture
                .image
                .append_upload(id, stranger, Body::from("stolen"), None)
                .await,
            Err(StoreError::UploadUnknown)
        ));
        assert!(matches!(
            fixture.image.cancel_upload(id, stranger).await,
            Err(StoreError::UploadUnknown)
        ));
        let other_image = fixture.image(&fixture.repository, "worker");
        assert!(matches!(
            other_image.upload_status(id, fixture.uploader).await,
            Err(StoreError::UploadUnknown)
        ));
        assert_eq!(
            fixture
                .image
                .upload_status(id, fixture.uploader)
                .await
                .unwrap(),
            Some(0),
            "{backend:?}"
        );
    }
}

#[tokio::test]
async fn push_pull_tag_listing_and_deletion() {
    for backend in BACKENDS {
        let fixture = Fixture::new(backend).await;
        let image = &fixture.image;
        let manifest = fixture
            .manifest(image, "v1", b"{\"config\":1}", &[b"layer one"])
            .await;
        image
            .put_manifest("v2", &manifest.media_type, &manifest.bytes)
            .await
            .unwrap();
        image
            .put_manifest("v0", &manifest.media_type, &manifest.bytes)
            .await
            .unwrap();

        let by_tag = image.get_manifest("v1").await.unwrap().unwrap();
        let by_digest = image.get_manifest(&manifest.digest).await.unwrap().unwrap();
        assert_eq!(
            (by_tag.bytes.as_slice(), by_tag.media_type.as_str()),
            (manifest.bytes.as_slice(), OCI_IMAGE_MANIFEST_MEDIA_TYPE)
        );
        assert_eq!(by_digest.digest, manifest.digest);
        assert_eq!(image.tags_page("", 2).await.unwrap(), ["v0", "v1"]);
        assert_eq!(image.tags_page("v1", 2).await.unwrap(), ["v2"]);
        assert_eq!(
            fixture
                .registry
                .list_images(fixture.repository.id)
                .await
                .unwrap(),
            [""]
        );

        // Deleting a tag keeps the manifest and its other tags.
        assert!(image.delete_manifest("v1").await.unwrap());
        assert!(!image.delete_manifest("v1").await.unwrap());
        assert!(image.get_manifest("v1").await.unwrap().is_none());
        assert!(
            image
                .get_manifest(&manifest.digest)
                .await
                .unwrap()
                .is_some()
        );
        assert_eq!(image.tags_page("", 10).await.unwrap(), ["v0", "v2"]);

        // A referenced blob cannot be deleted before its manifest.
        let layer = test_support::digest(b"layer one");
        assert!(matches!(
            image.delete_blob(&layer).await,
            Err(StoreError::Referenced)
        ));
        assert!(image.delete_manifest(&manifest.digest).await.unwrap());
        assert!(image.tags_page("", 10).await.unwrap().is_empty());
        assert!(
            !fixture
                .stored(&fixture.repository, ObjectKind::Manifest, &manifest.digest)
                .await
        );
        assert!(image.delete_blob(&layer).await.unwrap());
        assert!(image.blob_size(&layer).await.unwrap().is_none());
        assert!(
            !fixture
                .stored(&fixture.repository, ObjectKind::Blob, &layer)
                .await,
            "{backend:?}"
        );
    }
}

#[tokio::test]
async fn ranged_blob_reads_come_from_the_store() {
    for backend in BACKENDS {
        let fixture = Fixture::new(backend).await;
        let digest = fixture.blob(&fixture.image, b"ranged payload").await;
        let (mut reader, size) = fixture
            .image
            .blob_reader(&digest, Some((7, 14)))
            .await
            .unwrap()
            .unwrap();
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await.unwrap();
        assert_eq!(
            (bytes.as_slice(), size),
            (&b"payload"[..], 14),
            "{backend:?}"
        );
    }
}

#[tokio::test]
async fn referenced_manifest_deletion_preserves_pull_and_later_releases_blobs() {
    for backend in BACKENDS {
        let fixture = Fixture::new(backend).await;
        let image = &fixture.image;
        let child = fixture.manifest(image, "child", b"{}", &[]).await;
        let index = serde_json::to_vec(&serde_json::json!({
            "schemaVersion": 2, "mediaType": OCI_IMAGE_INDEX_MEDIA_TYPE,
            "manifests": [{"mediaType": OCI_IMAGE_MANIFEST_MEDIA_TYPE, "digest": child.digest, "size": child.bytes.len()}]
        }))
        .unwrap();
        let parent = image
            .put_manifest("index", OCI_IMAGE_INDEX_MEDIA_TYPE, &index)
            .await
            .unwrap();
        assert!(matches!(
            image.delete_manifest(&child.digest).await,
            Err(StoreError::Referenced)
        ));
        assert_eq!(
            image.get_manifest("child").await.unwrap().unwrap().bytes,
            child.bytes
        );
        image.delete_manifest(&parent.digest).await.unwrap();
        image.delete_manifest(&child.digest).await.unwrap();
        assert!(
            image
                .delete_blob(&test_support::digest(b"{}"))
                .await
                .unwrap(),
            "{backend:?}"
        );
    }
}

#[tokio::test]
async fn referrers_use_config_type_when_image_artifact_type_is_empty() {
    for backend in BACKENDS {
        let fixture = Fixture::new(backend).await;
        let config = fixture.blob(&fixture.image, b"{}").await;
        let subject = test_support::digest(b"missing");
        let config_type = "application/vnd.gitadel.proof.config";
        let bytes = serde_json::to_vec(&serde_json::json!({
            "schemaVersion": 2, "mediaType": OCI_IMAGE_MANIFEST_MEDIA_TYPE,
            "artifactType": "",
            "config": {"mediaType": config_type, "digest": config, "size": 2},
            "layers": [],
            "subject": {"mediaType": OCI_IMAGE_MANIFEST_MEDIA_TYPE, "digest": subject, "size": 7},
            "annotations": {"proof": "yes"}
        }))
        .unwrap();
        let manifest = fixture
            .image
            .put_manifest("proof", OCI_IMAGE_MANIFEST_MEDIA_TYPE, &bytes)
            .await
            .unwrap();
        let referrers = fixture
            .image
            .referrers(&subject, Some(config_type))
            .await
            .unwrap();
        assert_eq!(
            referrers,
            vec![serde_json::json!({
                "mediaType": OCI_IMAGE_MANIFEST_MEDIA_TYPE,
                "digest": manifest.digest,
                "size": bytes.len(),
                "artifactType": config_type,
                "annotations": {"proof": "yes"},
            })],
            "{backend:?}"
        );
        assert!(
            fixture
                .image
                .referrers(&subject, Some("application/other"))
                .await
                .unwrap()
                .is_empty()
        );
    }
}

#[tokio::test]
async fn referrers_allow_missing_subjects_and_ignore_unrecorded_payloads() {
    for backend in BACKENDS {
        let fixture = Fixture::new(backend).await;
        let subject = test_support::digest(b"missing");
        let proof = |name: &str| {
            serde_json::to_vec(&serde_json::json!({
                "schemaVersion": 2, "mediaType": OCI_IMAGE_INDEX_MEDIA_TYPE,
                "artifactType": "application/vnd.gitadel.proof", "manifests": [],
                "subject": {"mediaType": OCI_IMAGE_MANIFEST_MEDIA_TYPE, "digest": subject, "size": 7},
                "annotations": {"name": name}
            }))
            .unwrap()
        };
        let bytes = proof("recorded");
        let manifest = fixture
            .image
            .put_manifest("proof", OCI_IMAGE_INDEX_MEDIA_TYPE, &bytes)
            .await
            .unwrap();
        // A payload whose push never committed its metadata is not a referrer.
        let orphan = proof("interrupted");
        let orphan_digest = test_support::digest(&orphan);
        fixture
            .store
            .put_verified(
                &object_key(
                    fixture.repository.storage_key,
                    ObjectKind::Manifest,
                    &orphan_digest["sha256:".len()..],
                )
                .unwrap(),
                blob_digest(&orphan_digest).unwrap(),
                Box::pin(std::io::Cursor::new(orphan)),
            )
            .await
            .unwrap();
        let referrers = fixture
            .image
            .referrers(&subject, Some("application/vnd.gitadel.proof"))
            .await
            .unwrap();
        assert_eq!(
            referrers
                .iter()
                .map(|value| value["digest"].as_str().unwrap())
                .collect::<Vec<_>>(),
            vec![manifest.digest.as_str()]
        );
        fixture
            .image
            .delete_manifest(&manifest.digest)
            .await
            .unwrap();
        assert!(
            fixture
                .image
                .referrers(&subject, None)
                .await
                .unwrap()
                .is_empty(),
            "{backend:?}"
        );
    }
}

#[tokio::test]
async fn mounts_link_inside_a_repository_and_copy_across_repositories() {
    for backend in BACKENDS {
        let fixture = Fixture::new(backend).await;
        let layer = fixture.blob(&fixture.image, b"shared layer").await;
        let payloads = |store: &Arc<dyn BlobStore>| {
            let store = store.clone();
            async move {
                store
                    .list(&ObjectPrefix::new("registry").unwrap())
                    .await
                    .unwrap()
                    .len()
            }
        };
        let before = payloads(&fixture.store).await;

        let worker = fixture.image(&fixture.repository, "worker");
        assert!(worker.mount_blob(&fixture.image, &layer).await.unwrap());
        assert_eq!(payloads(&fixture.store).await, before, "{backend:?}");
        assert_eq!(
            fixture.read_blob(&worker, &layer).await.as_deref(),
            Some(&b"shared layer"[..])
        );

        let other = test_support::repository(&fixture.database, fixture.owner, "other").await;
        let foreign = fixture.image(&other, "");
        assert!(foreign.mount_blob(&fixture.image, &layer).await.unwrap());
        assert!(fixture.stored(&other, ObjectKind::Blob, &layer).await);
        assert_eq!(
            fixture.read_blob(&foreign, &layer).await.as_deref(),
            Some(&b"shared layer"[..])
        );
        let unknown = test_support::digest(b"never uploaded");
        assert!(!foreign.mount_blob(&fixture.image, &unknown).await.unwrap());

        // The payload stays while another image of the repository links it.
        assert!(fixture.image.delete_blob(&layer).await.unwrap());
        assert!(fixture.image.blob_size(&layer).await.unwrap().is_none());
        assert!(
            fixture
                .stored(&fixture.repository, ObjectKind::Blob, &layer)
                .await
        );
        assert!(worker.delete_blob(&layer).await.unwrap());
        assert!(
            !fixture
                .stored(&fixture.repository, ObjectKind::Blob, &layer)
                .await
        );
        assert!(fixture.stored(&other, ObjectKind::Blob, &layer).await);
    }
}

#[tokio::test]
async fn browsing_and_usage_count_shared_payloads_once() {
    for backend in BACKENDS {
        let fixture = Fixture::new(backend).await;
        let manifest = fixture
            .manifest(&fixture.image, "latest", b"{}", &[b"layer"])
            .await;
        let untagged = fixture
            .manifest(&fixture.image, "temporary", b"{}", &[b"other layer"])
            .await;
        fixture.image.delete_manifest("temporary").await.unwrap();
        let worker = fixture.image(&fixture.repository, "worker");
        worker.start_upload(fixture.uploader).await.unwrap();

        let images = fixture
            .registry
            .browse_images(fixture.repository.id)
            .await
            .unwrap();
        assert_eq!(images.len(), 1, "images without manifests are not browsed");
        let image = &images[0];
        let payload_bytes = (manifest.bytes.len() + untagged.bytes.len() + 2 + 5 + 11) as u64;
        assert_eq!(image.size_bytes, payload_bytes, "{backend:?}");
        let mut expected = vec![
            (Some("latest".to_owned()), manifest.digest.clone()),
            (None, untagged.digest.clone()),
        ];
        expected.sort_by(|left, right| left.1.cmp(&right.1));
        assert_eq!(
            image
                .references
                .iter()
                .map(|reference| (reference.tag.clone(), reference.digest.clone()))
                .collect::<Vec<_>>(),
            expected
        );
        assert!(
            image
                .references
                .iter()
                .all(|reference| reference.updated_at.is_some())
        );

        let usage = usage_by_repository_id(
            &fixture.database,
            &fixture.root.join("registry"),
            std::slice::from_ref(&fixture.repository),
        )
        .await
        .unwrap()
        .remove(&fixture.repository.id)
        .unwrap();
        assert_eq!(
            (
                usage.object_count,
                usage.total_bytes,
                usage.blob_count,
                usage.manifest_count,
                usage.tag_count,
                usage.image_count,
                usage.upload_count,
            ),
            (5, payload_bytes, 3, 2, 1, 2, 1)
        );
    }
}
