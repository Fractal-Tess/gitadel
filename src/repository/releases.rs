use std::{
    collections::{BTreeSet, HashMap},
    path::{Path, PathBuf},
    pin::Pin,
};

use axum::{
    Json,
    body::{Body, Bytes},
    extract::{Multipart, Path as AxumPath, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use axum_extra::extract::cookie::CookieJar;
use chrono::Utc;
use futures_util::{Stream, StreamExt as _, TryStreamExt as _};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, ExprTrait, PaginatorTrait, QueryFilter, QueryOrder,
    QuerySelect, Set, TransactionTrait,
};
use serde::{Deserialize, Serialize};
use tokio::{fs, io::AsyncWriteExt as _};
use tokio_util::io::ReaderStream;
use utoipa::ToSchema;
use uuid::Uuid;

use super::{
    Permission, RepositoryState,
    browser::{self, readable_repository},
    render_markdown,
    resources::{IN_QUERY_CHUNK, PageWindow, total_count_header},
    webhook_events::{self, ChangeAction},
};
use crate::{
    actions::tokens,
    entity::{release_asset, repository, repository_release, user},
    identity::{ApiError, SCOPE_READ, SCOPE_WRITE},
};

const MAX_RELEASE_TITLE_LENGTH: usize = 255;
const MAX_RELEASE_BODY_LENGTH: usize = 1_000_000;
pub(super) const MAX_RELEASE_ASSET_BYTES: u64 = 2 * 1024 * 1024 * 1024;

#[derive(Serialize, ToSchema)]
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

#[derive(Serialize, ToSchema)]
pub struct ExternalReleaseAuthorResponse {
    username: String,
    profile_url: Option<String>,
}

#[derive(Serialize, ToSchema)]
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

#[derive(Deserialize, ToSchema)]
pub struct CreateReleaseRequest {
    target_revision: String,
    title: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    prerelease: bool,
}

#[derive(Deserialize, ToSchema)]
pub struct UpdateReleaseRequest {
    target_revision: Option<String>,
    title: Option<String>,
    body: Option<String>,
    prerelease: Option<bool>,
}

#[derive(Deserialize)]
pub struct ListReleasesQuery {
    page: Option<u64>,
    per_page: Option<u64>,
}

#[derive(Deserialize)]
pub struct UploadAssetQuery {
    name: String,
}

#[derive(Deserialize, ToSchema)]
pub struct ForgejoCreateReleaseRequest {
    tag_name: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    prerelease: bool,
}

#[derive(Deserialize, ToSchema)]
pub struct ForgejoUpdateReleaseRequest {
    #[serde(default)]
    draft: Option<bool>,
    #[serde(default)]
    hide_archive_links: Option<bool>,
}

#[derive(Serialize, ToSchema)]
pub struct ForgejoTagResponse {
    name: String,
    commit: ForgejoCommitResponse,
}

#[derive(Serialize, ToSchema)]
struct ForgejoCommitResponse {
    sha: String,
}

#[derive(Serialize, ToSchema)]
pub struct ForgejoReleaseResponse {
    id: i64,
    tag_name: String,
    target_commitish: String,
    name: String,
    body: String,
    draft: bool,
    prerelease: bool,
    assets: Vec<ForgejoReleaseAssetResponse>,
}

#[derive(Serialize, ToSchema)]
pub struct ForgejoReleaseAssetResponse {
    id: i64,
    name: String,
    size: i64,
    browser_download_url: String,
}

#[derive(Serialize, ToSchema)]
pub struct ForgejoUserResponse {
    id: i64,
    login: String,
    username: String,
}

pub async fn forgejo_current_user(
    State(state): State<RepositoryState>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<ForgejoUserResponse>, ApiError> {
    let username = if let Some(job) =
        tokens::authenticate_job_api_token(state.identity().database(), &headers).await?
    {
        let actor_id = job
            .run
            .actor_id
            .ok_or_else(|| ApiError::forbidden("The workflow run has no actor."))?;
        user::Entity::find_by_id(actor_id)
            .one(state.identity().database())
            .await?
            .ok_or_else(ApiError::unauthorized)?
            .username
    } else {
        state
            .identity()
            .authenticate(&headers, &jar, SCOPE_READ)
            .await?
            .user
            .username
    };
    Ok(Json(ForgejoUserResponse {
        id: 1,
        login: username.clone(),
        username,
    }))
}

pub async fn list_releases(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name)): AxumPath<(String, String)>,
    Query(query): Query<ListReleasesQuery>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<impl IntoResponse, ApiError> {
    let repository = readable_repository(&state, &headers, &jar, &namespace, &name).await?;
    let window = PageWindow::new(query.page, query.per_page);
    let select = repository_release::Entity::find()
        .filter(repository_release::Column::RepositoryId.eq(repository.id));
    let total = select.clone().count(state.identity().database()).await?;
    let releases = select
        .order_by_desc(repository_release::Column::PublishedAt)
        // Releases published in the same instant keep one order across pages.
        .order_by_desc(repository_release::Column::Id)
        .offset(window.offset)
        .limit(window.limit)
        .all(state.identity().database())
        .await?;
    // The latest release may sit on another page than the one requested.
    let latest_id = latest_release_id(&state, repository.id).await?;
    Ok((
        total_count_header(total),
        Json(release_responses(&state, &repository, releases, latest_id).await?),
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
    let (repository, actor_id) =
        writable_release_repository(&state, &headers, &jar, &namespace, &name).await?;
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
    webhook_events::release(
        &state,
        &repository,
        actor_id,
        &release,
        ChangeAction::Created,
    )
    .await;
    let latest_id = latest_release_id(&state, repository.id).await?;
    Ok((
        StatusCode::CREATED,
        Json(release_response(&state, &repository, release, latest_id).await?),
    ))
}

pub async fn forgejo_get_tag(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name, tag)): AxumPath<(String, String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<ForgejoTagResponse>, ApiError> {
    let (repository, _) =
        writable_release_repository(&state, &headers, &jar, &namespace, &name).await?;
    let oid = resolve_exact_tag(&state, &repository, &tag).await?;
    Ok(Json(ForgejoTagResponse {
        name: tag,
        commit: ForgejoCommitResponse { sha: oid },
    }))
}

pub async fn forgejo_create_release(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name)): AxumPath<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<ForgejoCreateReleaseRequest>,
) -> Result<(StatusCode, Json<ForgejoReleaseResponse>), ApiError> {
    let (repository, actor_id) =
        writable_release_repository(&state, &headers, &jar, &namespace, &name).await?;
    let target_revision = validate_target_revision(&request.tag_name)?;
    let title = if request.name.trim().is_empty() {
        target_revision.clone()
    } else {
        validate_title(&request.name)?
    };
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
    webhook_events::release(
        &state,
        &repository,
        actor_id,
        &release,
        ChangeAction::Created,
    )
    .await;
    Ok((
        StatusCode::CREATED,
        Json(forgejo_release_response(&state, &repository, release).await?),
    ))
}

pub async fn forgejo_get_release_by_tag(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name, tag)): AxumPath<(String, String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<ForgejoReleaseResponse>, ApiError> {
    let (repository, _) =
        writable_release_repository(&state, &headers, &jar, &namespace, &name).await?;
    let release = repository_release::Entity::find()
        .filter(repository_release::Column::RepositoryId.eq(repository.id))
        .filter(repository_release::Column::TargetRevision.eq(tag))
        .order_by_desc(repository_release::Column::PublishedAt)
        .one(state.identity().database())
        .await?
        .ok_or_else(ApiError::not_found)?;
    Ok(Json(
        forgejo_release_response(&state, &repository, release).await?,
    ))
}

pub async fn forgejo_update_release(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name, id)): AxumPath<(String, String, i64)>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<ForgejoUpdateReleaseRequest>,
) -> Result<Json<ForgejoReleaseResponse>, ApiError> {
    let (repository, _) =
        writable_release_repository(&state, &headers, &jar, &namespace, &name).await?;
    let release = find_forgejo_release(&state, repository.id, id).await?;
    let _ = (request.draft, request.hide_archive_links);
    Ok(Json(
        forgejo_release_response(&state, &repository, release).await?,
    ))
}

