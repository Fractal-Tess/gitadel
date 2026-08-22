use std::{process::Stdio, time::Duration};

use axum::{
    Router,
    body::{Body, Bytes},
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::Deserialize;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
};
use tokio_util::io::ReaderStream;

use super::{Permission, RepositoryState};
use crate::{entity::repository, identity::SCOPE_REPOSITORY_READ};

#[derive(Deserialize)]
struct InfoRefsQuery {
    service: String,
}

pub fn router() -> Router<RepositoryState> {
    Router::new()
        .route("/{namespace}/{repository}/info/refs", get(info_refs))
        .route(
            "/{namespace}/{repository}/git-upload-pack",
            post(upload_pack),
        )
}

async fn info_refs(
    State(state): State<RepositoryState>,
    Path((namespace, repository_segment)): Path<(String, String)>,
    Query(query): Query<InfoRefsQuery>,
    headers: HeaderMap,
) -> Response {
    if query.service != "git-upload-pack" {
        return StatusCode::FORBIDDEN.into_response();
    }
    let repository =
        match authorized_repository(&state, &headers, &namespace, &repository_segment).await {
            Ok(repository) => repository,
            Err(response) => return response,
        };
    let path = state.repository_path(&repository);
    let output = match command(&headers)
        .args(["--stateless-rpc", "--advertise-refs"])
        .arg(&path)
        .output()
        .await
    {
        Ok(output) if output.status.success() => output.stdout,
        Ok(output) => {
            tracing::error!(
                repository = %repository_path(&repository),
                stderr = %String::from_utf8_lossy(&output.stderr),
                "git upload-pack advertisement failed"
            );
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
        Err(error) => {
            tracing::error!(%error, "could not start git upload-pack");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    let service_line = b"# service=git-upload-pack\n";
    let mut body = Vec::with_capacity(output.len() + service_line.len() + 8);
    body.extend_from_slice(format!("{:04x}", service_line.len() + 4).as_bytes());
    body.extend_from_slice(service_line);
    body.extend_from_slice(b"0000");
    body.extend_from_slice(&output);
    git_response(
        "application/x-git-upload-pack-advertisement",
        Body::from(body),
    )
}

async fn upload_pack(
    State(state): State<RepositoryState>,
    Path((namespace, repository_segment)): Path<(String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let repository =
        match authorized_repository(&state, &headers, &namespace, &repository_segment).await {
            Ok(repository) => repository,
            Err(response) => return response,
        };
    let path = state.repository_path(&repository);
    let mut child = match command(&headers)
        .arg("--stateless-rpc")
        .arg(&path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            tracing::error!(%error, "could not start git upload-pack");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    let Some(mut stdin) = child.stdin.take() else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    let Some(stdout) = child.stdout.take() else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    let stderr = child.stderr.take();
    tokio::spawn(async move {
        if let Err(error) = stdin.write_all(&body).await {
            tracing::debug!(%error, "git upload-pack request body closed early");
        }
    });
    let display_path = repository_path(&repository);
    tokio::spawn(async move {
        let stderr_task = tokio::spawn(async move {
            let mut bytes = Vec::new();
            if let Some(mut stderr) = stderr {
                let _ = stderr.read_to_end(&mut bytes).await;
            }
            bytes
        });
        let result = tokio::time::timeout(Duration::from_secs(30 * 60), child.wait()).await;
        let stderr = stderr_task.await.unwrap_or_default();
        match result {
            Ok(Ok(status)) if status.success() => {}
            Ok(Ok(status)) => {
                tracing::error!(
                    repository = %display_path,
                    ?status,
                    stderr = %String::from_utf8_lossy(&stderr),
                    "git upload-pack failed"
                );
            }
            Ok(Err(error)) => {
                tracing::error!(%error, repository = %display_path, "git upload-pack wait failed");
            }
            Err(_) => {
                tracing::warn!(repository = %display_path, "git upload-pack exceeded time limit");
            }
        }
    });

    git_response(
        "application/x-git-upload-pack-result",
        Body::from_stream(ReaderStream::new(stdout)),
    )
}

async fn authorized_repository(
    state: &RepositoryState,
    headers: &HeaderMap,
    namespace: &str,
    repository_segment: &str,
) -> Result<repository::Model, Response> {
    let Some(name) = repository_segment.strip_suffix(".git") else {
        return Err(StatusCode::NOT_FOUND.into_response());
    };
    let repository = match state.find(namespace, name).await {
        Ok(repository) => repository,
        Err(_) if headers.get(header::AUTHORIZATION).is_none() => {
            return Err(authentication_required());
        }
        Err(error) => return Err(error.into_response()),
    };
    if repository.visibility == "public" {
        return Ok(repository);
    }
    let token = token_from_headers(headers).ok_or_else(authentication_required)?;
    let actor = state
        .identity()
        .authenticate_token(&token, SCOPE_REPOSITORY_READ)
        .await
        .map_err(|_| authentication_required())?;
    state
        .authorize(&repository, Some(actor.user.id), Permission::Read)
        .await
        .map_err(IntoResponse::into_response)?;
    Ok(repository)
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

fn command(headers: &HeaderMap) -> Command {
    let mut command = Command::new("git-upload-pack");
    if let Some(protocol) = headers
        .get("git-protocol")
        .and_then(|value| value.to_str().ok())
    {
        command.env("GIT_PROTOCOL", protocol);
    }
    command
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
