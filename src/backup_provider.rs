use std::{collections::HashSet, path::PathBuf};

use anyhow::{Context, Result, ensure};
use chrono::{DateTime, Utc};
use sea_orm::{ActiveModelTrait as _, DatabaseConnection, EntityTrait as _, Set};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    blob_store::targets::StorageTargetConfiguration,
    config::{S3Settings, validate_s3_settings},
    entity::{backup_provider, backup_provider_exclusion, lfs_storage_target},
};

pub const RUNTIME_S3_PROVIDER_ID: Uuid = Uuid::nil();

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BackupProviderKind {
    Filesystem,
    S3,
}

impl BackupProviderKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Filesystem => "filesystem",
            Self::S3 => "s3",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct FilesystemSettings {
    pub path: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BackupProviderConfig {
    Filesystem(FilesystemSettings),
    S3(S3Settings),
}

impl BackupProviderConfig {
    pub fn kind(&self) -> BackupProviderKind {
        match self {
            Self::Filesystem(_) => BackupProviderKind::Filesystem,
            Self::S3(_) => BackupProviderKind::S3,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackupProviderSource {
    Stored,
    RuntimeConfig,
    StorageTarget,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackupProvider {
    pub id: Uuid,
    pub name: String,
    pub config: BackupProviderConfig,
    pub source: BackupProviderSource,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub async fn list(
    database: &DatabaseConnection,
    fallback_s3: Option<&S3Settings>,
) -> Result<Vec<BackupProvider>> {
    let mut providers = backup_provider::Entity::find()
        .all(database)
        .await?
        .into_iter()
        .map(from_model)
        .collect::<Result<Vec<_>>>()?;
    providers.sort_unstable_by(|left, right| {
        left.created_at
            .cmp(&right.created_at)
            .then_with(|| left.name.cmp(&right.name))
    });
    if let Some(settings) = fallback_s3
        && !providers.iter().any(
            |provider| matches!(&provider.config, BackupProviderConfig::S3(stored) if stored == settings),
        )
    {
        validate_s3_settings(settings)?;
        providers.push(runtime_s3_provider(settings));
    }
    Ok(providers)
}
pub async fn list_with_storage(
    database: &DatabaseConnection,
    _fallback_s3: Option<&S3Settings>,
    _fallback_path: &std::path::Path,
) -> Result<Vec<BackupProvider>> {
    let excluded = backup_provider_exclusion::Entity::find()
        .all(database)
        .await?
        .into_iter()
        .map(|exclusion| exclusion.provider_id)
        .collect::<HashSet<_>>();
    let mut providers = storage_providers(database, &excluded).await?;
    for provider in list(database, None).await? {
        if !providers
            .iter()
            .any(|candidate| candidate.id == provider.id)
        {
            providers.push(provider);
        }
    }
    Ok(providers)
}

pub async fn load(
    database: &DatabaseConnection,
    fallback_s3: Option<&S3Settings>,
    id: Uuid,
) -> Result<Option<BackupProvider>> {
    if let Some(model) = backup_provider::Entity::find_by_id(id)
        .one(database)
        .await?
    {
        return from_model(model).map(Some);
    }
    if id == RUNTIME_S3_PROVIDER_ID
        && let Some(settings) = fallback_s3
    {
        validate_s3_settings(settings)?;
        return Ok(Some(runtime_s3_provider(settings)));
    }
    Ok(None)
}
pub async fn load_with_storage(
    database: &DatabaseConnection,
    _fallback_s3: Option<&S3Settings>,
    _fallback_path: &std::path::Path,
    id: Uuid,
) -> Result<Option<BackupProvider>> {
    if backup_provider_exclusion::Entity::find_by_id(id)
        .one(database)
        .await?
        .is_none()
        && let Some(target) = lfs_storage_target::Entity::find_by_id(id)
            .one(database)
            .await?
    {
        return storage_target_provider(target).map(Some);
    }
    load(database, None, id).await
}

pub async fn save(database: &DatabaseConnection, provider: &BackupProvider) -> Result<()> {
    validate(provider)?;
    let configuration = serde_json::to_string(&provider.config)
        .context("could not serialize backup provider configuration")?;
    match backup_provider::Entity::find_by_id(provider.id)
        .one(database)
        .await?
    {
        Some(stored) => {
            let mut active: backup_provider::ActiveModel = stored.into();
            active.name = Set(provider.name.clone());
            active.provider = Set(provider.config.kind().as_str().to_owned());
            active.configuration = Set(configuration);
            active.updated_at = Set(provider.updated_at);
            active.update(database).await?;
        }
        None => {
            backup_provider::ActiveModel {
                id: Set(provider.id),
                name: Set(provider.name.clone()),
                provider: Set(provider.config.kind().as_str().to_owned()),
                configuration: Set(configuration),
                created_at: Set(provider.created_at),
                updated_at: Set(provider.updated_at),
            }
            .insert(database)
            .await?;
        }
    }
    Ok(())
}

pub async fn delete(database: &DatabaseConnection, id: Uuid) -> Result<bool> {
    Ok(backup_provider::Entity::delete_by_id(id)
        .exec(database)
        .await?
        .rows_affected
        > 0)
}

pub async fn exclude_storage_target(database: &DatabaseConnection, id: Uuid) -> Result<()> {
    backup_provider_exclusion::ActiveModel {
        provider_id: Set(id),
        created_at: Set(Utc::now()),
    }
    .insert(database)
    .await?;
    Ok(())
}

pub fn validate(provider: &BackupProvider) -> Result<()> {
    let name = provider.name.trim();
    ensure!(!name.is_empty(), "backup provider name must not be empty");
    ensure!(
        name.chars().count() <= 80,
        "backup provider name cannot exceed 80 characters"
    );
    match &provider.config {
        BackupProviderConfig::Filesystem(settings) => {
            ensure!(
                settings.path.is_absolute(),
                "backup filesystem path must be absolute"
            );
        }
        BackupProviderConfig::S3(settings) => validate_s3_settings(settings)?,
    }
    Ok(())
}

fn from_model(model: backup_provider::Model) -> Result<BackupProvider> {
    let config = serde_json::from_str::<BackupProviderConfig>(&model.configuration)
        .context("stored backup provider configuration is invalid")?;
    ensure!(
        model.provider == config.kind().as_str(),
        "stored backup provider type does not match its configuration"
    );
    let provider = BackupProvider {
        id: model.id,
        name: model.name,
        config,
        created_at: model.created_at,
        updated_at: model.updated_at,
        source: BackupProviderSource::Stored,
    };
    validate(&provider)?;
    Ok(provider)
}

fn runtime_s3_provider(settings: &S3Settings) -> BackupProvider {
    let now = Utc::now();
    BackupProvider {
        id: RUNTIME_S3_PROVIDER_ID,
        name: "S3 backups".to_owned(),
        config: BackupProviderConfig::S3(settings.clone()),
        created_at: now,
        source: BackupProviderSource::RuntimeConfig,
        updated_at: now,
    }
}

async fn storage_providers(
    database: &DatabaseConnection,
    excluded: &HashSet<Uuid>,
) -> Result<Vec<BackupProvider>> {
    let mut providers = Vec::new();
    for target in lfs_storage_target::Entity::find().all(database).await? {
        if !excluded.contains(&target.id) {
            providers.push(storage_target_provider(target)?);
        }
    }
    Ok(providers)
}

fn absolute_path(path: &std::path::Path) -> Result<PathBuf> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    Ok(std::env::current_dir()
        .context("could not resolve the configured storage path")?
        .join(path))
}

fn filesystem_backup_path(path: &std::path::Path) -> Result<PathBuf> {
    let path = absolute_path(path)?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("gitadel-storage");
    Ok(path.with_file_name(format!("{name}-backups")))
}

fn storage_target_provider(target: lfs_storage_target::Model) -> Result<BackupProvider> {
    let configuration = serde_json::from_str::<StorageTargetConfiguration>(&target.configuration)
        .context("stored LFS target configuration is invalid")?;
    let config = match configuration {
        StorageTargetConfiguration::Filesystem { path } => {
            BackupProviderConfig::Filesystem(FilesystemSettings {
                path: filesystem_backup_path(&path)?,
            })
        }
        StorageTargetConfiguration::S3 { mut s3 } => {
            s3.prefix = format!("{}/backups", s3.prefix.trim_end_matches('/'));
            BackupProviderConfig::S3(s3)
        }
    };
    Ok(BackupProvider {
        id: target.id,
        name: target.name,
        config,
        created_at: target.created_at,
        updated_at: target.updated_at,
        source: BackupProviderSource::StorageTarget,
    })
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path};

    use crate::{config::Settings, database};

    use super::*;

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Result<Self> {
            let path =
                std::env::temp_dir().join(format!("gitadel-provider-test-{}", Uuid::new_v4()));
            fs::create_dir(&path)?;
            Ok(Self(path))
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[tokio::test]
    async fn stored_providers_round_trip_without_hiding_distinct_runtime_s3() -> Result<()> {
        let directory = TestDirectory::new()?;
        let mut settings = Settings::default();
        settings.database.url = format!(
            "sqlite://{}?mode=rwc",
            directory.path().join("gitadel.db").display()
        );
        let database = database::connect_and_migrate(&settings.database).await?;
        let now = Utc::now();
        let filesystem = BackupProvider {
            id: Uuid::new_v4(),
            name: "Local".to_owned(),
            config: BackupProviderConfig::Filesystem(FilesystemSettings {
                path: directory.path().join("backups"),
            }),
            source: BackupProviderSource::Stored,
            created_at: now,
            updated_at: now,
        };
        save(&database, &filesystem).await?;
        let runtime_s3 = S3Settings {
            endpoint: "https://s3.example.com".parse()?,
            bucket: "gitadel".to_owned(),
            access_key: "access".to_owned(),
            secret_key: "secret".to_owned(),
            region: "us-east-1".to_owned(),
            prefix: "backups".to_owned(),
        };

        let with_runtime = list(&database, Some(&runtime_s3)).await?;
        assert_eq!(with_runtime.len(), 2);
        let distinct_s3 = BackupProvider {
            id: Uuid::new_v4(),
            name: "Stored S3".to_owned(),
            config: BackupProviderConfig::S3(S3Settings {
                prefix: "secondary".to_owned(),
                ..runtime_s3.clone()
            }),
            source: BackupProviderSource::Stored,
            created_at: now,
            updated_at: now,
        };
        save(&database, &distinct_s3).await?;

        let with_distinct_s3 = list(&database, Some(&runtime_s3)).await?;
        assert_eq!(with_distinct_s3.len(), 3);
        assert!(
            with_distinct_s3
                .iter()
                .any(|provider| provider.id == RUNTIME_S3_PROVIDER_ID)
        );

        let converted_s3 = BackupProvider {
            id: Uuid::new_v4(),
            name: "Converted S3".to_owned(),
            config: BackupProviderConfig::S3(runtime_s3.clone()),
            source: BackupProviderSource::Stored,
            created_at: now,
            updated_at: now,
        };
        save(&database, &converted_s3).await?;

        let providers = list(&database, Some(&runtime_s3)).await?;
        assert_eq!(providers.len(), 3);
        assert!(
            providers
                .iter()
                .any(|provider| provider.id == filesystem.id)
        );
        assert!(
            providers
                .iter()
                .any(|provider| provider.id == distinct_s3.id)
        );
        assert!(
            providers
                .iter()
                .any(|provider| provider.id == converted_s3.id)
        );
        database.close().await?;
        Ok(())
    }

    #[tokio::test]
    async fn storage_targets_are_available_as_backup_providers() -> Result<()> {
        let directory = TestDirectory::new()?;
        let mut settings = Settings::default();
        settings.database.url = format!(
            "sqlite://{}?mode=rwc",
            directory.path().join("gitadel.db").display()
        );
        let database = database::connect_and_migrate(&settings.database).await?;
        let lfs_root = directory.path().join("configured-lfs");
        assert!(
            list_with_storage(&database, None, &lfs_root)
                .await?
                .is_empty(),
            "Gitadel must not create a default backup provider"
        );
        let target_root = directory.path().join("secondary-lfs");
        fs::create_dir(&target_root)?;
        let target = crate::blob_store::targets::create(
            &database,
            "Secondary".to_owned(),
            StorageTargetConfiguration::Filesystem {
                path: target_root.clone(),
            },
        )
        .await?;
        let target_backup_root = directory.path().join("secondary-lfs-backups");
        let providers = list_with_storage(&database, None, &lfs_root).await?;
        assert!(providers.iter().any(|provider| {
            provider.id == target.id
                && provider.source == BackupProviderSource::StorageTarget
                && matches!(
                    &provider.config,
                    BackupProviderConfig::Filesystem(settings)
                        if settings.path == target_backup_root
                )
        }));
        assert_eq!(
            load_with_storage(&database, None, &lfs_root, target.id)
                .await?
                .map(|provider| provider.name),
            Some("Secondary".to_owned())
        );
        exclude_storage_target(&database, target.id).await?;
        assert!(
            list_with_storage(&database, None, &lfs_root)
                .await?
                .is_empty()
        );
        assert!(
            load_with_storage(&database, None, &lfs_root, target.id)
                .await?
                .is_none()
        );
        database.close().await?;
        Ok(())
    }
}
