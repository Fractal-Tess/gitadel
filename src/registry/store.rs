use std::{
    collections::{HashMap, hash_map::DefaultHasher},
    fmt,
    hash::{Hash, Hasher},
    io::ErrorKind,
    path::{Path, PathBuf},
    sync::{Arc, LazyLock, Mutex as StdMutex, Weak},
    time::{SystemTime, UNIX_EPOCH},
};

use axum::body::Body;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use tokio::{
    fs::{self, File, OpenOptions},
    io::{AsyncReadExt as _, AsyncSeekExt as _, AsyncWriteExt as _, SeekFrom},
    sync::{Mutex, MutexGuard, OwnedMutexGuard},
};
use uuid::Uuid;

pub(super) const MAX_MANIFEST_BYTES: usize = 4 * 1024 * 1024;
pub(super) const MAX_BLOB_BYTES: u64 = 10 * 1024 * 1024 * 1024;
pub(super) const UPLOAD_TTL_SECONDS: u64 = 86_400;
const LOCK_STRIPES: usize = 64;
const IO_BUFFER_BYTES: usize = 64 * 1024;

const OCI_IMAGE_MANIFEST_MEDIA_TYPE: &str = "application/vnd.oci.image.manifest.v1+json";
const OCI_IMAGE_INDEX_MEDIA_TYPE: &str = "application/vnd.oci.image.index.v1+json";
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

#[derive(Clone, Debug, Default)]
pub(super) struct RegistryStore;

#[derive(Clone, Debug)]
pub(super) struct ImageStore {
    image_dir: PathBuf,
    suffix: Arc<str>,
    valid: bool,
}

#[derive(Clone, Debug)]
pub(super) struct StoredManifest {
    pub digest: String,
    pub media_type: String,
    pub bytes: Vec<u8>,
}
#[derive(Debug, Serialize, Deserialize)]
struct ManifestMeta {
    media_type: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct ReferenceMeta {
    reference: String,
    digest: String,
}

static IMAGE_LOCKS: LazyLock<Vec<Arc<Mutex<()>>>> = LazyLock::new(|| {
    (0..LOCK_STRIPES)
        .map(|_| Arc::new(Mutex::new(())))
        .collect()
});
static UPLOAD_LOCKS: LazyLock<StdMutex<HashMap<PathBuf, Weak<Mutex<()>>>>> =
    LazyLock::new(|| StdMutex::new(HashMap::new()));

fn lock_index(path: &Path) -> usize {
    let mut hasher = DefaultHasher::new();
    path.hash(&mut hasher);
    (hasher.finish() as usize) % LOCK_STRIPES
}

async fn image_lock(path: &Path) -> MutexGuard<'static, ()> {
    IMAGE_LOCKS[lock_index(path)].lock().await
}

async fn image_locks(
    first: &Path,
    second: &Path,
) -> (MutexGuard<'static, ()>, Option<MutexGuard<'static, ()>>) {
    let first_index = lock_index(first);
    let second_index = lock_index(second);
    if first_index == second_index {
        return (IMAGE_LOCKS[first_index].lock().await, None);
    }
    if first_index < second_index {
        (
            IMAGE_LOCKS[first_index].lock().await,
            Some(IMAGE_LOCKS[second_index].lock().await),
        )
    } else {
        (
            IMAGE_LOCKS[second_index].lock().await,
            Some(IMAGE_LOCKS[first_index].lock().await),
        )
    }
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

impl RegistryStore {
    pub fn new() -> Self {
        Self
    }

    pub fn image(&self, repository_path: PathBuf, suffix: &str) -> ImageStore {
        let valid = valid_suffix(suffix);
        let suffix: Arc<str> = Arc::from(suffix.to_owned());
        let image_hash = hex_digest(suffix.as_bytes());
        ImageStore {
            image_dir: repository_path
                .join("gitadel-registry")
                .join("images")
                .join(image_hash),
            suffix,
            valid,
        }
    }

    pub async fn list_images(&self, repository_path: &Path) -> Result<Vec<String>, StoreError> {
        let mut result = Vec::new();
        if existing_directory(repository_path).await?.is_none() {
            return Ok(result);
        }
        let registry = repository_path.join("gitadel-registry");
        if existing_directory(&registry).await?.is_none() {
            return Ok(result);
        }
        let root = registry.join("images");
        if existing_directory(&root).await?.is_none() {
            return Ok(result);
        }
        let mut entries = fs::read_dir(&root).await?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).await?;
            if metadata.file_type().is_symlink() {
                return Err(StoreError::Invalid(
                    "image directory symlink is unsupported".to_owned(),
                ));
            }
            if !metadata.file_type().is_dir() {
                continue;
            }
            let suffix_path = path.join("suffix");
            let Some(_) = existing_regular_file(&suffix_path).await? else {
                continue;
            };
            let suffix = read_small_file(&suffix_path).await?;
            if valid_suffix(&suffix) {
                result.push(suffix);
            }
        }
        result.sort();
        Ok(result)
    }
}

