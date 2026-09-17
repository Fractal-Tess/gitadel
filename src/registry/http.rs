use std::{collections::BTreeSet, io::SeekFrom};

use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::{Path, Request, State, rejection::PathRejection},
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    middleware,
    response::{IntoResponse, Response},
    routing::any,
};
use sea_orm::{ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter, QueryOrder};
use tokio::{
    fs::File,
    io::{AsyncReadExt, AsyncSeekExt},
};
use tokio_util::io::ReaderStream;
use uuid::Uuid;

use super::{
    auth::{Actions, RegistryAuth, TokenQuery},
    error::RegistryError,
    parse_image_name,
    store::{
        ImageStore, MAX_BLOB_BYTES, MAX_MANIFEST_BYTES, RegistryStore, StoreError, StoredManifest,
    },
};
use crate::{
    entity::repository,
    repository::{Permission, RepositoryState},
};

const OCI_INDEX: &str = "application/vnd.oci.image.index.v1+json";
const MAX_PAGE_SIZE: usize = 1000;

#[derive(Clone)]
struct RegistryState {
    repositories: RepositoryState,
    store: RegistryStore,
    auth: RegistryAuth,
}

#[derive(Debug)]
enum Resource<'a> {
    Blob { image: &'a str, digest: &'a str },
    Upload { image: &'a str, id: Option<Uuid> },
    Manifest { image: &'a str, reference: &'a str },
    Tags { image: &'a str },
    Referrers { image: &'a str, digest: &'a str },
    Catalog,
}

pub fn router(repositories: RepositoryState) -> Router {
    Router::new()
        .route("/v2/", any(ping))
        .route("/v2/token", any(token))
        .route("/v2/{*path}", any(dispatch))
        .fallback(|| async { RegistryError::not_found("Registry resource not found.") })
        .layer(middleware::map_response(protocol_headers))
        .with_state(RegistryState {
            repositories,
            store: RegistryStore::new(),
            auth: RegistryAuth::new(),
        })
}

async fn protocol_headers(mut response: Response) -> Response {
    response.headers_mut().insert(
        "docker-distribution-api-version",
        HeaderValue::from_static("registry/2.0"),
    );
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn challenge(state: &RegistryState, image: &str, action: Actions) -> String {
    state
        .auth
        .challenge(image, action, state.repositories.public_url().as_str())
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    let mut parts = headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .split_ascii_whitespace();
    if !parts.next()?.eq_ignore_ascii_case("Bearer") {
        return None;
    }
    let value = parts.next()?;
    parts.next().is_none().then_some(value)
}

async fn ping(
    State(state): State<RegistryState>,
    method: Method,
    headers: HeaderMap,
) -> Result<Response, RegistryError> {
    if method != Method::GET && method != Method::HEAD {
        return Err(RegistryError::method_not_allowed("Use GET or HEAD."));
    }
    let value = bearer(&headers)
        .ok_or_else(|| RegistryError::challenge(challenge(&state, "", Actions::PULL)))?;
    state
        .auth
        .verify_root(&state.repositories, value)
        .await
        .map_err(|error| error.with_challenge(challenge(&state, "", Actions::PULL)))?;
    Ok(StatusCode::OK.into_response())
}

async fn token(
    State(state): State<RegistryState>,
    request: Request,
) -> Result<Response, RegistryError> {
    if request.method() != Method::GET {
        return Err(RegistryError::method_not_allowed(
            "Use GET for token exchange.",
        ));
    }
    let raw = request.uri().query().unwrap_or("");
    if raw.len() > 16 * 1024 {
        return Err(RegistryError::bad_request("Token query is too large."));
    }
    let mut query = TokenQuery {
        service: None,
        scopes: Vec::new(),
        account: None,
    };
    for (key, value) in url::form_urlencoded::parse(raw.as_bytes()) {
        match key.as_ref() {
            "service" => query.service = Some(value.into_owned()),
            "scope" => query.scopes.push(value.into_owned()),
            "account" => query.account = Some(value.into_owned()),
            _ => {}
        }
    }
    state
        .auth
        .issue(&state.repositories, request.headers(), &query)
        .await
}

async fn dispatch(
    State(state): State<RegistryState>,
    path: Result<Path<String>, PathRejection>,
    request: Request,
) -> Result<Response, RegistryError> {
    let Path(path) =
        path.map_err(|_| RegistryError::bad_request("Invalid registry path encoding."))?;
    let resource = parse_resource(&path)?;
    let (parts, body) = request.into_parts();
    let query = parts.uri.query().unwrap_or("");
    match resource {
        Resource::Catalog => catalog(&state, &parts.method, &parts.headers, query).await,
        Resource::Blob { image, digest } => {
            blob(&state, &parts.method, &parts.headers, image, digest).await
        }
        Resource::Manifest { image, reference } => {
            manifest(
                &state,
                &parts.method,
                &parts.headers,
                body,
                image,
                reference,
            )
            .await
        }
        Resource::Tags { image } => tags(&state, &parts.method, &parts.headers, query, image).await,
        Resource::Referrers { image, digest } => {
            referrers(&state, &parts.method, &parts.headers, query, image, digest).await
        }
        Resource::Upload { image, id } => {
            upload(
                &state,
                &parts.method,
                &parts.headers,
                body,
                query,
                image,
                id,
            )
            .await
        }
    }
}

fn parse_resource(path: &str) -> Result<Resource<'_>, RegistryError> {
    if path == "_catalog" {
        return Ok(Resource::Catalog);
    }
    if let Some(image) = path
        .strip_suffix("/blobs/uploads/")
        .or_else(|| path.strip_suffix("/blobs/uploads"))
    {
        parse_image_name(image)?;
        return Ok(Resource::Upload { image, id: None });
    }
    let (prefix, reference) = path
        .rsplit_once('/')
        .ok_or_else(|| RegistryError::not_found("Registry resource not found."))?;
    if let Some(image) = prefix.strip_suffix("/blobs/uploads") {
        parse_image_name(image)?;
        let id = Uuid::parse_str(reference)
            .map_err(|_| RegistryError::from(StoreError::UploadUnknown))?;
        return Ok(Resource::Upload {
            image,
            id: Some(id),
        });
    }
    for (suffix, kind) in [
        ("/blobs", 0),
        ("/manifests", 1),
        ("/referrers", 2),
        ("/tags", 3),
    ] {
        let Some(image) = prefix.strip_suffix(suffix) else {
            continue;
        };
        parse_image_name(image)?;
        return match kind {
            0 => {
                validate_digest(reference)?;
                Ok(Resource::Blob {
                    image,
                    digest: reference,
                })
            }
            1 => {
                validate_reference(reference)?;
                Ok(Resource::Manifest { image, reference })
            }
            2 => {
                validate_digest(reference)?;
                Ok(Resource::Referrers {
                    image,
                    digest: reference,
                })
            }
            _ if reference == "list" => Ok(Resource::Tags { image }),
            _ => Err(RegistryError::not_found("Registry resource not found.")),
        };
    }
    Err(RegistryError::not_found("Registry resource not found."))
}

fn validate_digest(value: &str) -> Result<(), RegistryError> {
    if value.strip_prefix("sha256:").is_some_and(|hash| {
        hash.len() == 64
            && hash
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    }) {
        Ok(())
    } else {
        Err(StoreError::DigestInvalid.into())
    }
}

fn validate_reference(value: &str) -> Result<(), RegistryError> {
    if value.contains(':') {
        return validate_digest(value);
    }
    if (1..=128).contains(&value.len())
        && value
            .as_bytes()
            .first()
            .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_')
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
    {
        Ok(())
    } else {
        Err(RegistryError::bad_request("Invalid manifest reference."))
    }
}

fn request_action(method: &Method) -> Actions {
    match *method {
        Method::GET | Method::HEAD => Actions::PULL,
        Method::DELETE => Actions::DELETE,
        _ => Actions::PUSH,
    }
}

async fn context(
    state: &RegistryState,
    image: &str,
    action: Actions,
) -> Result<(repository::Model, ImageStore), RegistryError> {
    let name = parse_image_name(image)?;
    let repository = state
        .repositories
        .find(name.namespace, name.repository)
        .await
        .map_err(|error| {
            if error.into_response().status().is_server_error() {
                RegistryError::internal()
            } else {
                RegistryError::challenge(challenge(state, image, action))
            }
        })?;
    let store = state
        .store
        .image(state.repositories.repository_path(&repository), name.suffix);
    Ok((repository, store))
}

async fn authorize(
    state: &RegistryState,
    headers: &HeaderMap,
    repository: &repository::Model,
    image: &str,
    action: Actions,
) -> Result<Option<Uuid>, RegistryError> {
    if !headers.contains_key(header::AUTHORIZATION) && action == Actions::PULL {
        state
            .repositories
            .authorize(repository, None, Permission::Read)
            .await
            .map_err(|_| RegistryError::challenge(challenge(state, image, action)))?;
        return Ok(None);
    }
    let token =
        bearer(headers).ok_or_else(|| RegistryError::challenge(challenge(state, image, action)))?;
    state
        .auth
        .verify(&state.repositories, token, image, action)
        .await
        .map_err(|error| error.with_challenge(challenge(state, image, action)))
}

fn missing(code: &'static str, message: &str) -> RegistryError {
    RegistryError::Status(StatusCode::NOT_FOUND, code, message.to_owned())
}

async fn blob(
    state: &RegistryState,
    method: &Method,
    headers: &HeaderMap,
    image: &str,
    digest: &str,
) -> Result<Response, RegistryError> {
    let (repository, store) = context(state, image, request_action(method)).await?;
    match *method {
        Method::GET | Method::HEAD => {
            authorize(state, headers, &repository, image, Actions::PULL).await?;
            if method == Method::HEAD {
                let size = store
                    .blob_size(digest)
                    .await?
                    .ok_or_else(|| missing("BLOB_UNKNOWN", "Blob not found."))?;
                return Ok(Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_LENGTH, size.to_string())
                    .header(header::CONTENT_TYPE, "application/octet-stream")
                    .header("docker-content-digest", digest)
                    .body(Body::empty())?);
            }
            let (file, size) = store
                .open_blob(digest)
                .await?
                .ok_or_else(|| missing("BLOB_UNKNOWN", "Blob not found."))?;
            serve_blob(file, size, digest, headers).await
        }
        Method::DELETE => {
            authorize(state, headers, &repository, image, Actions::DELETE).await?;
            if !store.delete_blob(digest).await? {
                return Err(missing("BLOB_UNKNOWN", "Blob not found."));
            }
            Ok(StatusCode::ACCEPTED.into_response())
        }
        _ => Err(RegistryError::method_not_allowed(
            "Unsupported blob method.",
        )),
    }
}

