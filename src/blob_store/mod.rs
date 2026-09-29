use std::{
    fmt,
    path::Component,
    pin::Pin,
    str::FromStr,
    task::{Context as TaskContext, Poll},
};

use anyhow::{Context as _, Result, bail};
use async_trait::async_trait;
use sha2::{Digest as _, Sha256};
use tokio::io::{AsyncRead, ReadBuf};

mod domain;
mod filesystem;
pub mod manager;
mod s3;
pub mod targets;

pub use domain::StorageDomain;
pub use filesystem::FilesystemBlobStore;
pub use manager::DomainStorage;
pub use s3::S3BlobStore;

pub type BlobReader = Pin<Box<dyn AsyncRead + Send>>;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ObjectKey(String);

impl ObjectKey {
    pub fn new(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        let path = std::path::Path::new(&value);
        if value.is_empty()
            || value.chars().any(char::is_control)
            || value.split('/').any(str::is_empty)
            || path.is_absolute()
            || path
                .components()
                .any(|component| !matches!(component, Component::Normal(_)))
        {
            bail!("blob object key must contain only relative normal path components");
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ObjectKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObjectPrefix(String);

impl ObjectPrefix {
    pub fn new(value: impl Into<String>) -> Result<Self> {
        let mut value = value.into();
        while value.ends_with('/') {
            value.pop();
        }
        if value.is_empty() {
            return Ok(Self(value));
        }
        ObjectKey::new(value.clone())?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlobDigest([u8; 32]);

impl BlobDigest {
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn to_hex(self) -> String {
        use std::fmt::Write as _;
        let mut value = String::with_capacity(64);
        for byte in self.0 {
            let _ = write!(value, "{byte:02x}");
        }
        value
    }
}

impl FromStr for BlobDigest {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            bail!("blob digest must be a 64-character SHA-256 value");
        }
        let mut digest = [0_u8; 32];
        for (index, chunk) in value.as_bytes().as_chunks::<2>().0.iter().enumerate() {
            digest[index] = u8::from_str_radix(std::str::from_utf8(chunk)?, 16)?;
        }
        Ok(Self(digest))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlobMetadata {
    pub key: ObjectKey,
    pub size: u64,
    pub digest: BlobDigest,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PutOutcome {
    pub size: u64,
    pub created: bool,
}

#[derive(Debug)]
pub struct DigestMismatch;

impl fmt::Display for DigestMismatch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("blob content does not match its SHA-256 key")
    }
}

impl std::error::Error for DigestMismatch {}

struct DigestVerifyingReader {
    inner: BlobReader,
    expected: BlobDigest,
    hasher: Sha256,
    verified: bool,
}

impl AsyncRead for DigestVerifyingReader {
    fn poll_read(
        self: Pin<&mut Self>,
        context: &mut TaskContext<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        let before = buffer.filled().len();
        match this.inner.as_mut().poll_read(context, buffer) {
            Poll::Ready(Ok(())) => {
                let bytes = &buffer.filled()[before..];
                if bytes.is_empty() && !this.verified {
                    let actual: [u8; 32] = this.hasher.clone().finalize().into();
                    if actual != *this.expected.as_bytes() {
                        return Poll::Ready(Err(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            DigestMismatch,
                        )));
                    }
                    this.verified = true;
                } else {
                    this.hasher.update(bytes);
                }
                Poll::Ready(Ok(()))
            }
            other => other,
        }
    }
}

/// Hashes a file and returns its size, or fails with [`DigestMismatch`].
async fn verify_file(path: &std::path::Path, expected: BlobDigest) -> Result<u64> {
    use tokio::io::AsyncReadExt as _;

    let mut file = tokio::fs::File::open(path).await?;
    let mut hasher = Sha256::new();
    let mut size = 0_u64;
    let mut buffer = vec![0_u8; 64 * 1024];
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

fn verifying_reader(inner: BlobReader, expected: BlobDigest) -> BlobReader {
    Box::pin(DigestVerifyingReader {
        inner,
        expected,
        hasher: Sha256::new(),
        verified: false,
    })
}

#[async_trait]
pub trait BlobStore: Send + Sync {
    async fn stat(&self, key: &ObjectKey) -> Result<Option<BlobMetadata>>;
    async fn read(&self, key: &ObjectKey) -> Result<BlobReader>;
    /// Streams a nonempty, in-bounds half-open byte range without staging the object.
    /// Partial reads cannot verify the full object's content digest.
    async fn read_range(&self, key: &ObjectKey, range: std::ops::Range<u64>) -> Result<BlobReader>;
    async fn put_verified(
        &self,
        key: &ObjectKey,
        expected: BlobDigest,
        body: BlobReader,
    ) -> Result<PutOutcome>;
    async fn list(&self, prefix: &ObjectPrefix) -> Result<Vec<BlobMetadata>>;
    async fn delete(&self, key: &ObjectKey) -> Result<()>;

    /// Publishes a local file. The file's digest is verified even when the key
    /// already exists, and stores on the same filesystem may move the file
    /// instead of copying it, so callers must treat `path` as consumed.
    async fn put_file(
        &self,
        key: &ObjectKey,
        expected: BlobDigest,
        path: &std::path::Path,
    ) -> Result<PutOutcome> {
        verify_file(path, expected).await?;
        let file = tokio::fs::File::open(path).await?;
        self.put_verified(key, expected, Box::pin(file)).await
    }

    /// Stores the content of `source` under `destination` as well. Stores
    /// that can share content without copying it override this.
    async fn duplicate(&self, source: &ObjectKey, destination: &ObjectKey) -> Result<PutOutcome> {
        let metadata = self
            .stat(source)
            .await?
            .with_context(|| format!("blob {source} does not exist"))?;
        let reader = self.read(source).await?;
        self.put_verified(destination, metadata.digest, reader)
            .await
    }
}

pub fn lfs_object_key(storage_key: uuid::Uuid, oid: &str) -> Result<ObjectKey> {
    let digest = BlobDigest::from_str(oid)?;
    let oid = digest.to_hex();
    ObjectKey::new(format!(
        "{storage_key}/{}/{}/{}",
        &oid[..2],
        &oid[2..4],
        oid
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn object_key_rejects_escaping_components() {
        for key in ["", "/absolute", "../escape", "nested/../escape", "a//b"] {
            assert!(ObjectKey::new(key).is_err(), "accepted {key:?}");
        }
    }

    #[test]
    fn lfs_key_preserves_existing_layout() {
        let storage_key = uuid::Uuid::nil();
        let oid = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        assert_eq!(
            lfs_object_key(storage_key, oid).unwrap().as_str(),
            format!("{storage_key}/01/23/{oid}")
        );
    }
}
