use std::path::PathBuf;

use axum::{
    Json,
    body::Body,
    extract::{Path as AxumPath, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use axum_extra::extract::cookie::CookieJar;
use chrono::Utc;
use futures_util::TryStreamExt as _;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, ExprTrait, QueryFilter, QueryOrder, Set,
    TransactionTrait,
};
use serde::{Deserialize, Serialize};
use tokio::{fs, io::AsyncWriteExt as _};
use tokio_util::io::ReaderStream;
use uuid::Uuid;

use super::{Permission, RepositoryState, browser, render_markdown};
use crate::{
    actions::tokens,
    entity::{release_asset, repository, repository_release, user},
    identity::{ApiError, SCOPE_READ, SCOPE_WRITE},
};

const MAX_RELEASE_TITLE_LENGTH: usize = 255;
const MAX_RELEASE_BODY_LENGTH: usize = 1_000_000;
pub(super) const MAX_RELEASE_ASSET_BYTES: u64 = 2 * 1024 * 1024 * 1024;

#[derive(Serialize)]
pub struct ReleaseAssetResponse {
    id: Uuid,
    name: String,
    content_type: String,
    size_bytes: i64,
    download_count: i64,
    created_at: chrono::DateTime<Utc>,
    download_url: String,
    external_url: Option<String>,
}

#[derive(Serialize)]
pub struct ExternalReleaseAuthorResponse {
    username: String,
    profile_url: Option<String>,
}

#[derive(Serialize)]
pub struct ReleaseResponse {
    id: Uuid,
    target_revision: String,
    target_oid: String,
    title: String,
    body: String,
    rendered_body: String,
    prerelease: bool,
    latest: bool,
    author: String,
    published_at: chrono::DateTime<Utc>,
    created_at: chrono::DateTime<Utc>,
    updated_at: chrono::DateTime<Utc>,
    assets: Vec<ReleaseAssetResponse>,
    external_url: Option<String>,
    external_author: Option<ExternalReleaseAuthorResponse>,
}

#[derive(Deserialize)]
pub struct CreateReleaseRequest {
    target_revision: String,
    title: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    prerelease: bool,
}

#[derive(Deserialize)]
pub struct UpdateReleaseRequest {
    target_revision: Option<String>,
    title: Option<String>,
    body: Option<String>,
    prerelease: Option<bool>,
}

#[derive(Deserialize)]
pub struct UploadAssetQuery {
    name: String,
}

pub async fn list_releases(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name)): AxumPath<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<Vec<ReleaseResponse>>, ApiError> {
    let repository = readable_repository(&state, &headers, &jar, &namespace, &name).await?;
    let releases = repository_release::Entity::find()
        .filter(repository_release::Column::RepositoryId.eq(repository.id))
        .order_by_desc(repository_release::Column::PublishedAt)
        .all(state.identity().database())
        .await?;
    let latest_id = releases
        .iter()
        .find(|release| !release.prerelease)
        .map(|release| release.id);
    Ok(Json(
        release_responses(&state, &repository, releases, latest_id).await?,
    ))
}

pub async fn get_release(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name, id)): AxumPath<(String, String, Uuid)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<ReleaseResponse>, ApiError> {
    let repository = readable_repository(&state, &headers, &jar, &namespace, &name).await?;
    let release = find_release(&state, repository.id, id).await?;
    let latest_id = latest_release_id(&state, repository.id).await?;
    Ok(Json(
        release_response(&state, &repository, release, latest_id).await?,
    ))
}

