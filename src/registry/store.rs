//! Container registry state. Tags, manifest records, and image membership live
//! in the database; blobs and manifest bytes are content-addressed payloads in
//! the registry domain's active store; resumable uploads stay on local disk
//! under `<registry_root>/uploads/` until they are verified and published.

use axum::body::Body;
use chrono::{DateTime, Utc};
use futures_util::StreamExt;
use sea_orm::{
    ColumnTrait as _, ConnectionTrait, DatabaseConnection, DbErr, EntityTrait as _, PaginatorTrait,
    QueryFilter as _, QueryOrder as _, QuerySelect as _, Set, TransactionTrait as _,
    sea_query::OnConflict,
};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use std::{
    collections::{HashMap, HashSet, hash_map::DefaultHasher},
    fmt,
    hash::{Hash, Hasher},
    io::ErrorKind,
    path::{Path, PathBuf},
    sync::{Arc, LazyLock, Mutex as StdMutex, Weak},
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::{
    fs::{self, File, OpenOptions},
    io::{AsyncReadExt as _, AsyncSeekExt as _, AsyncWriteExt as _, SeekFrom},
    sync::{Mutex, MutexGuard, OwnedMutexGuard},
};
use uuid::Uuid;

use super::storage::{ObjectKind, object_key};
use crate::{
    blob_store::{BlobDigest, BlobReader, BlobStore, DigestMismatch, ObjectKey},
    entity::{
        registry_image, registry_image_blob, registry_manifest, registry_manifest_reference,
        registry_object, registry_tag, repository,
    },
};

pub(super) const MAX_MANIFEST_BYTES: usize = 4 * 1024 * 1024;
pub(super) const MAX_BLOB_BYTES: u64 = 10 * 1024 * 1024 * 1024;
pub(super) const UPLOAD_TTL_SECONDS: u64 = 86_400;
const LOCK_STRIPES: usize = 64;

pub(super) const OCI_IMAGE_MANIFEST_MEDIA_TYPE: &str = "application/vnd.oci.image.manifest.v1+json";
pub(super) const OCI_IMAGE_INDEX_MEDIA_TYPE: &str = "application/vnd.oci.image.index.v1+json";
const DOCKER_MANIFEST_MEDIA_TYPE: &str = "application/vnd.docker.distribution.manifest.v2+json";
const DOCKER_MANIFEST_LIST_MEDIA_TYPE: &str =
    "application/vnd.docker.distribution.manifest.list.v2+json";

#[derive(Clone, Copy)]
enum DescriptorKind {
    Blob,
    Manifest,
    Subject,
}

#[derive(Debug)]
pub(crate) enum StoreError {
    Io(std::io::Error),
    Invalid(String),
    UploadUnknown,
    RangeInvalid,
    DigestInvalid,
    TooLarge,
    ManifestBlobUnknown(String),
    Referenced,
}

impl fmt::Display for StoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "registry storage I/O error: {error}"),
            Self::Invalid(message) => write!(formatter, "invalid registry data: {message}"),
            Self::UploadUnknown => formatter.write_str("upload is unknown or expired"),
            Self::RangeInvalid => formatter.write_str("upload range is invalid"),
            Self::DigestInvalid => formatter.write_str("digest is invalid or unsupported"),
            Self::TooLarge => formatter.write_str("object exceeds the registry size limit"),
            Self::ManifestBlobUnknown(digest) => {
                write!(formatter, "manifest references unknown blob {digest}")
            }
            Self::Referenced => formatter.write_str("object is still referenced"),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<std::io::Error> for StoreError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<DbErr> for StoreError {
    fn from(error: DbErr) -> Self {
        Self::Io(std::io::Error::other(error))
    }
}

impl From<anyhow::Error> for StoreError {
    fn from(error: anyhow::Error) -> Self {
        if error.downcast_ref::<DigestMismatch>().is_some() {
            Self::DigestInvalid
        } else {
            Self::Io(std::io::Error::other(error))
        }
    }
}

/// Entry point to registry metadata and upload staging.
#[derive(Clone)]
pub(crate) struct RegistryStore {
    database: DatabaseConnection,
    uploads_root: Arc<PathBuf>,
}

/// One image of a repository.
#[derive(Clone)]
pub(crate) struct ImageStore {
    database: DatabaseConnection,
    repository_id: Uuid,
    storage_key: Uuid,
    suffix: Arc<str>,
    valid: bool,
    store: Arc<dyn BlobStore>,
    /// `<registry_root>/uploads/<storage-key>`.
    uploads_dir: PathBuf,
}

#[derive(Clone, Debug)]
pub(crate) struct StoredManifest {
    pub digest: String,
    pub media_type: String,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug)]
pub(crate) struct ImageReferenceMetadata {
    pub tag: Option<String>,
    pub digest: String,
    pub updated_at: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct ImageMetadata {
    pub suffix: String,
    pub size_bytes: u64,
    pub updated_at: Option<String>,
    pub references: Vec<ImageReferenceMetadata>,
}

#[derive(Clone, Copy, Debug, Default, Serialize)]
pub(crate) struct RegistryUsage {
    pub object_count: u64,
    pub total_bytes: u64,
    pub blob_count: u64,
    pub manifest_count: u64,
    pub tag_count: u64,
    pub image_count: u64,
    pub upload_count: u64,
    pub upload_bytes: u64,
}

impl std::ops::AddAssign for RegistryUsage {
    fn add_assign(&mut self, other: Self) {
        self.object_count += other.object_count;
        self.total_bytes += other.total_bytes;
        self.blob_count += other.blob_count;
        self.manifest_count += other.manifest_count;
        self.tag_count += other.tag_count;
        self.image_count += other.image_count;
        self.upload_count += other.upload_count;
        self.upload_bytes += other.upload_bytes;
    }
}

/// What a manifest's JSON says about its relationships.
pub(super) struct ManifestFacts {
    pub subject_digest: Option<String>,
    pub artifact_type: Option<String>,
    pub annotations: Option<String>,
    pub references: HashSet<String>,
}

impl ManifestFacts {
    pub(super) fn parse(value: &Value, media_type: &str) -> Self {
        let subject_digest = value
            .get("subject")
            .and_then(Value::as_object)
            .and_then(|subject| subject.get("digest"))
            .and_then(Value::as_str)
            .and_then(|digest| normalize_digest(digest).ok());
        let artifact_type = value
            .get("artifactType")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .or_else(|| match media_type {
                OCI_IMAGE_MANIFEST_MEDIA_TYPE | DOCKER_MANIFEST_MEDIA_TYPE => {
                    value.get("config")?.get("mediaType")?.as_str()
                }
                _ => None,
            })
            .map(str::to_owned);
        let annotations = value
            .get("annotations")
            .filter(|annotations| annotations.is_object())
            .map(Value::to_string);
        let digest_of = |descriptor: &Value| {
            descriptor
                .get("digest")
                .and_then(Value::as_str)
                .and_then(|digest| normalize_digest(digest).ok())
        };
        let mut references = HashSet::new();
        references.extend(value.get("config").and_then(digest_of));
        for key in ["layers", "manifests"] {
            if let Some(descriptors) = value.get(key).and_then(Value::as_array) {
                references.extend(descriptors.iter().filter_map(digest_of));
            }
        }
        Self {
            subject_digest,
            artifact_type,
            annotations,
            references,
        }
    }
}

static IMAGE_LOCKS: LazyLock<Vec<Mutex<()>>> =
    LazyLock::new(|| (0..LOCK_STRIPES).map(|_| Mutex::new(())).collect());
/// Serializes decisions about one payload of one repository, which images of
/// that repository share. Always taken after the image locks.
static OBJECT_LOCKS: LazyLock<Vec<Mutex<()>>> =
    LazyLock::new(|| (0..LOCK_STRIPES).map(|_| Mutex::new(())).collect());
static UPLOAD_LOCKS: LazyLock<StdMutex<HashMap<PathBuf, Weak<Mutex<()>>>>> =
    LazyLock::new(|| StdMutex::new(HashMap::new()));

fn stripe(value: impl Hash) -> usize {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    (hasher.finish() as usize) % LOCK_STRIPES
}

/// Locks two stripes in index order, so concurrent pairs cannot deadlock.
async fn lock_pair(
    locks: &'static [Mutex<()>],
    first: usize,
    second: usize,
) -> (MutexGuard<'static, ()>, Option<MutexGuard<'static, ()>>) {
    if first == second {
        return (locks[first].lock().await, None);
    }
    let (low, high) = (first.min(second), first.max(second));
    (locks[low].lock().await, Some(locks[high].lock().await))
}

fn upload_mutex(path: &Path) -> Arc<Mutex<()>> {
    let mut locks = UPLOAD_LOCKS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(lock) = locks.get(path).and_then(Weak::upgrade) {
        return lock;
    }
    locks.retain(|_, lock| lock.strong_count() != 0);
    let lock = Arc::new(Mutex::new(()));
    locks.insert(path.to_path_buf(), Arc::downgrade(&lock));
    lock
}

async fn upload_lock(path: &Path) -> OwnedMutexGuard<()> {
    upload_mutex(path).lock_owned().await
}

fn database_size(value: i64) -> Result<u64, StoreError> {
    u64::try_from(value).map_err(|_| StoreError::Invalid("stored size is negative".to_owned()))
}

fn stored_size(value: u64) -> Result<i64, StoreError> {
    i64::try_from(value).map_err(|_| StoreError::TooLarge)
}

impl RegistryStore {
    pub fn new(database: DatabaseConnection, registry_root: &Path) -> Self {
        Self {
            database,
            uploads_root: Arc::new(uploads_root(registry_root)),
        }
    }

