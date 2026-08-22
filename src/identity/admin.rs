use axum::{
    Json,
    body::{Body, Bytes},
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::Response,
};
use axum_extra::extract::cookie::CookieJar;
use chrono::Utc;
use sea_orm::{ActiveModelTrait, ConnectionTrait, EntityTrait, Set, TransactionTrait};
use serde::{Deserialize, Serialize};

use crate::entity::{audit_event, instance, instance_asset};

pub const MAX_FAVICON_BYTES: usize = 512 * 1024;
const MIN_FAVICON_EDGE: u32 = 16;
const MAX_FAVICON_EDGE: u32 = 1024;
const PNG_SIGNATURE: [u8; 8] = [137, 80, 78, 71, 13, 10, 26, 10];
const DEFAULT_LIGHT_FAVICON: &[u8] = include_bytes!("../../frontend/static/favicon-light.png");
const DEFAULT_DARK_FAVICON: &[u8] = include_bytes!("../../frontend/static/favicon-dark.png");

use super::{ApiError, IdentityState, SCOPE_READ, SCOPE_WRITE};

#[derive(Clone, Copy)]
enum FaviconTheme {
    Light,
    Dark,
}

impl TryFrom<&str> for FaviconTheme {
    type Error = ApiError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "light" => Ok(Self::Light),
            "dark" => Ok(Self::Dark),
            _ => Err(ApiError::not_found()),
        }
    }
}

#[derive(Serialize)]
pub struct InstanceSettingsResponse {
    site_name: String,
    site_description: Option<String>,
    default_repository_visibility: String,
    updated_at: chrono::DateTime<Utc>,
}

impl From<instance::Model> for InstanceSettingsResponse {
    fn from(settings: instance::Model) -> Self {
        Self {
            site_name: settings.site_name,
            site_description: settings.site_description,
            default_repository_visibility: settings.default_repository_visibility,
            updated_at: settings.updated_at,
        }
    }
}

#[derive(Deserialize)]
pub struct UpdateInstanceSettingsRequest {
    site_name: String,
    site_description: Option<String>,
    default_repository_visibility: String,
}

pub async fn public_instance_settings(
    State(state): State<IdentityState>,
) -> Result<Json<InstanceSettingsResponse>, ApiError> {
    let settings = instance::Entity::find_by_id(1)
        .one(state.database())
        .await?
        .ok_or_else(|| ApiError::internal("instance settings row is missing"))?;
    Ok(Json(settings.into()))
}

pub async fn get_instance_settings(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<InstanceSettingsResponse>, ApiError> {
    require_admin(&state, &headers, &jar, SCOPE_READ).await?;
    let settings = instance::Entity::find_by_id(1)
        .one(state.database())
        .await?
        .ok_or_else(|| ApiError::internal("instance settings row is missing"))?;
    Ok(Json(settings.into()))
}

pub async fn update_instance_settings(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<UpdateInstanceSettingsRequest>,
) -> Result<Json<InstanceSettingsResponse>, ApiError> {
    let actor = require_admin(&state, &headers, &jar, SCOPE_WRITE).await?;
    let site_name = request.site_name.trim();
    if site_name.is_empty() || site_name.len() > 80 {
        return Err(ApiError::bad_request(
            "Site name must contain between 1 and 80 characters.",
        ));
    }
    let site_description = request
        .site_description
        .map(|description| description.trim().to_owned())
        .filter(|description| !description.is_empty());
    if site_description
        .as_ref()
        .is_some_and(|description| description.len() > 280)
    {
        return Err(ApiError::bad_request(
            "Site description cannot exceed 280 characters.",
        ));
    }
    if !matches!(
        request.default_repository_visibility.as_str(),
        "public" | "private"
    ) {
        return Err(ApiError::bad_request(
            "Default repository visibility must be public or private.",
        ));
    }

    let settings = instance::Entity::find_by_id(1)
        .one(state.database())
        .await?
        .ok_or_else(|| ApiError::internal("instance settings row is missing"))?;
    let now = Utc::now();
    let mut active: instance::ActiveModel = settings.into();
    active.site_name = Set(site_name.to_owned());
    active.site_description = Set(site_description);
    active.default_repository_visibility = Set(request.default_repository_visibility);
    active.updated_at = Set(now);
    let settings = active.update(state.database()).await?;

    audit_event::ActiveModel {
        id: sea_orm::ActiveValue::NotSet,
        actor_user_id: Set(Some(actor.user.id)),
        action: Set("instance.settings.update".to_owned()),
        target: Set(None),
        remote_address: Set(None),
        created_at: Set(now),
    }
    .insert(state.database())
    .await?;

    Ok(Json(settings.into()))
}

pub async fn public_instance_favicon(
    State(state): State<IdentityState>,
    Path(theme): Path<String>,
) -> Result<Response, ApiError> {
    let (name, fallback) = favicon_asset(FaviconTheme::try_from(theme.as_str())?);
    let asset = instance_asset::Entity::find_by_id(name)
        .one(state.database())
        .await?;
    let (content_type, content) = asset
        .map(|asset| (asset.content_type, asset.content))
        .unwrap_or_else(|| ("image/png".to_owned(), fallback.to_vec()));

    let mut response = Response::new(Body::from(content));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(&content_type).unwrap_or(HeaderValue::from_static("image/png")),
    );
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-store, max-age=0"),
    );
    response.headers_mut().insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    Ok(response)
}