pub async fn create_release(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name)): AxumPath<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<CreateReleaseRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let repository = state.find(&namespace, &name).await?;
    let actor_id = if let Some(job) =
        tokens::authenticate_job_bearer(state.identity().database(), &headers).await?
    {
        if job.repository.id != repository.id {
            return Err(ApiError::forbidden(
                "The Actions token belongs to another repository.",
            ));
        }
        job.run
            .actor_id
            .ok_or_else(|| ApiError::forbidden("The workflow run has no actor."))?
    } else {
        state
            .authenticated_repository(
                &headers,
                &jar,
                &namespace,
                &name,
                Permission::Write,
                SCOPE_WRITE,
            )
            .await?
            .0
            .user
            .id
    };
    let target_revision = validate_target_revision(&request.target_revision)?;
    let title = validate_title(&request.title)?;
    let body = validate_body(request.body)?;
    let target_oid = resolve_target(&state, &repository, &target_revision).await?;
    let now = Utc::now();
    let transaction = state.identity().database().begin().await?;
    let release = repository_release::ActiveModel {
        id: Set(Uuid::new_v4()),
        repository_id: Set(repository.id),
        author_user_id: Set(actor_id),
        target_revision: Set(target_revision),
        target_oid: Set(target_oid),
        title: Set(title),
        body: Set(body),
        prerelease: Set(request.prerelease),
        published_at: Set(now),
        created_at: Set(now),
        updated_at: Set(now),
        external_source: Set(None),
        external_instance_url: Set(None),
        external_id: Set(None),
        external_url: Set(None),
        external_author: Set(None),
        external_author_url: Set(None),
        external_updated_at: Set(None),
    }
    .insert(&transaction)
    .await?;
    state
        .identity()
        .audit_on(
            &transaction,
            Some(actor_id),
            "repository.release.create",
            Some(format!("{namespace}/{name}/{}", release.id)),
        )
        .await?;
    transaction.commit().await?;
    let latest_id = latest_release_id(&state, repository.id).await?;
    Ok((
        StatusCode::CREATED,
        Json(release_response(&state, &repository, release, latest_id).await?),
    ))
}

pub async fn update_release(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name, id)): AxumPath<(String, String, Uuid)>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<UpdateReleaseRequest>,
) -> Result<Json<ReleaseResponse>, ApiError> {
    let (actor, repository) = state
        .authenticated_repository(
            &headers,
            &jar,
            &namespace,
            &name,
            Permission::Write,
            SCOPE_WRITE,
        )
        .await?;
    let stored = find_release(&state, repository.id, id).await?;
    let mut release: repository_release::ActiveModel = stored.into();
    if let Some(target) = request.target_revision {
        let target = validate_target_revision(&target)?;
        let oid = resolve_target(&state, &repository, &target).await?;
        release.target_revision = Set(target);
        release.target_oid = Set(oid);
    }
    if let Some(title) = request.title {
        release.title = Set(validate_title(&title)?);
    }
    if let Some(body) = request.body {
        release.body = Set(validate_body(body)?);
    }
    if let Some(prerelease) = request.prerelease {
        release.prerelease = Set(prerelease);
    }
    release.updated_at = Set(Utc::now());
    let release = release.update(state.identity().database()).await?;
    state
        .identity()
        .audit(
            Some(actor.user.id),
            "repository.release.update",
            Some(format!("{namespace}/{name}/{id}")),
        )
        .await?;
    let latest_id = latest_release_id(&state, repository.id).await?;
    Ok(Json(
        release_response(&state, &repository, release, latest_id).await?,
    ))
}

pub async fn delete_release(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name, id)): AxumPath<(String, String, Uuid)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<StatusCode, ApiError> {
    let (actor, repository) = state
        .authenticated_repository(
            &headers,
            &jar,
            &namespace,
            &name,
            Permission::Write,
            SCOPE_WRITE,
        )
        .await?;
    find_release(&state, repository.id, id).await?;
    let transaction = state.identity().database().begin().await?;
    repository_release::Entity::delete_by_id(id)
        .exec(&transaction)
        .await?;
    state
        .identity()
        .audit_on(
            &transaction,
            Some(actor.user.id),
            "repository.release.delete",
            Some(format!("{namespace}/{name}/{id}")),
        )
        .await?;
    transaction.commit().await?;
    if let Err(error) = fs::remove_dir_all(release_directory(&state, &repository, id)).await
        && error.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(%error, release_id = %id, "could not remove release assets");
    }
    Ok(StatusCode::NO_CONTENT)
}