    /// `store` is the registry domain's active store.
    pub(crate) fn image(
        &self,
        repository: &repository::Model,
        suffix: &str,
        store: Arc<dyn BlobStore>,
    ) -> ImageStore {
        ImageStore {
            database: self.database.clone(),
            repository_id: repository.id,
            storage_key: repository.storage_key,
            suffix: Arc::from(suffix),
            valid: valid_suffix(suffix),
            store,
            uploads_dir: self.uploads_root.join(repository.storage_key.to_string()),
        }
    }

    /// Image names of a repository, sorted.
    pub async fn list_images(&self, repository_id: Uuid) -> Result<Vec<String>, StoreError> {
        Ok(registry_image::Entity::find()
            .filter(registry_image::Column::RepositoryId.eq(repository_id))
            .order_by_asc(registry_image::Column::Name)
            .all(&self.database)
            .await?
            .into_iter()
            .map(|image| image.name)
            .filter(|name| valid_suffix(name))
            .collect())
    }

    /// Images of a repository that hold at least one manifest.
    pub(crate) async fn browse_images(
        &self,
        repository_id: Uuid,
    ) -> Result<Vec<ImageMetadata>, StoreError> {
        let images = registry_image::Entity::find()
            .filter(registry_image::Column::RepositoryId.eq(repository_id))
            .all(&self.database)
            .await?;
        let blob_sizes = registry_object::Entity::find()
            .filter(registry_object::Column::RepositoryId.eq(repository_id))
            .filter(registry_object::Column::Kind.eq(ObjectKind::Blob.as_str()))
            .all(&self.database)
            .await?
            .into_iter()
            .map(|object| (object.digest, object.size))
            .collect::<HashMap<_, _>>();
        let mut result = Vec::with_capacity(images.len());
        for image in images {
            if !valid_suffix(&image.name) {
                continue;
            }
            let manifests = registry_manifest::Entity::find()
                .filter(registry_manifest::Column::ImageId.eq(image.id))
                .all(&self.database)
                .await?;
            if manifests.is_empty() {
                continue;
            }
            let blobs = registry_image_blob::Entity::find()
                .filter(registry_image_blob::Column::ImageId.eq(image.id))
                .all(&self.database)
                .await?;
            let tags = registry_tag::Entity::find()
                .filter(registry_tag::Column::ImageId.eq(image.id))
                .all(&self.database)
                .await?;
            let mut size_bytes = 0_u64;
            for size in manifests.iter().map(|manifest| manifest.size).chain(
                blobs
                    .iter()
                    .filter_map(|blob| blob_sizes.get(&blob.digest).copied()),
            ) {
                size_bytes = size_bytes
                    .checked_add(database_size(size)?)
                    .ok_or_else(|| StoreError::Invalid("image size overflow".to_owned()))?;
            }
            let mut tags_by_digest = HashMap::<&str, Vec<&registry_tag::Model>>::new();
            for tag in &tags {
                tags_by_digest.entry(&tag.digest).or_default().push(tag);
            }
            let mut latest = None::<DateTime<Utc>>;
            let mut references = Vec::new();
            for manifest in &manifests {
                latest = latest.max(manifest.pushed_at);
                let tagged = tags_by_digest
                    .get(manifest.digest.as_str())
                    .map(Vec::as_slice)
                    .unwrap_or_default();
                for tag in tagged {
                    latest = latest.max(tag.updated_at);
                    references.push(ImageReferenceMetadata {
                        tag: Some(tag.name.clone()),
                        digest: manifest.digest.clone(),
                        updated_at: tag.updated_at.map(|value| value.to_rfc3339()),
                    });
                }
                if tagged.is_empty() {
                    references.push(ImageReferenceMetadata {
                        tag: None,
                        digest: manifest.digest.clone(),
                        updated_at: manifest.pushed_at.map(|value| value.to_rfc3339()),
                    });
                }
            }
            references.sort_by(|left, right| {
                left.digest
                    .cmp(&right.digest)
                    .then_with(|| left.tag.cmp(&right.tag))
            });
            result.push(ImageMetadata {
                suffix: image.name,
                size_bytes,
                updated_at: latest.map(|value| value.to_rfc3339()),
                references,
            });
        }
        result.sort_by(|left, right| left.suffix.cmp(&right.suffix));
        Ok(result)
    }
}

/// Where resumable upload sessions are staged.
pub(crate) fn uploads_root(registry_root: &Path) -> PathBuf {
    registry_root.join("uploads")
}

/// Registry usage per repository id: payload counts and bytes, tags, and
/// images from the database, and staged uploads from `<registry_root>/uploads`.
pub(crate) async fn usage_by_repository_id(
    database: &DatabaseConnection,
    registry_root: &Path,
    repositories: &[repository::Model],
) -> Result<HashMap<Uuid, RegistryUsage>, StoreError> {
    let mut usage = HashMap::<Uuid, RegistryUsage>::new();
    for object in registry_object::Entity::find().all(database).await? {
        let entry = usage.entry(object.repository_id).or_default();
        entry.object_count += 1;
        entry.total_bytes = entry
            .total_bytes
            .checked_add(database_size(object.size)?)
            .ok_or_else(|| StoreError::Invalid("registry usage overflow".to_owned()))?;
        if object.kind == ObjectKind::Manifest.as_str() {
            entry.manifest_count += 1;
        } else {
            entry.blob_count += 1;
        }
    }
    let images = registry_image::Entity::find()
        .all(database)
        .await?
        .into_iter()
        .map(|image| (image.id, image.repository_id))
        .collect::<HashMap<_, _>>();
    for repository_id in images.values() {
        usage.entry(*repository_id).or_default().image_count += 1;
    }
    for tag in registry_tag::Entity::find().all(database).await? {
        if let Some(repository_id) = images.get(&tag.image_id) {
            usage.entry(*repository_id).or_default().tag_count += 1;
        }
    }
    let uploads = uploads_root(registry_root);
    for repository in repositories {
        let (count, bytes) =
            staged_uploads(&uploads.join(repository.storage_key.to_string())).await?;
        if count > 0 {
            let entry = usage.entry(repository.id).or_default();
            entry.upload_count += count;
            entry.upload_bytes += bytes;
        }
    }
    Ok(usage)
}

/// Upload sessions in one repository's upload directory, and their bytes.
async fn staged_uploads(directory: &Path) -> Result<(u64, u64), StoreError> {
    let mut uploads = match fs::read_dir(directory).await {
        Ok(uploads) => uploads,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok((0, 0)),
        Err(error) => return Err(error.into()),
    };
    let (mut count, mut bytes) = (0, 0);
    while let Some(upload) = uploads.next_entry().await? {
        if !upload.file_type().await?.is_dir()
            || upload
                .file_name()
                .to_str()
                .and_then(|name| Uuid::parse_str(name).ok())
                .is_none()
        {
            continue;
        }
        // A session finishing concurrently may disappear under us.
        match fs::symlink_metadata(upload.path().join("data")).await {
            Ok(metadata) if metadata.is_file() => {
                count += 1;
                bytes += metadata.len();
            }
            Ok(_) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok((count, bytes))
}

impl ImageStore {
    async fn lock(&self) -> Result<MutexGuard<'static, ()>, StoreError> {
        self.check_valid()?;
        Ok(IMAGE_LOCKS[self.image_stripe()].lock().await)
    }

    fn check_valid(&self) -> Result<(), StoreError> {
        if self.valid {
            Ok(())
        } else {
            Err(StoreError::Invalid("image name is invalid".to_owned()))
        }
    }

    fn image_stripe(&self) -> usize {
        stripe((self.repository_id, self.suffix.as_ref()))
    }

    fn object_stripe(&self, kind: ObjectKind, digest: &str) -> usize {
        stripe((self.repository_id, kind.as_str(), digest))
    }

    async fn lock_object(&self, kind: ObjectKind, digest: &str) -> MutexGuard<'static, ()> {
        OBJECT_LOCKS[self.object_stripe(kind, digest)].lock().await
    }

    /// The payload key of a normalized `sha256:` digest.
    fn key(&self, kind: ObjectKind, digest: &str) -> Result<ObjectKey, StoreError> {
        let hex = digest
            .strip_prefix("sha256:")
            .ok_or(StoreError::DigestInvalid)?;
        Ok(object_key(self.storage_key, kind, hex)?)
    }

    async fn image_id(&self) -> Result<Option<Uuid>, StoreError> {
        find_image(&self.database, self.repository_id, &self.suffix).await
    }

    /// The image's id, recording the image when it is new.
    async fn ensure_image(&self) -> Result<Uuid, StoreError> {
        ensure_image(&self.database, self.repository_id, &self.suffix).await
    }

    /// The size of a blob this image exposes.
    async fn linked_blob_size(
        &self,
        image_id: Uuid,
        digest: &str,
    ) -> Result<Option<u64>, StoreError> {
        if registry_image_blob::Entity::find_by_id((image_id, digest.to_owned()))
            .one(&self.database)
            .await?
            .is_none()
        {
            return Ok(None);
        }
        registry_object::Entity::find_by_id((
            self.repository_id,
            ObjectKind::Blob.as_str().to_owned(),
            digest.to_owned(),
        ))
        .one(&self.database)
        .await?
        .map(|object| database_size(object.size))
        .transpose()
    }

    async fn manifest(
        &self,
        image_id: Uuid,
        digest: &str,
    ) -> Result<Option<registry_manifest::Model>, StoreError> {
        Ok(
            registry_manifest::Entity::find_by_id((image_id, digest.to_owned()))
                .one(&self.database)
                .await?,
        )
    }

    /// Whether a manifest of this image references `digest`.
    async fn digest_referenced(&self, image_id: Uuid, digest: &str) -> Result<bool, StoreError> {
        Ok(registry_manifest_reference::Entity::find()
            .filter(registry_manifest_reference::Column::ImageId.eq(image_id))
            .filter(registry_manifest_reference::Column::ReferencedDigest.eq(digest))
            .count(&self.database)
            .await?
            > 0)
    }

    pub async fn blob_size(&self, digest: &str) -> Result<Option<u64>, StoreError> {
        self.check_valid()?;
        let digest = normalize_digest(digest)?;
        let Some(image_id) = self.image_id().await? else {
            return Ok(None);
        };
        self.linked_blob_size(image_id, &digest).await
    }

    pub async fn blob_reader(
        &self,
        digest: &str,
        range: Option<(u64, u64)>,
    ) -> Result<Option<(BlobReader, u64)>, StoreError> {
        let _guard = self.lock().await?;
        let digest = normalize_digest(digest)?;
        let Some(image_id) = self.image_id().await? else {
            return Ok(None);
        };
        let Some(size) = self.linked_blob_size(image_id, &digest).await? else {
            return Ok(None);
        };
        let (start, end) = range.unwrap_or((0, size));
        if range.is_some() && (start >= end || end > size) {
            return Err(StoreError::RangeInvalid);
        }
        let key = self.key(ObjectKind::Blob, &digest)?;
        let reader = if start == 0 && end == size {
            self.store.read(&key).await?
        } else {
            self.store.read_range(&key, start..end).await?
        };
        Ok(Some((reader, size)))
    }

    pub async fn delete_blob(&self, digest: &str) -> Result<bool, StoreError> {
        let _guard = self.lock().await?;
        let digest = normalize_digest(digest)?;
        let Some(image_id) = self.image_id().await? else {
            return Ok(false);
        };
        if self.linked_blob_size(image_id, &digest).await?.is_none() {
            return Ok(false);
        }
        if self.digest_referenced(image_id, &digest).await? {
            return Err(StoreError::Referenced);
        }
        let _object = self.lock_object(ObjectKind::Blob, &digest).await;
        let transaction = self.database.begin().await?;
        registry_image_blob::Entity::delete_by_id((image_id, digest.clone()))
            .exec(&transaction)
            .await?;
        let orphaned =
            !blob_linked_in_repository(&transaction, self.repository_id, &digest).await?;
        if orphaned {
            delete_object_row(&transaction, self.repository_id, ObjectKind::Blob, &digest).await?;
        }
        transaction.commit().await?;
        if orphaned {
            self.store
                .delete(&self.key(ObjectKind::Blob, &digest)?)
                .await?;
        }
        Ok(true)
    }

    /// Makes `digest` from `source` available in this image. Inside one
    /// repository only a link is recorded; across repositories the payload is
    /// duplicated through the store, which hard-links on a filesystem.
    pub async fn mount_blob(&self, source: &ImageStore, digest: &str) -> Result<bool, StoreError> {
        self.check_valid()?;
        source.check_valid()?;
        let digest = normalize_digest(digest)?;
        let (_first, _second) =
            lock_pair(&IMAGE_LOCKS, self.image_stripe(), source.image_stripe()).await;
        let image_id = self.ensure_image().await?;
        if self.linked_blob_size(image_id, &digest).await?.is_some() {
            return Ok(true);
        }
        let Some(source_image) = source.image_id().await? else {
            return Ok(false);
        };
        let Some(size) = source.linked_blob_size(source_image, &digest).await? else {
            return Ok(false);
        };
        let (_first_object, _second_object) = lock_pair(
            &OBJECT_LOCKS,
            self.object_stripe(ObjectKind::Blob, &digest),
            source.object_stripe(ObjectKind::Blob, &digest),
        )
        .await;
        if source.repository_id != self.repository_id {
            let source_key = source.key(ObjectKind::Blob, &digest)?;
            let destination_key = self.key(ObjectKind::Blob, &digest)?;
            if same_store(&self.store, &source.store) {
                self.store.duplicate(&source_key, &destination_key).await?;
            } else {
                let reader = source.store.read(&source_key).await?;
                self.store
                    .put_verified(&destination_key, blob_digest(&digest)?, reader)
                    .await?;
            }
        }
        self.record_blob(image_id, &digest, size).await?;
        Ok(true)
    }

    /// Records that this repository holds blob `digest` and this image
    /// exposes it.
    async fn record_blob(&self, image_id: Uuid, digest: &str, size: u64) -> Result<(), StoreError> {
        let now = Utc::now();
        let transaction = self.database.begin().await?;
        insert_object_row(
            &transaction,
            self.repository_id,
            ObjectKind::Blob,
            digest,
            size,
            now,
        )
        .await?;
        link_blob(&transaction, image_id, digest, now).await?;
        transaction.commit().await?;
        Ok(())
    }

    fn upload_path(&self, id: Uuid) -> PathBuf {
        self.uploads_dir.join(id.to_string())
    }

    pub async fn start_upload(&self, owner: Uuid) -> Result<Uuid, StoreError> {
        {
            let _guard = self.lock().await?;
            self.ensure_image().await?;
        }
        ensure_directory_all(&self.uploads_dir).await?;
        cleanup_expired_uploads(&self.uploads_dir).await?;
        for _ in 0..4 {
            let id = Uuid::new_v4();
            let directory = self.upload_path(id);
            let _upload_guard = upload_lock(&directory).await;
            match fs::create_dir(&directory).await {
                Ok(()) => {
                    if let Err(error) = async {
                        atomic_write(&directory.join("owner"), owner.to_string().as_bytes())
                            .await?;
                        atomic_write(&directory.join("image"), self.suffix.as_bytes()).await?;
                        atomic_write(
                            &directory.join("created"),
                            now_seconds().to_string().as_bytes(),
                        )
                        .await?;
                        let file = OpenOptions::new()
                            .write(true)
                            .create_new(true)
                            .open(directory.join("data"))
                            .await?;
                        file.sync_all().await?;
                        atomic_write(&directory.join("offset"), b"0").await
                    }
                    .await
                    {
                        let _ = fs::remove_dir_all(&directory).await;
                        return Err(error);
                    }
                    sync_directory(&self.uploads_dir).await?;
                    return Ok(id);
                }
                Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.into()),
            }
        }
        Err(StoreError::Invalid(
            "could not allocate upload identifier".to_owned(),
        ))
    }

