use std::{
    fs,
    path::Path,
    pin::Pin,
    task::{Context as TaskContext, Poll},
};

use ::s3::{Bucket, Region, creds::Credentials};
use anyhow::{Context, Result, ensure};
use tokio::io::{AsyncRead, AsyncWriteExt as _, ReadBuf};
use uuid::Uuid;

use crate::config::{S3Settings, validate_s3_settings};

use super::{
    BackupObject, MaintenanceProgressReporter, S3BackupDownload, generated_backup_metadata,
    sort_backups,
};

const UPLOAD_PROGRESS_INTERVAL: u64 = 1024 * 1024;

pub(super) fn bucket(settings: &S3Settings) -> Result<Box<Bucket>> {
    let credentials = Credentials::new(
        Some(&settings.access_key),
        Some(&settings.secret_key),
        None,
        None,
        None,
    )
    .context("could not configure S3 credentials")?;
    let region = Region::Custom {
        region: settings.region.clone(),
        endpoint: settings.endpoint.as_str().trim_end_matches('/').to_owned(),
    };
    let bucket =
        Bucket::new(&settings.bucket, region, credentials).context("could not configure S3")?;
    Ok(bucket.with_path_style())
}

pub(super) async fn object_exists(bucket: &Bucket, key: &str) -> Result<bool> {
    bucket
        .object_exists(key)
        .await
        .context("could not check S3 backup destination")
}

pub(super) async fn backup_size(bucket: &Bucket, key: &str) -> Result<u64> {
    let (metadata, status) = bucket
        .head_object(key)
        .await
        .context("could not read S3 backup metadata")?;
    ensure!(
        (200..300).contains(&status),
        "S3 backup metadata returned HTTP {status}"
    );
    metadata
        .content_length
        .and_then(|size| u64::try_from(size).ok())
        .context("S3 backup size is missing")
}

struct UploadProgressReader<R> {
    inner: R,
    reporter: MaintenanceProgressReporter,
    processed: u64,
    reported: u64,
    total: u64,
}

impl<R: AsyncRead + Unpin> AsyncRead for UploadProgressReader<R> {
    fn poll_read(
        self: Pin<&mut Self>,
        context: &mut TaskContext<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        let filled_before = buffer.filled().len();
        let result = Pin::new(&mut this.inner).poll_read(context, buffer);
        if let Poll::Ready(Ok(())) = &result {
            let read = buffer.filled().len().saturating_sub(filled_before) as u64;
            this.processed = this.processed.saturating_add(read);
            if this.processed != this.reported
                && (this.processed == this.total
                    || this.processed.saturating_sub(this.reported) >= UPLOAD_PROGRESS_INTERVAL)
            {
                this.reporter.upload(this.processed, this.total);
                this.reported = this.processed;
            }
        }
        result
    }
}

pub(super) async fn upload_archive(
    bucket: &Bucket,
    key: &str,
    archive: &Path,
    reporter: Option<&MaintenanceProgressReporter>,
) -> Result<()> {
    let expected_size = fs::metadata(archive)?.len();
    let input = tokio::fs::File::open(archive)
        .await
        .with_context(|| format!("could not open {}", archive.display()))?;
    let response = if let Some(reporter) = reporter {
        reporter.upload(0, expected_size);
        let mut input = UploadProgressReader {
            inner: input,
            reporter: reporter.clone(),
            processed: 0,
            reported: 0,
            total: expected_size,
        };
        bucket
            .put_object_stream_with_content_type(&mut input, key, "application/zstd")
            .await
    } else {
        let mut input = input;
        bucket
            .put_object_stream_with_content_type(&mut input, key, "application/zstd")
            .await
    }
    .context("could not upload S3 backup")?;
    ensure!(
        (200..300).contains(&response.status_code()),
        "S3 backup upload returned HTTP {}",
        response.status_code()
    );
    ensure!(
        response.uploaded_bytes() as u64 == expected_size,
        "S3 backup upload size mismatch"
    );
    let (metadata, status) = bucket
        .head_object(key)
        .await
        .context("could not verify uploaded S3 backup")?;
    ensure!(
        (200..300).contains(&status),
        "S3 backup verification returned HTTP {status}"
    );
    ensure!(
        metadata
            .content_length
            .and_then(|size| u64::try_from(size).ok())
            == Some(expected_size),
        "uploaded S3 backup size does not match the local archive"
    );
    Ok(())
}

pub(super) async fn download_archive(bucket: &Bucket, key: &str, archive: &Path) -> Result<()> {
    let mut output = tokio::fs::File::create(archive)
        .await
        .with_context(|| format!("could not create {}", archive.display()))?;
    let status = bucket
        .get_object_to_writer(key, &mut output)
        .await
        .context("could not download S3 backup")?;
    ensure!(
        (200..300).contains(&status),
        "S3 backup download returned HTTP {status}"
    );
    output
        .flush()
        .await
        .context("could not flush downloaded S3 backup")?;
    output
        .sync_all()
        .await
        .context("could not sync downloaded S3 backup")
}

