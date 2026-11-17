//! Source archives of a commit, generated once and then served from a cache.

use std::path::Path;

use axum::{
    body::Body,
    extract::{Path as AxumPath, Query, State},
    http::{HeaderMap, HeaderValue, header},
    response::Response,
};
use axum_extra::extract::cookie::CookieJar;
use serde::Deserialize;
use sley::ObjectId;

use super::{
    RepositoryState,
    browser::{blank_as_none, read_git, readable_repository},
};
use crate::{filesystem::create_private_directory_async, identity::ApiError};

#[derive(Clone, Copy, Deserialize, utoipa::ToSchema)]
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

#[derive(Deserialize, utoipa::ToSchema, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct SourceArchiveQuery {
    #[serde(default, deserialize_with = "blank_as_none")]
    rev: Option<String>,
    format: SourceArchiveFormat,
}

#[utoipa::path(
    get, path = "/repositories/{namespace}/{name}/source",
    operation_id = "get_repositories_namespace_name_source",
    summary = "Download a source archive",
    tag = "Repository content",
    params(
        ("namespace" = String, Path),
        ("name" = String, Path),
        SourceArchiveQuery,
    ),
    security((), ("token" = []), ("session" = [])),
    responses(
        (status = 200, description = "Success", body = crate::openapi::Binary, content_type = "application/octet-stream"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
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

use gitadel_repositories::archive::prune_archive_cache;

#[allow(clippy::too_many_arguments)]
fn write_archive_file(
    repository_path: &Path,
    output_path: &Path,
    archive_format: SourceArchiveFormat,
    commit_oid: ObjectId,
    tree_oid: ObjectId,
    mtime: u64,
    prefix: String,
) -> anyhow::Result<()> {
    let format = match archive_format {
        SourceArchiveFormat::Zip => gitadel_repositories::archive::ArchiveFormat::Zip,
        SourceArchiveFormat::TarGz => gitadel_repositories::archive::ArchiveFormat::TarGz,
    };
    gitadel_repositories::archive::write_archive_file(
        repository_path,
        output_path,
        format,
        commit_oid,
        tree_oid,
        mtime,
        prefix,
    )
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