    /// The directory of a live upload session this owner started for this
    /// image; expired sessions are removed.
    async fn open_upload(&self, id: Uuid, owner: Uuid) -> Result<Option<PathBuf>, StoreError> {
        let Some(directory) = self.upload_directory(id).await? else {
            return Ok(None);
        };
        if self.upload_expired(&directory).await? {
            let _ = fs::remove_dir_all(directory).await;
            return Ok(None);
        }
        let actual = read_small_file(&directory.join("owner")).await?;
        if actual.trim() != owner.to_string()
            || read_small_file(&directory.join("image")).await? != self.suffix.as_ref()
        {
            return Err(StoreError::UploadUnknown);
        }
        Ok(Some(directory))
    }

    pub async fn upload_status(&self, id: Uuid, owner: Uuid) -> Result<Option<u64>, StoreError> {
        self.check_valid()?;
        let _guard = upload_lock(&self.upload_path(id)).await;
        let directory = self
            .open_upload(id, owner)
            .await?
            .ok_or(StoreError::UploadUnknown)?;
        Ok(Some(self.reconcile_upload(&directory).await?))
    }

    pub async fn append_upload(
        &self,
        id: Uuid,
        owner: Uuid,
        body: Body,
        range: Option<(u64, u64)>,
    ) -> Result<u64, StoreError> {
        self.check_valid()?;
        let _guard = upload_lock(&self.upload_path(id)).await;
        let directory = self
            .open_upload(id, owner)
            .await?
            .ok_or(StoreError::UploadUnknown)?;
        let original = self.reconcile_upload(&directory).await?;
        let expected = if let Some((start, end)) = range {
            if end < start || start != original {
                return Err(StoreError::RangeInvalid);
            }
            Some(
                end.checked_sub(start)
                    .and_then(|length| length.checked_add(1))
                    .ok_or(StoreError::RangeInvalid)?,
            )
        } else {
            None
        };
        let data = directory.join("data");
        let mut output = OpenOptions::new().write(true).open(&data).await?;
        output.seek(SeekFrom::Start(original)).await?;
        let mut received = 0_u64;
        let mut stream = body.into_data_stream();
        let result = async {
            while let Some(chunk) = stream.next().await {
                let chunk =
                    chunk.map_err(|error| StoreError::Invalid(format!("upload body: {error}")))?;
                let chunk_len = u64::try_from(chunk.len()).map_err(|_| StoreError::TooLarge)?;
                received = received
                    .checked_add(chunk_len)
                    .ok_or(StoreError::TooLarge)?;
                let total = original.checked_add(received).ok_or(StoreError::TooLarge)?;
                if total > MAX_BLOB_BYTES {
                    return Err(StoreError::TooLarge);
                }
                if expected.is_some_and(|expected| received > expected) {
                    return Err(StoreError::RangeInvalid);
                }
                output.write_all(&chunk).await?;
            }
            if expected.is_some_and(|expected| expected != received) {
                return Err(StoreError::RangeInvalid);
            }
            output.flush().await?;
            output.sync_all().await?;
            let committed = original.checked_add(received).ok_or(StoreError::TooLarge)?;
            atomic_write(&directory.join("offset"), committed.to_string().as_bytes()).await?;
            Ok(committed)
        }
        .await;
        if result.is_err() {
            // A failed directory sync may occur after the offset rename became visible.
            // Restore that commit marker before truncating its corresponding data.
            atomic_write(&directory.join("offset"), original.to_string().as_bytes()).await?;
            output.set_len(original).await?;
            output.sync_all().await?;
        }
        result
    }