fn byte_range(value: &str, size: u64) -> Result<(u64, u64), RegistryError> {
    let range = value
        .strip_prefix("bytes=")
        .ok_or_else(|| RegistryError::range("Invalid range unit."))?;
    if size == 0 || range.contains(',') {
        return Err(RegistryError::range("Range is not satisfiable."));
    }
    let (start, end) = range
        .split_once('-')
        .ok_or_else(|| RegistryError::range("Invalid byte range."))?;
    let (start, end) = if start.is_empty() {
        let suffix = decimal(end)
            .filter(|size| *size > 0)
            .ok_or_else(|| RegistryError::range("Invalid suffix range."))?;
        (size.saturating_sub(suffix), size - 1)
    } else {
        let start = decimal(start).ok_or_else(|| RegistryError::range("Invalid range start."))?;
        let end = if end.is_empty() {
            size - 1
        } else {
            decimal(end)
                .ok_or_else(|| RegistryError::range("Invalid range end."))?
                .min(size - 1)
        };
        (start, end)
    };
    if start >= size || start > end {
        return Err(RegistryError::range("Range is outside the blob."));
    }
    Ok((start, end))
}

async fn serve_blob(
    mut file: File,
    size: u64,
    digest: &str,
    headers: &HeaderMap,
) -> Result<Response, RegistryError> {
    let range = match headers.get(header::RANGE) {
        None => None,
        Some(value) => match value
            .to_str()
            .ok()
            .and_then(|value| byte_range(value, size).ok())
        {
            Some(range) => Some(range),
            None => {
                let mut response =
                    RegistryError::range("Range is not satisfiable.").into_response();
                response.headers_mut().insert(
                    header::CONTENT_RANGE,
                    HeaderValue::from_str(&format!("bytes */{size}"))?,
                );
                return Ok(response);
            }
        },
    };
    let (start, length) = range.map_or((0, size), |(start, end)| (start, end - start + 1));
    if start != 0 {
        file.seek(SeekFrom::Start(start))
            .await
            .map_err(StoreError::from)?;
    }
    let mut response = Response::builder()
        .status(if range.is_some() {
            StatusCode::PARTIAL_CONTENT
        } else {
            StatusCode::OK
        })
        .header(header::CONTENT_LENGTH, length.to_string())
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header(header::ACCEPT_RANGES, "bytes")
        .header("docker-content-digest", digest);
    if let Some((start, end)) = range {
        response = response.header(header::CONTENT_RANGE, format!("bytes {start}-{end}/{size}"));
    }
    Ok(response.body(Body::from_stream(ReaderStream::new(file.take(length))))?)
}

