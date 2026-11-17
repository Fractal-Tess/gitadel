//! Backup administration, provider proofs and restore reservations.
use crate::{
    archive::{self, MaintenanceAction},
    backup_provider::{
        self, BackupProvider, BackupProviderConfig, BackupProviderKind, BackupProviderSource,
        FilesystemSettings, RUNTIME_S3_PROVIDER_ID,
    },
};
use chrono::{DateTime, Utc};
use gitadel_config::{S3Settings, Settings};
use gitadel_core::audit::AuditContext;
use gitadel_identity::{AccountService, IdentityError as ServiceError, Principal};
use sea_orm::DatabaseConnection;
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::{Mutex, mpsc};
use url::Url;
use uuid::Uuid;

const VALIDATED_BACKUP_LIFETIME: Duration = Duration::from_secs(30 * 60);
const TESTED_BACKUP_PROVIDER_LIFETIME: Duration = Duration::from_secs(10 * 60);
pub struct BackupRuntime {
    database: DatabaseConnection,
    settings: Option<Arc<Settings>>,
    maintenance_sender: Option<mpsc::Sender<MaintenanceAction>>,
    maintenance_pending: Arc<Mutex<bool>>,
    validated_backups: Arc<Mutex<HashMap<Uuid, ValidatedBackup>>>,
    tested_backup_providers: Arc<Mutex<HashMap<Uuid, TestedBackupProvider>>>,
}
impl BackupRuntime {
    pub fn new(
        database: DatabaseConnection,
        settings: Option<Arc<Settings>>,
        maintenance_sender: Option<mpsc::Sender<MaintenanceAction>>,
    ) -> Self {
        Self {
            database,
            settings,
            maintenance_sender,
            maintenance_pending: Default::default(),
            validated_backups: Default::default(),
            tested_backup_providers: Default::default(),
        }
    }
    pub fn service(&self, audit: AuditContext) -> BackupService<'_> {
        BackupService {
            runtime: self,
            audit,
        }
    }
}
pub struct BackupService<'a> {
    runtime: &'a BackupRuntime,
    audit: AuditContext,
}
impl std::ops::Deref for BackupService<'_> {
    type Target = BackupRuntime;
    fn deref(&self) -> &BackupRuntime {
        self.runtime
    }
}
impl BackupService<'_> {
    pub fn database(&self) -> &DatabaseConnection {
        &self.database
    }
    pub fn runtime_settings(&self) -> Result<&Settings, ServiceError> {
        self.settings
            .as_deref()
            .ok_or_else(|| ServiceError::internal("runtime settings are unavailable"))
    }
    pub async fn require_admin(
        &self,
        actor: &Principal,
        scope: i32,
    ) -> Result<gitadel_identity::AuthenticatedUser, ServiceError> {
        AccountService::new(&self.database, self.audit)
            .require_actor(actor, true, scope)
            .await
    }
    pub async fn audit(
        &self,
        actor: Option<Uuid>,
        action: impl Into<String>,
        target: Option<String>,
    ) -> Result<(), ServiceError> {
        gitadel_db::audit::record(self.database(), actor, action, target, self.audit).await?;
        Ok(())
    }
    pub(crate) async fn cache_validated_backup(
        &self,
        key: String,
        path: PathBuf,
        provider: BackupProvider,
    ) -> Uuid {
        let mut backups = self.validated_backups.lock().await;
        backups.retain(|_, backup| {
            if backup.validated_at.elapsed() <= VALIDATED_BACKUP_LIFETIME {
                true
            } else {
                let _ = std::fs::remove_file(&backup.path);
                false
            }
        });
        let token = Uuid::new_v4();
        backups.insert(
            token,
            ValidatedBackup {
                key,
                path,
                provider,
                validated_at: Instant::now(),
            },
        );
        token
    }
    pub(crate) async fn take_validated_backup(&self, token: Uuid) -> Option<ValidatedBackup> {
        let backup = self.validated_backups.lock().await.remove(&token)?;
        if backup.validated_at.elapsed() > VALIDATED_BACKUP_LIFETIME {
            let _ = std::fs::remove_file(backup.path);
            return None;
        }
        Some(backup)
    }
    pub(crate) async fn cache_tested_backup_provider(
        &self,
        user_id: Uuid,
        config: BackupProviderConfig,
    ) -> Uuid {
        let mut tested = self.tested_backup_providers.lock().await;
        tested.retain(|_, proof| proof.tested_at.elapsed() <= TESTED_BACKUP_PROVIDER_LIFETIME);
        let token = Uuid::new_v4();
        tested.insert(
            token,
            TestedBackupProvider {
                user_id,
                config,
                tested_at: Instant::now(),
            },
        );
        token
    }
    pub(crate) async fn consume_tested_backup_provider(
        &self,
        token: Uuid,
        user_id: Uuid,
        config: &BackupProviderConfig,
    ) -> bool {
        let Some(proof) = self.tested_backup_providers.lock().await.remove(&token) else {
            return false;
        };
        proof.tested_at.elapsed() <= TESTED_BACKUP_PROVIDER_LIFETIME
            && proof.user_id == user_id
            && proof.config == *config
    }
    pub async fn schedule_maintenance(
        &self,
        action: MaintenanceAction,
    ) -> Result<(), ServiceError> {
        let sender = self
            .maintenance_sender
            .clone()
            .ok_or_else(|| ServiceError::internal("maintenance operations are unavailable"))?;
        let mut pending = self.maintenance_pending.lock().await;
        if *pending {
            return Err(ServiceError::conflict(
                "Another backup or restore operation is already starting.",
            ));
        }
        *pending = true;
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(750)).await;
            if sender.send(action).await.is_err() {
                tracing::error!("could not schedule maintenance operation");
            }
        });
        Ok(())
    }
}
pub(crate) struct ValidatedBackup {
    pub key: String,
    pub path: PathBuf,
    pub provider: BackupProvider,
    validated_at: Instant,
}
struct TestedBackupProvider {
    user_id: Uuid,
    config: BackupProviderConfig,
    tested_at: Instant,
}
pub struct BackupProviderCatalogItem {
    pub slug: BackupProviderKind,
    pub name: &'static str,
    pub description: &'static str,
}
pub struct BackupProvidersResponse {
    pub providers: Vec<BackupProviderCatalogItem>,
    pub connections: Vec<BackupProviderResponse>,
}
pub struct BackupProviderResponse {
    pub id: Uuid,
    pub name: String,
    pub provider: BackupProviderKind,
    pub managed_by_config: bool,
    pub managed_by_storage: bool,
    pub path: Option<String>,
    pub endpoint: Option<String>,
    pub bucket: Option<String>,
    pub access_key_hint: Option<String>,
    pub region: Option<String>,
    pub prefix: Option<String>,
    pub schedule: Option<String>,
    pub next_backup_at: Option<DateTime<Utc>>,
}
pub struct UpdateBackupProviderRequest {
    pub id: Option<Uuid>,
    pub name: String,
    pub provider: BackupProviderKind,
    pub path: Option<String>,
    pub endpoint: Option<String>,
    pub bucket: Option<String>,
    pub access_key: Option<String>,
    pub secret_key: Option<String>,
    pub region: Option<String>,
    pub prefix: Option<String>,
    pub test_token: Option<Uuid>,
}
pub struct UpdateBackupScheduleRequest {
    pub schedule: Option<String>,
}
pub struct BackupProviderTestResponse {
    pub test_token: Uuid,
    pub message: &'static str,
}
pub struct CreateBackupRequest {
    pub name: Option<String>,
}
pub struct BackupScheduledResponse {
    pub key: String,
    pub operation_id: Uuid,
    pub message: &'static str,
}
pub struct RestorePreflightResponse {
    pub token: Uuid,
    pub key: String,
    pub inspection: archive::BackupInspection,
    pub required_free_space: u64,
    pub available_free_space: u64,
}
pub struct RestorePreflightRequest {
    pub key: String,
}
pub struct BackupObjectRequest {
    pub key: String,
}
pub struct RestoreBackupRequest {
    pub token: Uuid,
    pub password: String,
    pub create_safety_backup: bool,
}
pub(crate) async fn required_backup_provider(
    state: &BackupService<'_>,
    provider_id: Uuid,
) -> Result<BackupProvider, ServiceError> {
    let settings = state.runtime_settings()?;
    backup_provider::load_with_storage(
        state.database(),
        settings.backup.s3.as_ref(),
        &settings.storage.lfs_root,
        provider_id,
    )
    .await
    .map_err(ServiceError::internal)?
    .ok_or_else(ServiceError::not_found)
}
pub(crate) async fn requested_backup_provider_config(
    state: &BackupService<'_>,
    request: &UpdateBackupProviderRequest,
) -> Result<BackupProviderConfig, ServiceError> {
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
                .ok_or_else(|| ServiceError::bad_request("Access key is required."))?;
            let secret_key = request
                .secret_key
                .as_ref()
                .filter(|value| !value.is_empty())
                .cloned()
                .or_else(|| existing_s3.map(|settings| settings.secret_key.clone()))
                .ok_or_else(|| ServiceError::bad_request("Secret key is required."))?;
            let endpoint = required_provider_field(request.endpoint.as_ref(), "Endpoint")?;
            let settings = S3Settings {
                endpoint: Url::parse(endpoint)
                    .map_err(|_| ServiceError::bad_request("Endpoint must be a valid URL."))?,
                bucket: required_provider_field(request.bucket.as_ref(), "Bucket")?.to_owned(),
                access_key,
                secret_key,
                region: required_provider_field(request.region.as_ref(), "Region")?.to_owned(),
                prefix: request.prefix.as_deref().unwrap_or("backups").to_owned(),
            };
            gitadel_config::validate_s3_settings(&settings)
                .map_err(|error| ServiceError::bad_request(error.to_string()))?;
            Ok(BackupProviderConfig::S3(settings))
        }
    }
}
pub(crate) async fn consume_provider_test(
    state: &BackupService<'_>,
    user_id: Uuid,
    test_token: Option<Uuid>,
    config: &BackupProviderConfig,
) -> Result<(), ServiceError> {
    let test_token = test_token
        .ok_or_else(|| ServiceError::bad_request("Test this exact provider before saving."))?;
    if !state
        .consume_tested_backup_provider(test_token, user_id, config)
        .await
    {
        return Err(ServiceError::bad_request(
            "The provider test expired or the configuration changed. Test it again before saving.",
        ));
    }
    Ok(())
}
pub(crate) fn required_provider_field<'a>(
    value: Option<&'a String>,
    field: &str,
) -> Result<&'a str, ServiceError> {
    value
        .map(String::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| ServiceError::bad_request(format!("{field} is required.")))
}
pub(crate) fn backup_provider_response(
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
pub(crate) fn access_key_hint(access_key: &str) -> String {
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
pub async fn list_backup_providers(
    state: &BackupService<'_>,
    actor: &Principal,
) -> Result<BackupProvidersResponse, ServiceError> {
    let _actor = state
        .require_admin(actor, gitadel_identity::SCOPE_READ)
        .await?;
    let settings = state.runtime_settings()?;
    let providers = backup_provider::list_with_storage(
        state.database(),
        settings.backup.s3.as_ref(),
        &settings.storage.lfs_root,
    )
    .await
    .map_err(ServiceError::internal)?;
    let schedules = archive::list_backup_schedules(state.database())
        .await
        .map_err(ServiceError::internal)?;
    Ok(BackupProvidersResponse {
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
    })
}
pub async fn test_backup_provider(
    state: &BackupService<'_>,
    actor: &Principal,
    request: UpdateBackupProviderRequest,
) -> Result<BackupProviderTestResponse, ServiceError> {
    let actor = state
        .require_admin(actor, gitadel_identity::SCOPE_WRITE)
        .await?;
    let config = requested_backup_provider_config(state, &request).await?;
    archive::test_backup_provider(&config, state.runtime_settings()?)
        .await
        .map_err(|error| ServiceError::bad_request(error.to_string()))?;
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
    Ok(BackupProviderTestResponse {
        test_token,
        message: "Backup provider is available and writable.",
    })
}
pub async fn create_backup_provider(
    state: &BackupService<'_>,
    actor: &Principal,
    request: UpdateBackupProviderRequest,
) -> Result<BackupProviderResponse, ServiceError> {
    let actor = state
        .require_admin(actor, gitadel_identity::SCOPE_WRITE)
        .await?;
    if request.id.is_some() {
        return Err(ServiceError::bad_request(
            "A new backup provider cannot include an existing provider ID.",
        ));
    }
    let config = requested_backup_provider_config(state, &request).await?;
    consume_provider_test(state, actor.user.id, request.test_token, &config).await?;
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
        .map_err(|error| ServiceError::bad_request(error.to_string()))?;
    state
        .audit(
            Some(actor.user.id),
            "backup.provider.create",
            Some(provider.id.to_string()),
        )
        .await?;
    Ok(backup_provider_response(provider, None))
}
pub async fn update_backup_provider(
    state: &BackupService<'_>,
    actor: &Principal,
    provider_id: Uuid,
    request: UpdateBackupProviderRequest,
) -> Result<BackupProviderResponse, ServiceError> {
    let actor = state
        .require_admin(actor, gitadel_identity::SCOPE_WRITE)
        .await?;
    if request.id != Some(provider_id) {
        return Err(ServiceError::bad_request(
            "The backup provider ID does not match the requested resource.",
        ));
    }
    let mut existing = required_backup_provider(state, provider_id).await?;
    if existing.source == BackupProviderSource::StorageTarget {
        return Err(ServiceError::bad_request(
            "Storage targets are configured on the Storage page.",
        ));
    }
    let config = requested_backup_provider_config(state, &request).await?;
    consume_provider_test(state, actor.user.id, request.test_token, &config).await?;
    if existing.id == RUNTIME_S3_PROVIDER_ID {
        existing.id = Uuid::new_v4();
        existing.created_at = Utc::now();
    }
    existing.name = request.name.trim().to_owned();
    existing.config = config;
    existing.updated_at = Utc::now();
    backup_provider::save(state.database(), &existing)
        .await
        .map_err(|error| ServiceError::bad_request(error.to_string()))?;
    state
        .audit(
            Some(actor.user.id),
            "backup.provider.update",
            Some(existing.id.to_string()),
        )
        .await?;
    let schedule = archive::load_backup_schedule(state.database(), existing.id)
        .await
        .map_err(ServiceError::internal)?;
    Ok(backup_provider_response(existing, schedule))
}
pub async fn delete_backup_provider(
    state: &BackupService<'_>,
    actor: &Principal,
    provider_id: Uuid,
) -> Result<(), ServiceError> {
    let actor = state
        .require_admin(actor, gitadel_identity::SCOPE_WRITE)
        .await?;
    let provider = required_backup_provider(state, provider_id).await?;
    match provider.source {
        BackupProviderSource::Stored => {
            if !backup_provider::delete(state.database(), provider_id)
                .await
                .map_err(ServiceError::internal)?
            {
                return Err(ServiceError::not_found());
            }
        }
        BackupProviderSource::StorageTarget => {
            backup_provider::exclude_storage_target(state.database(), provider_id)
                .await
                .map_err(ServiceError::internal)?;
        }
        BackupProviderSource::RuntimeConfig => {
            return Err(ServiceError::not_found());
        }
    }
    state
        .audit(
            Some(actor.user.id),
            "backup.provider.delete",
            Some(provider_id.to_string()),
        )
        .await?;
    Ok(())
}
pub async fn update_backup_schedule(
    state: &BackupService<'_>,
    actor: &Principal,
    provider_id: Uuid,
    request: UpdateBackupScheduleRequest,
) -> Result<BackupProviderResponse, ServiceError> {
    let actor = state
        .require_admin(actor, gitadel_identity::SCOPE_WRITE)
        .await?;
    let mut provider = required_backup_provider(state, provider_id).await?;
    match provider.source {
        BackupProviderSource::RuntimeConfig => {
            provider.id = Uuid::new_v4();
            provider.created_at = Utc::now();
            provider.updated_at = provider.created_at;
            provider.source = BackupProviderSource::Stored;
            backup_provider::save(state.database(), &provider)
                .await
                .map_err(ServiceError::internal)?;
        }
        BackupProviderSource::StorageTarget => {
            let mut persisted = provider.clone();
            persisted.source = BackupProviderSource::Stored;
            backup_provider::save(state.database(), &persisted)
                .await
                .map_err(ServiceError::internal)?;
        }
        BackupProviderSource::Stored => {}
    }
    archive::validate_backup_schedule(request.schedule.as_deref())
        .map_err(|error| ServiceError::bad_request(error.to_string()))?;
    let schedule =
        archive::save_backup_schedule(state.database(), provider.id, request.schedule.as_deref())
            .await
            .map_err(ServiceError::internal)?;
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
    Ok(backup_provider_response(provider, schedule))
}
pub async fn list_backups(
    state: &BackupService<'_>,
    actor: &Principal,
    provider_id: Uuid,
) -> Result<Vec<archive::BackupObject>, ServiceError> {
    let _actor = state
        .require_admin(actor, gitadel_identity::SCOPE_READ)
        .await?;
    let provider = required_backup_provider(state, provider_id).await?;
    let backups = archive::list_backups(&provider.config, state.runtime_settings()?)
        .await
        .map_err(ServiceError::internal)?;
    Ok(backups)
}
pub async fn delete_backup(
    state: &BackupService<'_>,
    actor: &Principal,
    provider_id: Uuid,
    request: BackupObjectRequest,
) -> Result<(), ServiceError> {
    let actor = state
        .require_admin(actor, gitadel_identity::SCOPE_WRITE)
        .await?;
    let provider = required_backup_provider(state, provider_id).await?;
    archive::delete_backup(&provider.config, state.runtime_settings()?, &request.key)
        .await
        .map_err(ServiceError::internal)?;
    state
        .audit(Some(actor.user.id), "backup.delete", Some(request.key))
        .await?;
    Ok(())
}
pub async fn create_backup(
    state: &BackupService<'_>,
    actor: &Principal,
    provider_id: Uuid,
    request: CreateBackupRequest,
) -> Result<BackupScheduledResponse, ServiceError> {
    let actor = state
        .require_admin(actor, gitadel_identity::SCOPE_WRITE)
        .await?;
    let provider = required_backup_provider(state, provider_id).await?;
    let name = archive::validate_backup_name(request.name.as_deref())
        .map_err(|error| ServiceError::bad_request(error.to_string()))?;
    let key = archive::new_backup_key(&provider.config, name.as_deref())
        .map_err(ServiceError::internal)?;
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
    Ok(BackupScheduledResponse {
        operation_id,
        key,
        message: "Gitadel is restarting to create the backup.",
    })
}
pub async fn preflight_restore(
    state: &BackupService<'_>,
    actor: &Principal,
    provider_id: Uuid,
    request: RestorePreflightRequest,
) -> Result<RestorePreflightResponse, ServiceError> {
    let key = request.key;
    let _actor = state
        .require_admin(actor, gitadel_identity::SCOPE_WRITE)
        .await?;
    let mut provider = required_backup_provider(state, provider_id).await?;
    if provider.id == RUNTIME_S3_PROVIDER_ID {
        provider.id = Uuid::new_v4();
        provider.created_at = Utc::now();
        provider.updated_at = provider.created_at;
    }
    let runtime = state.runtime_settings()?;
    let (path, inspection) = archive::download_and_inspect_backup(runtime, &provider.config, &key)
        .await
        .map_err(|error| ServiceError::bad_request(error.to_string()))?;
    let available_free_space =
        archive::restore_available_space(runtime).map_err(ServiceError::internal)?;
    let required_free_space = inspection.uncompressed_size.saturating_mul(2);
    if available_free_space < required_free_space {
        let _ = std::fs::remove_file(path);
        return Err(ServiceError::bad_request(format!(
            "Restore needs at least {required_free_space} bytes free; {available_free_space} bytes are available."
        )));
    }
    let token = state
        .cache_validated_backup(key.clone(), path, provider)
        .await;
    Ok(RestorePreflightResponse {
        token,
        key,
        inspection,
        required_free_space,
        available_free_space,
    })
}
pub async fn restore_backup(
    state: &BackupService<'_>,
    actor: &Principal,
    _provider_id: Uuid,
    request: RestoreBackupRequest,
) -> Result<BackupScheduledResponse, ServiceError> {
    let actor = state
        .require_admin(actor, gitadel_identity::SCOPE_WRITE)
        .await?;
    if actor.via_api_token {
        return Err(ServiceError::forbidden(
            "Restore requires an interactive administrator session.",
        ));
    }
    if !gitadel_identity::verify_password(request.password, actor.user.password_hash.clone())
        .await?
    {
        return Err(ServiceError::forbidden("Password confirmation failed."));
    }
    let backup = state
        .take_validated_backup(request.token)
        .await
        .ok_or_else(|| {
            ServiceError::bad_request("The validated backup expired. Validate it again.")
        })?;
    let operation_id = Uuid::new_v4();
    let safety_key = if request.create_safety_backup {
        Some(
            archive::new_backup_key(&backup.provider.config, Some("pre-restore-safety"))
                .map_err(ServiceError::internal)?,
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
    Ok(BackupScheduledResponse {
        key: backup.key,
        operation_id,
        message: "Gitadel is restarting to restore the selected backup.",
    })
}
pub enum BackupDownload {
    File(tokio::fs::File, u64),
    S3(archive::S3BackupDownload),
}
pub async fn download_backup(
    state: &BackupService<'_>,
    actor: &Principal,
    provider_id: Uuid,
    key: &str,
) -> Result<BackupDownload, ServiceError> {
    let actor = state
        .require_admin(actor, gitadel_identity::SCOPE_WRITE)
        .await?;
    let provider = required_backup_provider(state, provider_id).await?;
    let download = match &provider.config {
        BackupProviderConfig::Filesystem(fs) => {
            let (file, size) = archive::open_filesystem_backup(state.runtime_settings()?, fs, key)
                .await
                .map_err(ServiceError::internal)?;
            BackupDownload::File(file, size)
        }
        BackupProviderConfig::S3(s3) => BackupDownload::S3(
            archive::stream_s3_backup(s3, key)
                .await
                .map_err(ServiceError::internal)?,
        ),
    };
    state
        .audit(Some(actor.user.id), "backup.download", Some(key.to_owned()))
        .await?;
    Ok(download)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gitadel_db::test_support::database;

    fn provider() -> BackupProvider {
        BackupProvider {
            id: Uuid::new_v4(),
            name: "fixture".into(),
            config: BackupProviderConfig::Filesystem(FilesystemSettings {
                path: PathBuf::from("unused"),
            }),
            source: BackupProviderSource::Stored,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[tokio::test]
    async fn provider_proofs_are_one_use_and_bind_both_actor_and_configuration() {
        let runtime = BackupRuntime::new(database().await, None, None);
        let service = runtime.service(AuditContext::default());
        let actor = Uuid::new_v4();
        let config = provider().config;
        let token = service
            .cache_tested_backup_provider(actor, config.clone())
            .await;
        assert!(
            !service
                .consume_tested_backup_provider(token, Uuid::new_v4(), &config)
                .await
        );
        assert!(
            !service
                .consume_tested_backup_provider(token, actor, &config)
                .await
        );
        let token = service
            .cache_tested_backup_provider(actor, config.clone())
            .await;
        assert!(
            service
                .consume_tested_backup_provider(token, actor, &config)
                .await
        );
        assert!(
            !service
                .consume_tested_backup_provider(token, actor, &config)
                .await
        );
    }

    #[tokio::test]
    async fn restore_keeps_the_cached_provider_and_reserves_maintenance_with_a_delay() {
        let database = database().await;
        let account =
            gitadel_identity::bootstrap_admin(&database, "admin", "long fixture password".into())
                .await
                .unwrap();
        let raw =
            gitadel_identity::create_session_on(&database, account.id, chrono::Duration::hours(1))
                .await
                .unwrap();
        let actor = gitadel_identity::IdentityService::new(&database)
            .authenticate_session_principal(&raw)
            .await
            .unwrap();
        let (sender, mut receiver) = mpsc::channel(1);
        let runtime = BackupRuntime::new(database, None, Some(sender));
        let service = runtime.service(AuditContext::default());
        let path = std::env::temp_dir().join(format!("gitadel-proof-{}", Uuid::new_v4()));
        std::fs::write(&path, b"fixture").unwrap();
        let cached = provider();
        let provider_id = cached.id;
        let token = service
            .cache_validated_backup("archive.tar.zst".into(), path.clone(), cached)
            .await;
        restore_backup(
            &service,
            &actor,
            Uuid::new_v4(),
            RestoreBackupRequest {
                token,
                password: "long fixture password".into(),
                create_safety_backup: false,
            },
        )
        .await
        .unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(100), receiver.recv())
                .await
                .is_err()
        );
        let action = receiver.recv().await.unwrap();
        assert!(
            matches!(action,MaintenanceAction::Restore{provider,..} if provider.id==provider_id)
        );
        assert!(matches!(
            service
                .schedule_maintenance(MaintenanceAction::Create {
                    operation_id: Uuid::new_v4(),
                    key: "next".into(),
                    provider: provider(),
                    name: None
                })
                .await,
            Err(ServiceError::Conflict(_))
        ));
        std::fs::remove_file(path).unwrap();
    }
}