pub async fn upload_asset(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name, id)): AxumPath<(String, String, Uuid)>,
    Query(query): Query<UploadAssetQuery>,
    headers: HeaderMap,
    jar: CookieJar,
    body: Body,
) -> Result<impl IntoResponse, ApiError> {
    let repository = state.find(&namespace, &name).await?;
    let actor_id = if let Some(job) =
        tokens::authenticate_job_bearer(state.identity().database(), &headers).await?
    {
        if job.repository.id != repository.id {
            return Err(ApiError::forbidden(
                "The Actions token belongs to another repository.",
            ));
        }
        job.run
            .actor_id
            .ok_or_else(|| ApiError::forbidden("The workflow run has no actor."))?
    } else {
        state
            .authenticated_repository(
                &headers,
                &jar,
                &namespace,
                &name,
                Permission::Write,
                SCOPE_WRITE,
            )
            .await?
            .0
            .user
            .id
    };
    find_release(&state, repository.id, id).await?;
    let asset_name = validate_asset_name(&query.name)?;
    if release_asset::Entity::find()
        .filter(release_asset::Column::ReleaseId.eq(id))
        .filter(release_asset::Column::Name.eq(&asset_name))
        .one(state.identity().database())
        .await?
        .is_some()
    {
        return Err(ApiError::conflict(
            "An asset with that file name already exists.",
        ));
    }
    let asset_id = Uuid::new_v4();
    let directory = release_directory(&state, &repository, id);
    fs::create_dir_all(&directory)
        .await
        .map_err(ApiError::internal)?;
    let path = directory.join(asset_id.to_string());
    let temporary_path = directory.join(format!(".{asset_id}.upload"));
    let mut output = fs::File::create(&temporary_path)
        .await
        .map_err(ApiError::internal)?;
    let mut stream = body.into_data_stream();
    let mut size = 0_u64;
    while let Some(chunk) = stream.try_next().await.map_err(ApiError::internal)? {
        size = size.saturating_add(chunk.len() as u64);
        if size > MAX_RELEASE_ASSET_BYTES {
            drop(output);
            let _ = fs::remove_file(&temporary_path).await;
            return Err(ApiError::bad_request("Release assets cannot exceed 2 GiB."));
        }
        output.write_all(&chunk).await.map_err(ApiError::internal)?;
    }
    output.flush().await.map_err(ApiError::internal)?;
    drop(output);
    fs::rename(&temporary_path, &path)
        .await
        .map_err(ApiError::internal)?;

    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("application/octet-stream")
        .chars()
        .take(255)
        .collect::<String>();
    let asset = release_asset::ActiveModel {
        id: Set(asset_id),
        release_id: Set(id),
        name: Set(asset_name),
        content_type: Set(content_type),
        size_bytes: Set(size as i64),
        download_count: Set(0),
        created_at: Set(Utc::now()),
        external_source: Set(None),
        external_instance_url: Set(None),
        external_id: Set(None),
        external_url: Set(None),
        external_updated_at: Set(None),
    }
    .insert(state.identity().database())
    .await;
    let asset = match asset {
        Ok(asset) => asset,
        Err(error) => {
            let _ = fs::remove_file(&path).await;
            return Err(error.into());
        }
    };
    state
        .identity()
        .audit(
            Some(actor_id),
            "repository.release.asset.upload",
            Some(format!("{namespace}/{name}/{id}/{}", asset.id)),
        )
        .await?;
    Ok((
        StatusCode::CREATED,
        Json(asset_response(&repository, asset)),
    ))
}

