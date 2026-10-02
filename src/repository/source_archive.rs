//! Source archives of a commit, generated once and then served from a cache.

use std::{
    fs::File,
    io::{self, Write},
    path::Path,
    time::{Duration as StdDuration, Instant},
};

use axum::{
    body::Body,
    extract::{Path as AxumPath, Query, State},
    http::{HeaderMap, HeaderValue, header},
    response::Response,
};
use axum_extra::extract::cookie::CookieJar;
use serde::Deserialize;
use sley::{ObjectId, Repository as GitRepository};
use sley_archive::{
    ArchiveConvert, ArchiveExtras, TarArchiveOptions, ZipArchiveOptions, write_tar_gz_archive_full,
    write_zip_archive_full,
};

use super::{
    RepositoryState,
    browser::{blank_as_none, read_git, readable_repository},
};
use crate::{
    filesystem::{create_private_directory_async, open_private_file},
    identity::ApiError,
};

#[derive(Clone, Copy, Deserialize)]
pub enum SourceArchiveFormat {
    #[serde(rename = "zip")]
    Zip,
    #[serde(rename = "tar.gz")]
    TarGz,
}

impl SourceArchiveFormat {
    fn extension(self) -> &'static str {
        match self {
            Self::Zip => "zip",
            Self::TarGz => "tar.gz",
        }
    }

    fn content_type(self) -> &'static str {
        match self {
            Self::Zip => "application/zip",
            Self::TarGz => "application/gzip",
        }
    }
}

#[derive(Deserialize)]
pub struct SourceArchiveQuery {
    #[serde(default, deserialize_with = "blank_as_none")]
    rev: Option<String>,
    format: SourceArchiveFormat,
}

pub async fn source_archive(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name)): AxumPath<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
    Query(query): Query<SourceArchiveQuery>,
) -> Result<Response, ApiError> {
    let repository = readable_repository(&state, &headers, &jar, &namespace, &name).await?;
    let revision = query
        .rev
        .or_else(|| repository.default_branch.clone())
        .ok_or_else(ApiError::not_found)?;
    let repository_path = state.repository_path(&repository);
    let (commit_oid, tree_oid, mtime) = read_git(repository_path.clone(), move |git| {
        let commit_oid = git.peel_to_commit_oid(git.rev_parse(&revision)?)?;
        let commit = git.read_commit(&commit_oid)?;
        let mtime = commit.committer_signature().map_or(0, |signature| {
            u64::try_from(signature.time.seconds).unwrap_or_default()
        });
        Ok((commit_oid, commit.tree, mtime))
    })
    .await?;
    let commit_hex = commit_oid.to_hex();
    let short_oid = &commit_hex[..commit_hex.len().min(8)];
    let archive_name = format!(
        "{}-{short_oid}.{}",
        repository.name,
        query.format.extension()
    );
    let cache_path =
        state.source_archive_cache_path(&repository, &commit_hex, query.format.extension());
    if !tokio::fs::try_exists(&cache_path)
        .await
        .map_err(ApiError::internal)?
    {
        let permit = state.try_archive_generation()?;
        if !tokio::fs::try_exists(&cache_path)
            .await
            .map_err(ApiError::internal)?
        {
            let cache_directory = state.source_archive_cache_directory(&repository);
            create_private_directory_async(&cache_directory)
                .await
                .map_err(ApiError::internal)?;
            let temporary_path = cache_directory.join(format!(
                ".{}.{}.tmp",
                commit_hex,
                uuid::Uuid::new_v4().simple()
            ));
            let prefix = format!("{}-{short_oid}/", repository.name);
            let format = query.format;
            let published_path = cache_path.clone();
            tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
                let _permit = permit;
                let result = write_archive_file(
                    &repository_path,
                    &temporary_path,
                    format,
                    commit_oid,
                    tree_oid,
                    mtime,
                    prefix,
                )
                .and_then(|()| {
                    if published_path.exists() {
                        Ok(())
                    } else {
                        std::fs::rename(&temporary_path, &published_path).map_err(Into::into)
                    }
                })
                .and_then(|()| prune_archive_cache(&cache_directory));
                let _ = std::fs::remove_file(&temporary_path);
                result
            })
            .await
            .map_err(ApiError::internal)?
            .map_err(ApiError::internal)?;
        }
    }
    source_archive_response(&cache_path, &archive_name, query.format).await
}

const MAX_SOURCE_ARCHIVE_BYTES: u64 = 512 * 1024 * 1024;
const SOURCE_ARCHIVE_GENERATION_TIMEOUT: StdDuration = StdDuration::from_secs(2 * 60);
const MAX_CACHED_ARCHIVES_PER_REPOSITORY: usize = 8;

struct ArchiveFileWriter {
    file: File,
    bytes_written: u64,
    deadline: Instant,
}