    /// Verifies the upload against `digest`, publishes it to the active store,
    /// and records it for this image.
    pub async fn finish_upload(
        &self,
        id: Uuid,
        owner: Uuid,
        digest: &str,
    ) -> Result<u64, StoreError> {
        self.check_valid()?;
        let _guard = upload_lock(&self.upload_path(id)).await;
        let directory = self
            .open_upload(id, owner)
            .await?
            .ok_or(StoreError::UploadUnknown)?;
        let size = self.reconcile_upload(&directory).await?;
        let digest = normalize_digest(digest)?;
        if size > MAX_BLOB_BYTES {
            return Err(StoreError::TooLarge);
        }
        let _image_guard = self.lock().await?;
        let image_id = self.ensure_image().await?;
        let _object = self.lock_object(ObjectKind::Blob, &digest).await;
        // The store verifies the upload and may move it into place.
        let published = self
            .store
            .put_file(
                &self.key(ObjectKind::Blob, &digest)?,
                blob_digest(&digest)?,
                &directory.join("data"),
            )
            .await?;
        self.record_blob(image_id, &digest, published.size).await?;
        fs::remove_dir_all(&directory).await?;
        sync_directory(&self.uploads_dir).await?;
        Ok(published.size)
    }

    pub async fn cancel_upload(&self, id: Uuid, owner: Uuid) -> Result<bool, StoreError> {
        self.check_valid()?;
        let _guard = upload_lock(&self.upload_path(id)).await;
        let Some(directory) = self.open_upload(id, owner).await? else {
            return Ok(false);
        };
        fs::remove_dir_all(directory).await?;
        sync_directory(&self.uploads_dir).await?;
        Ok(true)
    }