pub async fn download_asset(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name, release_id, asset_id)): AxumPath<(String, String, Uuid, Uuid)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Response, ApiError> {
    let repository = readable_repository(&state, &headers, &jar, &namespace, &name).await?;
    find_release(&state, repository.id, release_id).await?;
    let asset = find_asset(&state, release_id, asset_id).await?;
    let file = fs::File::open(
        release_directory(&state, &repository, release_id).join(asset.id.to_string()),
    )
    .await
    .map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            ApiError::not_found()
        } else {
            ApiError::internal(error)
        }
    })?;
    let _ = release_asset::Entity::update_many()
        .col_expr(
            release_asset::Column::DownloadCount,
            sea_orm::sea_query::Expr::col(release_asset::Column::DownloadCount).add(1),
        )
        .filter(release_asset::Column::Id.eq(asset.id))
        .exec(state.identity().database())
        .await;
    let mut response = Response::new(Body::from_stream(ReaderStream::new(file)));
    let content_type = HeaderValue::from_str(&asset.content_type)
        .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream"));
    let safe_name = ascii_download_name(&asset.name);
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, content_type);
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!("attachment; filename=\"{safe_name}\""))
            .unwrap_or_else(|_| HeaderValue::from_static("attachment")),
    );
    response.headers_mut().insert(
        header::CONTENT_LENGTH,
        HeaderValue::from_str(&asset.size_bytes.to_string()).map_err(ApiError::internal)?,
    );
    Ok(response)
}

pub async fn delete_asset(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name, release_id, asset_id)): AxumPath<(String, String, Uuid, Uuid)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<StatusCode, ApiError> {
    let (actor, repository) = state
        .authenticated_repository(
            &headers,
            &jar,
            &namespace,
            &name,
            Permission::Write,
            SCOPE_WRITE,
        )
        .await?;
    find_release(&state, repository.id, release_id).await?;
    let asset = find_asset(&state, release_id, asset_id).await?;
    release_asset::Entity::delete_by_id(asset.id)
        .exec(state.identity().database())
        .await?;
    let path = release_directory(&state, &repository, release_id).join(asset.id.to_string());
    if let Err(error) = fs::remove_file(path).await
        && error.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(%error, asset_id = %asset.id, "could not remove release asset");
    }
    state
        .identity()
        .audit(
            Some(actor.user.id),
            "repository.release.asset.delete",
            Some(format!("{namespace}/{name}/{release_id}/{}", asset.id)),
        )
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

// Consumed by the repository import metadata worker.
pub(super) struct ImportedRelease {
    pub(super) external_source: String,
    pub(super) external_instance_url: String,
    pub(super) external_id: String,
    pub(super) external_url: Option<String>,
    pub(super) external_author: Option<String>,
    pub(super) external_author_url: Option<String>,
    pub(super) external_updated_at: Option<chrono::DateTime<Utc>>,
    pub(super) target_revision: String,
    pub(super) title: String,
    pub(super) body: String,
    pub(super) prerelease: bool,
    pub(super) published_at: chrono::DateTime<Utc>,
    pub(super) created_at: chrono::DateTime<Utc>,
}

// Consumed by the repository import metadata worker.
pub(super) struct ImportedAsset {
    pub(super) external_source: String,
    pub(super) external_instance_url: String,
    pub(super) external_id: String,
    pub(super) external_url: String,
    pub(super) name: String,
    pub(super) content_type: String,
    pub(super) expected_size: Option<i64>,
    pub(super) external_updated_at: Option<chrono::DateTime<Utc>>,
}

