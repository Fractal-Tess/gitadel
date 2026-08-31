use std::{
    pin::Pin,
    sync::Arc,
    task::{Context as TaskContext, Poll},
};

use ::s3::{Bucket, Region, creds::Credentials, error::S3Error};
use anyhow::{Context, Result, ensure};
use async_trait::async_trait;
use sha2::{Digest as _, Sha256};
use tokio::io::{AsyncRead, ReadBuf};

use crate::config::{S3Settings, validate_s3_settings};

use super::{
    BlobDigest, BlobMetadata, BlobReader, BlobStore, DigestMismatch, ObjectKey, ObjectPrefix,
    PutOutcome, verifying_reader,
};

const DIGEST_METADATA: &str = "gitadel-sha256";

#[derive(Clone)]
pub struct S3BlobStore {
    bucket: Arc<Bucket>,
    prefix: Arc<str>,
}

impl std::fmt::Debug for S3BlobStore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("S3BlobStore")
            .field("bucket", &self.bucket.name)
            .field("prefix", &self.prefix)
            .finish()
    }
}

impl S3BlobStore {
    pub fn new(settings: &S3Settings) -> Result<Self> {
        validate_s3_settings(settings)?;
        let credentials = Credentials::new(
            Some(&settings.access_key),
            Some(&settings.secret_key),
            None,
            None,
            None,
        )
        .context("could not configure S3 blob credentials")?;
        let region = Region::Custom {
            region: settings.region.clone(),
            endpoint: settings.endpoint.as_str().trim_end_matches('/').to_owned(),
        };
        let bucket = Bucket::new(&settings.bucket, region, credentials)
            .context("could not configure S3 blob store")?
            .with_path_style();
        Ok(Self {
            bucket: Arc::new(*bucket),
            prefix: Arc::from(settings.prefix.trim_matches('/')),
        })
    }

    fn remote_key(&self, key: &ObjectKey) -> String {
        if self.prefix.is_empty() {
            key.as_str().to_owned()
        } else {
            format!("{}/{}", self.prefix, key.as_str())
        }
    }

    fn remote_prefix(&self, prefix: &ObjectPrefix) -> String {
        match (self.prefix.is_empty(), prefix.as_str().is_empty()) {
            (true, _) => prefix.as_str().to_owned(),
            (false, true) => format!("{}/", self.prefix),
            (false, false) => format!("{}/{}/", self.prefix, prefix.as_str()),
        }
    }

    fn local_key(&self, remote: &str) -> Result<ObjectKey> {
        let value = if self.prefix.is_empty() {
            remote
        } else {
            remote
                .strip_prefix(self.prefix.as_ref())
                .and_then(|value| value.strip_prefix('/'))
                .context("S3 object escaped configured prefix")?
        };
        ObjectKey::new(value)
    }

    async fn committed_metadata(&self, key: &ObjectKey) -> Result<Option<BlobMetadata>> {
        let remote = self.remote_key(key);
        if !self.bucket.object_exists(&remote).await? {
            return Ok(None);
        }
        let (head, status) = self.bucket.head_object(&remote).await?;
        ensure!(
            (200..300).contains(&status),
            "S3 HEAD returned HTTP {status}"
        );
        let size = head
            .content_length
            .and_then(|value| u64::try_from(value).ok())
            .context("S3 blob content length is missing")?;
        let digest = key
            .as_str()
            .rsplit('/')
            .next()
            .context("blob key has no digest")?
            .parse::<BlobDigest>()?;
        let metadata_digest = head
            .metadata
            .as_ref()
            .and_then(|metadata| {
                metadata
                    .get(DIGEST_METADATA)
                    .or_else(|| metadata.get(&format!("x-amz-meta-{DIGEST_METADATA}")))
            })
            .context("S3 blob is missing Gitadel digest metadata")?
            .parse::<BlobDigest>()?;
        ensure!(
            metadata_digest == digest,
            "S3 blob digest metadata mismatch"
        );
        Ok(Some(BlobMetadata {
            key: key.clone(),
            size,
            digest,
        }))
    }
}

struct VerifyingReader {
    inner: BlobReader,
    expected: BlobDigest,
    hasher: Sha256,
    size: u64,
    verified: bool,
}

impl AsyncRead for VerifyingReader {
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
                    this.size = this.size.saturating_add(bytes.len() as u64);
                }
                Poll::Ready(Ok(()))
            }
            other => other,
        }
    }
}

#[async_trait]
impl BlobStore for S3BlobStore {
    async fn stat(&self, key: &ObjectKey) -> Result<Option<BlobMetadata>> {
        self.committed_metadata(key).await
    }

    async fn read(&self, key: &ObjectKey) -> Result<BlobReader> {
        let metadata = self
            .committed_metadata(key)
            .await?
            .context("S3 blob is not committed")?;
        let response = self.bucket.get_object_stream(self.remote_key(key)).await?;
        ensure!(
            (200..300).contains(&response.status_code),
            "S3 GET returned HTTP {}",
            response.status_code
        );
        Ok(verifying_reader(Box::pin(response), metadata.digest))
    }

