use std::{collections::BTreeMap, io::ErrorKind};

use axum::{
    Json, Router,
    body::Body,
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use http_body_util::BodyExt as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use tokio::{
    fs::{self, OpenOptions},
    io::AsyncWriteExt as _,
};
use tokio_util::io::ReaderStream;
use uuid::Uuid;

use super::{LfsPermission, Permission, RepositoryState};
use crate::{
    entity::repository,
    identity::{ApiError, SCOPE_READ, SCOPE_WRITE},
};

const LFS_MEDIA_TYPE: &str = "application/vnd.git-lfs+json";
const TOKEN_LIFETIME_SECONDS: u64 = 15 * 60;

#[derive(Deserialize)]
struct BatchRequest {
    operation: String,
    objects: Vec<BatchObjectRequest>,
}

#[derive(Deserialize)]
struct BatchObjectRequest {
    oid: String,
    size: u64,
}

#[derive(Serialize)]
struct BatchResponse {
    transfer: &'static str,
    hash_algo: &'static str,
    objects: Vec<BatchObjectResponse>,
}

#[derive(Serialize)]
struct BatchObjectResponse {
    oid: String,
    size: u64,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    actions: BTreeMap<&'static str, LfsAction>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<LfsObjectError>,
}

#[derive(Serialize)]
struct LfsAction {
    href: String,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    header: BTreeMap<&'static str, String>,
    expires_in: u64,
}

#[derive(Serialize)]
struct LfsObjectError {
    code: u16,
    message: &'static str,
}

pub fn router() -> Router<RepositoryState> {
    Router::new()
        .route(
            "/{namespace}/{repository}/info/lfs/objects/batch",
            post(batch),
        )
        .route(
            "/{namespace}/{repository}/info/lfs/objects/{oid}",
            get(download).put(upload),
        )
}

async fn batch(
    State(state): State<RepositoryState>,
    Path((namespace, repository_segment)): Path<(String, String)>,
    headers: HeaderMap,
    Json(request): Json<BatchRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let permission = match request.operation.as_str() {
        "download" => LfsPermission::Read,
        "upload" => LfsPermission::Write,
        _ => return Err(ApiError::bad_request("Unsupported LFS batch operation.")),
    };
    let (repository, actor_user_id) = authorized_repository(
        &state,
        &headers,
        &namespace,
        &repository_segment,
        permission,
    )
    .await?;
    let endpoint = state.lfs_endpoint(&repository);
    let authorization = if let Some(user_id) = actor_user_id {
        let token = state
            .issue_lfs_token(repository.id, user_id, permission)
            .await;
        Some(format!("Bearer {token}"))
    } else {
        None
    };

    let mut objects = Vec::with_capacity(request.objects.len());
    for object in request.objects {
        if !valid_oid(&object.oid) {
            objects.push(BatchObjectResponse {
                oid: object.oid,
                size: object.size,
                actions: BTreeMap::new(),
                error: Some(LfsObjectError {
                    code: StatusCode::UNPROCESSABLE_ENTITY.as_u16(),
                    message: "The LFS object identifier is invalid.",
                }),
            });
            continue;
        }
        let object_path = state.lfs_object_path(&repository, &object.oid);
        let exists = fs::try_exists(&object_path)
            .await
            .map_err(ApiError::internal)?;
        let mut actions = BTreeMap::new();
        let error = match permission {
            LfsPermission::Read if exists => {
                actions.insert(
                    "download",
                    action(
                        format!("{endpoint}/objects/{}", object.oid),
                        authorization.as_deref(),
                    ),
                );
                None
            }
            LfsPermission::Read => Some(LfsObjectError {
                code: StatusCode::NOT_FOUND.as_u16(),
                message: "The LFS object does not exist.",
            }),
            LfsPermission::Write if !exists => {
                actions.insert(
                    "upload",
                    action(
                        format!("{endpoint}/objects/{}", object.oid),
                        authorization.as_deref(),
                    ),
                );
                None
            }
            LfsPermission::Write => None,
        };
        objects.push(BatchObjectResponse {
            oid: object.oid,
            size: object.size,
            actions,
            error,
        });
    }

    Ok((
        [(
            header::CONTENT_TYPE,
            HeaderValue::from_static(LFS_MEDIA_TYPE),
        )],
        Json(BatchResponse {
            transfer: "basic",
            hash_algo: "sha256",
            objects,
        }),
    ))
}

async fn download(
    State(state): State<RepositoryState>,
    Path((namespace, repository_segment, oid)): Path<(String, String, String)>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    if !valid_oid(&oid) {
        return Err(ApiError::not_found());
    }
    let (repository, _) = authorized_repository(
        &state,
        &headers,
        &namespace,
        &repository_segment,
        LfsPermission::Read,
    )
    .await?;
    let path = state.lfs_object_path(&repository, &oid);
    let file = fs::File::open(&path)
        .await
        .map_err(|error| match error.kind() {
            ErrorKind::NotFound => ApiError::not_found(),
            _ => ApiError::internal(error),
        })?;
    let size = file.metadata().await.map_err(ApiError::internal)?.len();
    let mut response = Response::new(Body::from_stream(ReaderStream::new(file)));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    response.headers_mut().insert(
        header::CONTENT_LENGTH,
        HeaderValue::from_str(&size.to_string()).map_err(ApiError::internal)?,
    );
    Ok(response)
}

async fn upload(
    State(state): State<RepositoryState>,
    Path((namespace, repository_segment, oid)): Path<(String, String, String)>,
    headers: HeaderMap,
    mut body: Body,
) -> Result<StatusCode, ApiError> {
    if !valid_oid(&oid) {
        return Err(ApiError::bad_request(
            "The LFS object identifier is invalid.",
        ));
    }
    let (repository, _) = authorized_repository(
        &state,
        &headers,
        &namespace,
        &repository_segment,
        LfsPermission::Write,
    )
    .await?;
    let path = state.lfs_object_path(&repository, &oid);
    if fs::try_exists(&path).await.map_err(ApiError::internal)? {
        return Ok(StatusCode::OK);
    }
    let parent = path
        .parent()
        .ok_or_else(|| ApiError::internal("LFS object path has no parent"))?;
    fs::create_dir_all(parent)
        .await
        .map_err(ApiError::internal)?;
    let temporary = parent.join(format!(".{}.upload", Uuid::new_v4().simple()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .await
        .map_err(ApiError::internal)?;
    let mut digest = Sha256::new();
    let result = async {
        while let Some(frame) = body.frame().await {
            let frame = frame.map_err(ApiError::internal)?;
            if let Ok(data) = frame.into_data() {
                digest.update(&data);
                file.write_all(&data).await.map_err(ApiError::internal)?;
            }
        }
        file.sync_all().await.map_err(ApiError::internal)?;
        let actual_oid = format!("{:x}", digest.finalize());
        if actual_oid != oid {
            return Err(ApiError::bad_request(
                "The uploaded LFS object does not match its identifier.",
            ));
        }
        fs::rename(&temporary, &path)
            .await
            .map_err(ApiError::internal)?;
        Ok(())
    }
    .await;
    if result.is_err() {
        let _ = fs::remove_file(&temporary).await;
    }
    result?;
    Ok(StatusCode::OK)
}

async fn authorized_repository(
    state: &RepositoryState,
    headers: &HeaderMap,
    namespace: &str,
    repository_segment: &str,
    permission: LfsPermission,
) -> Result<(repository::Model, Option<Uuid>), ApiError> {
    let name = repository_segment
        .strip_suffix(".git")
        .ok_or_else(ApiError::not_found)?;
    let repository = state.find(namespace, name).await?;

    if let Some(token) = bearer_token(headers)
        && let Some(user_id) = state
            .authenticate_lfs_token(&token, repository.id, permission)
            .await
    {
        state
            .authorize(
                &repository,
                Some(user_id),
                match permission {
                    LfsPermission::Read => Permission::Read,
                    LfsPermission::Write => Permission::Write,
                },
            )
            .await?;
        return Ok((repository, Some(user_id)));
    }

    if permission == LfsPermission::Read && repository.visibility == "public" {
        return Ok((repository, None));
    }

    let token = token_from_headers(headers).ok_or_else(ApiError::unauthorized)?;
    let actor = state
        .identity()
        .authenticate_token(
            &token,
            match permission {
                LfsPermission::Read => SCOPE_READ,
                LfsPermission::Write => SCOPE_WRITE,
            },
        )
        .await?;
    state
        .authorize(
            &repository,
            Some(actor.user.id),
            match permission {
                LfsPermission::Read => Permission::Read,
                LfsPermission::Write => Permission::Write,
            },
        )
        .await?;
    Ok((repository, Some(actor.user.id)))
}

fn action(href: String, authorization: Option<&str>) -> LfsAction {
    let mut headers = BTreeMap::new();
    if let Some(authorization) = authorization {
        headers.insert("Authorization", authorization.to_owned());
    }
    LfsAction {
        href,
        header: headers,
        expires_in: TOKEN_LIFETIME_SECONDS,
    }
}

fn valid_oid(oid: &str) -> bool {
    oid.len() == 64
        && oid
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn bearer_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .map(str::to_owned)
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