pub async fn forgejo_upload_asset(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name, id)): AxumPath<(String, String, i64)>,
    headers: HeaderMap,
    jar: CookieJar,
    mut multipart: Multipart,
) -> Result<impl IntoResponse, ApiError> {
    let (repository, actor_id) =
        writable_release_repository(&state, &headers, &jar, &namespace, &name).await?;
    let field = multipart
        .next_field()
        .await
        .map_err(ApiError::internal)?
        .filter(|field| field.name() == Some("attachment"))
        .ok_or_else(|| ApiError::bad_request("Expected an `attachment` file field."))?;
    let asset_name = field
        .file_name()
        .map(str::to_owned)
        .ok_or_else(|| ApiError::bad_request("The `attachment` field needs a file name."))?;
    let content_type = field
        .content_type()
        .unwrap_or("application/octet-stream")
        .to_owned();
    let stream = field.map_err(ApiError::internal).boxed();
    let release = find_forgejo_release(&state, repository.id, id).await?;
    let asset = store_release_asset(
        &state,
        &repository,
        actor_id,
        release.id,
        &asset_name,
        &content_type,
        stream,
        MAX_RELEASE_ASSET_BYTES,
    )
    .await?;
    Ok((
        StatusCode::CREATED,
        Json(forgejo_asset_response(&repository, asset)),
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
    webhook_events::release(
        &state,
        &repository,
        actor.user.id,
        &release,
        ChangeAction::Edited,
    )
    .await;
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
    let release = find_release(&state, repository.id, id).await?;
    let deleted_event = webhook_events::prepare_release(
        &state,
        &repository,
        actor.user.id,
        &release,
        ChangeAction::Deleted,
    )
    .await;
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
    webhook_events::send_prepared(&state, repository.id, "release", deleted_event).await;
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
    let (repository, actor_id) =
        writable_release_repository(&state, &headers, &jar, &namespace, &name).await?;
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_owned();
    let stream = body.into_data_stream().map_err(ApiError::internal).boxed();
    let asset = store_release_asset(
        &state,
        &repository,
        actor_id,
        id,
        &query.name,
        &content_type,
        stream,
        MAX_RELEASE_ASSET_BYTES,
    )
    .await?;
    Ok((
        StatusCode::CREATED,
        Json(asset_response(&repository, asset)),
    ))
}

type AssetStream<'a> = Pin<Box<dyn Stream<Item = Result<Bytes, ApiError>> + Send + 'a>>;

/// Streams a request body into `temporary_path`, returning the byte count, and
/// fails once the body exceeds `max_bytes`.
async fn write_upload<S>(
    temporary_path: &Path,
    stream: &mut S,
    max_bytes: u64,
) -> Result<u64, ApiError>
where
    S: Stream<Item = Result<Bytes, ApiError>> + Unpin,
{
    let mut output = fs::File::create(temporary_path)
        .await
        .map_err(ApiError::internal)?;
    let mut size = 0_u64;
    while let Some(chunk) = stream.try_next().await? {
        size = size.saturating_add(chunk.len() as u64);
        if size > max_bytes {
            return Err(ApiError::bad_request("Release assets cannot exceed 2 GiB."));
        }
        output.write_all(&chunk).await.map_err(ApiError::internal)?;
    }
    output.flush().await.map_err(ApiError::internal)?;
    Ok(size)
}

/// `max_bytes` is [`MAX_RELEASE_ASSET_BYTES`] for every real upload; tests pass
/// a small cap to exercise the limit without streaming gigabytes.
#[allow(clippy::too_many_arguments)]
async fn store_release_asset(
    state: &RepositoryState,
    repository: &repository::Model,
    actor_id: Uuid,
    release_id: Uuid,
    name: &str,
    content_type: &str,
    mut stream: AssetStream<'_>,
    max_bytes: u64,
) -> Result<release_asset::Model, ApiError> {
    find_release(state, repository.id, release_id).await?;
    let asset_name = validate_asset_name(name)?;
    if release_asset::Entity::find()
        .filter(release_asset::Column::ReleaseId.eq(release_id))
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
    let directory = release_directory(state, repository, release_id);
    fs::create_dir_all(&directory)
        .await
        .map_err(ApiError::internal)?;
    let path = directory.join(asset_id.to_string());
    let temporary_path = directory.join(format!(".{asset_id}.upload"));
    let size = match write_upload(&temporary_path, &mut stream, max_bytes).await {
        Ok(size) => size,
        Err(error) => {
            // Whatever interrupted the upload, the partial file must not stay
            // behind in the release directory.
            let _ = fs::remove_file(&temporary_path).await;
            return Err(error);
        }
    };
    fs::rename(&temporary_path, &path)
        .await
        .map_err(ApiError::internal)?;

    let content_type = content_type.chars().take(255).collect::<String>();
    let asset = release_asset::ActiveModel {
        id: Set(asset_id),
        release_id: Set(release_id),
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
            Some(format!(
                "{}/{}/{release_id}/{}",
                repository.namespace, repository.name, asset.id
            )),
        )
        .await?;
    Ok(asset)
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
    // A failed counter update must not fail the download, but it is a
    // database write going wrong and should not pass unnoticed.
    if let Err(error) = release_asset::Entity::update_many()
        .col_expr(
            release_asset::Column::DownloadCount,
            sea_orm::sea_query::Expr::col(release_asset::Column::DownloadCount).add(1),
        )
        .filter(release_asset::Column::Id.eq(asset.id))
        .exec(state.identity().database())
        .await
    {
        tracing::error!(
            %error,
            asset_id = %asset.id,
            "could not record release asset download"
        );
    }
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

async fn writable_release_repository(
    state: &RepositoryState,
    headers: &HeaderMap,
    jar: &CookieJar,
    namespace: &str,
    name: &str,
) -> Result<(repository::Model, Uuid), ApiError> {
    let repository = state.find(namespace, name).await?;
    if let Some(job) =
        tokens::authenticate_job_api_token(state.identity().database(), headers).await?
    {
        if job.repository.id != repository.id {
            return Err(ApiError::forbidden(
                "The Actions token belongs to another repository.",
            ));
        }
        let actor_id = job
            .run
            .actor_id
            .ok_or_else(|| ApiError::forbidden("The workflow run has no actor."))?;
        // The job token stands in for the membership check, not for the
        // repository's own state: a repository archived or turned into a
        // mirror while the job runs is read-only for the job too.
        ensure_accepts_writes(&repository)?;
        return Ok((repository, actor_id));
    }
    let (actor, repository) = state
        .authenticated_repository(
            headers,
            jar,
            namespace,
            name,
            Permission::Write,
            SCOPE_WRITE,
        )
        .await?;
    Ok((repository, actor.user.id))
}

/// The repository states [`RepositoryState::authorize`] refuses writes in.
fn ensure_accepts_writes(repository: &repository::Model) -> Result<(), ApiError> {
    if repository.archived_at.is_some() {
        return Err(ApiError::forbidden("Archived repositories are read-only."));
    }
    if repository.mirrored {
        return Err(ApiError::forbidden("Mirrored repositories are read-only."));
    }
    Ok(())
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

async fn find_forgejo_release(
    state: &RepositoryState,
    repository_id: Uuid,
    id: i64,
) -> Result<repository_release::Model, ApiError> {
    repository_release::Entity::find()
        .filter(repository_release::Column::RepositoryId.eq(repository_id))
        .all(state.identity().database())
        .await?
        .into_iter()
        .find(|release| forgejo_numeric_id(release.id) == id)
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
    if releases.is_empty() {
        return Ok(Vec::new());
    }
    let database = state.identity().database();
    // One query per related table for the whole page, however long it is.
    let author_ids = releases
        .iter()
        .map(|release| release.author_user_id)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let mut authors = HashMap::with_capacity(author_ids.len());
    for ids in author_ids.chunks(IN_QUERY_CHUNK) {
        authors.extend(
            user::Entity::find()
                .filter(user::Column::Id.is_in(ids.iter().copied()))
                .all(database)
                .await?
                .into_iter()
                .map(|author| (author.id, author.username)),
        );
    }
    let release_ids = releases
        .iter()
        .map(|release| release.id)
        .collect::<Vec<_>>();
    let mut assets = HashMap::<Uuid, Vec<ReleaseAssetResponse>>::new();
    for ids in release_ids.chunks(IN_QUERY_CHUNK) {
        // Ordered by name, so each release's assets arrive already sorted.
        for asset in release_asset::Entity::find()
            .filter(release_asset::Column::ReleaseId.is_in(ids.iter().copied()))
            .order_by_asc(release_asset::Column::Name)
            .all(database)
            .await?
        {
            assets
                .entry(asset.release_id)
                .or_default()
                .push(asset_response(repository, asset));
        }
    }

    Ok(releases
        .into_iter()
        .map(|release| ReleaseResponse {
            id: release.id,
            target_revision: release.target_revision,
            target_oid: release.target_oid,
            title: release.title,
            rendered_body: render_markdown(&release.body),
            body: release.body,
            prerelease: release.prerelease,
            latest: latest_id == Some(release.id),
            author: authors
                .get(&release.author_user_id)
                .cloned()
                .unwrap_or_else(|| "deleted-user".to_owned()),
            published_at: release.published_at,
            created_at: release.created_at,
            updated_at: release.updated_at,
            assets: assets.remove(&release.id).unwrap_or_default(),
            external_url: release.external_url,
            external_author: release.external_author.map(|username| {
                ExternalReleaseAuthorResponse {
                    username,
                    profile_url: release.external_author_url,
                }
            }),
        })
        .collect())
}

async fn release_response(
    state: &RepositoryState,
    repository: &repository::Model,
    release: repository_release::Model,
    latest_id: Option<Uuid>,
) -> Result<ReleaseResponse, ApiError> {
    release_responses(state, repository, vec![release], latest_id)
        .await?
        .pop()
        .ok_or_else(|| ApiError::internal("release response was not built"))
}

async fn forgejo_release_response(
    state: &RepositoryState,
    repository: &repository::Model,
    release: repository_release::Model,
) -> Result<ForgejoReleaseResponse, ApiError> {
    let assets = release_asset::Entity::find()
        .filter(release_asset::Column::ReleaseId.eq(release.id))
        .order_by_asc(release_asset::Column::Name)
        .all(state.identity().database())
        .await?
        .into_iter()
        .map(|asset| forgejo_asset_response(repository, asset))
        .collect();
    Ok(ForgejoReleaseResponse {
        id: forgejo_numeric_id(release.id),
        tag_name: release.target_revision,
        target_commitish: release.target_oid,
        name: release.title,
        body: release.body,
        draft: false,
        prerelease: release.prerelease,
        assets,
    })
}

fn forgejo_asset_response(
    repository: &repository::Model,
    asset: release_asset::Model,
) -> ForgejoReleaseAssetResponse {
    ForgejoReleaseAssetResponse {
        id: forgejo_numeric_id(asset.id),
        browser_download_url: format!(
            "/{}/{}/releases/{}/assets/{}/download",
            repository.namespace, repository.name, asset.release_id, asset.id
        ),
        name: asset.name,
        size: asset.size_bytes,
    }
}

fn forgejo_numeric_id(id: Uuid) -> i64 {
    let bytes = id.as_bytes();
    let high = u64::from_be_bytes(bytes[..8].try_into().expect("UUID half"));
    let low = u64::from_be_bytes(bytes[8..].try_into().expect("UUID half"));
    ((high ^ low) & ((1_u64 << 53) - 1)) as i64
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

#[cfg(test)]
mod tests {
    use axum::{
        Json,
        body::{Body, Bytes},
        extract::{Path as AxumPath, Query, State},
        http::{HeaderMap, StatusCode, header},
        response::{IntoResponse, Response},
    };
    use axum_extra::extract::cookie::CookieJar;
    use chrono::{Duration, Utc};
    use futures_util::stream;
    use sea_orm::{ActiveModelTrait, EntityTrait as _, Set};
    use serde_json::Value;
    use uuid::Uuid;

    use super::{
        CreateReleaseRequest, ListReleasesQuery, UpdateReleaseRequest, UploadAssetQuery,
        create_release, delete_release, download_asset, ensure_accepts_writes, get_release,
        list_releases, release_directory, store_release_asset, update_release, upload_asset,
        validate_asset_name,
    };
    use crate::{
        actions::test_support,
        entity::{release_asset, repository, repository_release},
        identity::ApiError,
        repository::{
            http_test_support as http,
            resources::test_support::{TestGit, TestInstance, instance},
        },
    };

    async fn insert_release(
        instance: &TestInstance,
        repository_id: Uuid,
        title: &str,
        age_days: i64,
        prerelease: bool,
    ) -> repository_release::Model {
        let published = Utc::now() - Duration::days(age_days);
        repository_release::ActiveModel {
            id: Set(Uuid::new_v4()),
            repository_id: Set(repository_id),
            author_user_id: Set(instance.owner.id),
            target_revision: Set(title.to_owned()),
            target_oid: Set("0".repeat(40)),
            title: Set(title.to_owned()),
            body: Set("Notes".to_owned()),
            prerelease: Set(prerelease),
            published_at: Set(published),
            created_at: Set(published),
            updated_at: Set(published),
            external_source: Set(None),
            external_instance_url: Set(None),
            external_id: Set(None),
            external_url: Set(None),
            external_author: Set(None),
            external_author_url: Set(None),
            external_updated_at: Set(None),
        }
        .insert(&instance.database)
        .await
        .unwrap()
    }

    /// The `X-Total-Count` header and the JSON array a list request returns.
    async fn list(
        instance: &TestInstance,
        page: Option<u64>,
        per_page: Option<u64>,
    ) -> (String, Vec<Value>) {
        let response = list_releases(
            State(instance.state.clone()),
            AxumPath(("alice".to_owned(), "project".to_owned())),
            Query(ListReleasesQuery { page, per_page }),
            HeaderMap::new(),
            CookieJar::new(),
        )
        .await
        .unwrap()
        .into_response();
        let total = response.headers()["x-total-count"]
            .to_str()
            .unwrap()
            .to_owned();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        (total, serde_json::from_slice(&body).unwrap())
    }

    fn titles(releases: &[Value]) -> Vec<&str> {
        releases
            .iter()
            .map(|release| release["title"].as_str().unwrap())
            .collect()
    }

    #[tokio::test]
    async fn release_list_pages_and_batches_authors_and_assets() {
        let instance = instance().await;
        let repository = instance.repository("project").await;
        insert_release(&instance, repository.id, "v1.0.0", 3, false).await;
        let stable = insert_release(&instance, repository.id, "v1.1.0", 2, false).await;
        insert_release(&instance, repository.id, "v2.0.0-rc1", 1, true).await;
        for name in ["b.zip", "a.zip"] {
            release_asset::ActiveModel {
                id: Set(Uuid::new_v4()),
                release_id: Set(stable.id),
                name: Set(name.to_owned()),
                content_type: Set("application/zip".to_owned()),
                size_bytes: Set(1),
                download_count: Set(0),
                created_at: Set(Utc::now()),
                external_source: Set(None),
                external_instance_url: Set(None),
                external_id: Set(None),
                external_url: Set(None),
                external_updated_at: Set(None),
            }
            .insert(&instance.database)
            .await
            .unwrap();
        }

        // The body stays a bare array, and defaults return everything here.
        let (total, releases) = list(&instance, None, None).await;
        assert_eq!(total, "3");
        assert_eq!(titles(&releases), ["v2.0.0-rc1", "v1.1.0", "v1.0.0"]);
        assert_eq!(releases[0]["latest"], false);
        assert_eq!(releases[1]["latest"], true);
        assert_eq!(releases[1]["author"], "alice");
        let assets = releases[1]["assets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|asset| asset["name"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(assets, ["a.zip", "b.zip"]);
        assert_eq!(releases[2]["assets"], Value::Array(Vec::new()));

        let (total, releases) = list(&instance, Some(1), Some(1)).await;
        assert_eq!(total, "3");
        assert_eq!(titles(&releases), ["v2.0.0-rc1"]);
        // The latest release is marked even when it is not on the first page.
        let (_, releases) = list(&instance, Some(2), Some(1)).await;
        assert_eq!(titles(&releases), ["v1.1.0"]);
        assert_eq!(releases[0]["latest"], true);
        let (_, releases) = list(&instance, Some(3), Some(1)).await;
        assert_eq!(titles(&releases), ["v1.0.0"]);
        assert_eq!(releases[0]["latest"], false);
        let (total, releases) = list(&instance, Some(4), Some(1)).await;
        assert_eq!(total, "3");
        assert!(releases.is_empty());
    }

    fn repository() -> repository::Model {
        let now = Utc::now();
        repository::Model {
            id: Uuid::new_v4(),
            namespace: "alice".to_owned(),
            name: "project".to_owned(),
            description: None,
            website_url: None,
            visibility: "private".to_owned(),
            object_format: "sha1".to_owned(),
            mirrored: false,
            default_branch: Some("main".to_owned()),
            issue_counter: 0,
            storage_key: Uuid::new_v4(),
            created_by: Uuid::new_v4(),
            archived_at: None,
            deleted_at: None,
            icon_updated_at: None,
            icon_source: None,
            created_at: now,
            updated_at: now,
        }
    }

    #[test]
    fn job_tokens_cannot_write_releases_to_read_only_repositories() {
        assert!(ensure_accepts_writes(&repository()).is_ok());

        let archived = repository::Model {
            archived_at: Some(Utc::now()),
            ..repository()
        };
        let mirrored = repository::Model {
            mirrored: true,
            ..repository()
        };
        for read_only in [archived, mirrored] {
            let error = ensure_accepts_writes(&read_only).expect_err("reject the write");
            assert_eq!(error.into_response().status(), StatusCode::FORBIDDEN);
        }
    }

    const OWNER_TOKEN: &str = "alice-token";
    const READER_TOKEN: &str = "bob-token";

    /// A public repository with one commit, its owner's token, and a
    /// read-only collaborator's token.
    struct Project {
        instance: TestInstance,
        repository: repository::Model,
        commit: String,
        _git: TestGit,
    }

    async fn project() -> Project {
        let instance = instance().await;
        let (repository, git) = instance.git_repository("project").await;
        let commit = git.commit_file("README.md", b"hello", "2024-01-01T00:00:00Z");
        http::api_token(&instance.database, &instance.owner, OWNER_TOKEN).await;
        let reader = test_support::owner(&instance.database, "bob").await;
        http::api_token(&instance.database, &reader, READER_TOKEN).await;
        http::collaborator(&instance.database, &repository, &reader, "read").await;
        Project {
            instance,
            repository,
            commit,
            _git: git,
        }
    }

    fn respond<T: IntoResponse>(result: Result<T, ApiError>) -> Response {
        match result {
            Ok(response) => response.into_response(),
            Err(error) => error.into_response(),
        }
    }

    async fn json_of(response: Response) -> (StatusCode, Value) {
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let value = if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&body).unwrap()
        };
        (status, value)
    }

    fn path() -> AxumPath<(String, String)> {
        AxumPath(("alice".to_owned(), "project".to_owned()))
    }

    fn create_request(target: &str, title: &str, prerelease: bool) -> CreateReleaseRequest {
        CreateReleaseRequest {
            target_revision: target.to_owned(),
            title: title.to_owned(),
            body: "Notes".to_owned(),
            prerelease,
        }
    }

    impl Project {
        async fn create(
            &self,
            headers: HeaderMap,
            request: CreateReleaseRequest,
        ) -> (StatusCode, Value) {
            json_of(respond(
                create_release(
                    State(self.instance.state.clone()),
                    path(),
                    headers,
                    CookieJar::new(),
                    Json(request),
                )
                .await,
            ))
            .await
        }

        async fn update(
            &self,
            headers: HeaderMap,
            id: Uuid,
            request: UpdateReleaseRequest,
        ) -> (StatusCode, Value) {
            json_of(respond(
                update_release(
                    State(self.instance.state.clone()),
                    AxumPath(("alice".to_owned(), "project".to_owned(), id)),
                    headers,
                    CookieJar::new(),
                    Json(request),
                )
                .await,
            ))
            .await
        }

        async fn get(&self, id: Uuid) -> (StatusCode, Value) {
            json_of(respond(
                get_release(
                    State(self.instance.state.clone()),
                    AxumPath(("alice".to_owned(), "project".to_owned(), id)),
                    HeaderMap::new(),
                    CookieJar::new(),
                )
                .await,
            ))
            .await
        }

        async fn delete(&self, headers: HeaderMap, id: Uuid) -> StatusCode {
            respond(
                delete_release(
                    State(self.instance.state.clone()),
                    AxumPath(("alice".to_owned(), "project".to_owned(), id)),
                    headers,
                    CookieJar::new(),
                )
                .await,
            )
            .status()
        }

        async fn stored(&self, id: Uuid) -> Option<repository_release::Model> {
            repository_release::Entity::find_by_id(id)
                .one(&self.instance.database)
                .await
                .unwrap()
        }

        /// Stores `chunks` as an asset through the shared streaming path
        /// with a small size cap, so the limit can be hit without 2 GiB.
        async fn store(
            &self,
            release_id: Uuid,
            name: &str,
            chunks: Vec<Result<Bytes, ApiError>>,
            max_bytes: u64,
        ) -> Result<release_asset::Model, ApiError> {
            store_release_asset(
                &self.instance.state,
                &self.repository,
                self.instance.owner.id,
                release_id,
                name,
                "application/octet-stream",
                Box::pin(stream::iter(chunks)),
                max_bytes,
            )
            .await
        }

        fn release_files(&self, release_id: Uuid) -> Vec<String> {
            let directory = release_directory(&self.instance.state, &self.repository, release_id);
            let Ok(entries) = std::fs::read_dir(directory) else {
                return Vec::new();
            };
            let mut names = entries
                .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                .collect::<Vec<_>>();
            names.sort();
            names
        }
    }

    fn release_id(value: &Value) -> Uuid {
        value["id"].as_str().unwrap().parse().unwrap()
    }

    #[tokio::test]
    async fn releases_change_only_with_write_access() {
        let project = project().await;
        let owner = http::bearer(OWNER_TOKEN);
        let reader = http::bearer(READER_TOKEN);

        let (status, _) = project
            .create(HeaderMap::new(), create_request("main", "v1.0.0", false))
            .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        // A read-only collaborator gets the same answer as a stranger: the
        // write is refused without confirming anything about the repository.
        let (status, _) = project
            .create(reader.clone(), create_request("main", "v1.0.0", false))
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(
            repository_release::Entity::find()
                .all(&project.instance.database)
                .await
                .unwrap()
                .is_empty()
        );

        let (status, created) = project
            .create(owner.clone(), create_request("main", "v1.0.0", false))
            .await;
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(created["target_revision"], "main");
        assert_eq!(created["target_oid"], project.commit.as_str());
        assert_eq!(created["author"], "alice");
        assert_eq!(created["latest"], true);
        let id = release_id(&created);

        let (status, _) = project
            .create(owner.clone(), create_request("no-such-ref", "v9", false))
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        let rename = || UpdateReleaseRequest {
            target_revision: None,
            title: Some("Renamed".to_owned()),
            body: None,
            prerelease: None,
        };
        let (status, _) = project.update(reader.clone(), id, rename()).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) = project.update(HeaderMap::new(), id, rename()).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(project.stored(id).await.unwrap().title, "v1.0.0");
        let (status, updated) = project.update(owner.clone(), id, rename()).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(updated["title"], "Renamed");
        let (status, error) = project
            .update(
                owner.clone(),
                id,
                UpdateReleaseRequest {
                    target_revision: Some("no-such-ref".to_owned()),
                    title: None,
                    body: None,
                    prerelease: None,
                },
            )
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(
            error["error"]["message"],
            "The release target must resolve to an existing commit."
        );
        assert_eq!(project.stored(id).await.unwrap().target_revision, "main");

        let asset = project
            .store(id, "build.tar", vec![Ok(Bytes::from_static(b"data"))], 1024)
            .await
            .unwrap();
        assert_eq!(project.release_files(id), [asset.id.to_string()]);
        assert_eq!(project.delete(reader, id).await, StatusCode::NOT_FOUND);
        assert_eq!(
            project.delete(HeaderMap::new(), id).await,
            StatusCode::UNAUTHORIZED
        );
        assert!(project.stored(id).await.is_some());
        assert_eq!(
            project.delete(owner.clone(), id).await,
            StatusCode::NO_CONTENT
        );
        assert!(project.stored(id).await.is_none());
        assert!(project.release_files(id).is_empty());
        let (status, _) = project.get(id).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(project.delete(owner, id).await, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn latest_is_the_newest_release_that_is_not_a_prerelease() {
        let project = project().await;
        let owner = http::bearer(OWNER_TOKEN);

        let (_, candidate) = project
            .create(owner.clone(), create_request("main", "v2.0.0-rc1", true))
            .await;
        let candidate = release_id(&candidate);
        // A repository with only prereleases has no latest release.
        assert_eq!(project.get(candidate).await.1["latest"], false);

        let (_, stable) = project
            .create(owner.clone(), create_request("main", "v1.0.0", false))
            .await;
        let stable = release_id(&stable);
        assert_eq!(project.get(stable).await.1["latest"], true);

        let (_, newer) = project
            .create(owner.clone(), create_request("main", "v2.0.0-rc2", true))
            .await;
        let newer = release_id(&newer);
        assert_ne!(newer, stable);
        assert_eq!(project.get(newer).await.1["latest"], false);
        assert_eq!(project.get(stable).await.1["latest"], true);

        // Promoting the newest candidate makes it the latest release, and
        // demoting it again hands the title back.
        let promote = |prerelease| UpdateReleaseRequest {
            target_revision: None,
            title: None,
            body: None,
            prerelease: Some(prerelease),
        };
        let (status, promoted) = project.update(owner.clone(), newer, promote(false)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(promoted["latest"], true);
        assert_eq!(project.get(stable).await.1["latest"], false);
        project.update(owner, newer, promote(true)).await;
        assert_eq!(project.get(newer).await.1["latest"], false);
        assert_eq!(project.get(stable).await.1["latest"], true);
    }

    #[tokio::test]
    async fn asset_uploads_are_capped_streamed_and_uniquely_named() {
        let project = project().await;
        let owner = http::bearer(OWNER_TOKEN);
        let (_, created) = project
            .create(owner.clone(), create_request("main", "v1.0.0", false))
            .await;
        let release = release_id(&created);
        let chunk = || Ok(Bytes::from_static(b"abcd"));

        // One byte over the cap is refused as soon as the stream crosses it.
        let error = project
            .store(release, "big.bin", vec![chunk(), chunk(), chunk()], 11)
            .await
            .unwrap_err();
        let (status, body) = json_of(error.into_response()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(
            body["error"]["message"],
            "Release assets cannot exceed 2 GiB."
        );
        assert!(project.release_files(release).is_empty());
        assert!(
            release_asset::Entity::find()
                .all(&project.instance.database)
                .await
                .unwrap()
                .is_empty()
        );

        // Exactly the cap is fine.
        let asset = project
            .store(release, " build.tar ", vec![chunk(), chunk()], 8)
            .await
            .unwrap();
        assert_eq!(asset.name, "build.tar");
        assert_eq!(asset.size_bytes, 8);
        assert_eq!(project.release_files(release), [asset.id.to_string()]);

        let duplicate = project
            .store(release, "build.tar", vec![chunk()], 1024)
            .await
            .unwrap_err();
        let (status, body) = json_of(duplicate.into_response()).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(
            body["error"]["message"],
            "An asset with that file name already exists."
        );
        assert_eq!(project.release_files(release).len(), 1);

        for invalid in ["", ".", "..", "dir/file", "dir\\file", "tab\tname"] {
            assert!(validate_asset_name(invalid).is_err(), "{invalid:?}");
        }

        // The upload handler enforces write access before touching the body.
        let upload = |headers: HeaderMap| {
            let state = project.instance.state.clone();
            async move {
                respond(
                    upload_asset(
                        State(state),
                        AxumPath(("alice".to_owned(), "project".to_owned(), release)),
                        Query(UploadAssetQuery {
                            name: "other.bin".to_owned(),
                        }),
                        headers,
                        CookieJar::new(),
                        Body::from("payload"),
                    )
                    .await,
                )
                .status()
            }
        };
        assert_eq!(upload(HeaderMap::new()).await, StatusCode::UNAUTHORIZED);
        assert_eq!(
            upload(http::bearer(READER_TOKEN)).await,
            StatusCode::NOT_FOUND
        );
        assert_eq!(project.release_files(release).len(), 1);
        assert_eq!(upload(owner).await, StatusCode::CREATED);
        assert_eq!(project.release_files(release).len(), 2);

        // Anyone who can read the repository downloads, and it counts.
        let response = respond(
            download_asset(
                State(project.instance.state.clone()),
                AxumPath(("alice".to_owned(), "project".to_owned(), release, asset.id)),
                HeaderMap::new(),
                CookieJar::new(),
            )
            .await,
        );
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CONTENT_LENGTH], "8");
        assert_eq!(
            response.headers()[header::CONTENT_DISPOSITION],
            "attachment; filename=\"build.tar\""
        );
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(body.as_ref(), b"abcdabcd");
        let counted = release_asset::Entity::find_by_id(asset.id)
            .one(&project.instance.database)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(counted.download_count, 1);
    }

    #[tokio::test]
    async fn an_upload_that_fails_midway_leaves_no_temporary_file() {
        let project = project().await;
        let (_, created) = project
            .create(
                http::bearer(OWNER_TOKEN),
                create_request("main", "v1.0.0", false),
            )
            .await;
        let release = release_id(&created);

        let error = project
            .store(
                release,
                "broken.bin",
                vec![
                    Ok(Bytes::from_static(b"abcd")),
                    Err(ApiError::bad_request("connection reset")),
                ],
                1024,
            )
            .await
            .unwrap_err();

        assert_eq!(error.into_response().status(), StatusCode::BAD_REQUEST);
        assert!(
            release_asset::Entity::find()
                .all(&project.instance.database)
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(project.release_files(release), Vec::<String>::new());
    }
}