    pub async fn put_manifest(
        &self,
        reference: &str,
        media_type: &str,
        bytes: &[u8],
    ) -> Result<StoredManifest, StoreError> {
        let _guard = self.lock().await?;
        if !supported_manifest_media_type(media_type) {
            return Err(StoreError::Invalid(
                "manifest media type is unsupported".to_owned(),
            ));
        }
        if bytes.len() > MAX_MANIFEST_BYTES {
            return Err(StoreError::TooLarge);
        }
        let value: Value = serde_json::from_slice(bytes)
            .map_err(|error| StoreError::Invalid(format!("manifest is not valid JSON: {error}")))?;
        let image_id = self.image_id().await?;
        self.validate_manifest(image_id, &value, media_type).await?;
        let digest = format!("sha256:{}", hex::encode(Sha256::digest(bytes)));
        if reference.starts_with("sha256:") && normalize_digest(reference)? != digest {
            return Err(StoreError::DigestInvalid);
        }
        let tag = if reference.starts_with("sha256:") {
            None
        } else if valid_tag(reference) {
            Some(reference)
        } else {
            return Err(StoreError::Invalid(
                "manifest reference is invalid".to_owned(),
            ));
        };
        let image_id = match image_id {
            Some(id) => id,
            None => self.ensure_image().await?,
        };
        let _object = self.lock_object(ObjectKind::Manifest, &digest).await;
        if let Some(existing) = self.manifest(image_id, &digest).await?
            && existing.media_type != media_type
        {
            return Err(StoreError::Invalid(
                "manifest media type collision".to_owned(),
            ));
        }
        // Idempotent when another image of the repository holds the bytes.
        self.store
            .put_verified(
                &self.key(ObjectKind::Manifest, &digest)?,
                blob_digest(&digest)?,
                Box::pin(std::io::Cursor::new(bytes.to_vec())),
            )
            .await?;
        let facts = ManifestFacts::parse(&value, media_type);
        let now = Utc::now();
        let size = bytes.len() as u64;
        let transaction = self.database.begin().await?;
        insert_object_row(
            &transaction,
            self.repository_id,
            ObjectKind::Manifest,
            &digest,
            size,
            now,
        )
        .await?;
        record_manifest(
            &transaction,
            ManifestRecord {
                image_id,
                digest: &digest,
                media_type,
                size,
                facts: &facts,
                created_at: now,
                pushed_at: Some(now),
            },
        )
        .await?;
        if let Some(tag) = tag {
            record_tag(&transaction, image_id, tag, &digest, Some(now)).await?;
        }
        transaction.commit().await?;
        Ok(StoredManifest {
            digest,
            media_type: media_type.to_owned(),
            bytes: bytes.to_vec(),
        })
    }

    pub async fn get_manifest(
        &self,
        reference: &str,
    ) -> Result<Option<StoredManifest>, StoreError> {
        let _guard = self.lock().await?;
        validate_reference(reference)?;
        let Some(image_id) = self.image_id().await? else {
            return Ok(None);
        };
        let digest = if reference.starts_with("sha256:") {
            normalize_digest(reference)?
        } else {
            match registry_tag::Entity::find_by_id((image_id, reference.to_owned()))
                .one(&self.database)
                .await?
            {
                Some(tag) => tag.digest,
                None => return Ok(None),
            }
        };
        let Some(manifest) = self.manifest(image_id, &digest).await? else {
            return Ok(None);
        };
        let bytes = self
            .read_bounded(
                &self.key(ObjectKind::Manifest, &digest)?,
                MAX_MANIFEST_BYTES,
            )
            .await?;
        if format!("sha256:{}", hex::encode(Sha256::digest(&bytes))) != digest {
            return Err(StoreError::DigestInvalid);
        }
        Ok(Some(StoredManifest {
            digest,
            media_type: manifest.media_type,
            bytes,
        }))
    }

    /// Deleting a tag keeps its manifest; deleting a manifest by digest
    /// removes its tags and fails while a manifest of the image references it.
    pub async fn delete_manifest(&self, reference: &str) -> Result<bool, StoreError> {
        let _guard = self.lock().await?;
        validate_reference(reference)?;
        let Some(image_id) = self.image_id().await? else {
            return Ok(false);
        };
        if !reference.starts_with("sha256:") {
            let deleted = registry_tag::Entity::delete_by_id((image_id, reference.to_owned()))
                .exec(&self.database)
                .await?;
            return Ok(deleted.rows_affected > 0);
        }
        let digest = normalize_digest(reference)?;
        if self.manifest(image_id, &digest).await?.is_none() {
            return Ok(false);
        }
        if self.digest_referenced(image_id, &digest).await? {
            return Err(StoreError::Referenced);
        }
        let _object = self.lock_object(ObjectKind::Manifest, &digest).await;
        let transaction = self.database.begin().await?;
        registry_tag::Entity::delete_many()
            .filter(registry_tag::Column::ImageId.eq(image_id))
            .filter(registry_tag::Column::Digest.eq(&digest))
            .exec(&transaction)
            .await?;
        registry_manifest_reference::Entity::delete_many()
            .filter(registry_manifest_reference::Column::ImageId.eq(image_id))
            .filter(registry_manifest_reference::Column::ManifestDigest.eq(&digest))
            .exec(&transaction)
            .await?;
        registry_manifest::Entity::delete_by_id((image_id, digest.clone()))
            .exec(&transaction)
            .await?;
        let orphaned =
            !manifest_held_in_repository(&transaction, self.repository_id, &digest).await?;
        if orphaned {
            delete_object_row(
                &transaction,
                self.repository_id,
                ObjectKind::Manifest,
                &digest,
            )
            .await?;
        }
        transaction.commit().await?;
        if orphaned {
            self.store
                .delete(&self.key(ObjectKind::Manifest, &digest)?)
                .await?;
        }
        Ok(true)
    }

    /// Tags sorted by name, starting after `last`, at most `limit` of them.
    pub async fn tags_page(&self, last: &str, limit: u64) -> Result<Vec<String>, StoreError> {
        self.check_valid()?;
        let Some(image_id) = self.image_id().await? else {
            return Ok(Vec::new());
        };
        Ok(registry_tag::Entity::find()
            .filter(registry_tag::Column::ImageId.eq(image_id))
            .filter(registry_tag::Column::Name.gt(last))
            .order_by_asc(registry_tag::Column::Name)
            .limit(limit)
            .all(&self.database)
            .await?
            .into_iter()
            .map(|tag| tag.name)
            .collect())
    }