// Consumed by the repository import metadata worker.
pub(super) async fn upsert_imported_release(
    state: &RepositoryState,
    repository: &repository::Model,
    importer_user_id: Uuid,
    imported: ImportedRelease,
) -> Result<repository_release::Model, ApiError> {
    let target_oid = resolve_exact_tag(state, repository, &imported.target_revision).await?;
    let database = state.identity().database();
    let current = repository_release::Entity::find()
        .filter(repository_release::Column::RepositoryId.eq(repository.id))
        .filter(repository_release::Column::ExternalSource.eq(&imported.external_source))
        .filter(repository_release::Column::ExternalInstanceUrl.eq(&imported.external_instance_url))
        .filter(repository_release::Column::ExternalId.eq(&imported.external_id))
        .one(database)
        .await?;

    if let Some(current) = current {
        let mut active: repository_release::ActiveModel = current.into();
        active.author_user_id = Set(importer_user_id);
        active.target_revision = Set(imported.target_revision);
        active.target_oid = Set(target_oid);
        active.title = Set(imported.title);
        active.body = Set(imported.body);
        active.prerelease = Set(imported.prerelease);
        active.published_at = Set(imported.published_at);
        active.external_url = Set(imported.external_url);
        active.external_author = Set(imported.external_author);
        active.external_author_url = Set(imported.external_author_url);
        active.external_updated_at = Set(imported.external_updated_at);
        active.created_at = Set(imported.created_at);
        active.updated_at = Set(imported.external_updated_at.unwrap_or(imported.created_at));
        return Ok(active.update(database).await?);
    }

    if repository_release::Entity::find()
        .filter(repository_release::Column::RepositoryId.eq(repository.id))
        .filter(repository_release::Column::TargetRevision.eq(&imported.target_revision))
        .filter(repository_release::Column::ExternalSource.is_null())
        .one(database)
        .await?
        .is_some()
    {
        return Err(ApiError::conflict(format!(
            "Release tag `{}` conflicts with an existing local release.",
            imported.target_revision
        )));
    }

    let now = imported.external_updated_at.unwrap_or(imported.created_at);
    Ok(repository_release::ActiveModel {
        id: Set(Uuid::new_v4()),
        repository_id: Set(repository.id),
        author_user_id: Set(importer_user_id),
        target_revision: Set(imported.target_revision),
        target_oid: Set(target_oid),
        title: Set(imported.title),
        body: Set(imported.body),
        prerelease: Set(imported.prerelease),
        published_at: Set(imported.published_at),
        created_at: Set(imported.created_at),
        updated_at: Set(now),
        external_source: Set(Some(imported.external_source)),
        external_instance_url: Set(Some(imported.external_instance_url)),
        external_id: Set(Some(imported.external_id)),
        external_url: Set(imported.external_url),
        external_author: Set(imported.external_author),
        external_author_url: Set(imported.external_author_url),
        external_updated_at: Set(imported.external_updated_at),
    }
    .insert(database)
    .await?)
}

// Consumed by the repository import metadata worker.
pub(super) async fn imported_asset_exists(
    state: &RepositoryState,
    release_id: Uuid,
    external_source: &str,
    external_instance_url: &str,
    external_id: &str,
) -> Result<bool, ApiError> {
    Ok(release_asset::Entity::find()
        .filter(release_asset::Column::ReleaseId.eq(release_id))
        .filter(release_asset::Column::ExternalSource.eq(external_source))
        .filter(release_asset::Column::ExternalInstanceUrl.eq(external_instance_url))
        .filter(release_asset::Column::ExternalId.eq(external_id))
        .one(state.identity().database())
        .await?
        .is_some())
}

// Consumed by the repository import metadata worker.
pub(super) async fn store_imported_asset(
    state: &RepositoryState,
    repository: &repository::Model,
    release_id: Uuid,
    imported: ImportedAsset,
    response: reqwest::Response,
) -> Result<(), ApiError> {
    if response
        .content_length()
        .is_some_and(|size| size > MAX_RELEASE_ASSET_BYTES)
    {
        return Err(ApiError::bad_request("Release assets cannot exceed 2 GiB."));
    }
    let name = validate_asset_name(&imported.name)?;
    let content_type = imported.content_type.chars().take(255).collect::<String>();
    let directory = release_directory(state, repository, release_id);
    fs::create_dir_all(&directory)
        .await
        .map_err(ApiError::internal)?;
    let asset_id = Uuid::new_v4();
    let path = directory.join(asset_id.to_string());
    let temporary_path = directory.join(format!(".{asset_id}.import"));
    let mut output = match fs::File::create(&temporary_path).await {
        Ok(output) => output,
        Err(error) => return Err(ApiError::internal(error)),
    };
    let mut response = response;
    let mut size = 0_u64;
    let write_result = async {
        while let Some(chunk) = response.chunk().await.map_err(ApiError::internal)? {
            size = size
                .checked_add(chunk.len() as u64)
                .ok_or_else(|| ApiError::bad_request("Release asset size overflowed."))?;
            if size > MAX_RELEASE_ASSET_BYTES {
                return Err(ApiError::bad_request("Release assets cannot exceed 2 GiB."));
            }
            output.write_all(&chunk).await.map_err(ApiError::internal)?;
        }
        output.flush().await.map_err(ApiError::internal)
    }
    .await;
    drop(output);
    if let Err(error) = write_result {
        let _ = fs::remove_file(&temporary_path).await;
        return Err(error);
    }
    if imported
        .expected_size
        .is_some_and(|expected| expected < 0 || expected as u64 != size)
    {
        let _ = fs::remove_file(&temporary_path).await;
        return Err(ApiError::bad_request(
            "Source release asset size did not match its metadata.",
        ));
    }
    fs::rename(&temporary_path, &path)
        .await
        .map_err(ApiError::internal)?;

    let asset = release_asset::ActiveModel {
        id: Set(asset_id),
        release_id: Set(release_id),
        name: Set(name),
        content_type: Set(content_type),
        size_bytes: Set(size as i64),
        download_count: Set(0),
        created_at: Set(Utc::now()),
        external_source: Set(Some(imported.external_source)),
        external_instance_url: Set(Some(imported.external_instance_url)),
        external_id: Set(Some(imported.external_id)),
        external_url: Set(Some(imported.external_url)),
        external_updated_at: Set(imported.external_updated_at),
    }
    .insert(state.identity().database())
    .await;
    if let Err(error) = asset {
        let _ = fs::remove_file(&path).await;
        return Err(error.into());
    }
    Ok(())
}

