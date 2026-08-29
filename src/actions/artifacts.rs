use std::{collections::HashSet, io::ErrorKind, path::PathBuf};

use super::{
    ActionsState,
    proto::artifact,
    tokens::{self, AuthenticatedJob},
};
use crate::{
    entity::{action_artifact, action_artifact_grant, action_job},
    identity::ApiError,
};
use axum::{
    Router,
    body::Body,
    extract::{Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{post, put},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{Duration, Utc};
use futures_util::TryStreamExt as _;
use quick_xml::{Reader, events::Event};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, Condition, EntityTrait, QueryFilter, QueryOrder, Set,
    sea_query::{Expr, ExprTrait as _},
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::{
    fs,
    io::{AsyncReadExt as _, AsyncWriteExt as _},
};
use uuid::Uuid;

const MAX_BLOCK_ID: usize = 256;

#[derive(Deserialize)]
struct SignedQuery {
    token: String,
    #[serde(alias = "blockid")]
    block_id: Option<String>,
    comp: Option<String>,
}

pub(crate) fn router() -> Router<ActionsState> {
    Router::new()
        .route(
            "/twirp/github.actions.results.api.v1.ArtifactService/CreateArtifact",
            post(create),
        )
        .route(
            "/twirp/github.actions.results.api.v1.ArtifactService/FinalizeArtifact",
            post(finalize),
        )
        .route(
            "/twirp/github.actions.results.api.v1.ArtifactService/ListArtifacts",
            post(list),
        )
        .route(
            "/twirp/github.actions.results.api.v1.ArtifactService/GetSignedArtifactURL",
            post(signed_download),
        )
        .route(
            "/twirp/github.actions.results.api.v1.ArtifactService/DeleteArtifact",
            post(delete_artifact),
        )
        .route(
            "/twirp/github.actions.results.api.v1.ArtifactService/UploadArtifact",
            put(upload).post(upload),
        )
        .route(
            "/twirp/github.actions.results.api.v1.ArtifactService/DownloadArtifact",
            put(download).get(download),
        )
}

fn database(state: &ActionsState) -> &sea_orm::DatabaseConnection {
    state.repository().identity().database()
}
fn artifact_path(state: &ActionsState, row: &action_artifact::Model) -> PathBuf {
    state
        .repository()
        .actions_artifact_root()
        .join(&row.storage_key)
}
fn blocks_dir(state: &ActionsState, row: &action_artifact::Model) -> PathBuf {
    state
        .repository()
        .actions_artifact_root()
        .join(format!(".blocks-{}", row.storage_key))
}
fn finalizing_path(state: &ActionsState, row: &action_artifact::Model) -> PathBuf {
    state
        .repository()
        .actions_artifact_root()
        .join(format!(".{}.finalizing", row.storage_key))
}

async fn job(state: &ActionsState, headers: &HeaderMap) -> Result<AuthenticatedJob, ApiError> {
    tokens::authenticate_job_bearer(database(state), headers)
        .await?
        .ok_or_else(ApiError::unauthorized)
}
fn parse_run(value: &str) -> Result<Uuid, ApiError> {
    Uuid::parse_str(value).map_err(|_| ApiError::bad_request("Invalid workflow run backend id."))
}
fn parse_job(value: &str) -> Result<i64, ApiError> {
    value
        .parse()
        .map_err(|_| ApiError::bad_request("Invalid workflow job run backend id."))
}
fn check_run(run: Uuid, auth: &AuthenticatedJob) -> Result<(), ApiError> {
    if auth.run.id != run {
        return Err(ApiError::forbidden(
            "The Actions token does not belong to this workflow run.",
        ));
    }
    Ok(())
}
fn check_ids(run: Uuid, job_id: i64, auth: &AuthenticatedJob) -> Result<(), ApiError> {
    check_run(run, auth)?;
    if auth.job.id != job_id {
        return Err(ApiError::forbidden(
            "The Actions token does not belong to this workflow job.",
        ));
    }
    Ok(())
}
fn valid_name(name: &str, max_bytes: usize) -> Result<(), ApiError> {
    if name.is_empty()
        || name.len() > max_bytes
        || name == "."
        || name == ".."
        || name.contains(['/', '\\'])
        || name.chars().any(char::is_control)
    {
        return Err(ApiError::bad_request("Artifact name is invalid."));
    }
    Ok(())
}
fn public_url(state: &ActionsState, path: &str) -> String {
    format!(
        "{}/{}",
        state
            .repository()
            .public_url()
            .as_str()
            .trim_end_matches('/'),
        path.trim_start_matches('/')
    )
}

async fn create(
    State(state): State<ActionsState>,
    headers: HeaderMap,
    axum::Json(request): axum::Json<artifact::CreateArtifactRequest>,
) -> Result<axum::Json<artifact::CreateArtifactResponse>, ApiError> {
    let auth = job(&state, &headers).await?;
    let run = parse_run(&request.workflow_run_backend_id)?;
    let job_id = parse_job(&request.workflow_job_run_backend_id)?;
    check_ids(run, job_id, &auth)?;
    valid_name(&request.name, state.settings().max_artifact_name_bytes)?;
    let now = Utc::now();
    let maximum_expiry = now + Duration::days(state.settings().retention_days);
    let expires_at = match request.expires_at {
        Some(timestamp) => {
            let nanos = u32::try_from(timestamp.nanos)
                .ok()
                .filter(|nanos| *nanos < 1_000_000_000)
                .ok_or_else(|| ApiError::bad_request("Artifact expiry is invalid."))?;
            let requested = chrono::DateTime::<Utc>::from_timestamp(timestamp.seconds, nanos)
                .ok_or_else(|| ApiError::bad_request("Artifact expiry is invalid."))?;
            if requested <= now {
                return Err(ApiError::bad_request(
                    "Artifact expiry must be in the future.",
                ));
            }
            std::cmp::Ord::min(requested, maximum_expiry)
        }
        None => maximum_expiry,
    };
    let row = action_artifact::ActiveModel {
        id: Default::default(),
        repository_id: Set(auth.repository.id),
        run_id: Set(run),
        creating_job_id: Set(job_id),
        name: Set(request.name.clone()),
        storage_key: Set(Uuid::new_v4().to_string()),
        status: Set("pending".to_owned()),
        size_bytes: Set(0),
        sha256: Set(None),
        expires_at: Set(expires_at),
        finalized_at: Set(None),
        deleted_at: Set(None),
        deleted_by_job_id: Set(None),
        metadata_json: Set("{}".to_owned()),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(database(&state))
    .await
    .map_err(|error| {
        if matches!(error, sea_orm::DbErr::Exec(_)) {
            ApiError::conflict("An artifact with this name already exists.")
        } else {
            error.into()
        }
    })?;
    let grant_token = format!(
        "gta_art_{}",
        URL_SAFE_NO_PAD.encode(rand::random::<[u8; 32]>())
    );
    action_artifact_grant::ActiveModel {
        id: Set(Uuid::new_v4()),
        artifact_id: Set(row.id),
        issued_to_job_id: Set(job_id),
        scope: Set("upload".to_owned()),
        token_hash: Set(tokens::digest(&grant_token)),
        expires_at: Set(now + Duration::seconds(state.settings().artifact_grant_lifetime_seconds)),
        revoked_at: Set(None),
        last_used_at: Set(None),
        use_count: Set(0),
        created_at: Set(now),
    }
    .insert(database(&state))
    .await?;
    Ok(axum::Json(artifact::CreateArtifactResponse {
        ok: true,
        signed_upload_url: public_url(
            &state,
            &format!(
                "twirp/github.actions.results.api.v1.ArtifactService/UploadArtifact?token={grant_token}"
            ),
        ),
    }))
}

async fn finalize(
    State(state): State<ActionsState>,
    headers: HeaderMap,
    axum::Json(request): axum::Json<artifact::FinalizeArtifactRequest>,
) -> Result<axum::Json<artifact::FinalizeArtifactResponse>, ApiError> {
    let auth = job(&state, &headers).await?;
    let run = parse_run(&request.workflow_run_backend_id)?;
    let job_id = parse_job(&request.workflow_job_run_backend_id)?;
    check_ids(run, job_id, &auth)?;
    valid_name(&request.name, state.settings().max_artifact_name_bytes)?;
    let row = action_artifact::Entity::find()
        .filter(action_artifact::Column::RunId.eq(run))
        .filter(action_artifact::Column::CreatingJobId.eq(job_id))
        .filter(action_artifact::Column::Name.eq(&request.name))
        .filter(action_artifact::Column::DeletedAt.is_null())
        .one(database(&state))
        .await?
        .ok_or_else(ApiError::not_found)?;
    if row.status == "finalized" {
        return Ok(axum::Json(artifact::FinalizeArtifactResponse {
            ok: true,
            artifact_id: row.id,
        }));
    }
    let expected_size = u64::try_from(request.size)
        .map_err(|_| ApiError::bad_request("Artifact size is invalid."))?;
    let max_artifact = u64::try_from(state.settings().max_artifact_bytes)
        .map_err(|_| ApiError::internal("invalid artifact size setting"))?;
    if expected_size > max_artifact {
        return Err(ApiError::bad_request(
            "Artifacts exceed the configured size limit.",
        ));
    }
    let expected_hash = request
        .hash
        .as_ref()
        .and_then(|value| value.value.strip_prefix("sha256:"))
        .map(str::to_ascii_lowercase)
        .filter(|value| value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .ok_or_else(|| ApiError::bad_request("Artifact SHA-256 is required."))?;
    let source = blocks_dir(&state, &row);
    let destination = artifact_path(&state, &row);
    fs::create_dir_all(state.repository().actions_artifact_root())
        .await
        .map_err(ApiError::internal)?;
    let temporary = finalizing_path(&state, &row);
    let mut output = fs::File::create(&temporary)
        .await
        .map_err(ApiError::internal)?;
    let mut hasher = Sha256::new();
    let mut total = 0u64;
    let metadata: serde_json::Value =
        serde_json::from_str(&row.metadata_json).unwrap_or_else(|_| serde_json::json!({}));
    let ordered_ids: Vec<String> = metadata
        .get("order")
        .and_then(|value| value.as_array())
        .map(|values| {
            values
                .iter()
                .filter_map(|value| value.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    let block_paths = if ordered_ids.is_empty() {
        let mut entries = fs::read_dir(&source).await.map_err(ApiError::internal)?;
        let mut paths = Vec::new();
        while let Some(entry) = entries.next_entry().await.map_err(ApiError::internal)? {
            if !entry.file_name().to_string_lossy().starts_with('.') {
                paths.push(entry.path());
            }
            if paths.len() > state.settings().max_artifact_blocks {
                let _ = fs::remove_file(&temporary).await;
                return Err(ApiError::bad_request(
                    "Artifact has too many uploaded blocks.",
                ));
            }
        }
        paths.sort();
        paths
    } else {
        ordered_ids
            .iter()
            .map(|id| source.join(format!("{:x}", Sha256::digest(id.as_bytes()))))
            .collect()
    };
    for path in block_paths {
        let mut input = fs::File::open(path).await.map_err(|_| {
            ApiError::bad_request("Artifact block list references a missing block.")
        })?;
        loop {
            let mut buf = [0u8; 64 * 1024];
            let n = input.read(&mut buf).await.map_err(ApiError::internal)?;
            if n == 0 {
                break;
            }
            total = total
                .checked_add(n as u64)
                .ok_or_else(|| ApiError::bad_request("Artifact size overflow."))?;
            if total > max_artifact {
                let _ = fs::remove_file(&temporary).await;
                return Err(ApiError::bad_request(
                    "Artifacts exceed the configured size limit.",
                ));
            }
            hasher.update(&buf[..n]);
            output
                .write_all(&buf[..n])
                .await
                .map_err(ApiError::internal)?;
        }
    }
    output.flush().await.map_err(ApiError::internal)?;
    drop(output);
    let actual = format!("{:x}", hasher.finalize());
    if total != expected_size || actual != expected_hash {
        let _ = fs::remove_file(&temporary).await;
        return Err(ApiError::bad_request(
            "Artifact size or SHA-256 does not match.",
        ));
    }
    fs::rename(&temporary, &destination)
        .await
        .map_err(ApiError::internal)?;
    let now = Utc::now();
    let mut active: action_artifact::ActiveModel = row.clone().into();
    active.status = Set("finalized".to_owned());
    active.size_bytes =
        Set(i64::try_from(total).map_err(|_| ApiError::bad_request("Artifact size is invalid."))?);
    active.sha256 = Set(Some(actual));
    active.finalized_at = Set(Some(now));
    active.updated_at = Set(now);
    active.update(database(&state)).await?;
    let _ = fs::remove_dir_all(source).await;
    Ok(axum::Json(artifact::FinalizeArtifactResponse {
        ok: true,
        artifact_id: row.id,
    }))
}

async fn list(
    State(state): State<ActionsState>,
    headers: HeaderMap,
    axum::Json(request): axum::Json<artifact::ListArtifactsRequest>,
) -> Result<axum::Json<artifact::ListArtifactsResponse>, ApiError> {
    let auth = job(&state, &headers).await?;
    let run = parse_run(&request.workflow_run_backend_id)?;
    parse_job(&request.workflow_job_run_backend_id)?;
    check_run(run, &auth)?;
    let mut select = action_artifact::Entity::find()
        .filter(action_artifact::Column::RunId.eq(run))
        .filter(action_artifact::Column::RepositoryId.eq(auth.repository.id))
        .filter(action_artifact::Column::Status.eq("finalized"))
        .filter(action_artifact::Column::DeletedAt.is_null())
        .filter(action_artifact::Column::ExpiresAt.gt(Utc::now()))
        .order_by_asc(action_artifact::Column::Id);
    if let Some(filter) = request.name_filter {
        select = select.filter(action_artifact::Column::Name.eq(filter.value));
    }
    if let Some(filter) = request.id_filter {
        select = select.filter(action_artifact::Column::Id.eq(filter.value));
    }
    let rows = select.all(database(&state)).await?;
    Ok(axum::Json(artifact::ListArtifactsResponse {
        artifacts: rows
            .into_iter()
            .map(|row| artifact::ListArtifactsResponseMonolithArtifact {
                workflow_run_backend_id: row.run_id.to_string(),
                workflow_job_run_backend_id: row.creating_job_id.to_string(),
                database_id: row.id,
                name: row.name,
                size: row.size_bytes,
                created_at: Some(pbjson_types::Timestamp {
                    seconds: row.created_at.timestamp(),
                    nanos: 0,
                }),
            })
            .collect(),
    }))
}

async fn signed_download(
    State(state): State<ActionsState>,
    headers: HeaderMap,
    axum::Json(request): axum::Json<artifact::GetSignedArtifactUrlRequest>,
) -> Result<axum::Json<artifact::GetSignedArtifactUrlResponse>, ApiError> {
    let auth = job(&state, &headers).await?;
    let run = parse_run(&request.workflow_run_backend_id)?;
    parse_job(&request.workflow_job_run_backend_id)?;
    check_run(run, &auth)?;
    let row = action_artifact::Entity::find()
        .filter(action_artifact::Column::RunId.eq(run))
        .filter(action_artifact::Column::RepositoryId.eq(auth.repository.id))
        .filter(action_artifact::Column::Name.eq(&request.name))
        .filter(action_artifact::Column::Status.eq("finalized"))
        .filter(action_artifact::Column::DeletedAt.is_null())
        .filter(action_artifact::Column::ExpiresAt.gt(Utc::now()))
        .one(database(&state))
        .await?
        .ok_or_else(ApiError::not_found)?;
    let grant_token = format!(
        "gta_art_{}",
        URL_SAFE_NO_PAD.encode(rand::random::<[u8; 32]>())
    );
    let now = Utc::now();
    action_artifact_grant::ActiveModel {
        id: Set(Uuid::new_v4()),
        artifact_id: Set(row.id),
        issued_to_job_id: Set(auth.job.id),
        scope: Set("download".to_owned()),
        token_hash: Set(tokens::digest(&grant_token)),
        expires_at: Set(now + Duration::seconds(state.settings().artifact_grant_lifetime_seconds)),
        revoked_at: Set(None),
        last_used_at: Set(None),
        use_count: Set(0),
        created_at: Set(now),
    }
    .insert(database(&state))
    .await?;
    Ok(axum::Json(artifact::GetSignedArtifactUrlResponse {
        signed_url: public_url(
            &state,
            &format!(
                "twirp/github.actions.results.api.v1.ArtifactService/DownloadArtifact?token={grant_token}"
            ),
        ),
    }))
}

async fn delete_artifact(
    State(state): State<ActionsState>,
    headers: HeaderMap,
    axum::Json(request): axum::Json<artifact::DeleteArtifactRequest>,
) -> Result<axum::Json<artifact::DeleteArtifactResponse>, ApiError> {
    let auth = job(&state, &headers).await?;
    let run = parse_run(&request.workflow_run_backend_id)?;
    parse_job(&request.workflow_job_run_backend_id)?;
    check_run(run, &auth)?;
    let row = action_artifact::Entity::find()
        .filter(action_artifact::Column::RunId.eq(run))
        .filter(action_artifact::Column::RepositoryId.eq(auth.repository.id))
        .filter(action_artifact::Column::Name.eq(&request.name))
        .filter(action_artifact::Column::DeletedAt.is_null())
        .one(database(&state))
        .await?
        .ok_or_else(ApiError::not_found)?;
    let now = Utc::now();
    let mut active: action_artifact::ActiveModel = row.clone().into();
    active.deleted_at = Set(Some(now));
    active.deleted_by_job_id = Set(Some(auth.job.id));
    active.updated_at = Set(now);
    active.update(database(&state)).await?;
    let _ = fs::remove_file(artifact_path(&state, &row)).await;
    Ok(axum::Json(artifact::DeleteArtifactResponse {
        ok: true,
        artifact_id: row.id,
    }))
}

async fn grant(
    state: &ActionsState,
    query: &SignedQuery,
    scope: &str,
) -> Result<(action_artifact::Model, action_artifact_grant::Model), ApiError> {
    if query.token.split('.').count() == 3 {
        return Err(ApiError::unauthorized());
    }
    let now = Utc::now();
    let grant = action_artifact_grant::Entity::find()
        .filter(action_artifact_grant::Column::TokenHash.eq(tokens::digest(&query.token)))
        .filter(action_artifact_grant::Column::Scope.eq(scope))
        .filter(action_artifact_grant::Column::RevokedAt.is_null())
        .filter(action_artifact_grant::Column::ExpiresAt.gt(now))
        .one(database(state))
        .await?
        .ok_or_else(ApiError::unauthorized)?;
    let issued_job = action_job::Entity::find_by_id(grant.issued_to_job_id)
        .filter(action_job::Column::Status.eq("running"))
        .filter(action_job::Column::LeaseDeadline.gt(now))
        .one(database(state))
        .await?
        .ok_or_else(ApiError::unauthorized)?;
    let row = action_artifact::Entity::find_by_id(grant.artifact_id)
        .one(database(state))
        .await?
        .ok_or_else(ApiError::not_found)?;
    if issued_job.run_id != row.run_id {
        return Err(ApiError::unauthorized());
    }
    Ok((row, grant))
}

async fn touch_grant(
    state: &ActionsState,
    grant: action_artifact_grant::Model,
) -> Result<(), ApiError> {
    let mut active: action_artifact_grant::ActiveModel = grant.into();
    active.last_used_at = Set(Some(Utc::now()));
    active.use_count = Set(active.use_count.as_ref() + 1);
    active.update(database(state)).await?;
    Ok(())
}

async fn write_upload_file(
    body: Body,
    path: &std::path::Path,
    append: bool,
    limit: usize,
) -> Result<u64, ApiError> {
    let mut output = if append {
        fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .await
            .map_err(ApiError::internal)?
    } else {
        fs::File::create(path).await.map_err(ApiError::internal)?
    };
    let mut stream = body.into_data_stream();
    let mut total = 0u64;
    while let Some(chunk) = stream.try_next().await.map_err(ApiError::internal)? {
        total = total
            .checked_add(chunk.len() as u64)
            .ok_or_else(|| ApiError::bad_request("Upload size overflow."))?;
        if total > limit as u64 {
            return Err(ApiError::bad_request("Artifact upload block is too large."));
        }
        output.write_all(&chunk).await.map_err(ApiError::internal)?;
    }
    output.flush().await.map_err(ApiError::internal)?;
    Ok(total)
}

async fn upload(
    State(state): State<ActionsState>,
    Query(query): Query<SignedQuery>,
    body: Body,
) -> Result<StatusCode, ApiError> {
    let (row, grant) = grant(&state, &query, "upload").await?;
    if row.status != "pending" || row.deleted_at.is_some() {
        return Err(ApiError::conflict(
            "Artifact is no longer accepting uploads.",
        ));
    }
    if query.comp.as_deref() == Some("blocklist") {
        let xml = read_limited(body, state.settings().max_artifact_block_list_bytes).await?;
        let order = parse_block_list(&xml, state.settings().max_artifact_blocks)?;
        let mut metadata: serde_json::Value =
            serde_json::from_str(&row.metadata_json).unwrap_or_else(|_| serde_json::json!({}));
        metadata["order"] = serde_json::json!(order);
        let mut active: action_artifact::ActiveModel = row.into();
        active.metadata_json = Set(serde_json::to_string(&metadata).map_err(ApiError::internal)?);
        active.updated_at = Set(Utc::now());
        active.update(database(&state)).await?;
        touch_grant(&state, grant).await?;
        return Ok(StatusCode::CREATED);
    }

    let block_id = query.block_id.unwrap_or_else(|| "full".to_owned());
    validate_block_id(&block_id)?;
    let directory = blocks_dir(&state, &row);
    fs::create_dir_all(&directory)
        .await
        .map_err(ApiError::internal)?;
    let path = directory.join(format!("{:x}", Sha256::digest(block_id.as_bytes())));
    let temporary = directory.join(format!(".upload-{}", Uuid::new_v4()));
    let old_metadata = match fs::metadata(&path).await {
        Ok(metadata) => Some(metadata),
        Err(error) if error.kind() == ErrorKind::NotFound => None,
        Err(error) => return Err(ApiError::internal(error)),
    };
    let old_size = old_metadata.as_ref().map_or(0, std::fs::Metadata::len);
    if old_metadata.is_none() {
        let mut entries = fs::read_dir(&directory).await.map_err(ApiError::internal)?;
        let mut block_count = 0usize;
        while let Some(entry) = entries.next_entry().await.map_err(ApiError::internal)? {
            if !entry.file_name().to_string_lossy().starts_with('.') {
                block_count += 1;
            }
            if block_count >= state.settings().max_artifact_blocks {
                return Err(ApiError::bad_request(
                    "Artifact has too many uploaded blocks.",
                ));
            }
        }
    }
    let append = query.comp.as_deref() == Some("appendBlock");
    if append && old_size > 0 {
        fs::copy(&path, &temporary)
            .await
            .map_err(ApiError::internal)?;
    }
    let uploaded = write_upload_file(
        body,
        &temporary,
        append && old_size > 0,
        state.settings().max_artifact_upload_request_bytes,
    )
    .await;
    if let Err(error) = uploaded {
        let _ = fs::remove_file(&temporary).await;
        return Err(error);
    }
    let new_size = fs::metadata(&temporary)
        .await
        .map_err(ApiError::internal)?
        .len();
    let additional_bytes = new_size.saturating_sub(old_size);
    let additional_bytes = i64::try_from(additional_bytes)
        .map_err(|_| ApiError::bad_request("Artifact size is invalid."))?;
    let maximum_bytes = i64::try_from(state.settings().max_artifact_bytes)
        .map_err(|_| ApiError::internal("invalid artifact size setting"))?;
    let reservation = action_artifact::Entity::update_many()
        .col_expr(
            action_artifact::Column::SizeBytes,
            Expr::col(action_artifact::Column::SizeBytes).add(additional_bytes),
        )
        .filter(action_artifact::Column::Id.eq(row.id))
        .filter(action_artifact::Column::Status.eq("pending"))
        .filter(
            Expr::col(action_artifact::Column::SizeBytes)
                .add(additional_bytes)
                .lte(maximum_bytes),
        )
        .exec(database(&state))
        .await?;
    if reservation.rows_affected != 1 {
        let _ = fs::remove_file(&temporary).await;
        return Err(ApiError::bad_request(
            "Artifacts exceed the configured size limit.",
        ));
    }
    if let Err(error) = fs::rename(&temporary, &path).await {
        let _ = action_artifact::Entity::update_many()
            .col_expr(
                action_artifact::Column::SizeBytes,
                Expr::col(action_artifact::Column::SizeBytes).sub(additional_bytes),
            )
            .filter(action_artifact::Column::Id.eq(row.id))
            .exec(database(&state))
            .await;
        let _ = fs::remove_file(&temporary).await;
        return Err(ApiError::internal(error));
    }
    touch_grant(&state, grant).await?;
    Ok(StatusCode::CREATED)
}

fn validate_block_id(value: &str) -> Result<(), ApiError> {
    if value.is_empty()
        || value.len() > MAX_BLOCK_ID
        || value.contains(['/', '\\'])
        || value.chars().any(char::is_control)
    {
        return Err(ApiError::bad_request("Block identifier is invalid."));
    }
    Ok(())
}

async fn read_limited(body: Body, limit: usize) -> Result<Vec<u8>, ApiError> {
    let mut stream = body.into_data_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.try_next().await.map_err(ApiError::internal)? {
        if bytes
            .len()
            .checked_add(chunk.len())
            .is_none_or(|size| size > limit)
        {
            return Err(ApiError::bad_request("Artifact block list is too large."));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn parse_block_list(bytes: &[u8], max_blocks: usize) -> Result<Vec<String>, ApiError> {
    let mut reader = Reader::from_reader(bytes);
    reader.config_mut().trim_text(true);
    let mut buffer = Vec::new();
    let mut order = Vec::new();
    let mut seen = HashSet::new();
    loop {
        match reader
            .read_event_into(&mut buffer)
            .map_err(|_| ApiError::bad_request("Malformed artifact block list."))?
        {
            Event::Start(event) if event.name().as_ref() == b"Latest" => {
                let value = match reader
                    .read_event_into(&mut buffer)
                    .map_err(|_| ApiError::bad_request("Malformed artifact block list."))?
                {
                    Event::Text(text) => text
                        .decode()
                        .map_err(|_| ApiError::bad_request("Malformed artifact block list."))?
                        .into_owned(),
                    _ => return Err(ApiError::bad_request("Malformed artifact block list.")),
                };
                validate_block_id(&value)?;
                if !seen.insert(value.clone()) {
                    return Err(ApiError::bad_request(
                        "Artifact block list contains a duplicate block.",
                    ));
                }
                order.push(value);
                if order.len() > max_blocks {
                    return Err(ApiError::bad_request(
                        "Artifact block list has too many blocks.",
                    ));
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    if order.is_empty() {
        return Err(ApiError::bad_request("Artifact block list is empty."));
    }
    Ok(order)
}

async fn download(
    State(state): State<ActionsState>,
    Query(query): Query<SignedQuery>,
) -> Result<Response, ApiError> {
    let (row, grant) = grant(&state, &query, "download").await?;
    if row.status != "finalized" || row.expires_at <= Utc::now() || row.deleted_at.is_some() {
        return Err(ApiError::not_found());
    }
    let file = fs::File::open(artifact_path(&state, &row))
        .await
        .map_err(|_| ApiError::not_found())?;
    touch_grant(&state, grant).await?;
    let mut response = Body::from_stream(tokio_util::io::ReaderStream::new(file)).into_response();
    set_download_headers(&mut response, &row);
    Ok(response)
}
fn safe_download_name(name: &str) -> String {
    name.chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect()
}

pub(crate) fn set_download_headers(response: &mut Response, row: &action_artifact::Model) {
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("application/zip"),
    );
    if let Ok(value) = header::HeaderValue::from_str(&row.size_bytes.to_string()) {
        response.headers_mut().insert(header::CONTENT_LENGTH, value);
    }
    let safe_name = safe_download_name(&row.name);
    let content_disposition = format!("attachment; filename=\"{safe_name}.zip\"");
    if let Ok(value) = header::HeaderValue::from_str(&content_disposition) {
        response
            .headers_mut()
            .insert(header::CONTENT_DISPOSITION, value);
    }
}

pub(crate) async fn cleanup(state: &ActionsState) -> Result<(), sea_orm::DbErr> {
    let now = Utc::now();
    let stale_pending = now
        - Duration::seconds(std::cmp::Ord::max(
            state.settings().artifact_grant_lifetime_seconds,
            60,
        ));
    let rows = action_artifact::Entity::find()
        .filter(
            Condition::any()
                .add(action_artifact::Column::DeletedAt.is_not_null())
                .add(action_artifact::Column::ExpiresAt.lte(now))
                .add(
                    Condition::all()
                        .add(action_artifact::Column::Status.eq("pending"))
                        .add(action_artifact::Column::UpdatedAt.lte(stale_pending)),
                ),
        )
        .all(database(state))
        .await?;
    for row in rows {
        let mut disk_cleanup_failed = false;
        for path in [artifact_path(state, &row), finalizing_path(state, &row)] {
            if let Err(error) = fs::remove_file(path).await
                && error.kind() != ErrorKind::NotFound
            {
                tracing::warn!(artifact_id = row.id, %error, "could not remove Actions artifact");
                disk_cleanup_failed = true;
            }
        }
        if let Err(error) = fs::remove_dir_all(blocks_dir(state, &row)).await
            && error.kind() != ErrorKind::NotFound
        {
            tracing::warn!(artifact_id = row.id, %error, "could not remove Actions artifact blocks");
            disk_cleanup_failed = true;
        }
        if disk_cleanup_failed {
            continue;
        }
        action_artifact::Entity::delete_by_id(row.id)
            .exec(database(state))
            .await?;
    }
    action_artifact_grant::Entity::delete_many()
        .filter(action_artifact_grant::Column::ExpiresAt.lte(now))
        .exec(database(state))
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_list_preserves_order_and_rejects_ambiguity() {
        let xml =
            br#"<?xml version="1.0"?><BlockList><Latest>second</Latest><Latest>first</Latest></BlockList>"#;
        assert_eq!(
            parse_block_list(xml, 2).unwrap(),
            ["second".to_owned(), "first".to_owned()]
        );
        assert!(parse_block_list(xml, 1).is_err());
        assert!(
            parse_block_list(
                br#"<BlockList><Latest>same</Latest><Latest>same</Latest></BlockList>"#,
                2,
            )
            .is_err()
        );
    }

    #[test]
    fn artifact_download_name_cannot_inject_headers_or_paths() {
        assert_eq!(safe_download_name("../report\r\n.zip"), ".._report__.zip");
        assert_eq!(safe_download_name("build-42_x64"), "build-42_x64");
    }
}