async fn manifest(
    state: &RegistryState,
    method: &Method,
    headers: &HeaderMap,
    body: Body,
    image: &str,
    reference: &str,
) -> Result<Response, RegistryError> {
    let (repository, store) = context(state, image, request_action(method)).await?;
    match *method {
        Method::GET | Method::HEAD => {
            authorize(state, headers, &repository, image, Actions::PULL).await?;
            let manifest = store
                .get_manifest(reference)
                .await?
                .ok_or_else(|| missing("MANIFEST_UNKNOWN", "Manifest not found."))?;
            manifest_response(manifest, method == Method::HEAD)
        }
        Method::PUT => {
            authorize(state, headers, &repository, image, Actions::PUSH).await?;
            if content_length(headers)?.is_some_and(|length| length > MAX_MANIFEST_BYTES as u64) {
                return Err(StoreError::TooLarge.into());
            }
            let media_type = content_type(headers)?
                .ok_or_else(|| RegistryError::bad_request("Manifest Content-Type is required."))?;
            let bytes = to_bytes(body, MAX_MANIFEST_BYTES + 1)
                .await
                .map_err(|_| RegistryError::from(StoreError::TooLarge))?;
            if bytes.len() > MAX_MANIFEST_BYTES {
                return Err(StoreError::TooLarge.into());
            }
            let stored = store
                .put_manifest(reference, media_type, &bytes)
                .await
                .map_err(|error| match error {
                    StoreError::Invalid(message) => {
                        RegistryError::Status(StatusCode::BAD_REQUEST, "MANIFEST_INVALID", message)
                    }
                    other => other.into(),
                })?;
            let mut response = committed(image, "manifests", &stored.digest)?;
            if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&stored.bytes)
                && let Some(subject) = value
                    .get("subject")
                    .and_then(|subject| subject.get("digest"))
                    .and_then(serde_json::Value::as_str)
            {
                response
                    .headers_mut()
                    .insert("oci-subject", HeaderValue::from_str(subject)?);
            }
            Ok(response)
        }
        Method::DELETE => {
            authorize(state, headers, &repository, image, Actions::DELETE).await?;
            if !store.delete_manifest(reference).await? {
                return Err(missing("MANIFEST_UNKNOWN", "Manifest not found."));
            }
            Ok(StatusCode::ACCEPTED.into_response())
        }
        _ => Err(RegistryError::method_not_allowed(
            "Unsupported manifest method.",
        )),
    }
}

