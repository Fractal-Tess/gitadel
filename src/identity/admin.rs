use axum::{Json, extract::State, http::HeaderMap};
use axum_extra::extract::cookie::CookieJar;
use chrono::Utc;
use sea_orm::{ActiveModelTrait, EntityTrait, Set};
use serde::{Deserialize, Serialize};

use crate::entity::{audit_event, instance};

use super::{ApiError, IdentityState, SCOPE_READ, SCOPE_WRITE};

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
