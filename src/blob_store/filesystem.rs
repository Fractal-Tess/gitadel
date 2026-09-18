use std::{io::SeekFrom, ops::Range, path::PathBuf, sync::Arc};

use anyhow::{Context, Result, ensure};
use async_trait::async_trait;
use sha2::{Digest as _, Sha256};
use tokio::{
    fs::{self, OpenOptions},
    io::{AsyncReadExt as _, AsyncSeekExt as _, AsyncWriteExt as _},
};
use uuid::Uuid;

use super::{
    BlobDigest, BlobMetadata, BlobReader, BlobStore, DigestMismatch, ObjectKey, ObjectPrefix,
    PutOutcome, verifying_reader,
};

#[derive(Clone, Debug)]
pub struct FilesystemBlobStore {
    root: Arc<PathBuf>,
}

impl FilesystemBlobStore {
    pub async fn new(root: PathBuf) -> Result<Self> {
        fs::create_dir_all(&root)
            .await
            .with_context(|| format!("could not create blob store {}", root.display()))?;
        let root = fs::canonicalize(&root)
            .await
            .with_context(|| format!("could not resolve blob store {}", root.display()))?;
        Ok(Self {
            root: Arc::new(root),
        })
    }

    fn path(&self, key: &ObjectKey) -> PathBuf {
        self.root.join(key.as_str())
    }
}

#[async_trait]
impl BlobStore for FilesystemBlobStore {
    async fn stat(&self, key: &ObjectKey) -> Result<Option<BlobMetadata>> {
        let path = self.path(key);
        let metadata = match fs::symlink_metadata(&path).await {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        ensure!(metadata.file_type().is_file(), "blob is not a regular file");
        let expected = key
            .as_str()
            .rsplit('/')
            .next()
            .context("blob key has no digest")?
            .parse::<BlobDigest>()?;
        Ok(Some(BlobMetadata {
            key: key.clone(),
            size: metadata.len(),
            digest: expected,
        }))
    }

    async fn read(&self, key: &ObjectKey) -> Result<BlobReader> {
        let path = self.path(key);
        let metadata = fs::symlink_metadata(&path)
            .await
            .with_context(|| format!("could not inspect blob {}", path.display()))?;
        ensure!(metadata.file_type().is_file(), "blob is not a regular file");
        let expected = key
            .as_str()
            .rsplit('/')
            .next()
            .context("blob key has no digest")?
            .parse::<BlobDigest>()?;
        let file = fs::File::open(&path)
            .await
            .with_context(|| format!("could not open blob {}", path.display()))?;
        Ok(verifying_reader(Box::pin(file), expected))
    }

    async fn read_range(&self, key: &ObjectKey, range: Range<u64>) -> Result<BlobReader> {
        ensure!(range.start < range.end, "blob byte range must not be empty");
        let metadata = self.stat(key).await?.context("blob does not exist")?;
        ensure!(
            range.end <= metadata.size,
            "blob byte range exceeds object size"
        );
        let path = self.path(key);
        let mut file = fs::File::open(&path)
            .await
            .with_context(|| format!("could not open blob {}", path.display()))?;
        file.seek(SeekFrom::Start(range.start)).await?;
        Ok(Box::pin(file.take(range.end - range.start)))
    }

    async fn put_verified(
        &self,
        key: &ObjectKey,
        expected: BlobDigest,
        mut body: BlobReader,
    ) -> Result<PutOutcome> {
        if let Some(existing) = self.stat(key).await? {
            ensure!(existing.digest == expected, DigestMismatch);
            let size = verify_file(&self.path(key), expected).await?;
            return Ok(PutOutcome {
                size,
                created: false,
            });
        }
        let path = self.path(key);
        let parent = path.parent().context("blob path has no parent")?;
        fs::create_dir_all(parent).await?;
        let temporary = parent.join(format!(".{}.upload", Uuid::new_v4().simple()));
        let result = async {
            let mut output = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)
                .await?;
            let mut hasher = Sha256::new();
            let mut size = 0_u64;
            let mut buffer = [0_u8; 64 * 1024];
            loop {
                let read = body.read(&mut buffer).await?;
                if read == 0 {
                    break;
                }
                hasher.update(&buffer[..read]);
                output.write_all(&buffer[..read]).await?;
                size = size.saturating_add(read as u64);
            }
            if hasher.finalize().as_slice() != expected.as_bytes() {
                return Err(DigestMismatch.into());
            }
            output.flush().await?;
            output.sync_all().await?;
            drop(output);
            match fs::rename(&temporary, &path).await {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    let existing = self
                        .stat(key)
                        .await?
                        .context("concurrent blob upload disappeared")?;
                    ensure!(existing.digest == expected, DigestMismatch);
                    return Ok(PutOutcome {
                        size: existing.size,
                        created: false,
                    });
                }
                Err(error) => return Err(error.into()),
            }
            let parent = parent.to_path_buf();
            tokio::task::spawn_blocking(move || -> std::io::Result<()> {
                std::fs::File::open(parent)?.sync_all()
            })
            .await??;
            Ok(PutOutcome {
                size,
                created: true,
            })
        }
        .await;
        if result.is_err() {
            let _ = fs::remove_file(&temporary).await;
        }
        result
    }

    async fn list(&self, prefix: &ObjectPrefix) -> Result<Vec<BlobMetadata>> {
        let root = self.root.clone();
        let prefix = prefix.as_str().to_owned();
        tokio::task::spawn_blocking(move || {
            let base = if prefix.is_empty() {
                root.as_ref().clone()
            } else {
                root.join(&prefix)
            };
            if !base.exists() {
                return Ok(Vec::new());
            }
            let mut objects = Vec::new();
            for entry in walkdir::WalkDir::new(&base).follow_links(false) {
                let entry = entry?;
                if !entry.file_type().is_file() {
                    continue;
                }
                let relative = entry.path().strip_prefix(root.as_ref())?;
                let Some(relative) = relative.to_str() else {
                    continue;
                };
                let key = ObjectKey::new(relative.replace(std::path::MAIN_SEPARATOR, "/"))?;
                let Some(oid) = key.as_str().rsplit('/').next() else {
                    continue;
                };
                let Ok(digest) = oid.parse::<BlobDigest>() else {
                    continue;
                };
                objects.push(BlobMetadata {
                    key,
                    size: entry.metadata()?.len(),
                    digest,
                });
            }
            objects.sort_by(|left, right| left.key.as_str().cmp(right.key.as_str()));
            Ok(objects)
        })
        .await?
    }

    async fn delete(&self, key: &ObjectKey) -> Result<()> {
        match fs::remove_file(self.path(key)).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
}