    pub async fn referrers(
        &self,
        digest: &str,
        artifact_type: Option<&str>,
    ) -> Result<Vec<Value>, StoreError> {
        self.check_valid()?;
        let digest = normalize_digest(digest)?;
        let Some(image_id) = self.image_id().await? else {
            return Ok(Vec::new());
        };
        let mut query = registry_manifest::Entity::find()
            .filter(registry_manifest::Column::ImageId.eq(image_id))
            .filter(registry_manifest::Column::SubjectDigest.eq(&digest));
        if let Some(artifact_type) = artifact_type {
            query = query.filter(registry_manifest::Column::ArtifactType.eq(artifact_type));
        }
        let manifests = query
            .order_by_asc(registry_manifest::Column::Digest)
            .all(&self.database)
            .await?;
        let mut result = Vec::with_capacity(manifests.len());
        for manifest in manifests {
            let mut descriptor = serde_json::Map::new();
            descriptor.insert("mediaType".to_owned(), Value::String(manifest.media_type));
            descriptor.insert("digest".to_owned(), Value::String(manifest.digest));
            descriptor.insert(
                "size".to_owned(),
                Value::from(database_size(manifest.size)?),
            );
            if let Some(artifact_type) = manifest.artifact_type {
                descriptor.insert("artifactType".to_owned(), Value::String(artifact_type));
            }
            if let Some(annotations) = manifest
                .annotations
                .as_deref()
                .and_then(|value| serde_json::from_str::<Value>(value).ok())
                .filter(Value::is_object)
            {
                descriptor.insert("annotations".to_owned(), annotations);
            }
            result.push(Value::Object(descriptor));
        }
        Ok(result)
    }

    async fn read_bounded(&self, key: &ObjectKey, limit: usize) -> Result<Vec<u8>, StoreError> {
        let mut reader = self.store.read(key).await?.take(limit as u64 + 1);
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        if bytes.len() > limit {
            return Err(StoreError::TooLarge);
        }
        Ok(bytes)
    }

