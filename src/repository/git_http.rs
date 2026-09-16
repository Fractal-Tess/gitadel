use std::{io, ops::Deref, time::Duration};

use axum::{
    Router,
    body::Body,
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use futures_util::TryStreamExt as _;
use serde::Deserialize;
use tokio::io::AsyncWriteExt;
use tokio_util::io::{ReaderStream, StreamReader};

use super::{
    Permission, RepositoryState,
    git_service::{self, BlockingReader, BlockingWriter, BridgeCancellation},
    resources::record_push,
    webhooks::{dispatch_push, snapshot_refs},
};
use crate::{
    actions::ActionsState,
    entity::repository,
    identity::{ApiError, SCOPE_REPOSITORY_READ, SCOPE_WRITE},
};

#[derive(Clone)]
pub(crate) struct GitHttpState {
    repository: RepositoryState,
    actions: ActionsState,
}

impl GitHttpState {
    pub(crate) fn new(repository: RepositoryState, actions: ActionsState) -> Self {
        Self {
            repository,
            actions,
        }
    }

    pub(super) fn actions_state(&self) -> &ActionsState {
        &self.actions
    }
}

impl Deref for GitHttpState {
    type Target = RepositoryState;

    fn deref(&self) -> &Self::Target {
        &self.repository
    }
}

#[derive(Deserialize)]
struct InfoRefsQuery {
    service: String,
}

pub fn router() -> Router<GitHttpState> {
    Router::new()
        .route("/{namespace}/{repository}/info/refs", get(info_refs))
        .route(
            "/{namespace}/{repository}/git-upload-pack",
            post(upload_pack),
        )
        .route(
            "/{namespace}/{repository}/git-receive-pack",
            post(receive_pack),
        )
}

async fn info_refs(
    State(state): State<GitHttpState>,
    Path((namespace, repository_segment)): Path<(String, String)>,
    Query(query): Query<InfoRefsQuery>,
    headers: HeaderMap,
) -> Response {
    let (service, repository) =
        match query.service.as_str() {
            "git-upload-pack" => {
                let repository =
                    match authorized_repository(&state, &headers, &namespace, &repository_segment)
                        .await
                    {
                        Ok(repository) => repository,
                        Err(error) => return error.into_response(),
                    };
                ("git-upload-pack", repository)
            }
            "git-receive-pack" => {
                let (repository, _) =
                    match writable_repository(&state, &headers, &namespace, &repository_segment)
                        .await
                    {
                        Ok(authorization) => authorization,
                        Err(error) => return error.into_response(),
                    };
                if repository.archived_at.is_some() {
                    return StatusCode::FORBIDDEN.into_response();
                }
                ("git-receive-pack", repository)
            }
            _ => return StatusCode::FORBIDDEN.into_response(),
        };
    let path = state.repository_path(&repository);
    let format = match git_service::object_format(&repository.object_format) {
        Ok(format) => format,
        Err(error) => {
            tracing::error!(%error, "unsupported repository object format");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    let v2 = git_service::protocol_v2(
        headers
            .get("git-protocol")
            .and_then(|value| value.to_str().ok()),
    );
    let mut output = Vec::new();
    if let Err(error) = git_service::write_advertisement(
        &path,
        format,
        service == "git-receive-pack",
        v2,
        &mut output,
    ) {
        tracing::error!(%error, repository = %repository_path(&repository), "Git service advertisement failed");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    if v2 && service == "git-upload-pack" {
        return git_response(
            "application/x-git-upload-pack-advertisement",
            Body::from(output),
        );
    }
    let service_line = format!("# service={service}\n");
    let mut body = Vec::with_capacity(output.len() + service_line.len() + 8);
    body.extend_from_slice(format!("{:04x}", service_line.len() + 4).as_bytes());
    body.extend_from_slice(service_line.as_bytes());
    body.extend_from_slice(b"0000");
    body.extend_from_slice(&output);
    git_response(
        if service == "git-upload-pack" {
            "application/x-git-upload-pack-advertisement"
        } else {
            "application/x-git-receive-pack-advertisement"
        },
        Body::from(body),
    )
}

async fn upload_pack(
    State(state): State<GitHttpState>,
    Path((namespace, repository_segment)): Path<(String, String)>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    let repository =
        match authorized_repository(&state, &headers, &namespace, &repository_segment).await {
            Ok(repository) => repository,
            Err(error) => return error.into_response(),
        };
    let path = state.repository_path(&repository);
    let format = match git_service::object_format(&repository.object_format) {
        Ok(format) => format,
        Err(error) => {
            tracing::error!(%error, "unsupported repository object format");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    let v2 = git_service::protocol_v2(
        headers
            .get("git-protocol")
            .and_then(|value| value.to_str().ok()),
    );
    let (request_io, response_io) = tokio::io::duplex(64 * 1024);
    let (response_reader, mut request_writer) = tokio::io::split(request_io);
    let (request_reader, response_writer) = tokio::io::split(response_io);
    let request_stream = body.into_data_stream().map_err(io::Error::other);
    let mut request_stream = StreamReader::new(request_stream);
    let feeder = tokio::spawn(async move {
        if let Err(error) = tokio::io::copy(&mut request_stream, &mut request_writer).await {
            tracing::debug!(%error, "upload-pack request body closed early");
        }
        let _ = request_writer.shutdown().await;
    });
    let cancellation = BridgeCancellation::default();
    let worker_cancellation = cancellation.clone();
    let handle = tokio::runtime::Handle::current();
    let mut worker = tokio::task::spawn_blocking(move || {
        let mut reader =
            BlockingReader::new(request_reader, handle.clone(), worker_cancellation.clone());
        let mut writer = BlockingWriter::new(response_writer, handle, worker_cancellation);
        git_service::serve_upload_pack(&path, format, v2, true, &mut reader, &mut writer)
    });
    let display_path = repository_path(&repository);
    state.spawn_task(async move {
        let result = tokio::time::timeout(Duration::from_secs(30 * 60), &mut worker).await;
        let result = match result {
            Err(_) => {
                cancellation.cancel();
                feeder.abort();
                tracing::warn!(repository = %display_path, "native upload-pack exceeded time limit");
                worker.await
            }
            Ok(result) => result,
        };
        match result {
            Ok(Ok(())) => {}
            Ok(Err(error)) => tracing::warn!(%error, repository = %display_path, "native upload-pack failed"),
            Err(error) => tracing::warn!(%error, repository = %display_path, "native upload-pack task failed"),
        }
    });
    git_response(
        "application/x-git-upload-pack-result",
        Body::from_stream(ReaderStream::new(response_reader)),
    )
}

async fn receive_pack(
    State(state): State<GitHttpState>,
    Path((namespace, repository_segment)): Path<(String, String)>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    let (repository, actor_user_id) =
        match writable_repository(&state, &headers, &namespace, &repository_segment).await {
            Ok(authorization) => authorization,
            Err(error) => return error.into_response(),
        };
    if repository.archived_at.is_some() {
        return StatusCode::FORBIDDEN.into_response();
    }
    let path = state.repository_path(&repository);
    let refs_before = match snapshot_refs(&path).await {
        Ok(refs) => Some(refs),
        Err(error) => {
            tracing::warn!(%error, %namespace, name = %repository.name, "could not snapshot refs before HTTP push");
            None
        }
    };
    let format = match git_service::object_format(&repository.object_format) {
        Ok(format) => format,
        Err(error) => {
            tracing::error!(%error, "unsupported repository object format");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    let (request_io, response_io) = tokio::io::duplex(64 * 1024);
    let (response_reader, mut request_writer) = tokio::io::split(request_io);
    let (request_reader, response_writer) = tokio::io::split(response_io);
    let request = body.into_data_stream().map_err(io::Error::other);
    let mut request = StreamReader::new(request);
    let feeder = tokio::spawn(async move {
        if let Err(error) = tokio::io::copy(&mut request, &mut request_writer).await {
            tracing::debug!(%error, "receive-pack request body closed early");
        }
        let _ = request_writer.shutdown().await;
    });
    let cancellation = BridgeCancellation::default();
    let worker_cancellation = cancellation.clone();
    let handle = tokio::runtime::Handle::current();
    let worker_path = path.clone();
    let mut worker = tokio::task::spawn_blocking(move || {
        let mut reader =
            BlockingReader::new(request_reader, handle.clone(), worker_cancellation.clone());
        let mut writer = BlockingWriter::new(response_writer, handle, worker_cancellation);
        git_service::serve_receive_pack(&worker_path, format, &mut reader, &mut writer)
    });
    let display_path = repository_path(&repository);
    let task_state = state.clone();
    let actions = state.actions_state().clone();
    let audit_repository = repository.clone();
    let maintenance_path = path.clone();
    state.spawn_task(async move {
        let result = tokio::time::timeout(Duration::from_secs(30 * 60), &mut worker).await;
        let result = match result {
            Err(_) => {
                cancellation.cancel();
                feeder.abort();
                tracing::warn!(repository = %display_path, "native receive-pack exceeded time limit");
                // spawn_blocking cannot be aborted. Await it before deciding
                // whether post-push side effects are required.
                worker.await
            }
            Ok(result) => result,
        };
        match result {
            Ok(Ok(outcome)) => {
                if let Some(error) = outcome.response_error {
                    tracing::debug!(%error, repository = %display_path, "receive-pack response delivery failed");
                }
                if outcome.landed {
                    if let Err(error) =
                        record_push(&task_state, audit_repository.id, actor_user_id, display_path.clone()).await
                    {
                        tracing::warn!(%error, repository = %display_path, "could not record HTTP push");
                    }
                    if let Some(refs_before) = refs_before
                        && let Err(error) = dispatch_push(
                            &task_state,
                            &actions,
                            &audit_repository,
                            actor_user_id,
                            refs_before,
                        )
                        .await
                    {
                        tracing::warn!(%error, repository = %display_path, "could not dispatch HTTP push");
                    }
                    super::maintenance::run(&maintenance_path, &display_path).await;
                }
            }
            Ok(Err(error)) => tracing::warn!(%error, repository = %display_path, "native receive-pack failed"),
            Err(error) => tracing::warn!(%error, repository = %display_path, "native receive-pack task failed"),
        }
    });
    git_response(
        "application/x-git-receive-pack-result",
        Body::from_stream(ReaderStream::new(response_reader)),
    )
}

enum AuthorizationError {
    Status(StatusCode),
    AuthenticationRequired,
    Api(ApiError),
}

impl IntoResponse for AuthorizationError {
    fn into_response(self) -> Response {
        match self {
            Self::Status(status) => status.into_response(),
            Self::AuthenticationRequired => authentication_required(),
            Self::Api(error) => error.into_response(),
        }
    }
}

async fn authorized_repository(
    state: &GitHttpState,
    headers: &HeaderMap,
    namespace: &str,
    repository_segment: &str,
) -> Result<repository::Model, AuthorizationError> {
    let Some(name) = repository_segment.strip_suffix(".git") else {
        return Err(AuthorizationError::Status(StatusCode::NOT_FOUND));
    };
    let repository = match state.find(namespace, name).await {
        Ok(repository) => repository,
        Err(_) if headers.get(header::AUTHORIZATION).is_none() => {
            return Err(AuthorizationError::AuthenticationRequired);
        }
        Err(error) => return Err(AuthorizationError::Api(error)),
    };
    if repository.visibility == "public" {
        return Ok(repository);
    }
    let token = token_from_headers(headers).ok_or(AuthorizationError::AuthenticationRequired)?;
    if crate::actions::tokens::authorize_job(state.identity().database(), &token, repository.id)
        .await
        .map_err(|_| AuthorizationError::Status(StatusCode::SERVICE_UNAVAILABLE))?
        .is_some()
    {
        return Ok(repository);
    }
    let actor = state
        .identity()
        .authenticate_token(&token, SCOPE_REPOSITORY_READ)
        .await
        .map_err(|_| AuthorizationError::AuthenticationRequired)?;
    state
        .authorize(&repository, Some(actor.user.id), Permission::Read)
        .await
        .map_err(AuthorizationError::Api)?;
    Ok(repository)
}

async fn writable_repository(
    state: &GitHttpState,
    headers: &HeaderMap,
    namespace: &str,
    repository_segment: &str,
) -> Result<(repository::Model, uuid::Uuid), AuthorizationError> {
    let Some(name) = repository_segment.strip_suffix(".git") else {
        return Err(AuthorizationError::Status(StatusCode::NOT_FOUND));
    };
    let token = token_from_headers(headers).ok_or(AuthorizationError::AuthenticationRequired)?;
    let actor = state
        .identity()
        .authenticate_token(&token, SCOPE_WRITE)
        .await
        .map_err(|_| AuthorizationError::AuthenticationRequired)?;
    let repository = state
        .find(namespace, name)
        .await
        .map_err(AuthorizationError::Api)?;
    state
        .authorize(&repository, Some(actor.user.id), Permission::Write)
        .await
        .map_err(AuthorizationError::Api)?;
    Ok((repository, actor.user.id))
}

fn token_from_headers(headers: &HeaderMap) -> Option<String> {
    let authorization = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    if let Some(token) = authorization.strip_prefix("Bearer ") {
        return Some(token.to_owned());
    }
    let encoded = authorization.strip_prefix("Basic ")?;
    let decoded = STANDARD.decode(encoded).ok()?;
    let decoded = String::from_utf8(decoded).ok()?;
    let (_, token) = decoded.split_once(':')?;
    (!token.is_empty()).then(|| token.to_owned())
}

fn git_response(content_type: &'static str, body: Body) -> Response {
    let mut response = Response::new(body);
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-cache, max-age=0, must-revalidate"),
    );
    response
}

fn authentication_required() -> Response {
    let mut response = StatusCode::UNAUTHORIZED.into_response();
    response.headers_mut().insert(
        header::WWW_AUTHENTICATE,
        HeaderValue::from_static("Basic realm=\"Gitadel\""),
    );
    response
}

fn repository_path(repository: &repository::Model) -> String {
    format!("{}/{}", repository.namespace, repository.name)
}