async fn verify_file(path: &std::path::Path, expected: BlobDigest) -> Result<u64> {
    let mut file = fs::File::open(path).await?;
    let mut hasher = Sha256::new();
    let mut size = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        size = size.saturating_add(read as u64);
    }
    let actual: [u8; 32] = hasher.finalize().into();
    if actual != *expected.as_bytes() {
        return Err(DigestMismatch.into());
    }
    Ok(size)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[tokio::test]
    async fn verified_put_publishes_complete_object() {
        let root = std::env::temp_dir().join(format!("gitadel-blob-test-{}", Uuid::new_v4()));
        let store = FilesystemBlobStore::new(root.clone()).await.unwrap();
        let payload = b"verified payload";
        let digest = BlobDigest::from_bytes(Sha256::digest(payload).into());
        let key = ObjectKey::new(format!("repository/ve/ri/{}", digest.to_hex())).unwrap();
        let outcome = store
            .put_verified(&key, digest, Box::pin(Cursor::new(payload)))
            .await
            .unwrap();
        assert!(outcome.created);
        assert_eq!(
            store.stat(&key).await.unwrap().unwrap().size,
            payload.len() as u64
        );
        fs::remove_dir_all(root).await.unwrap();
    }

    #[tokio::test]
    async fn range_reads_are_bounded_and_reject_invalid_offsets() {
        let root = std::env::temp_dir().join(format!("gitadel-blob-test-{}", Uuid::new_v4()));
        let store = FilesystemBlobStore::new(root.clone()).await.unwrap();
        let payload = b"range payload";
        let digest = BlobDigest::from_bytes(Sha256::digest(payload).into());
        let key = ObjectKey::new(digest.to_hex()).unwrap();
        store
            .put_verified(&key, digest, Box::pin(Cursor::new(payload)))
            .await
            .unwrap();

        let mut reader = store.read_range(&key, 3..8).await.unwrap();
        let mut actual = Vec::new();
        reader.read_to_end(&mut actual).await.unwrap();
        assert_eq!(actual, &payload[3..8]);
        assert!(store.read_range(&key, 0..0).await.is_err());
        assert!(
            store
                .read_range(&key, 0..payload.len() as u64 + 1)
                .await
                .is_err()
        );
        fs::remove_dir_all(root).await.unwrap();
    }

    #[tokio::test]
    async fn verified_put_rejects_wrong_digest_without_final_object() {
        let root = std::env::temp_dir().join(format!("gitadel-blob-test-{}", Uuid::new_v4()));
        let store = FilesystemBlobStore::new(root.clone()).await.unwrap();
        let digest = BlobDigest::from_bytes([7; 32]);
        let key = ObjectKey::new(format!("repository/07/07/{}", digest.to_hex())).unwrap();
        assert!(
            store
                .put_verified(&key, digest, Box::pin(Cursor::new(b"wrong")))
                .await
                .is_err()
        );
        assert!(store.stat(&key).await.unwrap().is_none());
        fs::remove_dir_all(root).await.unwrap();
    }
}
