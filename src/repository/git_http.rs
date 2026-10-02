use std::{io, ops::Deref, sync::Arc, time::Duration};

use axum::{
    Router,
    body::Body,
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use futures_util::{Stream, TryStreamExt as _};
use serde::Deserialize;
use tokio::{
    io::{AsyncWrite, AsyncWriteExt},
    sync::{OwnedSemaphorePermit, Semaphore},
};
use tokio_util::io::ReaderStream;
use tower_http::decompression::RequestDecompressionLayer;

use super::{
    Permission, RepositoryState,
    git_service::{self, BlockingReader, BlockingWriter, BridgeCancellation},
    protection::load_guard,
    resources::{CreateRepositoryOptions, create_owned_repository, record_push},
    webhooks::{dispatch_push, snapshot_refs},
};
use crate::{
    actions::ActionsState,
    entity::repository,
    identity::{ApiError, SCOPE_REPOSITORY_READ, SCOPE_WRITE},
};

/// A client that sends no request body bytes for this long is dropped, so a
/// stalled connection cannot hold a blocking thread until the service cap.
const BODY_IDLE_TIMEOUT: Duration = Duration::from_secs(120);
/// Upper bound for one upload-pack or receive-pack call.
const SERVICE_TIME_LIMIT: Duration = Duration::from_secs(30 * 60);
/// In-flight upload-pack and receive-pack calls. Each one occupies a blocking
/// thread; the bound stays well under Tokio's blocking pool (512 by default)
/// so stalled or slow Git clients cannot starve file and database work.
const MAX_CONCURRENT_SERVICES: usize = 64;
/// How long a request waits for a free service slot before it is turned away.
const SERVICE_SLOT_WAIT: Duration = Duration::from_secs(30);

#[derive(Clone)]
pub(crate) struct GitHttpState {
    repository: RepositoryState,
    actions: ActionsState,
    service_slots: Arc<Semaphore>,
}

impl GitHttpState {
    pub(crate) fn new(repository: RepositoryState, actions: ActionsState) -> Self {
        Self {
            repository,
            actions,
            service_slots: Arc::new(Semaphore::new(MAX_CONCURRENT_SERVICES)),
        }
    }

    /// Reserves a blocking-pool slot for one service call, or `None` when
    /// every slot stays busy.
    async fn service_slot(&self) -> Option<OwnedSemaphorePermit> {
        acquire_service_slot(&self.service_slots, SERVICE_SLOT_WAIT).await
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
        // Git gzips large request bodies, such as the negotiation of a full
        // clone of a repository with many refs.
        .layer(RequestDecompressionLayer::new())
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
    // Reading refs and configuration is synchronous file work.
    let receive = service == "git-receive-pack";
    let advertisement = tokio::task::spawn_blocking(move || {
        let mut output = Vec::new();
        git_service::write_advertisement(&path, format, receive, v2, &mut output).map(|()| output)
    })
    .await;
    let output = match advertisement {
        Ok(Ok(output)) => output,
        Ok(Err(error)) => {
            tracing::error!(%error, repository = %repository_path(&repository), "Git service advertisement failed");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
        Err(error) => {
            tracing::error!(%error, repository = %repository_path(&repository), "Git service advertisement task failed");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
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
    let Some(slot) = state.service_slot().await else {
        return service_busy();
    };
    let (request_io, response_io) = tokio::io::duplex(64 * 1024);
    let (response_reader, request_writer) = tokio::io::split(request_io);
    let (request_reader, response_writer) = tokio::io::split(response_io);
    let cancellation = BridgeCancellation::default();
    let feeder = tokio::spawn(feed_request_body(
        body.into_data_stream().map_err(io::Error::other),
        request_writer,
        cancellation.clone(),
        BODY_IDLE_TIMEOUT,
        "upload-pack",
    ));
    let worker_cancellation = cancellation.clone();
    let handle = tokio::runtime::Handle::current();
    let mut worker = tokio::task::spawn_blocking(move || {
        let mut reader =
            BlockingReader::new(request_reader, handle.clone(), worker_cancellation.clone());
        let mut writer = BlockingWriter::new(response_writer, handle, worker_cancellation);
        let _slot = slot;
        git_service::serve_upload_pack(&path, format, v2, true, &mut reader, &mut writer)
    });
    let display_path = repository_path(&repository);
    state.spawn_task(async move {
        let result = tokio::time::timeout(SERVICE_TIME_LIMIT, &mut worker).await;
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
    let guard = match load_guard(&state, &repository, Some(actor_user_id)).await {
        Ok(guard) => guard,
        Err(error) => return error.into_response(),
    };
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
    let Some(slot) = state.service_slot().await else {
        return service_busy();
    };
    let (request_io, response_io) = tokio::io::duplex(64 * 1024);
    let (response_reader, request_writer) = tokio::io::split(request_io);
    let (request_reader, response_writer) = tokio::io::split(response_io);
    let cancellation = BridgeCancellation::default();
    let feeder = tokio::spawn(feed_request_body(
        body.into_data_stream().map_err(io::Error::other),
        request_writer,
        cancellation.clone(),
        BODY_IDLE_TIMEOUT,
        "receive-pack",
    ));
    let worker_cancellation = cancellation.clone();
    let handle = tokio::runtime::Handle::current();
    let worker_path = path.clone();
    let mut worker = tokio::task::spawn_blocking(move || {
        let mut reader =
            BlockingReader::new(request_reader, handle.clone(), worker_cancellation.clone());
        let mut writer = BlockingWriter::new(response_writer, handle, worker_cancellation);
        let _slot = slot;
        git_service::serve_receive_pack(&worker_path, format, &guard, &mut reader, &mut writer)
    });
    let display_path = repository_path(&repository);
    let task_state = state.clone();
    let actions = state.actions_state().clone();
    let audit_repository = repository.clone();
    let maintenance_path = path.clone();
    state.spawn_task(async move {
        let result = tokio::time::timeout(SERVICE_TIME_LIMIT, &mut worker).await;
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

async fn acquire_service_slot(
    slots: &Arc<Semaphore>,
    wait: Duration,
) -> Option<OwnedSemaphorePermit> {
    tokio::time::timeout(wait, Arc::clone(slots).acquire_owned())
        .await
        .ok()?
        .ok()
}

fn service_busy() -> Response {
    tracing::warn!("Git HTTP service is at its concurrency limit");
    let mut response = StatusCode::SERVICE_UNAVAILABLE.into_response();
    response
        .headers_mut()
        .insert(header::RETRY_AFTER, HeaderValue::from_static("30"));
    response
}

/// Copies the request body into the pipe the blocking service reads from.
///
/// Only the wait for the client is timed: a slow service applies ordinary
/// back-pressure through the pipe and never counts as an idle client.
async fn feed_request_body<S, W>(
    body: S,
    mut pipe: W,
    cancellation: BridgeCancellation,
    idle: Duration,
    service: &'static str,
) where
    S: Stream<Item = io::Result<axum::body::Bytes>>,
    W: AsyncWrite + Unpin,
{
    tokio::pin!(body);
    loop {
        let chunk = match tokio::time::timeout(idle, body.try_next()).await {
            Ok(Ok(Some(chunk))) => chunk,
            Ok(Ok(None)) => break,
            Ok(Err(error)) => {
                // Closing the pipe here would look like a clean end of input,
                // and a truncated pack would then surface as a checksum
                // mismatch.
                tracing::warn!(%error, service, "request body ended before it was complete");
                cancellation.cancel();
                return;
            }
            Err(_) => {
                tracing::warn!(service, "request body stalled; dropping the client");
                cancellation.cancel();
                return;
            }
        };
        if pipe.write_all(&chunk).await.is_err() {
            // The service finished or failed and closed its end.
            return;
        }
    }
    let _ = pipe.shutdown().await;
}

#[derive(Debug)]
pub(super) enum AuthorizationError {
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

pub(super) async fn authorized_repository(
    state: &GitHttpState,
    headers: &HeaderMap,
    namespace: &str,
    repository_segment: &str,
) -> Result<repository::Model, AuthorizationError> {
    let name = repository_name(repository_segment);
    let repository = match state.find(namespace, name).await {
        Ok(repository) => repository,
        Err(error) => {
            // Answer exactly as a private repository the caller cannot read
            // would: credentials are checked first, so only an authenticated
            // caller learns that there is nothing here for them.
            let token =
                token_from_headers(headers).ok_or(AuthorizationError::AuthenticationRequired)?;
            state
                .identity()
                .authenticate_token(&token, SCOPE_REPOSITORY_READ)
                .await
                .map_err(|_| AuthorizationError::AuthenticationRequired)?;
            return Err(AuthorizationError::Api(error));
        }
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
    let name = repository_name(repository_segment);
    let token = token_from_headers(headers).ok_or(AuthorizationError::AuthenticationRequired)?;
    let actor = state
        .identity()
        .authenticate_token(&token, SCOPE_WRITE)
        .await
        .map_err(|_| AuthorizationError::AuthenticationRequired)?;
    // Pushing to a repository that does not exist creates it, private unless
    // the account prefers otherwise, exactly as over SSH. Creation is limited
    // to the actor's own namespace or organizations they own.
    let repository = match state.find(namespace, name).await {
        Ok(repository) => repository,
        Err(_) => {
            let options = CreateRepositoryOptions {
                namespace: namespace.to_owned(),
                name: name.to_owned(),
                description: None,
                visibility: None,
                object_format: None,
                mirror: None,
            };
            match create_owned_repository(state, actor.user.id, options).await {
                Ok(repository) => repository,
                Err(create_error) => state
                    .find(namespace, name)
                    .await
                    .map_err(|_| AuthorizationError::Api(create_error))?,
            }
        }
    };
    state
        .authorize(&repository, Some(actor.user.id), Permission::Write)
        .await
        .map_err(AuthorizationError::Api)?;
    Ok((repository, actor.user.id))
}

/// Accepts both `owner/name.git` and `owner/name`, as GitHub and Forgejo do;
/// `actions/checkout` fetches without the suffix. Names never end in `.git`.
fn repository_name(segment: &str) -> &str {
    segment.strip_suffix(".git").unwrap_or(segment)
}

/// Reads a Bearer token or the password of Basic credentials. Schemes are
/// case-insensitive; `actions/checkout` sends `basic`.
pub(super) fn token_from_headers(headers: &HeaderMap) -> Option<String> {
    let authorization = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, credentials) = authorization.split_once(' ')?;
    if scheme.eq_ignore_ascii_case("bearer") {
        return (!credentials.is_empty()).then(|| credentials.to_owned());
    }
    if !scheme.eq_ignore_ascii_case("basic") {
        return None;
    }
    let decoded = STANDARD.decode(credentials).ok()?;
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

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Duration};

    use axum::{
        body::Bytes,
        http::{HeaderMap, HeaderValue, StatusCode, header},
    };
    use futures_util::{StreamExt as _, stream};
    use tokio::{io::AsyncReadExt as _, sync::Semaphore};

    use super::{
        BlockingReader, BridgeCancellation, acquire_service_slot, feed_request_body,
        repository_name, service_busy, token_from_headers,
    };

    fn authorization(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(header::AUTHORIZATION, HeaderValue::from_str(value).unwrap());
        headers
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn stalled_request_body_cancels_the_service() {
        let (pipe, mut service_end) = tokio::io::duplex(1024);
        let cancellation = BridgeCancellation::default();
        let body = stream::iter([Ok(Bytes::from_static(b"0032want"))]).chain(stream::pending());

        feed_request_body(
            body,
            pipe,
            cancellation.clone(),
            Duration::from_millis(50),
            "upload-pack",
        )
        .await;

        // The bytes that did arrive were forwarded, then the stall cancelled
        // the bridge so a blocked reader fails instead of waiting.
        let mut received = [0u8; 8];
        service_end.read_exact(&mut received).await.unwrap();
        assert_eq!(&received, b"0032want");
        let handle = tokio::runtime::Handle::current();
        let read = tokio::task::spawn_blocking(move || {
            let mut reader = BlockingReader::new(service_end, handle, cancellation);
            std::io::Read::read(&mut reader, &mut [0u8; 8])
        })
        .await
        .unwrap();
        assert!(read.is_err());
    }

    #[tokio::test]
    async fn complete_request_body_ends_the_pipe_cleanly() {
        let (pipe, mut service_end) = tokio::io::duplex(1024);
        let cancellation = BridgeCancellation::default();
        let body = stream::iter([Ok(Bytes::from_static(b"0000"))]);

        feed_request_body(
            body,
            pipe,
            cancellation.clone(),
            Duration::from_secs(5),
            "upload-pack",
        )
        .await;

        let mut received = Vec::new();
        service_end.read_to_end(&mut received).await.unwrap();
        assert_eq!(received, b"0000");
    }

    #[tokio::test]
    async fn service_slots_bound_concurrency() {
        let slots = Arc::new(Semaphore::new(1));
        let wait = Duration::from_millis(20);
        let held = acquire_service_slot(&slots, wait).await.unwrap();
        assert!(acquire_service_slot(&slots, wait).await.is_none());
        assert_eq!(service_busy().status(), StatusCode::SERVICE_UNAVAILABLE);
        drop(held);
        assert!(acquire_service_slot(&slots, wait).await.is_some());
    }

    #[test]
    fn repository_name_accepts_optional_git_suffix() {
        assert_eq!(repository_name("project.git"), "project");
        assert_eq!(repository_name("project"), "project");
    }

    #[test]
    fn token_from_headers_accepts_case_insensitive_schemes() {
        // "x-access-token:secret"
        let basic = "eC1hY2Nlc3MtdG9rZW46c2VjcmV0";
        for scheme in ["Basic", "basic", "BASIC"] {
            let headers = authorization(&format!("{scheme} {basic}"));
            assert_eq!(token_from_headers(&headers).as_deref(), Some("secret"));
        }
        for scheme in ["Bearer", "bearer"] {
            let headers = authorization(&format!("{scheme} secret"));
            assert_eq!(token_from_headers(&headers).as_deref(), Some("secret"));
        }
        assert_eq!(token_from_headers(&authorization("Digest secret")), None);
        assert_eq!(token_from_headers(&authorization("Bearer ")), None);
    }
}