fn manifest_response(manifest: StoredManifest, head: bool) -> Result<Response, RegistryError> {
    let length = manifest.bytes.len();
    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, manifest.media_type)
        .header(header::CONTENT_LENGTH, length.to_string())
        .header("docker-content-digest", manifest.digest)
        .body(if head {
            Body::empty()
        } else {
            Body::from(manifest.bytes)
        })?)
}

fn committed(image: &str, kind: &str, digest: &str) -> Result<Response, RegistryError> {
    Ok(Response::builder()
        .status(StatusCode::CREATED)
        .header(header::LOCATION, format!("/v2/{image}/{kind}/{digest}"))
        .header("docker-content-digest", digest)
        .header(header::CONTENT_LENGTH, "0")
        .body(Body::empty())?)
}

async fn upload(
    state: &RegistryState,
    method: &Method,
    headers: &HeaderMap,
    body: Body,
    query: &str,
    image: &str,
    id: Option<Uuid>,
) -> Result<Response, RegistryError> {
    let (repository, store) = context(state, image, Actions::PUSH).await?;
    let owner = authorize(state, headers, &repository, image, Actions::PUSH)
        .await?
        .ok_or_else(|| RegistryError::challenge(challenge(state, image, Actions::PUSH)))?;
    let parameters = parameters(query)?;
    match (method, id) {
        (&Method::POST, None) => {
            let digest = parameter(&parameters, "digest");
            if let Some(digest) = digest {
                validate_digest(digest)?;
            }
            if let Some(digest) = parameter(&parameters, "mount") {
                validate_digest(digest)?;
                if let Some(source) = parameter(&parameters, "from")
                    && let Ok((source_repository, source_store)) =
                        context(state, source, Actions::PULL).await
                    && authorize(state, headers, &source_repository, source, Actions::PULL)
                        .await
                        .is_ok()
                    && store.mount_blob(&source_store, digest).await?
                {
                    return committed(image, "blobs", digest);
                }
            }
            check_blob_body(headers)?;
            let id = store.start_upload(owner).await?;
            let offset = match store.append_upload(id, owner, body, None).await {
                Ok(offset) => offset,
                Err(error) => {
                    let _ = store.cancel_upload(id, owner).await;
                    return Err(error.into());
                }
            };
            if let Some(digest) = digest {
                match store.finish_upload(id, owner, digest).await {
                    Ok(_) => committed(image, "blobs", digest),
                    Err(error) => {
                        let _ = store.cancel_upload(id, owner).await;
                        Err(error.into())
                    }
                }
            } else {
                upload_response(StatusCode::ACCEPTED, id, offset, image)
            }
        }
        (&Method::GET | &Method::HEAD, Some(id)) => {
            let offset = store
                .upload_status(id, owner)
                .await?
                .ok_or(StoreError::UploadUnknown)?;
            upload_response(StatusCode::NO_CONTENT, id, offset, image)
        }
        (&Method::PATCH, Some(id)) => {
            check_blob_body(headers)?;
            let range = upload_range(headers)?;
            match store.append_upload(id, owner, body, range).await {
                Ok(offset) => upload_response(StatusCode::ACCEPTED, id, offset, image),
                Err(StoreError::RangeInvalid) => upload_range_error(&store, id, owner, image).await,
                Err(error) => Err(error.into()),
            }
        }
        (&Method::PUT, Some(id)) => {
            let digest = parameter(&parameters, "digest")
                .ok_or_else(|| RegistryError::bad_request("Digest is required."))?;
            validate_digest(digest)?;
            check_blob_body(headers)?;
            let range = upload_range(headers)?;
            if let Err(error) = store.append_upload(id, owner, body, range).await {
                return match error {
                    StoreError::RangeInvalid => upload_range_error(&store, id, owner, image).await,
                    other => Err(other.into()),
                };
            }
            store.finish_upload(id, owner, digest).await?;
            committed(image, "blobs", digest)
        }
        (&Method::DELETE, Some(id)) => {
            if !store.cancel_upload(id, owner).await? {
                return Err(StoreError::UploadUnknown.into());
            }
            Ok(StatusCode::NO_CONTENT.into_response())
        }
        _ => Err(RegistryError::method_not_allowed(
            "Unsupported upload method.",
        )),
    }
}

