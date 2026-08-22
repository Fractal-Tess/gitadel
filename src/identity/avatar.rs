use std::io::Cursor;

use axum::{
    Json,
    body::Body,
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::Response,
};
use axum_extra::extract::cookie::CookieJar;
use base64::{Engine, engine::general_purpose::STANDARD};
use chrono::Utc;
use sea_orm::{ActiveModelTrait, EntityTrait, Set, TransactionTrait};
use serde::Deserialize;
use uuid::Uuid;

use crate::entity::{user, user_avatar};

use super::{ApiError, IdentityState, SCOPE_WRITE};

pub const MAX_AVATAR_REQUEST_BYTES: usize = 6 * 1024 * 1024;
const MAX_AVATAR_BYTES: usize = 4 * 1024 * 1024;
const AVATAR_EDGE: u32 = 512;
const PNG_SIGNATURE: [u8; 8] = [137, 80, 78, 71, 13, 10, 26, 10];

pub async fn public_avatar(
    State(state): State<IdentityState>,
    Path(user_id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let avatar = user_avatar::Entity::find_by_id(user_id)
        .one(state.database())
        .await?
        .ok_or_else(ApiError::not_found)?;

    let mut response = Response::new(Body::from(avatar.content));
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
pub struct UpdateAvatarRequest {
    image_base64: String,
}

pub async fn update_avatar(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<UpdateAvatarRequest>,
) -> Result<StatusCode, ApiError> {
    let actor = state.authenticate(&headers, &jar, SCOPE_WRITE).await?;
    let actor_id = actor.user.id;
    let body = STANDARD
        .decode(request.image_base64)
        .map_err(|_| invalid_png())?;
    validate_avatar_png(&body)?;

    let transaction = state.database().begin().await?;
    match user_avatar::Entity::find_by_id(actor.user.id)
        .one(&transaction)
        .await?
    {
        Some(avatar) => {
            let mut active: user_avatar::ActiveModel = avatar.into();
            active.content = Set(body);
            active.update(&transaction).await?;
        }
        None => {
            user_avatar::ActiveModel {
                user_id: Set(actor.user.id),
                content: Set(body),
            }
            .insert(&transaction)
            .await?;
        }
    }

    let now = Utc::now();
    let mut account: user::ActiveModel = actor.user.into();
    account.avatar_updated_at = Set(Some(now));
    account.updated_at = Set(now);
    account.update(&transaction).await?;
    state
        .audit_on(&transaction, Some(actor_id), "account.avatar.update", None)
        .await?;
    transaction.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn delete_avatar(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<StatusCode, ApiError> {
    let actor = state.authenticate(&headers, &jar, SCOPE_WRITE).await?;
    let actor_id = actor.user.id;
    let transaction = state.database().begin().await?;
    user_avatar::Entity::delete_by_id(actor.user.id)
        .exec(&transaction)
        .await?;

    let now = Utc::now();
    let mut account: user::ActiveModel = actor.user.into();
    account.avatar_updated_at = Set(None);
    account.updated_at = Set(now);
    account.update(&transaction).await?;
    state
        .audit_on(&transaction, Some(actor_id), "account.avatar.delete", None)
        .await?;
    transaction.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

fn validate_avatar_png(bytes: &[u8]) -> Result<(), ApiError> {
    if bytes.len() > MAX_AVATAR_BYTES {
        return Err(ApiError::bad_request(
            "Profile pictures cannot exceed 4 MiB.",
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
    if width != AVATAR_EDGE || height != AVATAR_EDGE {
        return Err(ApiError::bad_request(
            "Profile pictures must be 512 by 512 pixels.",
        ));
    }

    let decoder = png::Decoder::new(Cursor::new(bytes));
    let mut reader = decoder.read_info().map_err(|_| invalid_png())?;
    let output_size = reader.output_buffer_size().ok_or_else(invalid_png)?;
    let mut output = vec![0; output_size];
    reader.next_frame(&mut output).map_err(|_| invalid_png())?;
    Ok(())
}

fn invalid_png() -> ApiError {
    ApiError::bad_request("The uploaded profile picture is not a valid PNG.")
}