pub async fn update_instance_favicon(
    State(state): State<IdentityState>,
    Path(theme): Path<String>,
    headers: HeaderMap,
    jar: CookieJar,
    body: Bytes,
) -> Result<StatusCode, ApiError> {
    let actor = require_admin(&state, &headers, &jar, SCOPE_WRITE).await?;
    let (name, _) = favicon_asset(FaviconTheme::try_from(theme.as_str())?);
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    if content_type
        .split(';')
        .next()
        .is_none_or(|value| value.trim() != "image/png")
    {
        return Err(ApiError::bad_request("Favicons must be PNG images."));
    }
    validate_png(&body)?;

    let now = Utc::now();
    let transaction = state.database().begin().await?;
    match instance_asset::Entity::find_by_id(name)
        .one(&transaction)
        .await?
    {
        Some(asset) => {
            let mut active: instance_asset::ActiveModel = asset.into();
            active.content_type = Set("image/png".to_owned());
            active.content = Set(body.to_vec());
            active.updated_at = Set(now);
            active.update(&transaction).await?;
        }
        None => {
            instance_asset::ActiveModel {
                name: Set(name.to_owned()),
                content_type: Set("image/png".to_owned()),
                content: Set(body.to_vec()),
                updated_at: Set(now),
            }
            .insert(&transaction)
            .await?;
        }
    }
    touch_instance(&transaction, now).await?;
    state
        .audit_on(
            &transaction,
            Some(actor.user.id),
            "instance.favicon.update",
            Some(name.to_owned()),
        )
        .await?;
    transaction.commit().await?;

    Ok(StatusCode::NO_CONTENT)
}

pub async fn delete_instance_favicon(
    State(state): State<IdentityState>,
    Path(theme): Path<String>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<StatusCode, ApiError> {
    let actor = require_admin(&state, &headers, &jar, SCOPE_WRITE).await?;
    let (name, _) = favicon_asset(FaviconTheme::try_from(theme.as_str())?);
    let transaction = state.database().begin().await?;
    instance_asset::Entity::delete_by_id(name)
        .exec(&transaction)
        .await?;
    let now = Utc::now();
    touch_instance(&transaction, now).await?;
    state
        .audit_on(
            &transaction,
            Some(actor.user.id),
            "instance.favicon.delete",
            Some(name.to_owned()),
        )
        .await?;
    transaction.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

fn favicon_asset(theme: FaviconTheme) -> (&'static str, &'static [u8]) {
    match theme {
        FaviconTheme::Light => ("favicon-light", DEFAULT_LIGHT_FAVICON),
        FaviconTheme::Dark => ("favicon-dark", DEFAULT_DARK_FAVICON),
    }
}

fn validate_png(bytes: &[u8]) -> Result<(), ApiError> {
    if bytes.len() > MAX_FAVICON_BYTES {
        return Err(ApiError::bad_request(
            "Favicon files cannot exceed 512 KiB.",
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
    if width != height || !(MIN_FAVICON_EDGE..=MAX_FAVICON_EDGE).contains(&width) {
        return Err(ApiError::bad_request(
            "Favicons must be square and between 16 and 1024 pixels.",
        ));
    }

    let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    let mut reader = decoder.read_info().map_err(|_| invalid_png())?;
    let output_size = reader.output_buffer_size().ok_or_else(invalid_png)?;
    let mut output = vec![0; output_size];
    reader.next_frame(&mut output).map_err(|_| invalid_png())?;
    Ok(())
}

fn invalid_png() -> ApiError {
    ApiError::bad_request("The uploaded file is not a valid PNG.")
}

async fn touch_instance<C: ConnectionTrait>(
    connection: &C,
    now: chrono::DateTime<Utc>,
) -> Result<(), ApiError> {
    let settings = instance::Entity::find_by_id(1)
        .one(connection)
        .await?
        .ok_or_else(|| ApiError::internal("instance settings row is missing"))?;
    let mut active: instance::ActiveModel = settings.into();
    active.updated_at = Set(now);
    active.update(connection).await?;
    Ok(())
}

async fn require_admin(
    state: &IdentityState,
    headers: &HeaderMap,
    jar: &CookieJar,
    scope: i32,
) -> Result<super::AuthenticatedUser, ApiError> {
    let actor = state.authenticate(headers, jar, scope).await?;
    if !actor.user.is_admin {
        return Err(ApiError::forbidden("Administrator access is required."));
    }
    Ok(actor)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_favicons_are_valid_pngs() {
        validate_png(DEFAULT_LIGHT_FAVICON).expect("light favicon should be valid");
        validate_png(DEFAULT_DARK_FAVICON).expect("dark favicon should be valid");
    }

    #[test]
    fn favicon_validation_rejects_non_png_content() {
        let error = validate_png(b"not a png").expect_err("invalid content should fail");
        assert_eq!(error.code, "bad_request");
    }
}
