use std::io::Cursor;

use axum::{
    Json,
    body::Body,
    extract::{Path as AxumPath, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::Response,
};
use axum_extra::extract::cookie::CookieJar;
use base64::{Engine, engine::general_purpose::STANDARD};
use chrono::Utc;
use sea_orm::{ActiveModelTrait, EntityTrait, IntoActiveModel, Set, TransactionTrait};
use serde::Deserialize;

use super::{Permission, RepositoryState, browser::read_git};
use crate::{
    entity::{repository, repository_icon},
    identity::{ApiError, SCOPE_READ, SCOPE_WRITE},
};

pub const MAX_ICON_REQUEST_BYTES: usize = 6 * 1024 * 1024;
const MAX_ICON_BYTES: usize = 4 * 1024 * 1024;
const ICON_EDGE: u32 = 512;
const PNG_SIGNATURE: [u8; 8] = [137, 80, 78, 71, 13, 10, 26, 10];

pub const SOURCE_MANUAL: &str = "manual";
pub const SOURCE_DETECTED: &str = "detected";

// Only PNG is considered: the instance has no SVG rasterizer or ICO decoder, and
// serving an SVG straight from the repository would put attacker-authored markup
// on the app's own origin.
const CANDIDATE_PATHS: [&str; 12] = [
    "icon.png",
    "logo.png",
    "favicon.png",
    "static/favicon.png",
    "static/icon.png",
    "static/logo.png",
    "public/favicon.png",
    "public/icon.png",
    "public/logo.png",
    "assets/logo.png",
    "assets/icon.png",
    ".github/logo.png",
];

pub async fn public_icon(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name)): AxumPath<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Response, ApiError> {
    let repository = state.find(&namespace, &name).await?;
    let user_id = state
        .identity()
        .optional_user(&headers, &jar, SCOPE_READ)
        .await?
        .map(|account| account.id);
    state
        .authorize(&repository, user_id, Permission::Read)
        .await?;

    let icon = repository_icon::Entity::find_by_id(repository.id)
        .one(state.identity().database())
        .await?
        .ok_or_else(ApiError::not_found)?;

    let mut response = Response::new(Body::from(icon.content));
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static("image/png"));
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=0, must-revalidate"),
    );
    response.headers_mut().insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    Ok(response)
}

#[derive(Deserialize)]
pub struct UpdateIconRequest {
    image_base64: String,
}

pub async fn update_icon(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name)): AxumPath<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<UpdateIconRequest>,
) -> Result<StatusCode, ApiError> {
    let (actor, repository) = state
        .authenticated_repository(
            &headers,
            &jar,
            &namespace,
            &name,
            Permission::Manage,
            SCOPE_WRITE,
        )
        .await?;
    let body = STANDARD
        .decode(request.image_base64)
        .map_err(|_| invalid_png())?;
    validate_uploaded_png(&body)?;

    let transaction = state.identity().database().begin().await?;
    store_icon(&transaction, repository.id, body).await?;
    let now = Utc::now();
    let mut active = repository.into_active_model();
    active.icon_updated_at = Set(Some(now));
    active.icon_source = Set(Some(SOURCE_MANUAL.to_owned()));
    active.updated_at = Set(now);
    active.update(&transaction).await?;
    state
        .identity()
        .audit_on(
            &transaction,
            Some(actor.user.id),
            "repository.icon.update",
            Some(format!("{namespace}/{name}")),
        )
        .await?;
    transaction.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn delete_icon(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name)): AxumPath<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<StatusCode, ApiError> {
    let (actor, repository) = state
        .authenticated_repository(
            &headers,
            &jar,
            &namespace,
            &name,
            Permission::Manage,
            SCOPE_WRITE,
        )
        .await?;
    let repository_id = repository.id;

    let transaction = state.identity().database().begin().await?;
    repository_icon::Entity::delete_by_id(repository_id)
        .exec(&transaction)
        .await?;
    let now = Utc::now();
    let mut active = repository.into_active_model();
    active.icon_updated_at = Set(None);
    active.icon_source = Set(None);
    active.updated_at = Set(now);
    active.update(&transaction).await?;
    state
        .identity()
        .audit_on(
            &transaction,
            Some(actor.user.id),
            "repository.icon.delete",
            Some(format!("{namespace}/{name}")),
        )
        .await?;
    transaction.commit().await?;

    // Clearing an upload re-opens the repository to detection, so the icon in
    // the tree can take over rather than the row falling back to a monogram.
    state.queue_repository_analysis(repository_id).await;
    Ok(StatusCode::NO_CONTENT)
}