    async fn put_verified(
        &self,
        key: &ObjectKey,
        expected: BlobDigest,
        body: BlobReader,
    ) -> Result<PutOutcome> {
        if let Some(existing) = self.committed_metadata(key).await? {
            ensure!(
                existing.digest == expected,
                "existing S3 blob digest mismatch"
            );
            return Ok(PutOutcome {
                size: existing.size,
                created: false,
            });
        }
        let mut reader = VerifyingReader {
            inner: body,
            expected,
            hasher: Sha256::new(),
            size: 0,
            verified: false,
        };
        let response = match self
            .bucket
            .put_object_stream_builder(self.remote_key(key))
            .with_content_type("application/octet-stream")
            .with_metadata(DIGEST_METADATA, expected.to_hex())?
            .execute_stream(&mut reader)
            .await
        {
            Ok(response) => response,
            Err(S3Error::Io(error))
                if error
                    .get_ref()
                    .and_then(|cause| cause.downcast_ref::<DigestMismatch>())
                    .is_some() =>
            {
                return Err(DigestMismatch.into());
            }
            Err(error) => return Err(error).context("could not upload S3 blob"),
        };
        ensure!(
            reader.verified,
            "S3 upload completed before digest verification"
        );
        ensure!(
            (200..300).contains(&response.status_code()),
            "S3 upload returned HTTP {}",
            response.status_code()
        );
        let committed = self
            .committed_metadata(key)
            .await?
            .context("uploaded S3 blob is not committed")?;
        ensure!(committed.size == reader.size, "S3 uploaded size mismatch");
        Ok(PutOutcome {
            size: reader.size,
            created: true,
        })
    }

    async fn list(&self, prefix: &ObjectPrefix) -> Result<Vec<BlobMetadata>> {
        let pages = self.bucket.list(self.remote_prefix(prefix), None).await?;
        let mut objects = Vec::new();
        for object in pages.into_iter().flat_map(|page| page.contents) {
            let Ok(key) = self.local_key(&object.key) else {
                continue;
            };
            if let Some(metadata) = self.committed_metadata(&key).await? {
                objects.push(metadata);
            }
        }
        objects.sort_by(|left, right| left.key.as_str().cmp(right.key.as_str()));
        Ok(objects)
    }

    async fn delete(&self, key: &ObjectKey) -> Result<()> {
        let response = self.bucket.delete_object(self.remote_key(key)).await?;
        ensure!(
            (200..300).contains(&response.status_code()),
            "S3 delete returned HTTP {}",
            response.status_code()
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::s3::{BucketConfiguration, bucket::Bucket};
    use tokio::io::AsyncReadExt as _;
    use url::Url;

    #[tokio::test]
    #[ignore = "requires a disposable S3-compatible service"]
    async fn s3_blob_store_conformance() {
        let endpoint = std::env::var("GITADEL_TEST_S3_ENDPOINT")
            .expect("GITADEL_TEST_S3_ENDPOINT is required");
        let access_key = std::env::var("GITADEL_TEST_S3_ACCESS_KEY")
            .expect("GITADEL_TEST_S3_ACCESS_KEY is required");
        let secret_key = std::env::var("GITADEL_TEST_S3_SECRET_KEY")
            .expect("GITADEL_TEST_S3_SECRET_KEY is required");
        let bucket_name = format!("gitadel-test-{}", uuid::Uuid::new_v4().simple());
        let region = Region::Custom {
            region: "us-east-1".to_owned(),
            endpoint: endpoint.clone(),
        };
        let credentials =
            Credentials::new(Some(&access_key), Some(&secret_key), None, None, None).unwrap();
        Bucket::create_with_path_style(
            &bucket_name,
            region,
            credentials,
            BucketConfiguration::default(),
        )
        .await
        .unwrap();

        let settings = S3Settings {
            endpoint: Url::parse(&endpoint).unwrap(),
            bucket: bucket_name.clone(),
            access_key,
            secret_key,
            region: "us-east-1".to_owned(),
            prefix: "lfs".to_owned(),
        };
        let store = S3BlobStore::new(&settings).unwrap();
        let payload = b"S3 conformance payload";
        let digest = BlobDigest::from_bytes(Sha256::digest(payload).into());
        let key = ObjectKey::new(format!(
            "{}/ab/cd/{}",
            uuid::Uuid::new_v4(),
            digest.to_hex()
        ))
        .unwrap();

        let mismatch_key = ObjectKey::new(format!(
            "{}/ab/cd/{}",
            uuid::Uuid::new_v4(),
            digest.to_hex()
        ))
        .unwrap();
        let mismatch = store
            .put_verified(
                &mismatch_key,
                digest,
                Box::pin(std::io::Cursor::new(b"different content".as_slice())),
            )
            .await
            .unwrap_err();
        assert!(
            mismatch
                .chain()
                .any(|cause| cause.downcast_ref::<DigestMismatch>().is_some()),
            "{mismatch:#?}"
        );

        let created = store
            .put_verified(
                &key,
                digest,
                Box::pin(std::io::Cursor::new(payload.as_slice())),
            )
            .await
            .unwrap();
        assert!(created.created);
        assert_eq!(created.size, payload.len() as u64);
        assert_eq!(store.stat(&key).await.unwrap().unwrap().digest, digest);

        let mut reader = store.read(&key).await.unwrap();
        let mut actual = Vec::new();
        reader.read_to_end(&mut actual).await.unwrap();
        assert_eq!(actual, payload);

        let prefix = ObjectPrefix::new(key.as_str().split('/').next().unwrap()).unwrap();
        assert_eq!(store.list(&prefix).await.unwrap().len(), 1);
        assert!(
            !store
                .put_verified(
                    &key,
                    digest,
                    Box::pin(std::io::Cursor::new(payload.as_slice())),
                )
                .await
                .unwrap()
                .created
        );
        store.delete(&key).await.unwrap();
        assert!(store.stat(&key).await.unwrap().is_none());

        let bucket = super::S3BlobStore::new(&settings).unwrap().bucket;
        bucket.delete().await.unwrap();
    }
}