fn upload_response(
    status: StatusCode,
    id: Uuid,
    offset: u64,
    image: &str,
) -> Result<Response, RegistryError> {
    Ok(Response::builder()
        .status(status)
        .header(header::LOCATION, format!("/v2/{image}/blobs/uploads/{id}"))
        .header("docker-upload-uuid", id.to_string())
        .header(header::RANGE, format!("0-{}", offset.saturating_sub(1)))
        .header(header::CONTENT_LENGTH, "0")
        .body(Body::empty())?)
}

async fn upload_range_error(
    store: &ImageStore,
    id: Uuid,
    owner: Uuid,
    image: &str,
) -> Result<Response, RegistryError> {
    let offset = store
        .upload_status(id, owner)
        .await?
        .ok_or(StoreError::UploadUnknown)?;
    let mut response = RegistryError::from(StoreError::RangeInvalid).into_response();
    let position = upload_response(StatusCode::RANGE_NOT_SATISFIABLE, id, offset, image)?;
    for name in [
        header::LOCATION,
        header::RANGE,
        header::HeaderName::from_static("docker-upload-uuid"),
    ] {
        if let Some(value) = position.headers().get(&name) {
            response.headers_mut().insert(name, value.clone());
        }
    }
    Ok(response)
}

fn decimal(value: &str) -> Option<u64> {
    (!value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()))
        .then(|| value.parse().ok())
        .flatten()
}

