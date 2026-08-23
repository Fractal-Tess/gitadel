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
    entity::{release_asset, repository, repository_release, user},
    identity::{ApiError, SCOPE_READ, SCOPE_WRITE},
};

const MAX_RELEASE_TITLE_LENGTH: usize = 255;
const MAX_RELEASE_BODY_LENGTH: usize = 1_000_000;
const MAX_RELEASE_ASSET_BYTES: u64 = 2 * 1024 * 1024 * 1024;

#[derive(Serialize)]
pub struct ReleaseAssetResponse {
    id: Uuid,
    name: String,
    content_type: String,
    size_bytes: i64,
    download_count: i64,
    created_at: chrono::DateTime<Utc>,
    download_url: String,
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
    let target_revision = validate_target_revision(&request.target_revision)?;
    let title = validate_title(&request.title)?;
    let body = validate_body(request.body)?;
    let target_oid = resolve_target(&state, &repository, &target_revision).await?;
    let now = Utc::now();
    let transaction = state.identity().database().begin().await?;
    let release = repository_release::ActiveModel {
        id: Set(Uuid::new_v4()),
        repository_id: Set(repository.id),
        author_user_id: Set(actor.user.id),
        target_revision: Set(target_revision),
        target_oid: Set(target_oid),
        title: Set(title),
        body: Set(body),
        prerelease: Set(request.prerelease),
        published_at: Set(now),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(&transaction)
    .await?;
    state
        .identity()
        .audit_on(
            &transaction,
            Some(actor.user.id),
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
            Some(actor.user.id),
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