pub(super) async fn test_settings(settings: &S3Settings) -> Result<()> {
    validate_s3_settings(settings)?;
    list_backups(settings).await?;
    let bucket = bucket(settings)?;
    let filename = format!(".gitadel-connection-test-{}", Uuid::new_v4().simple());
    let key = if settings.prefix.is_empty() {
        filename
    } else {
        format!("{}/{filename}", settings.prefix)
    };
    let payload = Uuid::new_v4().to_string();
    let uploaded = bucket
        .put_object(&key, payload.as_bytes())
        .await
        .context("could not write S3 connection test object")?;
    ensure!(
        (200..300).contains(&uploaded.status_code()),
        "S3 connection test upload returned HTTP {}",
        uploaded.status_code()
    );
    let verified = async {
        let downloaded = bucket
            .get_object(&key)
            .await
            .context("could not read S3 connection test object")?;
        ensure!(
            (200..300).contains(&downloaded.status_code()),
            "S3 connection test download returned HTTP {}",
            downloaded.status_code()
        );
        ensure!(
            downloaded.as_slice() == payload.as_bytes(),
            "S3 connection test returned different content"
        );
        Ok(())
    }
    .await;
    let deleted = bucket
        .delete_object(&key)
        .await
        .context("could not delete S3 connection test object")?;
    ensure!(
        (200..300).contains(&deleted.status_code()),
        "S3 connection test cleanup returned HTTP {}",
        deleted.status_code()
    );
    verified
}

pub(super) async fn stream_backup(settings: &S3Settings, key: &str) -> Result<S3BackupDownload> {
    validate_managed_key(settings, key)?;
    let bucket = bucket(settings)?;
    let (metadata, status) = bucket
        .head_object(key)
        .await
        .context("could not inspect S3 backup")?;
    ensure!(
        (200..300).contains(&status),
        "S3 backup inspection returned HTTP {status}"
    );
    let response = bucket
        .get_object_stream(key)
        .await
        .context("could not stream S3 backup")?;
    ensure!(
        (200..300).contains(&response.status_code),
        "S3 backup download returned HTTP {}",
        response.status_code
    );
    Ok(S3BackupDownload {
        size: metadata
            .content_length
            .and_then(|size| u64::try_from(size).ok()),
        stream: response.bytes,
    })
}

pub(super) async fn delete_backup(settings: &S3Settings, key: &str) -> Result<()> {
    validate_managed_key(settings, key)?;
    let bucket = bucket(settings)?;
    ensure!(
        bucket
            .object_exists(key)
            .await
            .context("could not inspect S3 backup")?,
        "S3 backup object does not exist: s3://{}/{}",
        settings.bucket,
        key
    );
    let response = bucket
        .delete_object(key)
        .await
        .context("could not delete S3 backup")?;
    ensure!(
        (200..300).contains(&response.status_code()),
        "S3 backup deletion returned HTTP {}",
        response.status_code()
    );
    Ok(())
}

pub(super) fn validate_managed_key(settings: &S3Settings, key: &str) -> Result<()> {
    validate_key(key)?;
    ensure!(
        key.ends_with(".tar.zst"),
        "S3 backup key must end with .tar.zst"
    );
    if !settings.prefix.is_empty() {
        let prefix = format!("{}/", settings.prefix);
        ensure!(
            key.starts_with(&prefix),
            "S3 backup key is outside the configured prefix"
        );
    }
    Ok(())
}

pub(super) async fn list_backups(settings: &S3Settings) -> Result<Vec<BackupObject>> {
    validate_s3_settings(settings)?;
    let bucket = bucket(settings)?;
    let prefix = if settings.prefix.is_empty() {
        String::new()
    } else {
        format!("{}/", settings.prefix)
    };
    let pages = bucket
        .list(prefix, None)
        .await
        .context("could not list S3 backups")?;
    let mut backups = pages
        .into_iter()
        .flat_map(|page| page.contents)
        .filter(|object| object.key.ends_with(".tar.zst"))
        .map(|object| {
            let (name, gitadel_version) = generated_backup_metadata(&object.key);
            BackupObject {
                key: object.key,
                name,
                gitadel_version,
                size: object.size,
                created_at: object.last_modified,
            }
        })
        .collect::<Vec<_>>();
    sort_backups(&mut backups);
    Ok(backups)
}

pub(super) fn validate_key(key: &str) -> Result<()> {
    ensure!(
        !key.is_empty()
            && !key.starts_with('/')
            && !key.ends_with('/')
            && !key.contains('\\')
            && !key.chars().any(char::is_control),
        "S3 backup key must be non-empty, relative, and contain no control characters or backslashes"
    );
    Ok(())
}