impl ImageStore {
    async fn lock(&self) -> Result<MutexGuard<'static, ()>, StoreError> {
        if !self.valid {
            return Err(StoreError::Invalid("image name is invalid".to_owned()));
        }
        Ok(image_lock(&self.image_dir).await)
    }

    fn root(&self) -> PathBuf {
        self.image_dir.clone()
    }

    fn blobs_dir(&self) -> PathBuf {
        self.root().join("blobs")
    }

    fn manifests_dir(&self) -> PathBuf {
        self.root().join("manifests")
    }

    fn refs_dir(&self) -> PathBuf {
        self.root().join("refs")
    }

    fn uploads_dir(&self) -> PathBuf {
        self.root().join("uploads")
    }

    fn repository_root(&self) -> PathBuf {
        self.image_dir
            .parent()
            .and_then(Path::parent)
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.image_dir.clone())
    }

    async fn ensure_image(&self) -> Result<(), StoreError> {
        let repository_root = self.repository_root();
        if existing_directory(&repository_root).await?.is_none() {
            return Err(StoreError::Invalid("repository root is missing".to_owned()));
        }
        let registry = repository_root.join("gitadel-registry");
        ensure_directory(&registry).await?;
        let images = registry.join("images");
        ensure_directory(&images).await?;
        ensure_directory(&self.image_dir).await?;
        let suffix = self.root().join("suffix");
        if existing_regular_file(&suffix).await?.is_some() {
            if read_small_file(&suffix).await? != self.suffix.as_ref() {
                return Err(StoreError::Invalid(
                    "image suffix hash collision".to_owned(),
                ));
            }
        } else {
            atomic_write(&suffix, self.suffix.as_bytes()).await?;
        }
        ensure_directory(&self.blobs_dir()).await?;
        ensure_directory(&self.manifests_dir()).await?;
        ensure_directory(&self.manifests_dir().join("objects")).await?;
        ensure_directory(&self.refs_dir()).await?;
        ensure_directory(&self.uploads_dir()).await?;
        Ok(())
    }

    async fn blob_path(&self, digest: &str) -> Result<PathBuf, StoreError> {
        let digest = normalize_digest(digest)?;
        let hash = digest
            .strip_prefix("sha256:")
            .ok_or(StoreError::DigestInvalid)?;
        Ok(self
            .blobs_dir()
            .join(&hash[..2])
            .join(&hash[2..4])
            .join(hash))
    }

    async fn manifest_object_path(&self, digest: &str) -> Result<PathBuf, StoreError> {
        let digest = normalize_digest(digest)?;
        Ok(self.manifests_dir().join("objects").join(
            digest
                .strip_prefix("sha256:")
                .ok_or(StoreError::DigestInvalid)?,
        ))
    }

    fn reference_path(&self, reference: &str) -> PathBuf {
        self.refs_dir()
            .join(hex_digest(reference.as_bytes()))
            .with_extension("json")
    }

    async fn get_reference(&self, reference: &str) -> Result<Option<ReferenceMeta>, StoreError> {
        let path = self.reference_path(reference);
        let metadata = match fs::symlink_metadata(&path).await {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        if !metadata.file_type().is_file() {
            return Err(StoreError::Invalid(
                "reference metadata is not a file".to_owned(),
            ));
        }
        let bytes = read_bounded_file(&path, 4096).await?;
        let value: ReferenceMeta = serde_json::from_slice(&bytes)
            .map_err(|error| StoreError::Invalid(format!("invalid reference metadata: {error}")))?;
        if value.reference != reference {
            return Err(StoreError::Invalid(
                "reference metadata collision".to_owned(),
            ));
        }
        normalize_digest(&value.digest)?;
        Ok(Some(value))
    }

    pub async fn blob_size(&self, digest: &str) -> Result<Option<u64>, StoreError> {
        let _guard = self.lock().await?;
        let path = self.blob_path(digest).await?;
        regular_file_len(&path).await
    }

    pub async fn open_blob(&self, digest: &str) -> Result<Option<(File, u64)>, StoreError> {
        let _guard = self.lock().await?;
        let path = self.blob_path(digest).await?;
        let Some(size) = regular_file_len(&path).await? else {
            return Ok(None);
        };
        Ok(Some((File::open(path).await?, size)))
    }

    pub async fn delete_blob(&self, digest: &str) -> Result<bool, StoreError> {
        let _guard = self.lock().await?;
        let path = self.blob_path(digest).await?;
        if !regular_file_exists(&path).await? {
            return Ok(false);
        }
        let digest = normalize_digest(digest)?;
        if self.digest_referenced(&digest).await? {
            return Err(StoreError::Referenced);
        }
        fs::remove_file(&path).await?;
        if let Some(parent) = path.parent() {
            sync_directory(parent).await?;
        }
        Ok(true)
    }

    pub async fn mount_blob(&self, source: &ImageStore, digest: &str) -> Result<bool, StoreError> {
        if !self.valid || !source.valid {
            return Err(StoreError::Invalid("image name is invalid".to_owned()));
        }
        let (_first_guard, _second_guard) = image_locks(&self.image_dir, &source.image_dir).await;
        self.ensure_image().await?;
        let source_path = source.blob_path(digest).await?;
        let destination = self.blob_path(digest).await?;
        if regular_file_exists(&destination).await? {
            return Ok(true);
        }
        let Some(parent) = destination.parent() else {
            return Err(StoreError::Invalid("blob path has no parent".to_owned()));
        };
        if let Some(first) = parent.parent() {
            ensure_directory(first).await?;
        }
        ensure_directory(parent).await?;
        if !regular_file_exists(&source_path).await? {
            return Ok(false);
        }
        match fs::hard_link(source_path, &destination).await {
            Ok(()) => {
                sync_directory(parent).await?;
                Ok(true)
            }
            Err(error) if error.kind() == ErrorKind::AlreadyExists => Ok(true),
            Err(error) if error.kind() == ErrorKind::CrossesDevices => Ok(false),
            Err(error) => Err(error.into()),
        }
    }

    pub async fn start_upload(&self, owner: Uuid) -> Result<Uuid, StoreError> {
        if !self.valid {
            return Err(StoreError::Invalid("image name is invalid".to_owned()));
        }
        {
            let _guard = self.lock().await?;
            self.ensure_image().await?;
        }
        cleanup_expired_uploads(&self.uploads_dir()).await?;
        for _ in 0..4 {
            let id = Uuid::new_v4();
            let directory = self.uploads_dir().join(id.to_string());
            let _upload_guard = upload_lock(&directory).await;
            let _image_guard = self.lock().await?;
            self.ensure_image().await?;
            match fs::create_dir(&directory).await {
                Ok(()) => {
                    if let Err(error) = async {
                        atomic_write(&directory.join("owner"), owner.to_string().as_bytes())
                            .await?;
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
                    sync_directory(&self.uploads_dir()).await?;
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

    pub async fn upload_status(&self, id: Uuid, owner: Uuid) -> Result<Option<u64>, StoreError> {
        if !self.valid {
            return Err(StoreError::Invalid("image name is invalid".to_owned()));
        }
        let upload_path = self.uploads_dir().join(id.to_string());
        let _guard = upload_lock(&upload_path).await;
        let Some(directory) = self.upload_directory(id).await? else {
            return Err(StoreError::UploadUnknown);
        };
        if self.upload_expired(&directory).await? {
            let _ = fs::remove_dir_all(directory).await;
            return Err(StoreError::UploadUnknown);
        }
        self.check_upload_owner(&directory, owner).await?;
        Ok(Some(self.reconcile_upload(&directory).await?))
    }

    pub async fn append_upload(
        &self,
        id: Uuid,
        owner: Uuid,
        body: Body,
        range: Option<(u64, u64)>,
    ) -> Result<u64, StoreError> {
        if !self.valid {
            return Err(StoreError::Invalid("image name is invalid".to_owned()));
        }
        let upload_path = self.uploads_dir().join(id.to_string());
        let _guard = upload_lock(&upload_path).await;
        let Some(directory) = self.upload_directory(id).await? else {
            return Err(StoreError::UploadUnknown);
        };
        if self.upload_expired(&directory).await? {
            let _ = fs::remove_dir_all(directory).await;
            return Err(StoreError::UploadUnknown);
        }
        self.check_upload_owner(&directory, owner).await?;
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

    pub async fn finish_upload(
        &self,
        id: Uuid,
        owner: Uuid,
        digest: &str,
    ) -> Result<u64, StoreError> {
        if !self.valid {
            return Err(StoreError::Invalid("image name is invalid".to_owned()));
        }
        let upload_path = self.uploads_dir().join(id.to_string());
        let _guard = upload_lock(&upload_path).await;
        let Some(directory) = self.upload_directory(id).await? else {
            return Err(StoreError::UploadUnknown);
        };
        if self.upload_expired(&directory).await? {
            let _ = fs::remove_dir_all(directory).await;
            return Err(StoreError::UploadUnknown);
        }
        self.check_upload_owner(&directory, owner).await?;
        let size = self.reconcile_upload(&directory).await?;
        let digest = normalize_digest(digest)?;
        let _image_guard = self.lock().await?;
        self.ensure_image().await?;
        let data = directory.join("data");
        if size > MAX_BLOB_BYTES {
            return Err(StoreError::TooLarge);
        }
        verify_file_digest(&data, &digest).await?;
        let destination = self.blob_path(&digest).await?;
        if regular_file_exists(&destination).await? {
            let existing_size =
                regular_file_len(&destination)
                    .await?
                    .ok_or(StoreError::Invalid(
                        "blob disappeared while finishing upload".to_owned(),
                    ))?;
            verify_file_digest(&destination, &digest).await?;
            fs::remove_dir_all(&directory).await?;
            sync_directory(&self.uploads_dir()).await?;
            return Ok(existing_size);
        }
        let parent = destination
            .parent()
            .ok_or_else(|| StoreError::Invalid("blob path has no parent".to_owned()))?;
        if let Some(first) = parent.parent() {
            ensure_directory(first).await?;
        }
        ensure_directory(parent).await?;
        match fs::rename(&data, &destination).await {
            Ok(()) => {
                sync_directory(parent).await?;
                fs::remove_dir_all(directory).await?;
                sync_directory(&self.uploads_dir()).await?;
                Ok(size)
            }
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                fs::remove_dir_all(directory).await?;
                let destination_size = regular_file_len(&destination).await?.unwrap_or(size);
                verify_file_digest(&destination, &digest).await?;
                Ok(destination_size)
            }
            Err(error) => Err(error.into()),
        }
    }

    pub async fn cancel_upload(&self, id: Uuid, owner: Uuid) -> Result<bool, StoreError> {
        if !self.valid {
            return Err(StoreError::Invalid("image name is invalid".to_owned()));
        }
        let upload_path = self.uploads_dir().join(id.to_string());
        let _guard = upload_lock(&upload_path).await;
        let Some(directory) = self.upload_directory(id).await? else {
            return Ok(false);
        };
        if self.upload_expired(&directory).await? {
            let _ = fs::remove_dir_all(directory).await;
            return Ok(false);
        }
        self.check_upload_owner(&directory, owner).await?;
        fs::remove_dir_all(directory).await?;
        sync_directory(&self.uploads_dir()).await?;
        Ok(true)
    }

    pub async fn put_manifest(
        &self,
        reference: &str,
        media_type: &str,
        bytes: &[u8],
    ) -> Result<StoredManifest, StoreError> {
        let _guard = self.lock().await?;
        validate_reference(reference)?;
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
        self.validate_manifest(&value, media_type).await?;
        let digest = format!("sha256:{:x}", Sha256::digest(bytes));
        if reference.starts_with("sha256:") && normalize_digest(reference)? != digest {
            return Err(StoreError::DigestInvalid);
        }
        self.ensure_image().await?;
        let object = self.manifest_object_path(&digest).await?;
        let metadata_path = object.with_extension("meta");
        let metadata_bytes = serde_json::to_vec(&ManifestMeta {
            media_type: media_type.to_owned(),
        })
        .map_err(|error| StoreError::Invalid(error.to_string()))?;
        if existing_regular_file(&object).await?.is_some() {
            let existing = read_bounded_file(&object, MAX_MANIFEST_BYTES).await?;
            if existing != bytes {
                return Err(StoreError::DigestInvalid);
            }
            if existing_regular_file(&metadata_path).await?.is_some() {
                if self.read_manifest_media_type(&digest).await? != media_type {
                    return Err(StoreError::Invalid(
                        "manifest media type collision".to_owned(),
                    ));
                }
            } else {
                atomic_write(&metadata_path, &metadata_bytes).await?;
            }
        } else {
            atomic_write(&metadata_path, &metadata_bytes).await?;
            atomic_write(&object, bytes).await?;
        }
        let digest_reference = ReferenceMeta {
            reference: digest.clone(),
            digest: digest.clone(),
        };
        atomic_write(
            &self.reference_path(&digest),
            &serde_json::to_vec(&digest_reference)
                .map_err(|error| StoreError::Invalid(error.to_string()))?,
        )
        .await?;
        if !reference.starts_with("sha256:") {
            atomic_write(
                &self.reference_path(reference),
                &serde_json::to_vec(&ReferenceMeta {
                    reference: reference.to_owned(),
                    digest: digest.clone(),
                })
                .map_err(|error| StoreError::Invalid(error.to_string()))?,
            )
            .await?;
        }
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
        let reference = if reference.starts_with("sha256:") {
            normalize_digest(reference)?
        } else {
            reference.to_owned()
        };
        let Some(reference_meta) = self.get_reference(&reference).await? else {
            return Ok(None);
        };
        self.read_manifest(&reference_meta.digest).await.map(Some)
    }
    pub async fn delete_manifest(&self, reference: &str) -> Result<bool, StoreError> {
        let _guard = self.lock().await?;
        validate_reference(reference)?;
        let reference = if reference.starts_with("sha256:") {
            normalize_digest(reference)?
        } else {
            reference.to_owned()
        };
        let Some(reference_meta) = self.get_reference(&reference).await? else {
            return Ok(false);
        };
        if !reference.starts_with("sha256:") {
            fs::remove_file(self.reference_path(&reference)).await?;
            sync_directory(&self.refs_dir()).await?;
            return Ok(true);
        }
        let digest = reference_meta.digest;
        if self.digest_referenced(&digest).await? {
            return Err(StoreError::Referenced);
        }
        let mut entries = fs::read_dir(self.refs_dir()).await?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).await?;
            if !metadata.file_type().is_file() {
                continue;
            }
            let Ok(bytes) = read_bounded_file(&path, 4096).await else {
                continue;
            };
            let Ok(meta) = serde_json::from_slice::<ReferenceMeta>(&bytes) else {
                continue;
            };
            if meta.digest == digest {
                fs::remove_file(path).await?;
            }
        }
        sync_directory(&self.refs_dir()).await?;
        let object = self.manifest_object_path(&digest).await?;
        fs::remove_file(&object).await?;
        fs::remove_file(object.with_extension("meta")).await?;
        sync_directory(&self.manifests_dir().join("objects")).await?;
        Ok(true)
    }

    pub async fn tags(&self) -> Result<Vec<String>, StoreError> {
        let _guard = self.lock().await?;
        let mut result = Vec::new();
        let mut entries = match fs::read_dir(self.refs_dir()).await {
            Ok(entries) => entries,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(result),
            Err(error) => return Err(error.into()),
        };
        while let Some(entry) = entries.next_entry().await? {
            let metadata = fs::symlink_metadata(entry.path()).await?;
            if !metadata.file_type().is_file() {
                continue;
            }
            let Ok(bytes) = read_bounded_file(&entry.path(), 4096).await else {
                continue;
            };
            let Ok(meta) = serde_json::from_slice::<ReferenceMeta>(&bytes) else {
                continue;
            };
            if !meta.reference.starts_with("sha256:") && valid_tag(&meta.reference) {
                result.push(meta.reference);
            }
        }
        result.sort();
        Ok(result)
    }

    pub async fn referrers(
        &self,
        digest: &str,
        artifact_type: Option<&str>,
    ) -> Result<Vec<Value>, StoreError> {
        let _guard = self.lock().await?;
        let digest = normalize_digest(digest)?;
        let mut result = Vec::new();
        let mut entries = match fs::read_dir(self.manifests_dir().join("objects")).await {
            Ok(entries) => entries,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(result),
            Err(error) => return Err(error.into()),
        };
        while let Some(entry) = entries.next_entry().await? {
            let metadata = fs::symlink_metadata(entry.path()).await?;
            if !metadata.file_type().is_file()
                || entry.file_name().to_string_lossy().ends_with(".meta")
            {
                continue;
            }
            let object_digest = format!("sha256:{}", entry.file_name().to_string_lossy());
            if normalize_digest(&object_digest).is_err()
                || self.get_reference(&object_digest).await?.is_none()
            {
                continue;
            }
            let bytes = read_bounded_file(&entry.path(), MAX_MANIFEST_BYTES).await?;
            let value: Value = match serde_json::from_slice(&bytes) {
                Ok(value) => value,
                Err(_) => continue,
            };
            let Some(subject) = value.get("subject").and_then(Value::as_object) else {
                continue;
            };
            let Some(subject_digest) = subject
                .get("digest")
                .and_then(Value::as_str)
                .and_then(|value| normalize_digest(value).ok())
            else {
                continue;
            };
            if subject_digest != digest {
                continue;
            }
            let media_type = self.read_manifest_media_type(&object_digest).await?;
            let value_artifact_type = value
                .get("artifactType")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .or_else(|| match media_type.as_str() {
                    OCI_IMAGE_MANIFEST_MEDIA_TYPE | DOCKER_MANIFEST_MEDIA_TYPE => {
                        value.get("config")?.get("mediaType")?.as_str()
                    }
                    _ => None,
                });
            if artifact_type.is_some() && value_artifact_type != artifact_type {
                continue;
            }
            let mut descriptor = serde_json::Map::new();
            descriptor.insert("mediaType".to_owned(), Value::String(media_type));
            descriptor.insert("digest".to_owned(), Value::String(object_digest));
            descriptor.insert("size".to_owned(), Value::from(bytes.len() as u64));
            if let Some(artifact_type) = value_artifact_type {
                descriptor.insert(
                    "artifactType".to_owned(),
                    Value::String(artifact_type.to_owned()),
                );
            }
            if let Some(annotations) = value.get("annotations").and_then(Value::as_object) {
                descriptor.insert("annotations".to_owned(), Value::Object(annotations.clone()));
            }
            result.push(Value::Object(descriptor));
        }
        result.sort_by(|left, right| {
            left.get("digest")
                .and_then(Value::as_str)
                .cmp(&right.get("digest").and_then(Value::as_str))
        });
        Ok(result)
    }

    async fn upload_directory(&self, id: Uuid) -> Result<Option<PathBuf>, StoreError> {
        let directory = self.uploads_dir().join(id.to_string());
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
            Some(_) => read_small_file(&offset_path)
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

    async fn check_upload_owner(&self, directory: &Path, owner: Uuid) -> Result<(), StoreError> {
        let actual = read_small_file(&directory.join("owner")).await?;
        if actual.trim() != owner.to_string() {
            return Err(StoreError::UploadUnknown);
        }
        Ok(())
    }

    async fn validate_manifest(&self, value: &Value, media_type: &str) -> Result<(), StoreError> {
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
                self.validate_descriptor(descriptor, DescriptorKind::Manifest, true)
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
            self.validate_descriptor(config, DescriptorKind::Blob, true)
                .await?;
            let layers = object
                .get("layers")
                .and_then(Value::as_array)
                .ok_or_else(|| {
                    StoreError::Invalid("image manifest layers must be an array".to_owned())
                })?;
            for descriptor in layers {
                self.validate_descriptor(descriptor, DescriptorKind::Blob, true)
                    .await?;
            }
        }
        if let Some(subject) = object.get("subject") {
            self.validate_descriptor(subject, DescriptorKind::Subject, false)
                .await?;
        }
        Ok(())
    }

    async fn validate_descriptor(
        &self,
        descriptor: &Value,
        kind: DescriptorKind,
        require_local: bool,
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
        if !require_local {
            return Ok(());
        }
        match kind {
            DescriptorKind::Manifest => {
                if !supported_manifest_media_type(media_type) {
                    return Err(StoreError::Invalid(
                        "index descriptor is not a manifest".to_owned(),
                    ));
                }
                let object_path = self.manifest_object_path(&digest).await?;
                let Some(actual) = regular_file_len(&object_path).await? else {
                    return Err(StoreError::ManifestBlobUnknown(digest));
                };
                if actual != size || self.read_manifest_media_type(&digest).await? != media_type {
                    return Err(StoreError::ManifestBlobUnknown(digest));
                }
                verify_file_digest(&object_path, &digest).await?;
            }
            DescriptorKind::Blob => {
                if supported_manifest_media_type(media_type) {
                    return Err(StoreError::Invalid(
                        "blob descriptor is a manifest".to_owned(),
                    ));
                }
                let blob = self.blob_path(&digest).await?;
                let Some(actual) = regular_file_len(&blob).await? else {
                    return Err(StoreError::ManifestBlobUnknown(digest));
                };
                if actual != size {
                    return Err(StoreError::ManifestBlobUnknown(digest));
                }
            }
            DescriptorKind::Subject => {
                unreachable!("subject descriptors do not require local objects")
            }
        }
        Ok(())
    }

    async fn read_manifest(&self, digest: &str) -> Result<StoredManifest, StoreError> {
        let digest = normalize_digest(digest)?;
        let path = self.manifest_object_path(&digest).await?;
        let bytes = read_bounded_file(&path, MAX_MANIFEST_BYTES).await?;
        let actual = format!("sha256:{:x}", Sha256::digest(&bytes));
        if actual != digest {
            return Err(StoreError::DigestInvalid);
        }
        let media_type = self.read_manifest_media_type(&digest).await?;
        if !supported_manifest_media_type(&media_type) {
            return Err(StoreError::Invalid(
                "manifest media type is unsupported".to_owned(),
            ));
        }
        Ok(StoredManifest {
            digest,
            media_type,
            bytes,
        })
    }

    async fn read_manifest_media_type(&self, digest: &str) -> Result<String, StoreError> {
        let path = self
            .manifest_object_path(digest)
            .await?
            .with_extension("meta");
        let bytes = read_bounded_file(&path, 1024).await?;
        let metadata: ManifestMeta = serde_json::from_slice(&bytes)
            .map_err(|error| StoreError::Invalid(format!("invalid manifest metadata: {error}")))?;
        if !supported_manifest_media_type(&metadata.media_type) {
            return Err(StoreError::Invalid(
                "manifest media type is unsupported".to_owned(),
            ));
        }
        Ok(metadata.media_type)
    }

    async fn digest_referenced(&self, digest: &str) -> Result<bool, StoreError> {
        let mut entries = match fs::read_dir(self.manifests_dir().join("objects")).await {
            Ok(entries) => entries,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error.into()),
        };
        while let Some(entry) = entries.next_entry().await? {
            let metadata = fs::symlink_metadata(entry.path()).await?;
            if !metadata.file_type().is_file()
                || entry.file_name().to_string_lossy().ends_with(".meta")
            {
                continue;
            }
            let object_digest = format!("sha256:{}", entry.file_name().to_string_lossy());
            if normalize_digest(&object_digest).is_err()
                || self.get_reference(&object_digest).await?.is_none()
            {
                continue;
            }
            let bytes = read_bounded_file(&entry.path(), MAX_MANIFEST_BYTES).await?;
            if json_contains_digest(&bytes, digest) {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

fn valid_suffix(value: &str) -> bool {
    value.is_empty() || (value.len() <= 512 && value.split('/').all(super::valid_component))
}

fn supported_manifest_media_type(value: &str) -> bool {
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

fn valid_tag(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.as_bytes()[0].is_ascii_alphanumeric()
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

fn normalize_digest(value: &str) -> Result<String, StoreError> {
    let Some(hex) = value.strip_prefix("sha256:") else {
        return Err(StoreError::DigestInvalid);
    };
    if hex.len() != 64 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(StoreError::DigestInvalid);
    }
    Ok(format!("sha256:{}", hex.to_ascii_lowercase()))
}

fn hex_digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
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

async fn ensure_directory(path: &Path) -> Result<(), StoreError> {
    if existing_directory(path).await?.is_some() {
        return Ok(());
    }
    match fs::create_dir(path).await {
        Ok(()) => {}
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    if existing_directory(path).await?.is_none() {
        return Err(StoreError::Invalid("directory disappeared".to_owned()));
    }
    if let Some(parent) = path.parent() {
        sync_directory(parent).await?;
    }
    Ok(())
}

async fn sync_directory(path: &Path) -> Result<(), StoreError> {
    File::open(path).await?.sync_all().await?;
    Ok(())
}

async fn regular_file_exists(path: &Path) -> Result<bool, StoreError> {
    match fs::symlink_metadata(path).await {
        Ok(metadata) => {
            if !metadata.file_type().is_file() {
                return Err(StoreError::Invalid("expected a regular file".to_owned()));
            }
            Ok(true)
        }
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
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

async fn read_small_file(path: &Path) -> Result<String, StoreError> {
    let bytes = read_bounded_file(path, 1024).await?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

async fn read_bounded_file(path: &Path, limit: usize) -> Result<Vec<u8>, StoreError> {
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
    ensure_directory(parent).await?;
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

async fn verify_file_digest(path: &Path, expected: &str) -> Result<(), StoreError> {
    let mut file = File::open(path).await?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; IO_BUFFER_BYTES];
    loop {
        let read = file.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let actual = format!("sha256:{:x}", hasher.finalize());
    if actual != expected {
        return Err(StoreError::DigestInvalid);
    }
    Ok(())
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

fn json_contains_digest(bytes: &[u8], digest: &str) -> bool {
    let Ok(value) = serde_json::from_slice::<Value>(bytes) else {
        return false;
    };
    let references = |descriptor: &Value| {
        descriptor
            .get("digest")
            .and_then(Value::as_str)
            .is_some_and(|value| normalize_digest(value).ok().as_deref() == Some(digest))
    };
    value.get("config").is_some_and(references)
        || ["layers", "manifests"].into_iter().any(|key| {
            value
                .get(key)
                .and_then(Value::as_array)
                .is_some_and(|descriptors| descriptors.iter().any(references))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Bytes;

    struct Fixture {
        root: PathBuf,
        image: ImageStore,
        owner: Uuid,
    }

    impl Fixture {
        async fn new() -> Self {
            let root =
                std::env::temp_dir().join(format!("gitadel-registry-test-{}", Uuid::new_v4()));
            fs::create_dir(&root).await.unwrap();
            Self {
                image: RegistryStore::new().image(root.clone(), ""),
                root,
                owner: Uuid::new_v4(),
            }
        }

        async fn blob(&self, bytes: &'static [u8]) -> String {
            let digest = format!("sha256:{}", hex_digest(bytes));
            let id = self.image.start_upload(self.owner).await.unwrap();
            self.image
                .append_upload(id, self.owner, Body::from(bytes), None)
                .await
                .unwrap();
            self.image
                .finish_upload(id, self.owner, &digest)
                .await
                .unwrap();
            digest
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[tokio::test]
    async fn cancelled_chunks_resume_without_blocking_other_uploads() {
        let fixture = Fixture::new().await;
        let id = fixture.image.start_upload(fixture.owner).await.unwrap();
        fixture
            .image
            .append_upload(id, fixture.owner, Body::from("head"), None)
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
        let owner = fixture.owner;
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
        let reopened = RegistryStore::new().image(fixture.root.clone(), "");
        assert_eq!(reopened.upload_status(id, owner).await.unwrap(), Some(4));
        reopened
            .append_upload(id, owner, Body::from("tail"), Some((4, 7)))
            .await
            .unwrap();
        let digest = format!("sha256:{}", hex_digest(b"headtail"));
        reopened.finish_upload(id, owner, &digest).await.unwrap();
        let (mut blob, _) = reopened.open_blob(&digest).await.unwrap().unwrap();
        let mut bytes = Vec::new();
        blob.read_to_end(&mut bytes).await.unwrap();
        assert_eq!(bytes, b"headtail");
    }

    #[tokio::test]
    async fn upload_owners_cannot_read_append_or_cancel_each_others_sessions() {
        let fixture = Fixture::new().await;
        let id = fixture.image.start_upload(fixture.owner).await.unwrap();
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
        assert_eq!(
            fixture
                .image
                .upload_status(id, fixture.owner)
                .await
                .unwrap(),
            Some(0)
        );
    }

    #[tokio::test]
    async fn referenced_manifest_deletion_preserves_pull_and_later_releases_blobs() {
        let fixture = Fixture::new().await;
        let config = fixture.blob(b"{}").await;
        let bytes = serde_json::to_vec(&serde_json::json!({
            "schemaVersion": 2, "mediaType": OCI_IMAGE_MANIFEST_MEDIA_TYPE,
            "config": {"mediaType": "application/vnd.oci.image.config.v1+json", "digest": config, "size": 2},
            "layers": []
        })).unwrap();
        let child = fixture
            .image
            .put_manifest("child", OCI_IMAGE_MANIFEST_MEDIA_TYPE, &bytes)
            .await
            .unwrap();
        let index = serde_json::to_vec(&serde_json::json!({
            "schemaVersion": 2, "mediaType": OCI_IMAGE_INDEX_MEDIA_TYPE,
            "manifests": [{"mediaType": OCI_IMAGE_MANIFEST_MEDIA_TYPE, "digest": child.digest, "size": child.bytes.len()}]
        })).unwrap();
        let parent = fixture
            .image
            .put_manifest("index", OCI_IMAGE_INDEX_MEDIA_TYPE, &index)
            .await
            .unwrap();
        assert!(matches!(
            fixture.image.delete_manifest(&child.digest).await,
            Err(StoreError::Referenced)
        ));
        assert_eq!(
            fixture
                .image
                .get_manifest("child")
                .await
                .unwrap()
                .unwrap()
                .bytes,
            bytes
        );
        fixture.image.delete_manifest(&parent.digest).await.unwrap();
        fixture.image.delete_manifest(&child.digest).await.unwrap();
        assert!(fixture.image.delete_blob(&config).await.unwrap());
    }

    #[tokio::test]
    async fn referrers_use_config_type_when_image_artifact_type_is_empty() {
        let fixture = Fixture::new().await;
        let config = fixture.blob(b"{}").await;
        let subject = format!("sha256:{}", hex_digest(b"missing"));
        let config_type = "application/vnd.gitadel.proof.config";
        let bytes = serde_json::to_vec(&serde_json::json!({
            "schemaVersion": 2, "mediaType": OCI_IMAGE_MANIFEST_MEDIA_TYPE,
            "artifactType": "",
            "config": {"mediaType": config_type, "digest": config, "size": 2},
            "layers": [],
            "subject": {"mediaType": OCI_IMAGE_MANIFEST_MEDIA_TYPE, "digest": subject, "size": 7}
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
            referrers
                .iter()
                .map(|value| (value["digest"].as_str(), value["artifactType"].as_str()))
                .collect::<Vec<_>>(),
            vec![(Some(manifest.digest.as_str()), Some(config_type))]
        );
    }

    #[tokio::test]
    async fn referrers_allow_missing_subjects_and_ignore_uncommitted_objects() {
        let fixture = Fixture::new().await;
        let subject = format!("sha256:{}", hex_digest(b"missing"));
        let bytes = serde_json::to_vec(&serde_json::json!({
            "schemaVersion": 2, "mediaType": OCI_IMAGE_INDEX_MEDIA_TYPE,
            "artifactType": "application/vnd.gitadel.proof", "manifests": [],
            "subject": {"mediaType": OCI_IMAGE_MANIFEST_MEDIA_TYPE, "digest": subject, "size": 7}
        }))
        .unwrap();
        let manifest = fixture
            .image
            .put_manifest("proof", OCI_IMAGE_INDEX_MEDIA_TYPE, &bytes)
            .await
            .unwrap();
        fs::write(
            fixture
                .image
                .manifests_dir()
                .join("objects")
                .join(".tmp-interrupted"),
            &bytes,
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
                .is_empty()
        );
    }
}
