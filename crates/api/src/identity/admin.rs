use std::{convert::Infallible, path::PathBuf, time::Duration};

use axum::{
    Json,
    body::{Body, Bytes},
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{
        Response,
        sse::{Event, Sse},
    },
};
use axum_extra::extract::cookie::CookieJar;
use chrono::{DateTime, Utc};
use futures_util::stream;
#[cfg(test)]
use sea_orm::{ActiveModelTrait, EntityTrait};
use serde::{Deserialize, Serialize};
use tokio_util::io::ReaderStream;
use url::Url;
use uuid::Uuid;

use crate::{
    archive::{self},
    backup_provider::BackupProviderKind,
    blob_store::targets::{MeasuredUsage, StorageTargetView},
    entity::instance,
};

pub const MAX_FAVICON_BYTES: usize = 512 * 1024;
const DEFAULT_LIGHT_FAVICON: &[u8] =
    include_bytes!("../../../../frontend/static/favicon-light.png");
const DEFAULT_DARK_FAVICON: &[u8] = include_bytes!("../../../../frontend/static/favicon-dark.png");

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

#[derive(Serialize, utoipa::ToSchema)]
pub struct InstanceSettingsResponse {
    site_name: String,
    site_description: Option<String>,
    updated_at: chrono::DateTime<Utc>,
}