async fn readable_repository(
    state: &RepositoryState,
    headers: &HeaderMap,
    jar: &CookieJar,
    namespace: &str,
    name: &str,
) -> Result<repository::Model, ApiError> {
    let repository = state.find(namespace, name).await?;
    let user_id = state
        .identity()
        .optional_user(headers, jar, SCOPE_READ)
        .await?
        .map(|actor| actor.id);
    state
        .authorize(&repository, user_id, Permission::Read)
        .await?;
    Ok(repository)
}

async fn find_release(
    state: &RepositoryState,
    repository_id: Uuid,
    id: Uuid,
) -> Result<repository_release::Model, ApiError> {
    repository_release::Entity::find_by_id(id)
        .filter(repository_release::Column::RepositoryId.eq(repository_id))
        .one(state.identity().database())
        .await?
        .ok_or_else(ApiError::not_found)
}

async fn find_asset(
    state: &RepositoryState,
    release_id: Uuid,
    id: Uuid,
) -> Result<release_asset::Model, ApiError> {
    release_asset::Entity::find_by_id(id)
        .filter(release_asset::Column::ReleaseId.eq(release_id))
        .one(state.identity().database())
        .await?
        .ok_or_else(ApiError::not_found)
}

async fn latest_release_id(
    state: &RepositoryState,
    repository_id: Uuid,
) -> Result<Option<Uuid>, ApiError> {
    Ok(repository_release::Entity::find()
        .filter(repository_release::Column::RepositoryId.eq(repository_id))
        .filter(repository_release::Column::Prerelease.eq(false))
        .order_by_desc(repository_release::Column::PublishedAt)
        .one(state.identity().database())
        .await?
        .map(|release| release.id))
}

async fn release_responses(
    state: &RepositoryState,
    repository: &repository::Model,
    releases: Vec<repository_release::Model>,
    latest_id: Option<Uuid>,
) -> Result<Vec<ReleaseResponse>, ApiError> {
    let mut responses = Vec::with_capacity(releases.len());
    for release in releases {
        responses.push(release_response(state, repository, release, latest_id).await?);
    }
    Ok(responses)
}

async fn release_response(
    state: &RepositoryState,
    repository: &repository::Model,
    release: repository_release::Model,
    latest_id: Option<Uuid>,
) -> Result<ReleaseResponse, ApiError> {
    let author = user::Entity::find_by_id(release.author_user_id)
        .one(state.identity().database())
        .await?
        .map(|author| author.username)
        .unwrap_or_else(|| "deleted-user".to_owned());
    let assets = release_asset::Entity::find()
        .filter(release_asset::Column::ReleaseId.eq(release.id))
        .order_by_asc(release_asset::Column::Name)
        .all(state.identity().database())
        .await?
        .into_iter()
        .map(|asset| asset_response(repository, asset))
        .collect();
    Ok(ReleaseResponse {
        id: release.id,
        target_revision: release.target_revision,
        target_oid: release.target_oid,
        title: release.title,
        rendered_body: render_markdown(&release.body),
        body: release.body,
        prerelease: release.prerelease,
        latest: latest_id == Some(release.id),
        author,
        published_at: release.published_at,
        created_at: release.created_at,
        updated_at: release.updated_at,
        assets,
        external_url: release.external_url,
        external_author: release
            .external_author
            .map(|username| ExternalReleaseAuthorResponse {
                username,
                profile_url: release.external_author_url,
            }),
    })
}