fn content_length(headers: &HeaderMap) -> Result<Option<u64>, RegistryError> {
    headers
        .get(header::CONTENT_LENGTH)
        .map(|value| {
            value
                .to_str()
                .ok()
                .and_then(decimal)
                .ok_or_else(|| RegistryError::bad_request("Invalid Content-Length."))
        })
        .transpose()
}

fn content_type(headers: &HeaderMap) -> Result<Option<&str>, RegistryError> {
    headers
        .get(header::CONTENT_TYPE)
        .map(|value| {
            value
                .to_str()
                .map(|value| value.split(';').next().unwrap_or("").trim())
                .map_err(|_| RegistryError::bad_request("Invalid Content-Type."))
        })
        .transpose()
}

fn check_blob_body(headers: &HeaderMap) -> Result<(), RegistryError> {
    if content_length(headers)?.is_some_and(|length| length > MAX_BLOB_BYTES) {
        return Err(StoreError::TooLarge.into());
    }
    if content_type(headers)?.is_some_and(|value| value != "application/octet-stream") {
        return Err(RegistryError::Status(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "UNSUPPORTED",
            "Blob uploads use application/octet-stream.".to_owned(),
        ));
    }
    Ok(())
}

fn upload_range(headers: &HeaderMap) -> Result<Option<(u64, u64)>, RegistryError> {
    headers
        .get(header::CONTENT_RANGE)
        .map(|value| {
            let text = value.to_str().map_err(|_| StoreError::RangeInvalid)?;
            let (start, end) = text.split_once('-').ok_or(StoreError::RangeInvalid)?;
            let start = decimal(start).ok_or(StoreError::RangeInvalid)?;
            let end = decimal(end).ok_or(StoreError::RangeInvalid)?;
            if end < start {
                return Err(StoreError::RangeInvalid.into());
            }
            Ok((start, end))
        })
        .transpose()
}

type QueryPair<'a> = (std::borrow::Cow<'a, str>, std::borrow::Cow<'a, str>);

fn parameters(query: &str) -> Result<Vec<QueryPair<'_>>, RegistryError> {
    if query.len() > 16 * 1024 {
        return Err(RegistryError::bad_request("Query is too large."));
    }
    let parameters: Vec<_> = url::form_urlencoded::parse(query.as_bytes()).collect();
    if parameters.len() > 32 {
        return Err(RegistryError::bad_request("Too many query parameters."));
    }
    Ok(parameters)
}

fn parameter<'a>(parameters: &'a [QueryPair<'_>], name: &str) -> Option<&'a str> {
    parameters
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_ref())
}

