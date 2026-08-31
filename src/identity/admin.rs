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
use sea_orm::{ActiveModelTrait, ConnectionTrait, EntityTrait, Set, Statement, TransactionTrait};
use serde::{Deserialize, Serialize};
use tokio_util::io::ReaderStream;
use url::Url;
use uuid::Uuid;

use crate::{
    archive::{self, MaintenanceAction},
    backup_provider::{
        self, BackupProvider, BackupProviderConfig, BackupProviderKind, BackupProviderSource,
        FilesystemSettings, RUNTIME_S3_PROVIDER_ID,
    },
    blob_store::targets::{self, StorageTargetConfiguration, StorageTargetView},
    config::S3Settings,
    entity::{audit_event, instance, instance_asset},
};

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

#[derive(Deserialize)]
pub struct UpdateInstanceSettingsRequest {
    site_name: String,
    site_description: Option<String>,
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

    let settings = instance::Entity::find_by_id(1)
        .one(state.database())
        .await?
        .ok_or_else(|| ApiError::internal("instance settings row is missing"))?;
    let now = Utc::now();
    let mut active: instance::ActiveModel = settings.into();
    active.site_name = Set(site_name.to_owned());
    active.site_description = Set(site_description);
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

#[derive(Serialize)]
pub struct BackupProviderCatalogItem {
    slug: BackupProviderKind,
    name: &'static str,
    description: &'static str,
}

#[derive(Serialize)]
pub struct BackupProvidersResponse {
    providers: Vec<BackupProviderCatalogItem>,
    connections: Vec<BackupProviderResponse>,
}

#[derive(Serialize)]
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

#[derive(Deserialize)]
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

#[derive(Deserialize)]
pub struct UpdateBackupScheduleRequest {
    schedule: Option<String>,
}

#[derive(Serialize)]
pub struct BackupProviderTestResponse {
    test_token: Uuid,
    message: &'static str,
}

#[derive(Deserialize)]
pub struct CreateBackupRequest {
    name: Option<String>,
}

#[derive(Serialize)]
pub struct BackupScheduledResponse {
    key: String,
    operation_id: Uuid,
    message: &'static str,
}

#[derive(Serialize)]
pub struct RestorePreflightResponse {
    token: Uuid,
    key: String,
    #[serde(flatten)]
    inspection: archive::BackupInspection,
    required_free_space: u64,
    available_free_space: u64,
}

#[derive(Deserialize)]
pub struct RestorePreflightRequest {
    key: String,
}

#[derive(Deserialize)]
pub struct BackupObjectRequest {
    key: String,
}

#[derive(Deserialize)]
pub struct RestoreBackupRequest {
    token: Uuid,
    password: String,
    create_safety_backup: bool,
}

#[derive(Deserialize)]
pub struct StorageTargetRequest {
    name: Option<String>,
    kind: String,
    path: Option<PathBuf>,
    endpoint: Option<Url>,
    bucket: Option<String>,
    access_key: Option<String>,
    secret_key: Option<String>,
    region: Option<String>,
    prefix: Option<String>,
}

impl StorageTargetRequest {
    fn configuration(&self) -> Result<StorageTargetConfiguration, ApiError> {
        match self.kind.as_str() {
            "filesystem" => Ok(StorageTargetConfiguration::Filesystem {
                path: self
                    .path
                    .clone()
                    .ok_or_else(|| ApiError::bad_request("A filesystem path is required."))?,
            }),
            "s3" => Ok(StorageTargetConfiguration::S3 {
                s3: S3Settings {
                    endpoint: self
                        .endpoint
                        .clone()
                        .ok_or_else(|| ApiError::bad_request("An S3 endpoint is required."))?,
                    bucket: required_storage_value(&self.bucket, "An S3 bucket is required.")?,
                    access_key: required_storage_value(
                        &self.access_key,
                        "An S3 access key is required.",
                    )?,
                    secret_key: required_storage_value(
                        &self.secret_key,
                        "An S3 secret key is required.",
                    )?,
                    region: self
                        .region
                        .clone()
                        .unwrap_or_else(|| "us-east-1".to_owned()),
                    prefix: self
                        .prefix
                        .clone()
                        .unwrap_or_else(|| "gitadel-lfs".to_owned()),
                },
            }),
            _ => Err(ApiError::bad_request("The storage target kind is invalid.")),
        }
    }
}

fn required_storage_value(
    value: &Option<String>,
    message: &'static str,
) -> Result<String, ApiError> {
    value
        .as_ref()
        .filter(|value| !value.trim().is_empty())
        .cloned()
        .ok_or_else(|| ApiError::bad_request(message))
}

#[derive(Serialize)]
pub struct StorageTargetTestResponse {
    message: &'static str,
}

#[derive(Deserialize)]
pub struct StorageMigrationRequest {
    target_id: Uuid,
    #[serde(default = "default_storage_migration_batch_size")]
    batch_size: usize,
}

fn default_storage_migration_batch_size() -> usize {
    100
}

#[derive(Serialize)]
pub struct StorageMigrationScheduledResponse {
    operation_id: Uuid,
    target_id: Uuid,
    message: &'static str,
}

pub async fn list_storage_targets(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<Vec<StorageTargetView>>, ApiError> {
    require_admin(&state, &headers, &jar, SCOPE_READ).await?;
    let settings = state.runtime_settings()?;
    Ok(Json(
        targets::list(state.database(), &settings.storage.lfs_root)
            .await
            .map_err(ApiError::internal)?,
    ))
}

#[derive(Serialize)]
pub struct LfsStorageStatusResponse {
    object_count: u64,
    total_bytes: u64,
}

pub async fn lfs_storage_status(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<LfsStorageStatusResponse>, ApiError> {
    require_admin(&state, &headers, &jar, SCOPE_READ).await?;
    let statement = Statement::from_string(
        state.database().get_database_backend(),
        "SELECT COUNT(*) AS object_count, COALESCE(SUM(size), 0) AS total_bytes FROM lfs_objects",
    );
    let row = state
        .database()
        .query_one_raw(statement)
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::internal(anyhow::anyhow!("LFS usage query returned no row")))?;
    let object_count = row
        .try_get::<i64>("", "object_count")
        .map_err(ApiError::internal)?;
    let total_bytes = row
        .try_get::<i64>("", "total_bytes")
        .map_err(ApiError::internal)?;
    Ok(Json(LfsStorageStatusResponse {
        object_count: u64::try_from(object_count).map_err(ApiError::internal)?,
        total_bytes: u64::try_from(total_bytes).map_err(ApiError::internal)?,
    }))
}

pub async fn test_storage_target(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<StorageTargetRequest>,
) -> Result<Json<StorageTargetTestResponse>, ApiError> {
    require_admin(&state, &headers, &jar, SCOPE_WRITE).await?;
    targets::test_configuration(&request.configuration()?)
        .await
        .map_err(|error| ApiError::bad_request(format!("Storage target test failed: {error:#}")))?;
    Ok(Json(StorageTargetTestResponse {
        message: "Storage target passed write, stat, read, and delete checks.",
    }))
}

pub async fn test_saved_storage_target(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(target_id): Path<Uuid>,
) -> Result<Json<StorageTargetTestResponse>, ApiError> {
    require_admin(&state, &headers, &jar, SCOPE_WRITE).await?;
    let (_, configuration) = targets::find(state.database(), target_id)
        .await
        .map_err(ApiError::internal)?;
    targets::test_configuration(&configuration)
        .await
        .map_err(|error| ApiError::bad_request(format!("Storage target test failed: {error:#}")))?;
    Ok(Json(StorageTargetTestResponse {
        message: "Storage target passed write, stat, read, and delete checks.",
    }))
}

pub async fn create_storage_target(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<StorageTargetRequest>,
) -> Result<(StatusCode, Json<StorageTargetView>), ApiError> {
    let actor = require_admin(&state, &headers, &jar, SCOPE_WRITE).await?;
    let name = request
        .name
        .clone()
        .filter(|name| !name.trim().is_empty())
        .ok_or_else(|| ApiError::bad_request("A storage target name is required."))?;
    let target = targets::create(state.database(), name, request.configuration()?)
        .await
        .map_err(|error| {
            ApiError::bad_request(format!("Could not create storage target: {error:#}"))
        })?;
    state
        .audit(
            Some(actor.user.id),
            "storage.target.create",
            Some(target.id.to_string()),
        )
        .await?;
    Ok((StatusCode::CREATED, Json(target)))
}

pub async fn delete_storage_target(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(target_id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let actor = require_admin(&state, &headers, &jar, SCOPE_WRITE).await?;
    targets::delete(state.database(), target_id)
        .await
        .map_err(|error| {
            ApiError::bad_request(format!("Could not delete storage target: {error:#}"))
        })?;
    state
        .audit(
            Some(actor.user.id),
            "storage.target.delete",
            Some(target_id.to_string()),
        )
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn migrate_storage(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<StorageMigrationRequest>,
) -> Result<(StatusCode, Json<StorageMigrationScheduledResponse>), ApiError> {
    let actor = require_admin(&state, &headers, &jar, SCOPE_WRITE).await?;
    if request.batch_size == 0 {
        return Err(ApiError::bad_request(
            "The storage migration batch size must be greater than zero.",
        ));
    }
    if !request.target_id.is_nil() {
        targets::find(state.database(), request.target_id)
            .await
            .map_err(|_| ApiError::bad_request("The storage target does not exist."))?;
    }
    let operation_id = Uuid::new_v4();
    state
        .audit(
            Some(actor.user.id),
            "storage.migration.schedule",
            Some(request.target_id.to_string()),
        )
        .await?;
    state
        .schedule_maintenance(MaintenanceAction::LfsMigrate {
            operation_id,
            target_id: request.target_id,
            batch_size: request.batch_size,
        })
        .await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(StorageMigrationScheduledResponse {
            operation_id,
            target_id: request.target_id,
            message: "Gitadel is restarting to migrate Git LFS storage.",
        }),
    ))
}

pub async fn storage_progress(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(_operation_id): Path<Uuid>,
) -> Result<Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>>, ApiError> {
    require_admin(&state, &headers, &jar, SCOPE_WRITE).await?;
    let events = stream::once(async {
        Ok(Event::default()
            .event("reconnecting")
            .data(r#"{"phase":"scheduled","message":"Waiting for maintenance mode."}"#))
    });
    Ok(Sse::new(events))
}

pub async fn list_backup_providers(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<BackupProvidersResponse>, ApiError> {
    require_admin(&state, &headers, &jar, SCOPE_READ).await?;
    let settings = state.runtime_settings()?;
    let providers = backup_provider::list_with_storage(
        state.database(),
        settings.backup.s3.as_ref(),
        &settings.storage.lfs_root,
    )
    .await
    .map_err(ApiError::internal)?;
    let schedules = archive::list_backup_schedules(state.database())
        .await
        .map_err(ApiError::internal)?;
    Ok(Json(BackupProvidersResponse {
        providers: vec![
            BackupProviderCatalogItem {
                slug: BackupProviderKind::Filesystem,
                name: "Filesystem",
                description: "Write backup archives to a directory on the Gitadel host.",
            },
            BackupProviderCatalogItem {
                slug: BackupProviderKind::S3,
                name: "S3-compatible storage",
                description: "Store backups in AWS S3, RustFS, MinIO, or another compatible service.",
            },
        ],
        connections: providers
            .into_iter()
            .map(|provider| {
                let schedule = schedules.get(&provider.id).cloned();
                backup_provider_response(provider, schedule)
            })
            .collect(),
    }))
}

pub async fn test_backup_provider(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<UpdateBackupProviderRequest>,
) -> Result<Json<BackupProviderTestResponse>, ApiError> {
    let actor = require_admin(&state, &headers, &jar, SCOPE_WRITE).await?;
    let config = requested_backup_provider_config(&state, &request).await?;
    archive::test_backup_provider(&config, state.runtime_settings()?)
        .await
        .map_err(|error| ApiError::bad_request(error.to_string()))?;
    let provider = config.kind().as_str();
    let test_token = state
        .cache_tested_backup_provider(actor.user.id, config)
        .await;
    state
        .audit(
            Some(actor.user.id),
            "backup.provider.test",
            Some(provider.to_owned()),
        )
        .await?;
    Ok(Json(BackupProviderTestResponse {
        test_token,
        message: "Backup provider is available and writable.",
    }))
}

pub async fn create_backup_provider(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<UpdateBackupProviderRequest>,
) -> Result<(StatusCode, Json<BackupProviderResponse>), ApiError> {
    let actor = require_admin(&state, &headers, &jar, SCOPE_WRITE).await?;
    if request.id.is_some() {
        return Err(ApiError::bad_request(
            "A new backup provider cannot include an existing provider ID.",
        ));
    }
    let config = requested_backup_provider_config(&state, &request).await?;
    consume_provider_test(&state, actor.user.id, request.test_token, &config).await?;
    let now = Utc::now();
    let provider = BackupProvider {
        id: Uuid::new_v4(),
        name: request.name.trim().to_owned(),
        config,
        source: BackupProviderSource::Stored,
        created_at: now,
        updated_at: now,
    };
    backup_provider::save(state.database(), &provider)
        .await
        .map_err(|error| ApiError::bad_request(error.to_string()))?;
    state
        .audit(
            Some(actor.user.id),
            "backup.provider.create",
            Some(provider.id.to_string()),
        )
        .await?;
    Ok((
        StatusCode::CREATED,
        Json(backup_provider_response(provider, None)),
    ))
}

pub async fn update_backup_provider(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(provider_id): Path<Uuid>,
    Json(request): Json<UpdateBackupProviderRequest>,
) -> Result<Json<BackupProviderResponse>, ApiError> {
    let actor = require_admin(&state, &headers, &jar, SCOPE_WRITE).await?;
    if request.id != Some(provider_id) {
        return Err(ApiError::bad_request(
            "The backup provider ID does not match the requested resource.",
        ));
    }
    let mut existing = required_backup_provider(&state, provider_id).await?;
    if existing.source == BackupProviderSource::StorageTarget {
        return Err(ApiError::bad_request(
            "Storage targets are configured on the Storage page.",
        ));
    }
    let config = requested_backup_provider_config(&state, &request).await?;
    consume_provider_test(&state, actor.user.id, request.test_token, &config).await?;
    if existing.id == RUNTIME_S3_PROVIDER_ID {
        existing.id = Uuid::new_v4();
        existing.created_at = Utc::now();
    }
    existing.name = request.name.trim().to_owned();
    existing.config = config;
    existing.updated_at = Utc::now();
    backup_provider::save(state.database(), &existing)
        .await
        .map_err(|error| ApiError::bad_request(error.to_string()))?;
    state
        .audit(
            Some(actor.user.id),
            "backup.provider.update",
            Some(existing.id.to_string()),
        )
        .await?;
    let schedule = archive::load_backup_schedule(state.database(), existing.id)
        .await
        .map_err(ApiError::internal)?;
    Ok(Json(backup_provider_response(existing, schedule)))
}

pub async fn delete_backup_provider(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(provider_id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let actor = require_admin(&state, &headers, &jar, SCOPE_WRITE).await?;
    let provider = required_backup_provider(&state, provider_id).await?;
    match provider.source {
        BackupProviderSource::Stored => {
            if !backup_provider::delete(state.database(), provider_id)
                .await
                .map_err(ApiError::internal)?
            {
                return Err(ApiError::not_found());
            }
        }
        BackupProviderSource::StorageTarget => {
            backup_provider::exclude_storage_target(state.database(), provider_id)
                .await
                .map_err(ApiError::internal)?;
        }
        BackupProviderSource::RuntimeConfig => {
            return Err(ApiError::not_found());
        }
    }
    state
        .audit(
            Some(actor.user.id),
            "backup.provider.delete",
            Some(provider_id.to_string()),
        )
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn update_backup_schedule(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(provider_id): Path<Uuid>,
    Json(request): Json<UpdateBackupScheduleRequest>,
) -> Result<Json<BackupProviderResponse>, ApiError> {
    let actor = require_admin(&state, &headers, &jar, SCOPE_WRITE).await?;
    let mut provider = required_backup_provider(&state, provider_id).await?;
    match provider.source {
        BackupProviderSource::RuntimeConfig => {
            provider.id = Uuid::new_v4();
            provider.created_at = Utc::now();
            provider.updated_at = provider.created_at;
            provider.source = BackupProviderSource::Stored;
            backup_provider::save(state.database(), &provider)
                .await
                .map_err(ApiError::internal)?;
        }
        BackupProviderSource::StorageTarget => {
            let mut persisted = provider.clone();
            persisted.source = BackupProviderSource::Stored;
            backup_provider::save(state.database(), &persisted)
                .await
                .map_err(ApiError::internal)?;
        }
        BackupProviderSource::Stored => {}
    }
    archive::validate_backup_schedule(request.schedule.as_deref())
        .map_err(|error| ApiError::bad_request(error.to_string()))?;
    let schedule =
        archive::save_backup_schedule(state.database(), provider.id, request.schedule.as_deref())
            .await
            .map_err(ApiError::internal)?;
    state
        .audit(
            Some(actor.user.id),
            "backup.schedule.update",
            Some(
                schedule
                    .as_ref()
                    .map_or_else(|| "disabled".to_owned(), |value| value.schedule.clone()),
            ),
        )
        .await?;
    Ok(Json(backup_provider_response(provider, schedule)))
}

pub async fn list_backups(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(provider_id): Path<Uuid>,
) -> Result<Json<Vec<archive::BackupObject>>, ApiError> {
    require_admin(&state, &headers, &jar, SCOPE_READ).await?;
    let provider = required_backup_provider(&state, provider_id).await?;
    let backups = archive::list_backups(&provider.config, state.runtime_settings()?)
        .await
        .map_err(ApiError::internal)?;
    Ok(Json(backups))
}

pub async fn download_backup(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(provider_id): Path<Uuid>,
    Query(request): Query<BackupObjectRequest>,
) -> Result<Response, ApiError> {
    let actor = require_admin(&state, &headers, &jar, SCOPE_WRITE).await?;
    let provider = required_backup_provider(&state, provider_id).await?;
    let key = request.key;
    let (body, size) = match &provider.config {
        BackupProviderConfig::Filesystem(filesystem) => {
            let (file, size) =
                archive::open_filesystem_backup(state.runtime_settings()?, filesystem, &key)
                    .await
                    .map_err(ApiError::internal)?;
            (Body::from_stream(ReaderStream::new(file)), Some(size))
        }
        BackupProviderConfig::S3(s3) => {
            let download = archive::stream_s3_backup(s3, &key)
                .await
                .map_err(ApiError::internal)?;
            (Body::from_stream(download.stream), download.size)
        }
    };
    state
        .audit(Some(actor.user.id), "backup.download", Some(key.clone()))
        .await?;

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

pub async fn delete_backup(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(provider_id): Path<Uuid>,
    Query(request): Query<BackupObjectRequest>,
) -> Result<StatusCode, ApiError> {
    let actor = require_admin(&state, &headers, &jar, SCOPE_WRITE).await?;
    let provider = required_backup_provider(&state, provider_id).await?;
    archive::delete_backup(&provider.config, state.runtime_settings()?, &request.key)
        .await
        .map_err(ApiError::internal)?;
    state
        .audit(Some(actor.user.id), "backup.delete", Some(request.key))
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn backup_progress(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(_operation_id): Path<Uuid>,
) -> Result<Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>>, ApiError> {
    require_admin(&state, &headers, &jar, SCOPE_WRITE).await?;
    let events = stream::once(async {
        Ok(Event::default()
            .comment("Maintenance is starting.")
            .retry(Duration::from_millis(500)))
    });
    Ok(Sse::new(events))
}

pub async fn create_backup(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(provider_id): Path<Uuid>,
    Json(request): Json<CreateBackupRequest>,
) -> Result<(StatusCode, Json<BackupScheduledResponse>), ApiError> {
    let actor = require_admin(&state, &headers, &jar, SCOPE_WRITE).await?;
    let provider = required_backup_provider(&state, provider_id).await?;
    let name = archive::validate_backup_name(request.name.as_deref())
        .map_err(|error| ApiError::bad_request(error.to_string()))?;
    let key =
        archive::new_backup_key(&provider.config, name.as_deref()).map_err(ApiError::internal)?;
    let operation_id = Uuid::new_v4();
    state
        .audit(Some(actor.user.id), "backup.create", Some(key.clone()))
        .await?;
    state
        .schedule_maintenance(MaintenanceAction::Create {
            operation_id,
            key: key.clone(),
            provider,
            name,
        })
        .await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(BackupScheduledResponse {
            operation_id,
            key,
            message: "Gitadel is restarting to create the backup.",
        }),
    ))
}

pub async fn preflight_restore(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(provider_id): Path<Uuid>,
    Json(request): Json<RestorePreflightRequest>,
) -> Result<Json<RestorePreflightResponse>, ApiError> {
    let key = request.key;
    require_admin(&state, &headers, &jar, SCOPE_WRITE).await?;
    let mut provider = required_backup_provider(&state, provider_id).await?;
    if provider.id == RUNTIME_S3_PROVIDER_ID {
        provider.id = Uuid::new_v4();
        provider.created_at = Utc::now();
        provider.updated_at = provider.created_at;
    }
    let runtime = state.runtime_settings()?;
    let (path, inspection) = archive::download_and_inspect_backup(runtime, &provider.config, &key)
        .await
        .map_err(|error| ApiError::bad_request(error.to_string()))?;
    let available_free_space =
        archive::restore_available_space(runtime).map_err(ApiError::internal)?;
    let required_free_space = inspection.uncompressed_size.saturating_mul(2);
    if available_free_space < required_free_space {
        let _ = std::fs::remove_file(path);
        return Err(ApiError::bad_request(format!(
            "Restore needs at least {required_free_space} bytes free; {available_free_space} bytes are available."
        )));
    }
    let token = state
        .cache_validated_backup(key.clone(), path, provider)
        .await;
    Ok(Json(RestorePreflightResponse {
        token,
        key,
        inspection,
        required_free_space,
        available_free_space,
    }))
}

pub async fn restore_backup(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(_provider_id): Path<Uuid>,
    Json(request): Json<RestoreBackupRequest>,
) -> Result<(StatusCode, Json<BackupScheduledResponse>), ApiError> {
    let actor = require_admin(&state, &headers, &jar, SCOPE_WRITE).await?;
    if actor.via_api_token {
        return Err(ApiError::forbidden(
            "Restore requires an interactive administrator session.",
        ));
    }
    if !super::verify_password(request.password, actor.user.password_hash.clone()).await? {
        return Err(ApiError::forbidden("Password confirmation failed."));
    }
    let backup = state
        .take_validated_backup(request.token)
        .await
        .ok_or_else(|| ApiError::bad_request("The validated backup expired. Validate it again."))?;
    let operation_id = Uuid::new_v4();
    let safety_key = if request.create_safety_backup {
        Some(
            archive::new_backup_key(&backup.provider.config, Some("pre-restore-safety"))
                .map_err(ApiError::internal)?,
        )
    } else {
        None
    };
    state
        .audit(
            Some(actor.user.id),
            "backup.restore",
            Some(backup.key.clone()),
        )
        .await?;
    if let Err(error) = state
        .schedule_maintenance(MaintenanceAction::Restore {
            operation_id,
            archive: backup.path.clone(),
            key: backup.key.clone(),
            provider: backup.provider,
            safety_key,
        })
        .await
    {
        let _ = std::fs::remove_file(backup.path);
        return Err(error);
    }
    Ok((
        StatusCode::ACCEPTED,
        Json(BackupScheduledResponse {
            key: backup.key,
            operation_id,
            message: "Gitadel is restarting to restore the selected backup.",
        }),
    ))
}

async fn required_backup_provider(
    state: &IdentityState,
    provider_id: Uuid,
) -> Result<BackupProvider, ApiError> {
    let settings = state.runtime_settings()?;
    backup_provider::load_with_storage(
        state.database(),
        settings.backup.s3.as_ref(),
        &settings.storage.lfs_root,
        provider_id,
    )
    .await
    .map_err(ApiError::internal)?
    .ok_or_else(ApiError::not_found)
}

async fn requested_backup_provider_config(
    state: &IdentityState,
    request: &UpdateBackupProviderRequest,
) -> Result<BackupProviderConfig, ApiError> {
    let existing = if let Some(id) = request.id {
        Some(required_backup_provider(state, id).await?)
    } else {
        None
    };
    match request.provider {
        BackupProviderKind::Filesystem => {
            let path = required_provider_field(request.path.as_ref(), "Path")?;
            Ok(BackupProviderConfig::Filesystem(FilesystemSettings {
                path: PathBuf::from(path),
            }))
        }
        BackupProviderKind::S3 => {
            let existing_s3 = existing.as_ref().and_then(|provider| {
                if let BackupProviderConfig::S3(settings) = &provider.config {
                    Some(settings)
                } else {
                    None
                }
            });
            let access_key = request
                .access_key
                .as_ref()
                .filter(|value| !value.is_empty())
                .cloned()
                .or_else(|| existing_s3.map(|settings| settings.access_key.clone()))
                .ok_or_else(|| ApiError::bad_request("Access key is required."))?;
            let secret_key = request
                .secret_key
                .as_ref()
                .filter(|value| !value.is_empty())
                .cloned()
                .or_else(|| existing_s3.map(|settings| settings.secret_key.clone()))
                .ok_or_else(|| ApiError::bad_request("Secret key is required."))?;
            let endpoint = required_provider_field(request.endpoint.as_ref(), "Endpoint")?;
            let settings = S3Settings {
                endpoint: Url::parse(endpoint)
                    .map_err(|_| ApiError::bad_request("Endpoint must be a valid URL."))?,
                bucket: required_provider_field(request.bucket.as_ref(), "Bucket")?.to_owned(),
                access_key,
                secret_key,
                region: required_provider_field(request.region.as_ref(), "Region")?.to_owned(),
                prefix: request.prefix.as_deref().unwrap_or("backups").to_owned(),
            };
            crate::config::validate_s3_settings(&settings)
                .map_err(|error| ApiError::bad_request(error.to_string()))?;
            Ok(BackupProviderConfig::S3(settings))
        }
    }
}

async fn consume_provider_test(
    state: &IdentityState,
    user_id: Uuid,
    test_token: Option<Uuid>,
    config: &BackupProviderConfig,
) -> Result<(), ApiError> {
    let test_token = test_token
        .ok_or_else(|| ApiError::bad_request("Test this exact provider before saving."))?;
    if !state
        .consume_tested_backup_provider(test_token, user_id, config)
        .await
    {
        return Err(ApiError::bad_request(
            "The provider test expired or the configuration changed. Test it again before saving.",
        ));
    }
    Ok(())
}

fn required_provider_field<'a>(
    value: Option<&'a String>,
    field: &str,
) -> Result<&'a str, ApiError> {
    value
        .map(String::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| ApiError::bad_request(format!("{field} is required.")))
}

fn backup_provider_response(
    provider: BackupProvider,
    schedule: Option<archive::BackupSchedule>,
) -> BackupProviderResponse {
    let (schedule, next_backup_at) = schedule
        .map(|schedule| (Some(schedule.schedule), Some(schedule.next_backup_at)))
        .unwrap_or((None, None));
    let managed_by_config = provider.source == BackupProviderSource::RuntimeConfig;
    let managed_by_storage = provider.source == BackupProviderSource::StorageTarget;
    match provider.config {
        BackupProviderConfig::Filesystem(settings) => BackupProviderResponse {
            id: provider.id,
            name: provider.name,
            provider: BackupProviderKind::Filesystem,
            managed_by_config,
            managed_by_storage,
            path: Some(settings.path.to_string_lossy().into_owned()),
            endpoint: None,
            bucket: None,
            access_key_hint: None,
            region: None,
            prefix: None,
            schedule,
            next_backup_at,
        },
        BackupProviderConfig::S3(settings) => BackupProviderResponse {
            id: provider.id,
            name: provider.name,
            provider: BackupProviderKind::S3,
            managed_by_config,
            managed_by_storage,
            path: None,
            endpoint: Some(settings.endpoint.to_string()),
            bucket: Some(settings.bucket),
            access_key_hint: Some(access_key_hint(&settings.access_key)),
            region: Some(settings.region),
            prefix: Some(settings.prefix),
            schedule,
            next_backup_at,
        },
    }
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

fn access_key_hint(access_key: &str) -> String {
    let suffix = access_key
        .chars()
        .rev()
        .take(4)
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>();
    format!("ending in {suffix}")
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

pub(super) async fn require_admin(
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