fn asset_response(
    repository: &repository::Model,
    asset: release_asset::Model,
) -> ReleaseAssetResponse {
    ReleaseAssetResponse {
        id: asset.id,
        download_url: format!(
            "/api/v1/repositories/{}/{}/releases/{}/assets/{}",
            repository.namespace, repository.name, asset.release_id, asset.id
        ),
        name: asset.name,
        content_type: asset.content_type,
        size_bytes: asset.size_bytes,
        download_count: asset.download_count,
        created_at: asset.created_at,
        external_url: asset.external_url,
    }
}

async fn resolve_target(
    state: &RepositoryState,
    repository: &repository::Model,
    target: &str,
) -> Result<String, ApiError> {
    let path = state.repository_path(repository);
    let target = target.to_owned();
    browser::read_git(path, move |git| {
        let oid = git.peel_to_commit_oid(git.rev_parse(&target)?)?;
        Ok(oid.to_hex())
    })
    .await
    .map_err(|_| ApiError::bad_request("The release target must resolve to an existing commit."))
}

// Consumed by the repository import metadata worker.
pub(super) async fn resolve_exact_tag(
    state: &RepositoryState,
    repository: &repository::Model,
    tag: &str,
) -> Result<String, ApiError> {
    if tag.is_empty() || tag.len() > 255 || tag.chars().any(char::is_control) {
        return Err(ApiError::bad_request(
            "The imported release tag is invalid.",
        ));
    }
    let path = state.repository_path(repository);
    let revision = format!("refs/tags/{tag}");
    browser::read_git(path, move |git| {
        let oid = git.peel_to_commit_oid(git.rev_parse(&revision)?)?;
        Ok(oid.to_hex())
    })
    .await
    .map_err(|_| ApiError::bad_request("The imported release tag is not present locally."))
}

fn validate_target_revision(value: &str) -> Result<String, ApiError> {
    let value = value.trim();
    if value.is_empty() || value.len() > 255 || value.chars().any(char::is_control) {
        return Err(ApiError::bad_request(
            "Release targets must be a tag, branch, or commit up to 255 characters.",
        ));
    }
    Ok(value.to_owned())
}

fn validate_title(value: &str) -> Result<String, ApiError> {
    let value = value.trim();
    if value.is_empty() || value.len() > MAX_RELEASE_TITLE_LENGTH {
        return Err(ApiError::bad_request(
            "Release titles must be between 1 and 255 characters.",
        ));
    }
    Ok(value.to_owned())
}

fn validate_body(value: String) -> Result<String, ApiError> {
    if value.len() > MAX_RELEASE_BODY_LENGTH {
        return Err(ApiError::bad_request(
            "Release notes must be at most 1,000,000 characters.",
        ));
    }
    Ok(value)
}

fn validate_asset_name(value: &str) -> Result<String, ApiError> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > 255
        || value == "."
        || value == ".."
        || value.contains(['/', '\\'])
        || value.chars().any(char::is_control)
    {
        return Err(ApiError::bad_request(
            "The release asset file name is not valid.",
        ));
    }
    Ok(value.to_owned())
}

fn release_directory(
    state: &RepositoryState,
    repository: &repository::Model,
    release_id: Uuid,
) -> PathBuf {
    state
        .lfs_repository_path(repository)
        .join("releases")
        .join(release_id.to_string())
}

fn ascii_download_name(value: &str) -> String {
    let value = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    if value.is_empty() {
        "download".to_owned()
    } else {
        value
    }
}