impl From<instance::Model> for InstanceSettingsResponse {
    fn from(settings: instance::Model) -> Self {
        Self {
            site_name: settings.site_name,
            site_description: settings.site_description,
            updated_at: settings.updated_at,
        }
    }
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct UpdateInstanceSettingsRequest {
    site_name: String,
    site_description: Option<String>,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct IntegritySettingsResponse {
    enabled: bool,
    schedule: String,
    last_checked_at: Option<DateTime<Utc>>,
    last_result: Option<String>,
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct UpdateIntegritySettingsRequest {
    enabled: bool,
    schedule: String,
}

#[utoipa::path(
    get, path = "/admin/integrity",
    operation_id = "get_admin_integrity",
    summary = "Get integrity settings",
    tag = "Administration",
    responses(
        (status = 200, description = "Success", body = IntegritySettingsResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn get_integrity_settings(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<IntegritySettingsResponse>, ApiError> {
    let actor = require_admin_principal(&state, &headers, &jar, SCOPE_READ).await?;
    Ok(Json(
        instance_service::get_integrity_settings(&state.instance_service(), &actor)
            .await?
            .into(),
    ))
}

#[utoipa::path(
    put, path = "/admin/integrity",
    operation_id = "put_admin_integrity",
    summary = "Update integrity settings",
    tag = "Administration",

    responses(
        (status = 200, description = "Success", body = IntegritySettingsResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn update_integrity_settings(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<UpdateIntegritySettingsRequest>,
) -> Result<Json<IntegritySettingsResponse>, ApiError> {
    let actor = require_admin_principal(&state, &headers, &jar, SCOPE_WRITE).await?;
    Ok(Json(
        instance_service::update_integrity_settings(
            &state.instance_service(),
            &actor,
            instance_service::UpdateIntegritySettingsRequest {
                enabled: request.enabled,
                schedule: request.schedule,
            },
        )
        .await?
        .into(),
    ))
}

#[utoipa::path(
    get, path = "/instance",
    operation_id = "get_instance",
    summary = "Public instance settings",
    tag = "Instance",
    security(),
    responses(
        (status = 200, description = "Success", body = InstanceSettingsResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn public_instance_settings(
    State(state): State<IdentityState>,
) -> Result<Json<InstanceSettingsResponse>, ApiError> {
    Ok(Json(
        instance_service::public_instance_settings(&state.instance_service())
            .await?
            .into(),
    ))
}

#[utoipa::path(
    get, path = "/admin/instance",
    operation_id = "get_admin_instance",
    summary = "Get instance settings",
    tag = "Administration",
    responses(
        (status = 200, description = "Success", body = InstanceSettingsResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn get_instance_settings(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<InstanceSettingsResponse>, ApiError> {
    let actor = require_admin_principal(&state, &headers, &jar, SCOPE_READ).await?;
    Ok(Json(
        instance_service::get_instance_settings(&state.instance_service(), &actor)
            .await?
            .into(),
    ))
}

#[utoipa::path(
    put, path = "/admin/instance",
    operation_id = "put_admin_instance",
    summary = "Update instance settings",
    tag = "Administration",

    responses(
        (status = 200, description = "Success", body = InstanceSettingsResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn update_instance_settings(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<UpdateInstanceSettingsRequest>,
) -> Result<Json<InstanceSettingsResponse>, ApiError> {
    let actor = require_admin_principal(&state, &headers, &jar, SCOPE_WRITE).await?;
    Ok(Json(
        instance_service::update_instance_settings(
            &state.instance_service(),
            &actor,
            instance_service::UpdateInstanceSettingsRequest {
                site_name: request.site_name,
                site_description: request.site_description,
            },
        )
        .await?
        .into(),
    ))
}

#[utoipa::path(
    get, path = "/instance/favicon/{theme}",
    operation_id = "get_instance_favicon_theme",
    summary = "Public instance favicon",
    tag = "Instance",
    params(
        ("theme" = String, Path),
    ),
    security(),
    responses(
        (status = 200, description = "Success", body = crate::openapi::Binary, content_type = "image/png"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn public_instance_favicon(
    State(state): State<IdentityState>,
    Path(theme): Path<String>,
) -> Result<Response, ApiError> {
    let (_name, fallback) = favicon_asset(FaviconTheme::try_from(theme.as_str())?);
    let asset = instance_service::favicon(state.database(), &theme).await?;
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

#[utoipa::path(
    put, path = "/admin/instance/favicon/{theme}",
    operation_id = "put_admin_instance_favicon_theme",
    summary = "Update instance favicon",
    tag = "Administration",
    request_body(content = crate::openapi::Binary, content_type = "application/octet-stream"),
    params(
        ("theme" = String, Path),
    ),
    responses(
        (status = 204, description = "Success"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn update_instance_favicon(
    State(state): State<IdentityState>,
    Path(theme): Path<String>,
    headers: HeaderMap,
    jar: CookieJar,
    body: Bytes,
) -> Result<StatusCode, ApiError> {
    let actor = require_admin_principal(&state, &headers, &jar, SCOPE_WRITE).await?;
    instance_service::update_instance_favicon(
        &state.instance_service(),
        &actor,
        theme,
        headers
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default(),
        &body,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    delete, path = "/admin/instance/favicon/{theme}",
    operation_id = "delete_admin_instance_favicon_theme",
    summary = "Delete instance favicon",
    tag = "Administration",
    params(
        ("theme" = String, Path),
    ),
    responses(
        (status = 204, description = "Success"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn delete_instance_favicon(
    State(state): State<IdentityState>,
    Path(theme): Path<String>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<StatusCode, ApiError> {
    let actor = require_admin_principal(&state, &headers, &jar, SCOPE_WRITE).await?;
    instance_service::delete_instance_favicon(&state.instance_service(), &actor, theme).await?;
    Ok(StatusCode::NO_CONTENT)
}

fn favicon_asset(theme: FaviconTheme) -> (&'static str, &'static [u8]) {
    match theme {
        FaviconTheme::Light => ("favicon-light", DEFAULT_LIGHT_FAVICON),
        FaviconTheme::Dark => ("favicon-dark", DEFAULT_DARK_FAVICON),
    }
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct BackupProviderCatalogItem {
    slug: BackupProviderKind,
    name: &'static str,
    description: &'static str,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct BackupProvidersResponse {
    providers: Vec<BackupProviderCatalogItem>,
    connections: Vec<BackupProviderResponse>,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct BackupProviderResponse {
    id: Uuid,
    name: String,
    provider: BackupProviderKind,
    managed_by_config: bool,
    managed_by_storage: bool,
    path: Option<String>,
    endpoint: Option<String>,
    bucket: Option<String>,
    access_key_hint: Option<String>,
    region: Option<String>,
    prefix: Option<String>,
    schedule: Option<String>,
    next_backup_at: Option<DateTime<Utc>>,
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct UpdateBackupProviderRequest {
    id: Option<Uuid>,
    name: String,
    provider: BackupProviderKind,
    path: Option<String>,
    endpoint: Option<String>,
    bucket: Option<String>,
    access_key: Option<String>,
    secret_key: Option<String>,
    region: Option<String>,
    prefix: Option<String>,
    test_token: Option<Uuid>,
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct UpdateBackupScheduleRequest {
    schedule: Option<String>,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct BackupProviderTestResponse {
    test_token: Uuid,
    message: &'static str,
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct CreateBackupRequest {
    name: Option<String>,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct BackupScheduledResponse {
    key: String,
    operation_id: Uuid,
    message: &'static str,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct RestorePreflightResponse {
    token: Uuid,
    key: String,
    #[serde(flatten)]
    inspection: archive::BackupInspection,
    required_free_space: u64,
    available_free_space: u64,
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct RestorePreflightRequest {
    key: String,
}

#[derive(Deserialize, utoipa::ToSchema, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct BackupObjectRequest {
    key: String,
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct RestoreBackupRequest {
    token: Uuid,
    password: String,
    create_safety_backup: bool,
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct StorageTargetRequest {
    name: Option<String>,
    kind: String,
    #[schema(value_type = Option<String>)]
    path: Option<PathBuf>,
    #[schema(value_type = Option<String>, format = "uri")]
    endpoint: Option<Url>,
    bucket: Option<String>,
    access_key: Option<String>,
    secret_key: Option<String>,
    region: Option<String>,
    prefix: Option<String>,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct StorageTargetTestResponse {
    message: &'static str,
}

#[utoipa::path(
    get, path = "/admin/storage/targets",
    operation_id = "get_admin_storage_targets",
    summary = "List storage targets",
    tag = "Storage",
    responses(
        (status = 200, description = "Success", body = Vec<StorageTargetView>, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn list_storage_targets(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<Vec<StorageTargetView>>, ApiError> {
    let actor = require_admin_principal(&state, &headers, &jar, SCOPE_READ).await?;
    Ok(Json(
        storage_service::list_storage_targets(&state.storage_service(), &actor).await?,
    ))
}

/// Walks the destination and sums what is actually stored there. The database
/// figure on every card covers LFS only, so this is what an administrator runs
/// when they need to account for backup archives or objects left behind by an
/// earlier migration.
#[utoipa::path(
    post, path = "/admin/storage/targets/{target_id}/usage",
    operation_id = "post_admin_storage_targets_target_id_usage",
    summary = "Measure storage target",
    tag = "Storage",
    params(
        ("target_id" = Uuid, Path),
    ),
    responses(
        (status = 200, description = "Success", body = MeasuredUsage, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn measure_storage_target(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(target_id): Path<Uuid>,
) -> Result<Json<MeasuredUsage>, ApiError> {
    let actor = require_admin_principal(&state, &headers, &jar, SCOPE_READ).await?;
    Ok(Json(
        storage_service::measure_storage_target(&state.storage_service(), &actor, target_id)
            .await?,
    ))
}

#[utoipa::path(
    post, path = "/admin/storage/targets/test",
    operation_id = "post_admin_storage_targets_test",
    summary = "Test storage target",
    tag = "Storage",

    responses(
        (status = 200, description = "Success", body = StorageTargetTestResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn test_storage_target(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<StorageTargetRequest>,
) -> Result<Json<StorageTargetTestResponse>, ApiError> {
    let actor = require_admin_principal(&state, &headers, &jar, SCOPE_WRITE).await?;
    Ok(Json(
        storage_service::test_storage_target(
            &state.storage_service(),
            &actor,
            storage_service::StorageTargetRequest {
                name: request.name,
                kind: request.kind,
                path: request.path,
                endpoint: request.endpoint,
                bucket: request.bucket,
                access_key: request.access_key,
                secret_key: request.secret_key,
                region: request.region,
                prefix: request.prefix,
            },
        )
        .await?
        .into(),
    ))
}

#[utoipa::path(
    post, path = "/admin/storage/targets/{target_id}/test",
    operation_id = "post_admin_storage_targets_target_id_test",
    summary = "Test saved storage target",
    tag = "Storage",
    params(
        ("target_id" = Uuid, Path),
    ),
    responses(
        (status = 200, description = "Success", body = StorageTargetTestResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn test_saved_storage_target(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(target_id): Path<Uuid>,
) -> Result<Json<StorageTargetTestResponse>, ApiError> {
    let actor = require_admin_principal(&state, &headers, &jar, SCOPE_WRITE).await?;
    Ok(Json(
        storage_service::test_saved_storage_target(&state.storage_service(), &actor, target_id)
            .await?
            .into(),
    ))
}

#[utoipa::path(
    post, path = "/admin/storage/targets",
    operation_id = "post_admin_storage_targets",
    summary = "Create storage target",
    tag = "Storage",

    responses(
        (status = 201, description = "Success", body = StorageTargetView, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn create_storage_target(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<StorageTargetRequest>,
) -> Result<(StatusCode, Json<StorageTargetView>), ApiError> {
    let actor = require_admin_principal(&state, &headers, &jar, SCOPE_WRITE).await?;
    Ok((
        StatusCode::CREATED,
        Json(
            storage_service::create_storage_target(
                &state.storage_service(),
                &actor,
                storage_service::StorageTargetRequest {
                    name: request.name,
                    kind: request.kind,
                    path: request.path,
                    endpoint: request.endpoint,
                    bucket: request.bucket,
                    access_key: request.access_key,
                    secret_key: request.secret_key,
                    region: request.region,
                    prefix: request.prefix,
                },
            )
            .await?,
        ),
    ))
}

#[utoipa::path(
    delete, path = "/admin/storage/targets/{target_id}",
    operation_id = "delete_admin_storage_targets_target_id",
    summary = "Delete storage target",
    tag = "Storage",
    params(
        ("target_id" = Uuid, Path),
    ),
    responses(
        (status = 204, description = "Success"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn delete_storage_target(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(target_id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let actor = require_admin_principal(&state, &headers, &jar, SCOPE_WRITE).await?;
    storage_service::delete_storage_target(&state.storage_service(), &actor, target_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    get, path = "/admin/backup/providers",
    operation_id = "get_admin_backup_providers",
    summary = "List backup providers",
    tag = "Backups",
    responses(
        (status = 200, description = "Success", body = BackupProvidersResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn list_backup_providers(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<BackupProvidersResponse>, ApiError> {
    let actor = require_admin_principal(&state, &headers, &jar, SCOPE_READ).await?;
    Ok(Json(
        backups::list_backup_providers(&state.backup_service(), &actor)
            .await?
            .into(),
    ))
}

#[utoipa::path(
    post, path = "/admin/backup/providers/test",
    operation_id = "post_admin_backup_providers_test",
    summary = "Test backup provider",
    tag = "Backups",

    responses(
        (status = 200, description = "Success", body = BackupProviderTestResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn test_backup_provider(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<UpdateBackupProviderRequest>,
) -> Result<Json<BackupProviderTestResponse>, ApiError> {
    let actor = require_admin_principal(&state, &headers, &jar, SCOPE_WRITE).await?;
    Ok(Json(
        backups::test_backup_provider(
            &state.backup_service(),
            &actor,
            backups::UpdateBackupProviderRequest {
                id: request.id,
                name: request.name,
                provider: request.provider,
                path: request.path,
                endpoint: request.endpoint,
                bucket: request.bucket,
                access_key: request.access_key,
                secret_key: request.secret_key,
                region: request.region,
                prefix: request.prefix,
                test_token: request.test_token,
            },
        )
        .await?
        .into(),
    ))
}

#[utoipa::path(
    post, path = "/admin/backup/providers",
    operation_id = "post_admin_backup_providers",
    summary = "Create backup provider",
    tag = "Backups",

    responses(
        (status = 201, description = "Success", body = BackupProviderResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn create_backup_provider(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<UpdateBackupProviderRequest>,
) -> Result<(StatusCode, Json<BackupProviderResponse>), ApiError> {
    let actor = require_admin_principal(&state, &headers, &jar, SCOPE_WRITE).await?;
    Ok((
        StatusCode::CREATED,
        Json(
            backups::create_backup_provider(
                &state.backup_service(),
                &actor,
                backups::UpdateBackupProviderRequest {
                    id: request.id,
                    name: request.name,
                    provider: request.provider,
                    path: request.path,
                    endpoint: request.endpoint,
                    bucket: request.bucket,
                    access_key: request.access_key,
                    secret_key: request.secret_key,
                    region: request.region,
                    prefix: request.prefix,
                    test_token: request.test_token,
                },
            )
            .await?
            .into(),
        ),
    ))
}

#[utoipa::path(
    put, path = "/admin/backup/providers/{provider_id}",
    operation_id = "put_admin_backup_providers_provider_id",
    summary = "Update backup provider",
    tag = "Backups",

    params(
        ("provider_id" = Uuid, Path),
    ),
    responses(
        (status = 200, description = "Success", body = BackupProviderResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn update_backup_provider(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(provider_id): Path<Uuid>,
    Json(request): Json<UpdateBackupProviderRequest>,
) -> Result<Json<BackupProviderResponse>, ApiError> {
    let actor = require_admin_principal(&state, &headers, &jar, SCOPE_WRITE).await?;
    Ok(Json(
        backups::update_backup_provider(
            &state.backup_service(),
            &actor,
            provider_id,
            backups::UpdateBackupProviderRequest {
                id: request.id,
                name: request.name,
                provider: request.provider,
                path: request.path,
                endpoint: request.endpoint,
                bucket: request.bucket,
                access_key: request.access_key,
                secret_key: request.secret_key,
                region: request.region,
                prefix: request.prefix,
                test_token: request.test_token,
            },
        )
        .await?
        .into(),
    ))
}

#[utoipa::path(
    delete, path = "/admin/backup/providers/{provider_id}",
    operation_id = "delete_admin_backup_providers_provider_id",
    summary = "Delete backup provider",
    tag = "Backups",
    params(
        ("provider_id" = Uuid, Path),
    ),
    responses(
        (status = 204, description = "Success"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn delete_backup_provider(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(provider_id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let actor = require_admin_principal(&state, &headers, &jar, SCOPE_WRITE).await?;
    backups::delete_backup_provider(&state.backup_service(), &actor, provider_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    put, path = "/admin/backup/providers/{provider_id}/schedule",
    operation_id = "put_admin_backup_providers_provider_id_schedule",
    summary = "Update backup schedule",
    tag = "Backups",

    params(
        ("provider_id" = Uuid, Path),
    ),
    responses(
        (status = 200, description = "Success", body = BackupProviderResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn update_backup_schedule(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(provider_id): Path<Uuid>,
    Json(request): Json<UpdateBackupScheduleRequest>,
) -> Result<Json<BackupProviderResponse>, ApiError> {
    let actor = require_admin_principal(&state, &headers, &jar, SCOPE_WRITE).await?;
    Ok(Json(
        backups::update_backup_schedule(
            &state.backup_service(),
            &actor,
            provider_id,
            backups::UpdateBackupScheduleRequest {
                schedule: request.schedule,
            },
        )
        .await?
        .into(),
    ))
}

#[utoipa::path(
    get, path = "/admin/backup/providers/{provider_id}/backups",
    operation_id = "get_admin_backup_providers_provider_id_backups",
    summary = "List backups",
    tag = "Backups",
    params(
        ("provider_id" = Uuid, Path),
    ),
    responses(
        (status = 200, description = "Success", body = Vec<archive::BackupObject>, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn list_backups(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(provider_id): Path<Uuid>,
) -> Result<Json<Vec<archive::BackupObject>>, ApiError> {
    let actor = require_admin_principal(&state, &headers, &jar, SCOPE_READ).await?;
    Ok(Json(
        backups::list_backups(&state.backup_service(), &actor, provider_id).await?,
    ))
}

#[utoipa::path(
    get, path = "/admin/backup/providers/{provider_id}/backups/object",
    operation_id = "get_admin_backup_providers_provider_id_backups_object",
    summary = "Download backup",
    tag = "Backups",
    params(
        ("provider_id" = Uuid, Path),
        BackupObjectRequest,
    ),
    responses(
        (status = 200, description = "Success", body = crate::openapi::Binary, content_type = "application/octet-stream"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn download_backup(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(provider_id): Path<Uuid>,
    Query(request): Query<BackupObjectRequest>,
) -> Result<Response, ApiError> {
    let actor = require_admin_principal(&state, &headers, &jar, SCOPE_WRITE).await?;
    let key = request.key;
    let (body, size) =
        match backups::download_backup(&state.backup_service(), &actor, provider_id, &key).await? {
            backups::BackupDownload::File(file, size) => {
                (Body::from_stream(ReaderStream::new(file)), Some(size))
            }
            backups::BackupDownload::S3(download) => {
                (Body::from_stream(download.stream), download.size)
            }
        };
    let filename = download_filename(&key);
    let mut response = Response::new(body);
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/zstd"),
    );
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!("attachment; filename=\"{filename}\""))
            .map_err(ApiError::internal)?,
    );
    if let Some(size) = size {
        response.headers_mut().insert(
            header::CONTENT_LENGTH,
            HeaderValue::from_str(&size.to_string()).map_err(ApiError::internal)?,
        );
    }
    Ok(response)
}

#[utoipa::path(
    delete, path = "/admin/backup/providers/{provider_id}/backups/object",
    operation_id = "delete_admin_backup_providers_provider_id_backups_object",
    summary = "Delete backup",
    tag = "Backups",
    params(
        ("provider_id" = Uuid, Path),
        BackupObjectRequest,
    ),
    responses(
        (status = 204, description = "Success"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn delete_backup(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(provider_id): Path<Uuid>,
    Query(request): Query<BackupObjectRequest>,
) -> Result<StatusCode, ApiError> {
    let actor = require_admin_principal(&state, &headers, &jar, SCOPE_WRITE).await?;
    backups::delete_backup(
        &state.backup_service(),
        &actor,
        provider_id,
        backups::BackupObjectRequest { key: request.key },
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    get, path = "/admin/backups/progress/{operation_id}",
    operation_id = "get_admin_backups_progress_operation_id",
    summary = "Backup progress",
    tag = "Backups",
    params(
        ("operation_id" = Uuid, Path),
    ),
    responses(
        (status = 200, description = "Success", body = String, content_type = "text/event-stream"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn backup_progress(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(_operation_id): Path<Uuid>,
) -> Result<Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>>, ApiError> {
    require_admin_principal(&state, &headers, &jar, SCOPE_WRITE).await?;
    let events = stream::once(async {
        Ok(Event::default()
            .comment("Maintenance is starting.")
            .retry(Duration::from_millis(500)))
    });
    Ok(Sse::new(events))
}

#[utoipa::path(
    post, path = "/admin/backup/providers/{provider_id}/backups",
    operation_id = "post_admin_backup_providers_provider_id_backups",
    summary = "Create backup",
    tag = "Backups",

    params(
        ("provider_id" = Uuid, Path),
    ),
    responses(
        (status = 202, description = "Success", body = BackupScheduledResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn create_backup(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(provider_id): Path<Uuid>,
    Json(request): Json<CreateBackupRequest>,
) -> Result<(StatusCode, Json<BackupScheduledResponse>), ApiError> {
    let actor = require_admin_principal(&state, &headers, &jar, SCOPE_WRITE).await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(
            backups::create_backup(
                &state.backup_service(),
                &actor,
                provider_id,
                backups::CreateBackupRequest { name: request.name },
            )
            .await?
            .into(),
        ),
    ))
}

#[utoipa::path(
    post, path = "/admin/backup/providers/{provider_id}/backups/preflight",
    operation_id = "post_admin_backup_providers_provider_id_backups_preflight",
    summary = "Preflight restore",
    tag = "Backups",

    params(
        ("provider_id" = Uuid, Path),
    ),
    responses(
        (status = 200, description = "Success", body = RestorePreflightResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn preflight_restore(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(provider_id): Path<Uuid>,
    Json(request): Json<RestorePreflightRequest>,
) -> Result<Json<RestorePreflightResponse>, ApiError> {
    let actor = require_admin_principal(&state, &headers, &jar, SCOPE_WRITE).await?;
    Ok(Json(
        backups::preflight_restore(
            &state.backup_service(),
            &actor,
            provider_id,
            backups::RestorePreflightRequest { key: request.key },
        )
        .await?
        .into(),
    ))
}

#[utoipa::path(
    post, path = "/admin/backup/providers/{provider_id}/backups/restore",
    operation_id = "post_admin_backup_providers_provider_id_backups_restore",
    summary = "Restore backup",
    tag = "Backups",

    params(
        ("provider_id" = Uuid, Path),
    ),
    responses(
        (status = 202, description = "Success", body = BackupScheduledResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn restore_backup(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(_provider_id): Path<Uuid>,
    Json(request): Json<RestoreBackupRequest>,
) -> Result<(StatusCode, Json<BackupScheduledResponse>), ApiError> {
    let actor = require_admin_principal(&state, &headers, &jar, SCOPE_WRITE).await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(
            backups::restore_backup(
                &state.backup_service(),
                &actor,
                _provider_id,
                backups::RestoreBackupRequest {
                    token: request.token,
                    password: request.password,
                    create_safety_backup: request.create_safety_backup,
                },
            )
            .await?
            .into(),
        ),
    ))
}

fn download_filename(key: &str) -> String {
    let filename = key.rsplit('/').next().unwrap_or("gitadel-backup.tar.zst");
    let sanitized = filename
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    if sanitized.is_empty() {
        "gitadel-backup.tar.zst".to_owned()
    } else {
        sanitized
    }
}

pub(crate) async fn require_admin(
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

/// Removes audit events older than `retention_days`. A retention of `0`
/// keeps every event and removes nothing.
#[cfg(test)]
use gitadel_instance::service::prune_audit_events;

/// Hourly audit retention sweep. Returns at once when retention is off, so
/// an instance that keeps everything never runs the query.
pub(crate) async fn serve_audit_retention(state: IdentityState) {
    gitadel_instance::service::serve_audit_retention(
        state.database().clone(),
        state
            .runtime_settings()
            .map(|s| s.audit.retention_days)
            .unwrap_or(0),
    )
    .await
}

use gitadel_backups::service as backups;
impl From<backups::BackupProviderCatalogItem> for BackupProviderCatalogItem {
    fn from(value: backups::BackupProviderCatalogItem) -> Self {
        Self {
            slug: value.slug,
            name: value.name,
            description: value.description,
        }
    }
}
impl From<backups::BackupProvidersResponse> for BackupProvidersResponse {
    fn from(value: backups::BackupProvidersResponse) -> Self {
        Self {
            providers: value.providers.into_iter().map(Into::into).collect(),
            connections: value.connections.into_iter().map(Into::into).collect(),
        }
    }
}
impl From<backups::BackupProviderResponse> for BackupProviderResponse {
    fn from(value: backups::BackupProviderResponse) -> Self {
        Self {
            id: value.id,
            name: value.name,
            provider: value.provider,
            managed_by_config: value.managed_by_config,
            managed_by_storage: value.managed_by_storage,
            path: value.path,
            endpoint: value.endpoint,
            bucket: value.bucket,
            access_key_hint: value.access_key_hint,
            region: value.region,
            prefix: value.prefix,
            schedule: value.schedule,
            next_backup_at: value.next_backup_at,
        }
    }
}
impl From<backups::BackupProviderTestResponse> for BackupProviderTestResponse {
    fn from(value: backups::BackupProviderTestResponse) -> Self {
        Self {
            test_token: value.test_token,
            message: value.message,
        }
    }
}
impl From<backups::BackupScheduledResponse> for BackupScheduledResponse {
    fn from(value: backups::BackupScheduledResponse) -> Self {
        Self {
            key: value.key,
            operation_id: value.operation_id,
            message: value.message,
        }
    }
}
impl From<backups::RestorePreflightResponse> for RestorePreflightResponse {
    fn from(value: backups::RestorePreflightResponse) -> Self {
        Self {
            token: value.token,
            key: value.key,
            inspection: value.inspection,
            required_free_space: value.required_free_space,
            available_free_space: value.available_free_space,
        }
    }
}

use gitadel_instance::service as instance_service;
impl From<instance_service::IntegritySettingsResponse> for IntegritySettingsResponse {
    fn from(v: instance_service::IntegritySettingsResponse) -> Self {
        Self {
            enabled: v.enabled,
            schedule: v.schedule,
            last_checked_at: v.last_checked_at,
            last_result: v.last_result,
        }
    }
}

use gitadel_storage::service as storage_service;
impl From<storage_service::StorageTargetTestResponse> for StorageTargetTestResponse {
    fn from(value: storage_service::StorageTargetTestResponse) -> Self {
        Self {
            message: value.message,
        }
    }
}

pub(crate) async fn require_admin_principal(
    state: &IdentityState,
    headers: &HeaderMap,
    jar: &CookieJar,
    scope: i32,
) -> Result<gitadel_identity::Principal, ApiError> {
    let principal = state.authenticate_principal(headers, jar, scope).await?;
    principal.authorize(state.database(), scope).await?;
    if !principal.user.is_admin {
        return Err(ApiError::forbidden("Administrator access is required."));
    }
    Ok(principal)
}

#[cfg(test)]
use crate::entity::audit_event;
#[cfg(test)]
use sea_orm::Set;

#[cfg(test)]
mod tests {
    use sea_orm::{ConnectOptions, Database, PaginatorTrait as _};
    use sea_orm_migration::MigratorTrait as _;

    use super::*;

    #[tokio::test]
    async fn audit_events_are_pruned_only_past_a_positive_retention() {
        let mut options = ConnectOptions::new("sqlite::memory:");
        options.max_connections(1);
        let database = Database::connect(options).await.unwrap();
        crate::migration::Migrator::up(&database, None)
            .await
            .unwrap();
        let now = Utc::now();
        for days_ago in [40, 10] {
            audit_event::ActiveModel {
                actor_user_id: Set(None),
                action: Set(format!("test.{days_ago}")),
                target: Set(None),
                remote_address: Set(None),
                created_at: Set(now - chrono::Duration::days(days_ago)),
                ..Default::default()
            }
            .insert(&database)
            .await
            .unwrap();
        }

        assert_eq!(prune_audit_events(&database, 0, now).await.unwrap(), 0);
        assert_eq!(
            audit_event::Entity::find().count(&database).await.unwrap(),
            2
        );
        assert_eq!(prune_audit_events(&database, 30, now).await.unwrap(), 1);
        let remaining = audit_event::Entity::find().all(&database).await.unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].action, "test.10");
    }
}