/// Adopts an icon committed to the default branch when the maintainer has not
/// uploaded one. Runs as part of the post-push analysis pass, so a repository
/// that ships a logo gets one without anybody visiting settings.
pub(super) async fn detect_icon(
    state: &RepositoryState,
    repository: &repository::Model,
) -> Result<(), ApiError> {
    if repository.icon_source.as_deref() == Some(SOURCE_MANUAL) {
        return Ok(());
    }

    let path = state.repository_path(repository);
    let revision = repository.default_branch.clone();
    let found = read_git(path, move |git| {
        for candidate in CANDIDATE_PATHS {
            let Ok(resolved) = git.resolve_path(&revision, candidate) else {
                continue;
            };
            let Ok(content) = git.blobs().read(resolved.oid) else {
                continue;
            };
            if content.len() <= MAX_ICON_BYTES && decode_png(&content).is_ok() {
                return Ok(Some(content));
            }
        }
        Ok(None)
    })
    .await?;

    match found {
        Some(content) => {
            let existing = repository_icon::Entity::find_by_id(repository.id)
                .one(state.identity().database())
                .await?;
            if existing.as_ref().map(|icon| &icon.content) == Some(&content) {
                return Ok(());
            }
            let transaction = state.identity().database().begin().await?;
            store_icon(&transaction, repository.id, content).await?;
            let mut active = repository.clone().into_active_model();
            active.icon_updated_at = Set(Some(Utc::now()));
            active.icon_source = Set(Some(SOURCE_DETECTED.to_owned()));
            active.update(&transaction).await?;
            transaction.commit().await?;
        }
        None if repository.icon_source.as_deref() == Some(SOURCE_DETECTED) => {
            // The file that produced the icon is gone from the branch, so the
            // repository should stop advertising it.
            let transaction = state.identity().database().begin().await?;
            repository_icon::Entity::delete_by_id(repository.id)
                .exec(&transaction)
                .await?;
            let mut active = repository.clone().into_active_model();
            active.icon_updated_at = Set(None);
            active.icon_source = Set(None);
            active.update(&transaction).await?;
            transaction.commit().await?;
        }
        None => {}
    }
    Ok(())
}

async fn store_icon<C>(
    connection: &C,
    repository_id: uuid::Uuid,
    content: Vec<u8>,
) -> Result<(), ApiError>
where
    C: sea_orm::ConnectionTrait,
{
    match repository_icon::Entity::find_by_id(repository_id)
        .one(connection)
        .await?
    {
        Some(icon) => {
            let mut active: repository_icon::ActiveModel = icon.into();
            active.content = Set(content);
            active.update(connection).await?;
        }
        None => {
            repository_icon::ActiveModel {
                repository_id: Set(repository_id),
                content: Set(content),
            }
            .insert(connection)
            .await?;
        }
    }
    Ok(())
}

// Uploads come from the crop dialog, which always emits a square PNG, so they
// are held to the same fixed edge as profile pictures. Detected icons skip this
// and keep whatever the repository committed.
fn validate_uploaded_png(bytes: &[u8]) -> Result<(), ApiError> {
    if bytes.len() > MAX_ICON_BYTES {
        return Err(ApiError::bad_request(
            "Repository icons cannot exceed 4 MiB.",
        ));
    }
    if bytes.len() < 24
        || !bytes.starts_with(&PNG_SIGNATURE)
        || bytes.get(12..16) != Some(b"IHDR".as_slice())
    {
        return Err(invalid_png());
    }
    let width = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
    let height = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
    if width != ICON_EDGE || height != ICON_EDGE {
        return Err(ApiError::bad_request(
            "Repository icons must be 512 by 512 pixels.",
        ));
    }
    decode_png(bytes)
}

fn decode_png(bytes: &[u8]) -> Result<(), ApiError> {
    if !bytes.starts_with(&PNG_SIGNATURE) {
        return Err(invalid_png());
    }
    let decoder = png::Decoder::new(Cursor::new(bytes));
    let mut reader = decoder.read_info().map_err(|_| invalid_png())?;
    let output_size = reader.output_buffer_size().ok_or_else(invalid_png)?;
    let mut output = vec![0; output_size];
    reader.next_frame(&mut output).map_err(|_| invalid_png())?;
    Ok(())
}

fn invalid_png() -> ApiError {
    ApiError::bad_request("The uploaded repository icon is not a valid PNG.")
}
