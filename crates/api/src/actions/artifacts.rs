use super::{ActionsState, proto::artifact};
use crate::{entity::action_artifact, identity::ApiError};
use axum::{
    Router,
    body::Body,
    extract::{Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{post, put},
};
use chrono::Utc;
use futures_util::TryStreamExt as _;
use serde::Deserialize;
use uuid::Uuid;

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
fn job(headers: &HeaderMap) -> Result<&str, ApiError> {
    super::tokens::bearer_token(headers).ok_or_else(ApiError::unauthorized)
}
fn parse_run(value: &str) -> Result<Uuid, ApiError> {
    Uuid::parse_str(value).map_err(|_| ApiError::bad_request("Invalid workflow run backend id."))
}
fn parse_job(value: &str) -> Result<i64, ApiError> {
    value
        .parse()
        .map_err(|_| ApiError::bad_request("Invalid workflow job run backend id."))
}
pub(crate) use gitadel_actions::artifact_service::run_scope;
fn service(state: &ActionsState) -> gitadel_actions::artifact_service::ArtifactService<'_> {
    gitadel_actions::artifact_service::ArtifactService::new(
        database(state),
        state.repository().actions_artifact_root(),
        state.settings(),
    )
}
fn job_url(state: &ActionsState, path: &str) -> String {
    format!("{}/{}", state.job_origin(), path.trim_start_matches('/'))
}

async fn create(
    State(state): State<ActionsState>,
    headers: HeaderMap,
    axum::Json(request): axum::Json<artifact::CreateArtifactRequest>,
) -> Result<axum::Json<artifact::CreateArtifactResponse>, ApiError> {
    let auth = job(&headers)?;
    let run = parse_run(&request.workflow_run_backend_id)?;
    let job_id = parse_job(&request.workflow_job_run_backend_id)?;
    let expires_at = request
        .expires_at
        .map(|timestamp| {
            let nanos = u32::try_from(timestamp.nanos)
                .ok()
                .filter(|nanos| *nanos < 1_000_000_000)
                .ok_or_else(|| ApiError::bad_request("Artifact expiry is invalid."))?;
            chrono::DateTime::<Utc>::from_timestamp(timestamp.seconds, nanos)
                .ok_or_else(|| ApiError::bad_request("Artifact expiry is invalid."))
        })
        .transpose()?;
    let grant_token = service(&state)
        .create(auth, run, job_id, &request.name, expires_at)
        .await?;
    Ok(axum::Json(artifact::CreateArtifactResponse {
        ok: true,
        signed_upload_url: job_url(
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
    let auth = job(&headers)?;
    let run = parse_run(&request.workflow_run_backend_id)?;
    let job_id = parse_job(&request.workflow_job_run_backend_id)?;
    let artifact_id = service(&state)
        .finalize(
            auth,
            run,
            job_id,
            &request.name,
            request.size,
            request.hash.as_ref().map(|hash| hash.value.as_str()),
        )
        .await?;
    Ok(axum::Json(artifact::FinalizeArtifactResponse {
        ok: true,
        artifact_id,
    }))
}
async fn list(
    State(state): State<ActionsState>,
    headers: HeaderMap,
    axum::Json(request): axum::Json<artifact::ListArtifactsRequest>,
) -> Result<axum::Json<artifact::ListArtifactsResponse>, ApiError> {
    let auth = job(&headers)?;
    let run = parse_run(&request.workflow_run_backend_id)?;
    parse_job(&request.workflow_job_run_backend_id)?;
    let rows = service(&state)
        .list(
            auth,
            run,
            request.name_filter.map(|filter| filter.value),
            request.id_filter.map(|filter| filter.value),
        )
        .await?;
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
    let auth = job(&headers)?;
    let run = parse_run(&request.workflow_run_backend_id)?;
    parse_job(&request.workflow_job_run_backend_id)?;
    let token = service(&state)
        .signed_download(auth, run, &request.name)
        .await?;
    Ok(axum::Json(artifact::GetSignedArtifactUrlResponse {
        signed_url: job_url(
            &state,
            &format!(
                "twirp/github.actions.results.api.v1.ArtifactService/DownloadArtifact?token={token}"
            ),
        ),
    }))
}
async fn delete_artifact(
    State(state): State<ActionsState>,
    headers: HeaderMap,
    axum::Json(request): axum::Json<artifact::DeleteArtifactRequest>,
) -> Result<axum::Json<artifact::DeleteArtifactResponse>, ApiError> {
    let auth = job(&headers)?;
    let run = parse_run(&request.workflow_run_backend_id)?;
    parse_job(&request.workflow_job_run_backend_id)?;
    let artifact_id = service(&state).delete(auth, run, &request.name).await?;
    Ok(axum::Json(artifact::DeleteArtifactResponse {
        ok: true,
        artifact_id,
    }))
}

async fn upload(
    State(state): State<ActionsState>,
    Query(query): Query<SignedQuery>,
    body: Body,
) -> Result<StatusCode, ApiError> {
    let service = service(&state);
    let authorization = service.authorize_upload(&query.token).await?;
    if query.comp.as_deref() == Some("blocklist") {
        let xml = read_limited(body, state.settings().max_artifact_block_list_bytes).await?;
        let order = parse_block_list(&xml, state.settings().max_artifact_blocks)?;
        service.set_block_order(authorization, order).await?;
    } else {
        service
            .upload(
                authorization,
                query.block_id.unwrap_or_else(|| "full".to_owned()),
                query.comp.as_deref() == Some("appendBlock"),
                body.into_data_stream(),
            )
            .await?;
    }
    Ok(StatusCode::CREATED)
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
    gitadel_actions::artifacts::parse_block_list(bytes, max_blocks).map_err(ApiError::bad_request)
}

async fn download(
    State(state): State<ActionsState>,
    Query(query): Query<SignedQuery>,
) -> Result<Response, ApiError> {
    let (row, file) = service(&state).download(&query.token).await?;
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
    gitadel_actions::artifacts::cleanup(
        database(state),
        state.repository().actions_artifact_root(),
        state.settings().artifact_grant_lifetime_seconds,
    )
    .await
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
