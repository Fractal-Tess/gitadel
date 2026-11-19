//! Storage administration handles and target workflows.
use crate::blob_store::{
    DomainStorage, DomainUsage, StorageDomain,
    targets::{self, MeasuredUsage, StorageTargetConfiguration, StorageTargetView},
};
use gitadel_config::{S3Settings, Settings};
use gitadel_core::audit::AuditContext;
use gitadel_identity::{AccountService, IdentityError as ServiceError, Principal};
use sea_orm::DatabaseConnection;
use std::{collections::HashMap, path::PathBuf, sync::Arc};
use tokio::sync::{Mutex, RwLock};
use url::Url;
use uuid::Uuid;
pub struct StorageRuntime {
    database: DatabaseConnection,
    settings: Option<Arc<Settings>>,
    pub lfs_storage: Arc<RwLock<Option<Arc<DomainStorage>>>>,
    pub registry_storage: Arc<RwLock<Option<Arc<DomainStorage>>>>,
    measured_storage: Arc<Mutex<HashMap<Uuid, MeasuredUsage>>>,
    registry_domain: &'static dyn StorageDomain,
    registry_usage: &'static dyn DomainUsage,
}
impl StorageRuntime {
    pub fn new(
        database: DatabaseConnection,
        settings: Option<Arc<Settings>>,
        lfs: Option<Arc<DomainStorage>>,
        registry: Option<Arc<DomainStorage>>,
        registry_domain: &'static dyn StorageDomain,
        registry_usage: &'static dyn DomainUsage,
    ) -> Self {
        Self {
            database,
            settings,
            lfs_storage: Arc::new(RwLock::new(lfs)),
            registry_storage: Arc::new(RwLock::new(registry)),
            measured_storage: Default::default(),
            registry_domain,
            registry_usage,
        }
    }
    pub fn service(&self, audit: AuditContext) -> StorageService<'_> {
        StorageService {
            runtime: self,
            audit,
        }
    }
    pub async fn measured_storage(&self) -> HashMap<Uuid, MeasuredUsage> {
        self.measured_storage.lock().await.clone()
    }
    pub async fn record_measured_storage(&self, id: Uuid, usage: MeasuredUsage) {
        self.measured_storage.lock().await.insert(id, usage);
    }
    pub async fn forget_measured_storage(&self, id: Uuid) {
        self.measured_storage.lock().await.remove(&id);
    }
    pub async fn lfs_storage(&self) -> Option<Arc<DomainStorage>> {
        self.lfs_storage.read().await.clone()
    }
    pub async fn registry_storage(&self) -> Option<Arc<DomainStorage>> {
        self.registry_storage.read().await.clone()
    }

    pub(crate) fn registry_domain(&self) -> &dyn StorageDomain {
        self.registry_domain
    }

    pub(crate) fn registry_usage(&self) -> &dyn DomainUsage {
        self.registry_usage
    }
}
pub struct StorageService<'a> {
    runtime: &'a StorageRuntime,
    audit: AuditContext,
}
impl std::ops::Deref for StorageService<'_> {
    type Target = StorageRuntime;
    fn deref(&self) -> &StorageRuntime {
        self.runtime
    }
}
impl StorageService<'_> {
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
    pub async fn require_admin_id(&self, id: Uuid) -> Result<(), ServiceError> {
        let account = gitadel_identity::IdentityService::new(&self.database)
            .enabled_user(id)
            .await?
            .ok_or_else(ServiceError::unauthorized)?;
        if !account.is_admin {
            return Err(ServiceError::forbidden("Administrator access is required."));
        }
        Ok(())
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
}
pub struct StorageTargetRequest {
    pub name: Option<String>,
    pub kind: String,
    pub path: Option<PathBuf>,
    pub endpoint: Option<Url>,
    pub bucket: Option<String>,
    pub access_key: Option<String>,
    pub secret_key: Option<String>,
    pub region: Option<String>,
    pub prefix: Option<String>,
}
pub struct StorageTargetTestResponse {
    pub message: &'static str,
}
impl StorageTargetRequest {
    fn configuration(&self) -> Result<StorageTargetConfiguration, ServiceError> {
        match self.kind.as_str() {
            "filesystem" => Ok(StorageTargetConfiguration::Filesystem {
                path: self
                    .path
                    .clone()
                    .ok_or_else(|| ServiceError::bad_request("A filesystem path is required."))?,
            }),
            "s3" => Ok(StorageTargetConfiguration::S3 {
                s3: S3Settings {
                    endpoint: self
                        .endpoint
                        .clone()
                        .ok_or_else(|| ServiceError::bad_request("An S3 endpoint is required."))?,
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
            _ => Err(ServiceError::bad_request(
                "The storage target kind is invalid.",
            )),
        }
    }
}
fn required_storage_value(
    value: &Option<String>,
    message: &'static str,
) -> Result<String, ServiceError> {
    value
        .as_ref()
        .filter(|value| !value.trim().is_empty())
        .cloned()
        .ok_or_else(|| ServiceError::bad_request(message))
}
pub async fn list_storage_targets(
    state: &StorageService<'_>,
    actor: &Principal,
) -> Result<Vec<StorageTargetView>, ServiceError> {
    let _actor = state
        .require_admin(actor, gitadel_identity::SCOPE_READ)
        .await?;
    let settings = state.runtime_settings()?;
    let measured = state.measured_storage().await;
    targets::list(
        state.database(),
        &settings.storage.lfs_root,
        &measured,
        state.registry_domain(),
    )
    .await
    .map_err(ServiceError::internal)
}
pub async fn measure_storage_target(
    state: &StorageService<'_>,
    actor: &Principal,
    target_id: Uuid,
) -> Result<MeasuredUsage, ServiceError> {
    let _actor = state
        .require_admin(actor, gitadel_identity::SCOPE_READ)
        .await?;
    let configuration = if target_id.is_nil() {
        let settings = state.runtime_settings()?;
        StorageTargetConfiguration::Filesystem {
            path: settings.storage.lfs_root.clone(),
        }
    } else {
        targets::find(state.database(), target_id)
            .await
            .map_err(|error| ServiceError::bad_request(format!("{error:#}")))?
            .1
    };
    let usage = targets::measure(&configuration).await.map_err(|error| {
        ServiceError::bad_request(format!("Could not measure storage: {error:#}"))
    })?;
    state.record_measured_storage(target_id, usage).await;
    Ok(usage)
}
pub async fn test_storage_target(
    state: &StorageService<'_>,
    actor: &Principal,
    request: StorageTargetRequest,
) -> Result<StorageTargetTestResponse, ServiceError> {
    let _actor = state
        .require_admin(actor, gitadel_identity::SCOPE_WRITE)
        .await?;
    targets::test_configuration(&request.configuration()?)
        .await
        .map_err(|error| {
            ServiceError::bad_request(format!("Storage target test failed: {error:#}"))
        })?;
    Ok(StorageTargetTestResponse {
        message: "Storage target passed write, stat, read, and delete checks.",
    })
}
pub async fn test_saved_storage_target(
    state: &StorageService<'_>,
    actor: &Principal,
    target_id: Uuid,
) -> Result<StorageTargetTestResponse, ServiceError> {
    let _actor = state
        .require_admin(actor, gitadel_identity::SCOPE_WRITE)
        .await?;
    let (_, configuration) = targets::find(state.database(), target_id)
        .await
        .map_err(ServiceError::internal)?;
    targets::test_configuration(&configuration)
        .await
        .map_err(|error| {
            ServiceError::bad_request(format!("Storage target test failed: {error:#}"))
        })?;
    Ok(StorageTargetTestResponse {
        message: "Storage target passed write, stat, read, and delete checks.",
    })
}
pub async fn create_storage_target(
    state: &StorageService<'_>,
    actor: &Principal,
    request: StorageTargetRequest,
) -> Result<StorageTargetView, ServiceError> {
    let actor = state
        .require_admin(actor, gitadel_identity::SCOPE_WRITE)
        .await?;
    let name = request
        .name
        .clone()
        .filter(|name| !name.trim().is_empty())
        .ok_or_else(|| ServiceError::bad_request("A storage target name is required."))?;
    let target = targets::create(state.database(), name, request.configuration()?)
        .await
        .map_err(|error| {
            ServiceError::bad_request(format!("Could not create storage target: {error:#}"))
        })?;
    state
        .audit(
            Some(actor.user.id),
            "storage.target.create",
            Some(target.id.to_string()),
        )
        .await?;
    Ok(target)
}
pub async fn delete_storage_target(
    state: &StorageService<'_>,
    actor: &Principal,
    target_id: Uuid,
) -> Result<(), ServiceError> {
    let actor = state
        .require_admin(actor, gitadel_identity::SCOPE_WRITE)
        .await?;
    targets::delete(state.database(), target_id)
        .await
        .map_err(|error| {
            ServiceError::bad_request(format!("Could not delete storage target: {error:#}"))
        })?;
    state.forget_measured_storage(target_id).await;
    state
        .audit(
            Some(actor.user.id),
            "storage.target.delete",
            Some(target_id.to_string()),
        )
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn target_creation_rejects_non_admins_before_touching_storage() {
        let database = gitadel_db::test_support::database().await;
        let user = gitadel_db::test_support::owner(&database, "alice").await;
        let raw =
            gitadel_identity::create_session_on(&database, user.id, chrono::Duration::hours(1))
                .await
                .unwrap();
        let principal = gitadel_identity::IdentityService::new(&database)
            .authenticate_session_principal(&raw)
            .await
            .unwrap();
        let runtime = StorageRuntime::new(
            database,
            None,
            None,
            None,
            &crate::lfs::LfsDomain,
            &crate::lfs::LfsUsage,
        );
        let result = test_storage_target(
            &runtime.service(AuditContext::default()),
            &principal,
            StorageTargetRequest {
                name: None,
                kind: "filesystem".into(),
                path: None,
                endpoint: None,
                bucket: None,
                access_key: None,
                secret_key: None,
                region: None,
                prefix: None,
            },
        )
        .await;
        assert!(matches!(result, Err(ServiceError::Forbidden(_))));
    }
}