fn page_size(parameters: &[QueryPair<'_>]) -> Result<usize, RegistryError> {
    match parameter(parameters, "n") {
        None => Ok(MAX_PAGE_SIZE),
        Some(value) => decimal(value)
            .filter(|value| *value > 0)
            .map(|value| value.min(MAX_PAGE_SIZE as u64) as usize)
            .ok_or_else(|| RegistryError::bad_request("Page size must be a positive integer.")),
    }
}

fn next_link(
    response: &mut Response,
    path: &str,
    size: usize,
    last: &str,
) -> Result<(), RegistryError> {
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("n", &size.to_string())
        .append_pair("last", last)
        .finish();
    response.headers_mut().insert(
        header::LINK,
        HeaderValue::from_str(&format!("<{path}?{query}>; rel=\"next\""))?,
    );
    Ok(())
}

async fn tags(
    state: &RegistryState,
    method: &Method,
    headers: &HeaderMap,
    query: &str,
    image: &str,
) -> Result<Response, RegistryError> {
    if method != Method::GET {
        return Err(RegistryError::method_not_allowed("Use GET."));
    }
    let (repository, store) = context(state, image, Actions::PULL).await?;
    authorize(state, headers, &repository, image, Actions::PULL).await?;
    let parameters = parameters(query)?;
    let size = page_size(&parameters)?;
    let last = parameter(&parameters, "last").unwrap_or("");
    let mut tags = store.tags().await?;
    tags.retain(|tag| tag.as_str() > last);
    let more = tags.len() > size;
    tags.truncate(size);
    let mut response = Json(serde_json::json!({"name": image, "tags": tags})).into_response();
    if more && let Some(last) = tags.last() {
        next_link(&mut response, &format!("/v2/{image}/tags/list"), size, last)?;
    }
    Ok(response)
}

async fn catalog(
    state: &RegistryState,
    method: &Method,
    headers: &HeaderMap,
    query: &str,
) -> Result<Response, RegistryError> {
    if method != Method::GET {
        return Err(RegistryError::method_not_allowed("Use GET."));
    }
    let actor = if headers.contains_key(header::AUTHORIZATION) {
        let token = bearer(headers)
            .ok_or_else(|| RegistryError::challenge(challenge(state, "_catalog", Actions::PULL)))?;
        state
            .auth
            .verify(&state.repositories, token, "_catalog", Actions::PULL)
            .await
            .map_err(|error| error.with_challenge(challenge(state, "_catalog", Actions::PULL)))?
    } else {
        None
    };
    let parameters = parameters(query)?;
    let size = page_size(&parameters)?;
    let last = parameter(&parameters, "last").unwrap_or("");
    let mut names = BTreeSet::new();
    let mut pages = repository::Entity::find()
        .filter(repository::Column::DeletedAt.is_null())
        .order_by_asc(repository::Column::Id)
        .paginate(state.repositories.identity().database(), 128);
    while let Some(repositories) = pages.fetch_and_next().await.map_err(|error| {
        tracing::error!(%error, "Registry catalog query failed");
        RegistryError::internal()
    })? {
        for repository in repositories {
            if !state
                .repositories
                .can_access(&repository, actor, Permission::Read)
                .await
                .map_err(|_| RegistryError::internal())?
            {
                continue;
            }
            for suffix in state
                .store
                .list_images(&state.repositories.repository_path(&repository))
                .await?
            {
                let name = if suffix.is_empty() {
                    format!("{}/{}", repository.namespace, repository.name)
                } else {
                    format!("{}/{}/{}", repository.namespace, repository.name, suffix)
                };
                if name.as_str() > last && parse_image_name(&name).is_ok() {
                    names.insert(name);
                    if names.len() > size + 1 {
                        names.pop_last();
                    }
                }
            }
        }
    }
    let more = names.len() > size;
    if more {
        names.pop_last();
    }
    let names: Vec<_> = names.into_iter().collect();
    let mut response = Json(serde_json::json!({"repositories": names})).into_response();
    if more && let Some(last) = names.last() {
        next_link(&mut response, "/v2/_catalog", size, last)?;
    }
    Ok(response)
}

async fn referrers(
    state: &RegistryState,
    method: &Method,
    headers: &HeaderMap,
    query: &str,
    image: &str,
    digest: &str,
) -> Result<Response, RegistryError> {
    if method != Method::GET {
        return Err(RegistryError::method_not_allowed("Use GET."));
    }
    let (repository, store) = context(state, image, Actions::PULL).await?;
    authorize(state, headers, &repository, image, Actions::PULL).await?;
    let parameters = parameters(query)?;
    let artifact_type = parameter(&parameters, "artifactType");
    let descriptors = store.referrers(digest, artifact_type).await?;
    let mut response = Json(
        serde_json::json!({"schemaVersion": 2, "mediaType": OCI_INDEX, "manifests": descriptors}),
    )
    .into_response();
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(OCI_INDEX));
    if artifact_type.is_some() {
        response.headers_mut().insert(
            "oci-filters-applied",
            HeaderValue::from_static("artifactType"),
        );
    }
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_words_are_valid_tags_and_image_suffixes() {
        assert!(matches!(
            parse_resource("owner/repo/manifests/manifests"),
            Ok(Resource::Manifest {
                image: "owner/repo",
                reference: "manifests"
            })
        ));
        assert!(matches!(
            parse_resource("owner/repo/blobs/manifests/latest"),
            Ok(Resource::Manifest {
                image: "owner/repo/blobs",
                reference: "latest"
            })
        ));
    }

    #[test]
    fn ranged_reads_handle_empty_and_suffix_boundaries() {
        assert!(byte_range("bytes=0-0", 0).is_err());
        assert!(byte_range("bytes=4-", 4).is_err());
        assert_eq!(byte_range("bytes=-10", 4).unwrap(), (0, 3));
        assert_eq!(byte_range("bytes=2-99", 4).unwrap(), (2, 3));
    }

    #[test]
    fn malformed_upload_ranges_are_not_treated_as_unranged() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_RANGE,
            HeaderValue::from_static("bytes 0-3/4"),
        );
        assert!(upload_range(&headers).is_err());
    }
}