impl Write for ArchiveFileWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if Instant::now() >= self.deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "source archive generation timed out",
            ));
        }
        if self.bytes_written.saturating_add(bytes.len() as u64) > MAX_SOURCE_ARCHIVE_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::FileTooLarge,
                "source archive exceeds the output size limit",
            ));
        }
        let written = self.file.write(bytes)?;
        self.bytes_written = self.bytes_written.saturating_add(written as u64);
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

fn write_archive_file(
    repository_path: &Path,
    output_path: &Path,
    archive_format: SourceArchiveFormat,
    commit_oid: ObjectId,
    tree_oid: ObjectId,
    mtime: u64,
    prefix: String,
) -> anyhow::Result<()> {
    let repository = GitRepository::open_exact_bare(repository_path)?;
    let config = repository.config()?;
    let converter = ArchiveConvert::from_tree(
        repository_path,
        repository_path,
        &config,
        repository.object_database(),
        repository.object_format(),
        &tree_oid,
    )?;
    let mut writer = ArchiveFileWriter {
        file: open_private_file(output_path, true)?,
        bytes_written: 0,
        deadline: Instant::now() + SOURCE_ARCHIVE_GENERATION_TIMEOUT,
    };
    match archive_format {
        SourceArchiveFormat::Zip => write_zip_archive_full(
            &mut writer,
            repository.object_database(),
            repository.object_format(),
            &tree_oid,
            ZipArchiveOptions {
                prefix: prefix.into_bytes(),
                mtime,
                commit_id: Some(commit_oid),
                ..ZipArchiveOptions::default()
            },
            &converter,
            &ArchiveExtras::default(),
        )?,
        SourceArchiveFormat::TarGz => write_tar_gz_archive_full(
            &mut writer,
            repository.object_database(),
            repository.object_format(),
            &tree_oid,
            TarArchiveOptions {
                prefix: prefix.into_bytes(),
                mtime,
                commit_id: Some(commit_oid),
                ..TarArchiveOptions::default()
            },
            &converter,
            &ArchiveExtras::default(),
            6,
        )?,
    }
    writer.flush()?;
    writer.file.sync_all()?;
    Ok(())
}

fn prune_archive_cache(directory: &Path) -> anyhow::Result<()> {
    let mut entries = std::fs::read_dir(directory)?
        .filter_map(Result::ok)
        .filter(|entry| !entry.file_name().to_string_lossy().ends_with(".tmp"))
        .filter_map(|entry| {
            let metadata = entry.metadata().ok()?;
            metadata
                .is_file()
                .then(|| (metadata.modified().ok(), entry.path()))
        })
        .collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.0);
    let remove = entries
        .len()
        .saturating_sub(MAX_CACHED_ARCHIVES_PER_REPOSITORY);
    for (_, path) in entries.into_iter().take(remove) {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

async fn source_archive_response(
    path: &Path,
    archive_name: &str,
    format: SourceArchiveFormat,
) -> Result<Response, ApiError> {
    let file = tokio::fs::File::open(path)
        .await
        .map_err(ApiError::internal)?;
    let size = file.metadata().await.map_err(ApiError::internal)?.len();
    let mut response = Response::new(Body::from_stream(tokio_util::io::ReaderStream::new(file)));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(format.content_type()),
    );
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!("attachment; filename=\"{archive_name}\""))
            .map_err(ApiError::internal)?,
    );
    response.headers_mut().insert(
        header::CONTENT_LENGTH,
        HeaderValue::from_str(&size.to_string()).map_err(ApiError::internal)?,
    );
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=31536000, immutable"),
    );
    Ok(response)
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        io::Write as _,
        time::{Duration, Instant},
    };

    use uuid::Uuid;

    use super::{ArchiveFileWriter, MAX_SOURCE_ARCHIVE_BYTES};

    #[test]
    fn source_archive_writer_rejects_output_over_limit() {
        let path =
            std::env::temp_dir().join(format!("gitadel-archive-limit-test-{}", Uuid::new_v4()));
        let mut writer = ArchiveFileWriter {
            file: fs::File::create(&path).expect("create archive output"),
            bytes_written: MAX_SOURCE_ARCHIVE_BYTES,
            deadline: Instant::now() + Duration::from_secs(1),
        };

        let error = writer
            .write_all(&[0])
            .expect_err("reject oversized archive");
        fs::remove_file(path).expect("remove archive output");

        assert_eq!(error.kind(), std::io::ErrorKind::FileTooLarge);
    }

    #[test]
    fn source_archive_writer_rejects_expired_generation() {
        let path =
            std::env::temp_dir().join(format!("gitadel-archive-timeout-test-{}", Uuid::new_v4()));
        let mut writer = ArchiveFileWriter {
            file: fs::File::create(&path).expect("create archive output"),
            bytes_written: 0,
            deadline: Instant::now(),
        };

        let error = writer.write_all(&[0]).expect_err("reject expired archive");
        fs::remove_file(path).expect("remove archive output");

        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
    }
}