    async fn upload_directory(&self, id: Uuid) -> Result<Option<PathBuf>, StoreError> {
        let directory = self.upload_path(id);
        match fs::symlink_metadata(&directory).await {
            Ok(metadata) if metadata.file_type().is_dir() => Ok(Some(directory)),
            Ok(metadata) if metadata.file_type().is_symlink() => Err(StoreError::Invalid(
                "upload directory symlink is unsupported".to_owned(),
            )),
            Ok(_) => Err(StoreError::UploadUnknown),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    async fn reconcile_upload(&self, directory: &Path) -> Result<u64, StoreError> {
        let data = directory.join("data");
        let Some(length) = regular_file_len(&data).await? else {
            return Err(StoreError::UploadUnknown);
        };
        let offset_path = directory.join("offset");
        let committed = match existing_regular_file(&offset_path).await? {
            Some(()) => read_small_file(&offset_path)
                .await?
                .trim()
                .parse::<u64>()
                .map_err(|_| StoreError::UploadUnknown)?,
            None => return Err(StoreError::UploadUnknown),
        };
        if committed > MAX_BLOB_BYTES || length < committed {
            return Err(StoreError::UploadUnknown);
        }
        if length > committed {
            let file = OpenOptions::new().write(true).open(&data).await?;
            file.set_len(committed).await?;
            file.sync_all().await?;
        }
        Ok(committed)
    }

    async fn upload_expired(&self, directory: &Path) -> Result<bool, StoreError> {
        let created = read_small_file(&directory.join("created")).await?;
        let Ok(created) = created.parse::<u64>() else {
            return Ok(true);
        };
        Ok(now_seconds().saturating_sub(created) > UPLOAD_TTL_SECONDS)
    }

    async fn validate_manifest(
        &self,
        image_id: Option<Uuid>,
        value: &Value,
        media_type: &str,
    ) -> Result<(), StoreError> {
        let object = value
            .as_object()
            .ok_or_else(|| StoreError::Invalid("manifest must be a JSON object".to_owned()))?;
        if object.get("schemaVersion").and_then(Value::as_u64) != Some(2) {
            return Err(StoreError::Invalid(
                "manifest schemaVersion must be 2".to_owned(),
            ));
        }
        let is_index = matches!(
            media_type,
            OCI_IMAGE_INDEX_MEDIA_TYPE | DOCKER_MANIFEST_LIST_MEDIA_TYPE
        );
        if is_index {
            if object.get("config").is_some()
                || object.get("layers").is_some()
                || object.get("blobs").is_some()
            {
                return Err(StoreError::Invalid(
                    "index contains image manifest fields".to_owned(),
                ));
            }
            let manifests = object
                .get("manifests")
                .and_then(Value::as_array)
                .ok_or_else(|| {
                    StoreError::Invalid("index manifests must be an array".to_owned())
                })?;
            for descriptor in manifests {
                self.validate_descriptor(image_id, descriptor, DescriptorKind::Manifest)
                    .await?;
            }
        } else {
            if object.get("manifests").is_some() || object.get("blobs").is_some() {
                return Err(StoreError::Invalid(
                    "image manifest contains index fields".to_owned(),
                ));
            }
            let config = object.get("config").ok_or_else(|| {
                StoreError::Invalid("image manifest config is missing".to_owned())
            })?;
            self.validate_descriptor(image_id, config, DescriptorKind::Blob)
                .await?;
            let layers = object
                .get("layers")
                .and_then(Value::as_array)
                .ok_or_else(|| {
                    StoreError::Invalid("image manifest layers must be an array".to_owned())
                })?;
            for descriptor in layers {
                self.validate_descriptor(image_id, descriptor, DescriptorKind::Blob)
                    .await?;
            }
        }
        if let Some(subject) = object.get("subject") {
            self.validate_descriptor(image_id, subject, DescriptorKind::Subject)
                .await?;
        }
        Ok(())
    }

    async fn validate_descriptor(
        &self,
        image_id: Option<Uuid>,
        descriptor: &Value,
        kind: DescriptorKind,
    ) -> Result<(), StoreError> {
        let object = descriptor.as_object().ok_or_else(|| {
            StoreError::Invalid("manifest descriptor is not an object".to_owned())
        })?;
        let digest = object
            .get("digest")
            .and_then(Value::as_str)
            .ok_or_else(|| StoreError::Invalid("descriptor digest is missing".to_owned()))?;
        let digest = normalize_digest(digest)?;
        let size = object
            .get("size")
            .and_then(Value::as_u64)
            .ok_or_else(|| StoreError::Invalid("descriptor size is missing".to_owned()))?;
        if size > MAX_BLOB_BYTES {
            return Err(StoreError::TooLarge);
        }
        let media_type = object
            .get("mediaType")
            .and_then(Value::as_str)
            .ok_or_else(|| StoreError::Invalid("descriptor mediaType is missing".to_owned()))?;
        if !valid_descriptor_media_type(media_type) {
            return Err(StoreError::Invalid(
                "descriptor media type is invalid".to_owned(),
            ));
        }
        if object.contains_key("urls") {
            let urls = object
                .get("urls")
                .and_then(Value::as_array)
                .ok_or_else(|| {
                    StoreError::Invalid("descriptor URLs must be an array".to_owned())
                })?;
            if !urls.is_empty() {
                return Err(StoreError::Invalid(
                    "remote descriptor URLs are unsupported".to_owned(),
                ));
            }
        }
        match kind {
            // Subjects may name manifests that are not pushed (yet).
            DescriptorKind::Subject => {}
            DescriptorKind::Manifest => {
                if !supported_manifest_media_type(media_type) {
                    return Err(StoreError::Invalid(
                        "index descriptor is not a manifest".to_owned(),
                    ));
                }
                let manifest = match image_id {
                    Some(image_id) => self.manifest(image_id, &digest).await?,
                    None => None,
                };
                let Some(manifest) = manifest else {
                    return Err(StoreError::ManifestBlobUnknown(digest));
                };
                if database_size(manifest.size)? != size || manifest.media_type != media_type {
                    return Err(StoreError::ManifestBlobUnknown(digest));
                }
            }
            DescriptorKind::Blob => {
                if supported_manifest_media_type(media_type) {
                    return Err(StoreError::Invalid(
                        "blob descriptor is a manifest".to_owned(),
                    ));
                }
                let actual = match image_id {
                    Some(image_id) => self.linked_blob_size(image_id, &digest).await?,
                    None => None,
                };
                if actual != Some(size) {
                    return Err(StoreError::ManifestBlobUnknown(digest));
                }
            }
        }
        Ok(())
    }
}

async fn find_image(
    connection: &impl ConnectionTrait,
    repository_id: Uuid,
    name: &str,
) -> Result<Option<Uuid>, StoreError> {
    Ok(registry_image::Entity::find()
        .filter(registry_image::Column::RepositoryId.eq(repository_id))
        .filter(registry_image::Column::Name.eq(name))
        .one(connection)
        .await?
        .map(|image| image.id))
}

/// The id of a repository's image `name`, recording the image when it is new.
pub(super) async fn ensure_image(
    connection: &impl ConnectionTrait,
    repository_id: Uuid,
    name: &str,
) -> Result<Uuid, StoreError> {
    if let Some(id) = find_image(connection, repository_id, name).await? {
        return Ok(id);
    }
    registry_image::Entity::insert(registry_image::ActiveModel {
        id: Set(Uuid::new_v4()),
        repository_id: Set(repository_id),
        name: Set(name.to_owned()),
        created_at: Set(Utc::now()),
    })
    .on_conflict_do_nothing_on([
        registry_image::Column::RepositoryId,
        registry_image::Column::Name,
    ])
    .exec_without_returning(connection)
    .await?;
    find_image(connection, repository_id, name)
        .await?
        .ok_or_else(|| StoreError::Invalid("image record disappeared".to_owned()))
}

/// Records that `repository_id` holds a payload; a no-op when it already does.
pub(super) async fn insert_object_row(
    connection: &impl ConnectionTrait,
    repository_id: Uuid,
    kind: ObjectKind,
    digest: &str,
    size: u64,
    now: DateTime<Utc>,
) -> Result<(), StoreError> {
    registry_object::Entity::insert(registry_object::ActiveModel {
        repository_id: Set(repository_id),
        kind: Set(kind.as_str().to_owned()),
        digest: Set(digest.to_owned()),
        size: Set(stored_size(size)?),
        created_at: Set(now),
    })
    .on_conflict_do_nothing_on([
        registry_object::Column::RepositoryId,
        registry_object::Column::Kind,
        registry_object::Column::Digest,
    ])
    .exec_without_returning(connection)
    .await?;
    Ok(())
}

/// Lets an image expose a blob its repository holds.
pub(super) async fn link_blob(
    connection: &impl ConnectionTrait,
    image_id: Uuid,
    digest: &str,
    now: DateTime<Utc>,
) -> Result<(), StoreError> {
    registry_image_blob::Entity::insert(registry_image_blob::ActiveModel {
        image_id: Set(image_id),
        digest: Set(digest.to_owned()),
        created_at: Set(now),
    })
    .on_conflict_do_nothing_on([
        registry_image_blob::Column::ImageId,
        registry_image_blob::Column::Digest,
    ])
    .exec_without_returning(connection)
    .await?;
    Ok(())
}

async fn delete_object_row(
    connection: &impl ConnectionTrait,
    repository_id: Uuid,
    kind: ObjectKind,
    digest: &str,
) -> Result<(), StoreError> {
    registry_object::Entity::delete_by_id((
        repository_id,
        kind.as_str().to_owned(),
        digest.to_owned(),
    ))
    .exec(connection)
    .await?;
    Ok(())
}

/// Image ids of a repository, for queries that span its images.
async fn repository_images(
    connection: &impl ConnectionTrait,
    repository_id: Uuid,
) -> Result<Vec<Uuid>, StoreError> {
    Ok(registry_image::Entity::find()
        .filter(registry_image::Column::RepositoryId.eq(repository_id))
        .all(connection)
        .await?
        .into_iter()
        .map(|image| image.id)
        .collect())
}

async fn blob_linked_in_repository(
    connection: &impl ConnectionTrait,
    repository_id: Uuid,
    digest: &str,
) -> Result<bool, StoreError> {
    let images = repository_images(connection, repository_id).await?;
    Ok(registry_image_blob::Entity::find()
        .filter(registry_image_blob::Column::ImageId.is_in(images))
        .filter(registry_image_blob::Column::Digest.eq(digest))
        .count(connection)
        .await?
        > 0)
}

async fn manifest_held_in_repository(
    connection: &impl ConnectionTrait,
    repository_id: Uuid,
    digest: &str,
) -> Result<bool, StoreError> {
    let images = repository_images(connection, repository_id).await?;
    Ok(registry_manifest::Entity::find()
        .filter(registry_manifest::Column::ImageId.is_in(images))
        .filter(registry_manifest::Column::Digest.eq(digest))
        .count(connection)
        .await?
        > 0)
}

/// One manifest of an image, as [`record_manifest`] stores it.
pub(super) struct ManifestRecord<'a> {
    pub image_id: Uuid,
    pub digest: &'a str,
    pub media_type: &'a str,
    pub size: u64,
    pub facts: &'a ManifestFacts,
    pub created_at: DateTime<Utc>,
    /// `None` keeps an existing record's push time.
    pub pushed_at: Option<DateTime<Utc>>,
}

/// Records a manifest of an image with its references. An existing record
/// keeps its creation time and takes the new push time when one is given.
pub(super) async fn record_manifest(
    connection: &impl ConnectionTrait,
    record: ManifestRecord<'_>,
) -> Result<(), StoreError> {
    let mut on_conflict = OnConflict::columns([
        registry_manifest::Column::ImageId,
        registry_manifest::Column::Digest,
    ]);
    if record.pushed_at.is_some() {
        on_conflict.update_column(registry_manifest::Column::PushedAt);
    } else {
        on_conflict.do_nothing();
    }
    registry_manifest::Entity::insert(registry_manifest::ActiveModel {
        image_id: Set(record.image_id),
        digest: Set(record.digest.to_owned()),
        media_type: Set(record.media_type.to_owned()),
        size: Set(stored_size(record.size)?),
        subject_digest: Set(record.facts.subject_digest.clone()),
        artifact_type: Set(record.facts.artifact_type.clone()),
        annotations: Set(record.facts.annotations.clone()),
        created_at: Set(record.created_at),
        pushed_at: Set(record.pushed_at),
    })
    .on_conflict(on_conflict)
    .try_insert()
    .exec_without_returning(connection)
    .await?;
    for referenced in &record.facts.references {
        registry_manifest_reference::Entity::insert(registry_manifest_reference::ActiveModel {
            image_id: Set(record.image_id),
            manifest_digest: Set(record.digest.to_owned()),
            referenced_digest: Set(referenced.clone()),
        })
        .on_conflict_do_nothing_on([
            registry_manifest_reference::Column::ImageId,
            registry_manifest_reference::Column::ManifestDigest,
            registry_manifest_reference::Column::ReferencedDigest,
        ])
        .exec_without_returning(connection)
        .await?;
    }
    Ok(())
}

/// Points `tag` at `digest`.
pub(super) async fn record_tag(
    connection: &impl ConnectionTrait,
    image_id: Uuid,
    tag: &str,
    digest: &str,
    updated_at: Option<DateTime<Utc>>,
) -> Result<(), StoreError> {
    registry_tag::Entity::insert(registry_tag::ActiveModel {
        image_id: Set(image_id),
        name: Set(tag.to_owned()),
        digest: Set(digest.to_owned()),
        updated_at: Set(updated_at),
    })
    .on_conflict(
        OnConflict::columns([registry_tag::Column::ImageId, registry_tag::Column::Name])
            .update_columns([
                registry_tag::Column::Digest,
                registry_tag::Column::UpdatedAt,
            ])
            .to_owned(),
    )
    .exec_without_returning(connection)
    .await?;
    Ok(())
}

/// Whether two handles point at the same store instance.
fn same_store(left: &Arc<dyn BlobStore>, right: &Arc<dyn BlobStore>) -> bool {
    std::ptr::addr_eq(Arc::as_ptr(left), Arc::as_ptr(right))
}

fn blob_digest(digest: &str) -> Result<BlobDigest, StoreError> {
    Ok(digest
        .strip_prefix("sha256:")
        .ok_or(StoreError::DigestInvalid)?
        .parse()?)
}

pub(super) fn valid_suffix(value: &str) -> bool {
    value.is_empty() || (value.len() <= 512 && value.split('/').all(super::valid_component))
}

pub(super) fn supported_manifest_media_type(value: &str) -> bool {
    matches!(
        value,
        OCI_IMAGE_MANIFEST_MEDIA_TYPE
            | OCI_IMAGE_INDEX_MEDIA_TYPE
            | DOCKER_MANIFEST_MEDIA_TYPE
            | DOCKER_MANIFEST_LIST_MEDIA_TYPE
    )
}

fn valid_descriptor_media_type(value: &str) -> bool {
    let Some((kind, subtype)) = value.split_once('/') else {
        return false;
    };
    !kind.is_empty()
        && !subtype.is_empty()
        && value.len() <= 255
        && !value
            .bytes()
            .any(|byte| byte.is_ascii_control() || byte.is_ascii_whitespace())
}

pub(super) fn valid_tag(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && (value.as_bytes()[0].is_ascii_alphanumeric() || value.as_bytes()[0] == b'_')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-'))
}

fn validate_reference(value: &str) -> Result<(), StoreError> {
    if value.starts_with("sha256:") {
        normalize_digest(value).map(|_| ())
    } else if valid_tag(value) {
        Ok(())
    } else {
        Err(StoreError::Invalid(
            "manifest reference is invalid".to_owned(),
        ))
    }
}

pub(super) fn normalize_digest(value: &str) -> Result<String, StoreError> {
    let Some(hex) = value.strip_prefix("sha256:") else {
        return Err(StoreError::DigestInvalid);
    };
    if hex.len() != 64 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(StoreError::DigestInvalid);
    }
    Ok(format!("sha256:{}", hex.to_ascii_lowercase()))
}

fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

async fn existing_regular_file(path: &Path) -> Result<Option<()>, StoreError> {
    match fs::symlink_metadata(path).await {
        Ok(metadata) if metadata.file_type().is_file() => Ok(Some(())),
        Ok(_) => Err(StoreError::Invalid("expected a regular file".to_owned())),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

async fn existing_directory(path: &Path) -> Result<Option<()>, StoreError> {
    match fs::symlink_metadata(path).await {
        Ok(metadata) if metadata.file_type().is_dir() => Ok(Some(())),
        Ok(_) => Err(StoreError::Invalid(
            "expected a regular directory".to_owned(),
        )),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

/// Creates a private directory and its parents.
async fn ensure_directory_all(path: &Path) -> Result<(), StoreError> {
    if existing_directory(path).await?.is_some() {
        return Ok(());
    }
    crate::filesystem::create_private_directory_async(path).await?;
    if let Some(parent) = path.parent() {
        sync_directory(parent).await?;
    }
    Ok(())
}

async fn sync_directory(path: &Path) -> Result<(), StoreError> {
    File::open(path).await?.sync_all().await?;
    Ok(())
}

async fn regular_file_len(path: &Path) -> Result<Option<u64>, StoreError> {
    match fs::symlink_metadata(path).await {
        Ok(metadata) => {
            if !metadata.file_type().is_file() {
                return Err(StoreError::Invalid("expected a regular file".to_owned()));
            }
            Ok(Some(metadata.len()))
        }
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

pub(super) async fn read_small_file(path: &Path) -> Result<String, StoreError> {
    let bytes = read_bounded_file(path, 1024).await?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

pub(super) async fn read_bounded_file(path: &Path, limit: usize) -> Result<Vec<u8>, StoreError> {
    let metadata = fs::symlink_metadata(path).await?;
    if !metadata.file_type().is_file() {
        return Err(StoreError::Invalid("expected a regular file".to_owned()));
    }
    if metadata.len() > limit as u64 {
        return Err(StoreError::TooLarge);
    }
    let file = File::open(path).await?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    let read = file
        .take((limit as u64).saturating_add(1))
        .read_to_end(&mut bytes)
        .await?;
    if read > limit {
        return Err(StoreError::TooLarge);
    }
    Ok(bytes)
}

async fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), StoreError> {
    let parent = path
        .parent()
        .ok_or_else(|| StoreError::Invalid("storage path has no parent".to_owned()))?;
    let temporary = parent.join(format!(".tmp-{}", Uuid::new_v4()));
    let result = async {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .await?;
        file.write_all(bytes).await?;
        file.flush().await?;
        file.sync_all().await?;
        drop(file);
        fs::rename(&temporary, path).await?;
        sync_directory(parent).await
    }
    .await;
    if result.is_err() {
        let _ = fs::remove_file(&temporary).await;
    }
    result
}

async fn cleanup_expired_uploads(root: &Path) -> Result<(), StoreError> {
    if existing_directory(root).await?.is_none() {
        return Ok(());
    }
    let mut entries = fs::read_dir(root).await?;
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).await?;
        if metadata.file_type().is_symlink() {
            return Err(StoreError::Invalid(
                "upload directory symlink is unsupported".to_owned(),
            ));
        }
        if !metadata.file_type().is_dir() {
            continue;
        }
        let Ok(_guard) = upload_mutex(&path).try_lock_owned() else {
            continue;
        };
        let expired = match read_small_file(&path.join("created")).await {
            Ok(created) => created.trim().parse::<u64>().map_or(true, |created| {
                now_seconds().saturating_sub(created) > UPLOAD_TTL_SECONDS
            }),
            Err(_) => true,
        };
        if expired {
            let _ = fs::remove_dir_all(&path).await;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
