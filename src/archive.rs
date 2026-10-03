use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::{Read as _, Write as _},
    path::{Component, Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use ::s3::request::DataStream;
use anyhow::{Context, Result, bail, ensure};
use chrono::{DateTime, SecondsFormat, Utc};
use fs2::FileExt as _;
use sea_orm::{
    ActiveModelTrait as _, ColumnTrait as _, ConnectOptions, ConnectionTrait as _, Database,
    DatabaseConnection, EntityTrait as _, QueryFilter as _, QueryOrder as _, Set, sea_query::Expr,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use tokio::sync::watch;
use uuid::Uuid;
use walkdir::WalkDir;

use crate::{
    backup_provider::{self, BackupProvider, BackupProviderConfig, FilesystemSettings},
    blob_store::{BlobMetadata, DomainStorage, StorageDomain as _, manager},
    config::{BackupCommand, DatabaseSettings, S3Settings, Settings},
    database,
    entity::backup_provider_schedule,
    filesystem::{create_private_directory, open_private_file},
    identity::IdentityState,
    registry::storage::RegistryDomain,
    storage::LfsDomain,
};

mod create;
mod filesystem;
mod restore;
mod s3;
mod schedule;

use create::{create, create_provider_locked, create_s3, restore_s3, temporary_archive_path};
pub use create::{new_backup_key, validate_backup_name};
use restore::{
    copy_tree, extract_archive, lock_path, manifest_files, replace_archive, restore,
    restore_archive, sqlite_path, verify_manifest,
};

use schedule::resolve_s3_settings;
pub use schedule::{
    list_backup_schedules, load_backup_schedule, save_backup_schedule, serve_backup_scheduler,
    validate_backup_schedule,
};

/// Version 4 adds `registry/`, the registry root. Older archives keep registry
/// data inside repository directories; startup moves it after a restore.
const FORMAT_VERSION: u32 = 4;
const MIN_FORMAT_VERSION: u32 = 1;
const ARCHIVE_ROOT: &str = "backup";
const MANIFEST_NAME: &str = "manifest.json";
const SETTINGS_NAME: &str = "gitadel.toml";
const MAX_BACKUP_SCHEDULE_LENGTH: usize = 255;
const BACKUP_SCHEDULER_INTERVAL: Duration = Duration::from_secs(30);

pub struct StorageLock {
    _file: File,
}

#[derive(Debug)]
pub enum MaintenanceAction {
    Create {
        operation_id: Uuid,
        key: String,
        name: Option<String>,
        provider: BackupProvider,
    },
    Restore {
        operation_id: Uuid,
        archive: PathBuf,
        key: String,
        provider: BackupProvider,
        safety_key: Option<String>,
    },
}

impl MaintenanceAction {
    pub fn operation_id(&self) -> Uuid {
        match self {
            Self::Create { operation_id, .. } | Self::Restore { operation_id, .. } => *operation_id,
        }
    }

    fn operation(&self) -> MaintenanceOperation {
        match self {
            Self::Create { .. } => MaintenanceOperation::Create,
            Self::Restore { .. } => MaintenanceOperation::Restore,
        }
    }

    fn key(&self) -> String {
        match self {
            Self::Create { key, .. } | Self::Restore { key, .. } => key.clone(),
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MaintenanceOperation {
    Create,
    Restore,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MaintenancePhase {
    Scheduled,
    CheckingDestination,
    SnapshottingDatabase,
    CopyingRepositories,
    CopyingLfs,
    WritingMetadata,
    Compressing,
    Uploading,
    Restoring,
    Completed,
    Failed,
}

#[derive(Clone, Debug, Serialize)]
pub struct MaintenanceProgress {
    pub operation_id: Uuid,
    pub key: String,
    pub operation: MaintenanceOperation,
    pub phase: MaintenancePhase,
    pub message: String,
    pub processed_bytes: Option<u64>,
    pub total_bytes: Option<u64>,
}

#[derive(Clone)]
pub struct MaintenanceProgressReporter {
    sender: watch::Sender<MaintenanceProgress>,
}

impl MaintenanceProgressReporter {
    pub fn new(action: &MaintenanceAction) -> Self {
        let progress = MaintenanceProgress {
            operation_id: action.operation_id(),
            key: action.key(),
            operation: action.operation(),
            phase: MaintenancePhase::Scheduled,
            message: "Maintenance is scheduled.".to_owned(),
            processed_bytes: None,
            total_bytes: None,
        };
        let (sender, _) = watch::channel(progress);
        Self { sender }
    }

    pub fn subscribe(&self) -> watch::Receiver<MaintenanceProgress> {
        self.sender.subscribe()
    }

    pub fn complete(&self) {
        let message = match self.sender.borrow().operation {
            MaintenanceOperation::Create => "Backup completed. Gitadel is restarting.",
            MaintenanceOperation::Restore => "Restore completed. Gitadel is restarting.",
        };
        self.report(MaintenancePhase::Completed, message);
    }

    pub fn fail(&self, error: &anyhow::Error) {
        let operation = match self.sender.borrow().operation {
            MaintenanceOperation::Create => "Backup",
            MaintenanceOperation::Restore => "Restore",
        };
        self.report(
            MaintenancePhase::Failed,
            format!("{operation} failed: {error:#}"),
        );
    }

    pub(crate) fn report(&self, phase: MaintenancePhase, message: impl Into<String>) {
        let (operation_id, key, operation) = self.identity();
        self.sender.send_replace(MaintenanceProgress {
            operation_id,
            key,
            operation,
            phase,
            message: message.into(),
            processed_bytes: None,
            total_bytes: None,
        });
    }

    fn upload(&self, processed_bytes: u64, total_bytes: u64) {
        let (operation_id, key, operation) = self.identity();
        self.sender.send_replace(MaintenanceProgress {
            operation_id,
            key,
            operation,
            phase: MaintenancePhase::Uploading,
            message: "Uploading snapshot to S3.".to_owned(),
            processed_bytes: Some(processed_bytes),
            total_bytes: Some(total_bytes),
        });
    }

    fn identity(&self) -> (Uuid, String, MaintenanceOperation) {
        let current = self.sender.borrow();
        (current.operation_id, current.key.clone(), current.operation)
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct BackupObject {
    pub key: String,
    pub name: Option<String>,
    pub gitadel_version: Option<String>,
    pub size: u64,
    pub created_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct BackupSchedule {
    pub schedule: String,
    pub next_backup_at: DateTime<Utc>,
}

struct ClaimedBackupSchedule {
    provider_id: Uuid,
    schedule: String,
    previous_next_backup_at: DateTime<Utc>,
    next_backup_at: DateTime<Utc>,
}
pub struct S3BackupDownload {
    pub size: Option<u64>,
    pub stream: DataStream,
}

#[derive(Clone, Debug, Serialize)]
pub struct BackupInspection {
    pub format_version: u32,
    pub gitadel_version: Option<String>,
    pub backup_name: Option<String>,
    pub version_warning: Option<String>,
    pub created_at: String,
    pub file_count: usize,
    pub uncompressed_size: u64,
    pub includes_settings: bool,
    pub includes_host_key: bool,
}

#[derive(Serialize, Deserialize)]
struct Manifest {
    format_version: u32,
    #[serde(default)]
    gitadel_version: Option<String>,
    #[serde(default)]
    backup_name: Option<String>,
    created_at: String,
    host_key: bool,
    files: Vec<ManifestFile>,
}

#[derive(Serialize, Deserialize)]
struct ManifestFile {
    path: String,
    size: u64,
    sha256: String,
}

pub async fn run(command: &BackupCommand, settings: &Settings, config_path: &Path) -> Result<()> {
    match command {
        BackupCommand::Create { output } => create(output, settings).await,
        BackupCommand::CreateS3 { key } => {
            let s3 = resolve_s3_settings(settings).await?;
            create_s3(key.as_deref(), settings, &s3).await
        }
        BackupCommand::Restore { input } => restore(input, settings, config_path),
        BackupCommand::RestoreS3 { key } => {
            let s3 = resolve_s3_settings(settings).await?;
            restore_s3(key, settings, config_path, &s3).await
        }
    }
}

pub fn acquire_storage_lock(database: &DatabaseSettings) -> Result<StorageLock> {
    let database_path = sqlite_path(&database.url)?;
    let lock_path = lock_path(&database_path);
    if let Some(parent) = lock_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("could not create {}", parent.display()))?;
    }
    let file = open_private_file(&lock_path, false)
        .with_context(|| format!("could not open storage lock {}", lock_path.display()))?;
    file.try_lock_exclusive().with_context(|| {
        format!(
            "could not lock {}; stop every Gitadel process using this database",
            lock_path.display()
        )
    })?;
    Ok(StorageLock { _file: file })
}

pub async fn perform_maintenance(
    action: MaintenanceAction,
    settings: &Settings,
    reporter: &MaintenanceProgressReporter,
) -> Result<()> {
    match action {
        MaintenanceAction::Create {
            operation_id: _,
            key,
            name,
            provider,
        } => {
            create_provider_locked(&key, settings, &provider.config, name.as_deref(), reporter)
                .await?;
            tracing::info!(
                provider_id = %provider.id,
                provider = provider.config.kind().as_str(),
                %key,
                ?name,
                "administrative backup completed"
            );
            Ok(())
        }
        MaintenanceAction::Restore {
            operation_id: _,
            archive,
            key,
            mut provider,
            safety_key,
        } => {
            let result: Result<()> = async {
                if let Some(safety_key) = &safety_key {
                    create_provider_locked(
                        safety_key,
                        settings,
                        &provider.config,
                        Some("pre-restore-safety"),
                        reporter,
                    )
                    .await?;
                }
                reporter.report(
                    MaintenancePhase::Restoring,
                    "Replacing the instance from the verified snapshot.",
                );
                replace_archive(&archive, settings)?;
                let database = database::connect_and_migrate(&settings.database).await?;
                provider.updated_at = Utc::now();
                backup_provider::save(&database, &provider).await?;
                database
                    .close()
                    .await
                    .context("could not close restored database")?;
                Ok(())
            }
            .await;
            let _ = fs::remove_file(&archive);
            result?;
            tracing::info!(
                provider_id = %provider.id,
                provider = provider.config.kind().as_str(),
                %key,
                ?safety_key,
                "administrative restore completed"
            );
            Ok(())
        }
    }
}

pub async fn test_backup_provider(
    provider: &BackupProviderConfig,
    settings: &Settings,
) -> Result<()> {
    match provider {
        BackupProviderConfig::Filesystem(filesystem) => {
            filesystem::test_filesystem_settings(filesystem, settings)
        }
        BackupProviderConfig::S3(s3) => s3::test_settings(s3).await,
    }
}

pub async fn open_filesystem_backup(
    settings: &Settings,
    filesystem: &FilesystemSettings,
    key: &str,
) -> Result<(tokio::fs::File, u64)> {
    filesystem::open_filesystem_backup(settings, filesystem, key).await
}

pub async fn list_backups(
    provider: &BackupProviderConfig,
    settings: &Settings,
) -> Result<Vec<BackupObject>> {
    match provider {
        BackupProviderConfig::Filesystem(filesystem) => {
            filesystem::list_filesystem_backups(filesystem, settings).await
        }
        BackupProviderConfig::S3(s3) => s3::list_backups(s3).await,
    }
}

pub async fn delete_backup(
    provider: &BackupProviderConfig,
    settings: &Settings,
    key: &str,
) -> Result<()> {
    match provider {
        BackupProviderConfig::Filesystem(filesystem) => {
            filesystem::delete_filesystem_backup(settings, filesystem, key).await
        }
        BackupProviderConfig::S3(s3) => s3::delete_backup(s3, key).await,
    }
}

pub async fn stream_s3_backup(settings: &S3Settings, key: &str) -> Result<S3BackupDownload> {
    s3::stream_backup(settings, key).await
}

pub(super) fn sort_backups(backups: &mut [BackupObject]) {
    backups.sort_unstable_by(|left, right| {
        right
            .created_at
            .cmp(&left.created_at)
            .then_with(|| right.key.cmp(&left.key))
    });
}

pub(super) fn generated_backup_metadata(key: &str) -> (Option<String>, Option<String>) {
    let Some(filename) = key.rsplit('/').next() else {
        return (None, None);
    };
    let Some(stem) = filename
        .strip_prefix("gitadel-")
        .and_then(|filename| filename.strip_suffix(".tar.zst"))
    else {
        return (None, None);
    };
    let mut parts = stem.splitn(4, "__");
    let Some(_created_at) = parts.next() else {
        return (None, None);
    };
    let version = parts
        .next()
        .and_then(|value| value.strip_prefix('v'))
        .map(str::to_owned);
    let name = parts
        .next()
        .and_then(|value| (value != "unnamed").then(|| value.replace('-', " ")));
    if parts.next().is_none() {
        return (None, None);
    }
    (name, version)
}

pub async fn download_and_inspect_backup(
    settings: &Settings,
    provider: &BackupProviderConfig,
    key: &str,
) -> Result<(PathBuf, BackupInspection)> {
    match provider {
        BackupProviderConfig::Filesystem(filesystem) => {
            copy_and_inspect_filesystem(settings, filesystem, key).await
        }
        BackupProviderConfig::S3(s3) => download_and_inspect_s3(settings, s3, key).await,
    }
}

async fn copy_and_inspect_filesystem(
    settings: &Settings,
    filesystem: &FilesystemSettings,
    key: &str,
) -> Result<(PathBuf, BackupInspection)> {
    let source = filesystem::filesystem_backup_path(filesystem, settings, key)?;
    let source_metadata = tokio::fs::symlink_metadata(&source)
        .await
        .with_context(|| format!("could not inspect filesystem backup {}", source.display()))?;
    ensure!(
        source_metadata.file_type().is_file(),
        "filesystem backup is not a regular file: {}",
        source.display()
    );
    let archive = temporary_archive_path(settings, "filesystem-validate")?;
    let parent = archive.parent().unwrap_or_else(|| Path::new("."));
    ensure!(
        fs2::available_space(parent)? >= source_metadata.len().saturating_mul(2),
        "not enough free disk space to copy and validate this backup"
    );
    let result = async {
        filesystem::copy_backup(&source, &archive).await?;
        tokio::fs::File::open(&archive)
            .await?
            .sync_all()
            .await
            .context("could not sync copied filesystem backup")?;
        inspect_backup(&archive)
    }
    .await;
    match result {
        Ok(inspection) => Ok((archive, inspection)),
        Err(error) => {
            let _ = fs::remove_file(&archive);
            Err(error)
        }
    }
}

pub async fn download_and_inspect_s3(
    settings: &Settings,
    s3: &S3Settings,
    key: &str,
) -> Result<(PathBuf, BackupInspection)> {
    s3::validate_managed_key(s3, key)?;
    let bucket = s3::bucket(s3)?;
    let object_size = s3::backup_size(&bucket, key).await?;
    let archive = temporary_archive_path(settings, "s3-validate")?;
    let parent = archive.parent().unwrap_or_else(|| Path::new("."));
    let required_space = object_size.saturating_mul(2);
    ensure!(
        fs2::available_space(parent)? >= required_space,
        "not enough free disk space to download and validate this backup"
    );

    let result = async {
        s3::download_archive(&bucket, key, &archive).await?;
        inspect_backup(&archive)
    }
    .await;
    match result {
        Ok(inspection) => Ok((archive, inspection)),
        Err(error) => {
            let _ = fs::remove_file(&archive);
            Err(error)
        }
    }
}

pub fn inspect_backup(input: &Path) -> Result<BackupInspection> {
    ensure!(
        input.is_file(),
        "backup archive does not exist: {}",
        input.display()
    );
    let parent = input.parent().unwrap_or_else(|| Path::new("."));
    let staging = parent.join(format!(".gitadel-inspect-{}", Uuid::new_v4().simple()));
    fs::create_dir(&staging).with_context(|| format!("could not create {}", staging.display()))?;
    let result = (|| {
        extract_archive(input, &staging)?;
        let root = staging.join(ARCHIVE_ROOT);
        let manifest_path = root.join(MANIFEST_NAME);
        let manifest_metadata =
            fs::metadata(&manifest_path).context("backup manifest is missing")?;
        ensure!(
            manifest_metadata.len() <= 16 * 1024 * 1024,
            "backup manifest is too large"
        );
        let manifest: Manifest = serde_json::from_slice(&fs::read(&manifest_path)?)
            .context("backup manifest is invalid")?;
        ensure!(
            (MIN_FORMAT_VERSION..=FORMAT_VERSION).contains(&manifest.format_version),
            "unsupported backup format version {}",
            manifest.format_version
        );
        verify_manifest(&root, &manifest)?;
        ensure!(
            root.join("database.sqlite").is_file()
                && root.join("repositories").is_dir()
                && root.join("lfs").is_dir(),
            "backup is missing required instance data"
        );
        let includes_settings = root.join(SETTINGS_NAME).is_file();
        ensure!(
            manifest.format_version < 2 || includes_settings,
            "backup settings are missing"
        );
        ensure!(
            !manifest.host_key || root.join("ssh-host-key").is_file(),
            "backup SSH host key is missing"
        );
        let uncompressed_size = manifest.files.iter().try_fold(0_u64, |total, file| {
            total
                .checked_add(file.size)
                .context("backup content size overflowed")
        })?;
        let version_warning = backup_version_warning(manifest.gitadel_version.as_deref());
        Ok(BackupInspection {
            format_version: manifest.format_version,
            gitadel_version: manifest.gitadel_version,
            backup_name: manifest.backup_name,
            version_warning,
            created_at: manifest.created_at,
            file_count: manifest.files.len(),
            uncompressed_size,
            includes_settings,
            includes_host_key: manifest.host_key,
        })
    })();
    let _ = fs::remove_dir_all(&staging);
    result
}

fn backup_version_warning(backup_version: Option<&str>) -> Option<String> {
    let current = semver::Version::parse(env!("CARGO_PKG_VERSION"))
        .expect("Gitadel package version must be valid semantic versioning");
    let Some(backup_version) = backup_version else {
        return Some(
            "This backup does not record its Gitadel version. Verify compatibility before restoring."
                .to_owned(),
        );
    };
    let Ok(backup) = semver::Version::parse(backup_version) else {
        return Some(format!(
            "This backup records an unrecognized Gitadel version ({backup_version}). Verify compatibility before restoring."
        ));
    };
    if backup < current {
        Some(format!(
            "This backup was created by older Gitadel {backup}; the current version is {current}."
        ))
    } else if backup > current {
        Some(format!(
            "This backup was created by newer Gitadel {backup}; the current version is {current}."
        ))
    } else {
        None
    }
}

pub fn restore_available_space(settings: &Settings) -> Result<u64> {
    let database_path = sqlite_path(&settings.database.url)?;
    let database_parent = database_path.parent().unwrap_or_else(|| Path::new("."));
    [
        database_parent,
        settings
            .storage
            .repository_root
            .parent()
            .unwrap_or_else(|| Path::new(".")),
        settings
            .storage
            .lfs_root
            .parent()
            .unwrap_or_else(|| Path::new(".")),
        settings
            .storage
            .registry_root
            .parent()
            .unwrap_or_else(|| Path::new(".")),
        settings
            .storage
            .actions_artifact_root
            .parent()
            .unwrap_or_else(|| Path::new(".")),
    ]
    .into_iter()
    .map(|path| {
        fs2::available_space(path)
            .with_context(|| format!("could not determine free space for {}", path.display()))
    })
    .try_fold(u64::MAX, |available, value| {
        value.map(|value| available.min(value))
    })
}

#[cfg(test)]
mod tests {
    use chrono::Duration as ChronoDuration;
    use sea_orm::Set;

    use super::create::{
        create_archive, prepare_restored_snapshot, snapshot_database, write_archive,
    };
    use super::s3::validate_managed_key as validate_managed_s3_backup_key;
    use super::schedule::claim_due_backup_schedule;
    use std::os::unix::fs::PermissionsExt as _;

    use super::*;
    use crate::{
        blob_store::{BlobDigest, lfs_object_key},
        entity::{backup_provider_schedule, instance, user},
    };

    #[derive(Debug, PartialEq, Eq)]
    struct RestoredState {
        username: String,
        site_name: String,
        site_description: Option<String>,
        repository: Vec<u8>,
        lfs: Vec<u8>,
        upload: Vec<u8>,
        registry: Vec<u8>,
        actions_artifact: Vec<u8>,
        host_key: Vec<u8>,
        effective_settings: String,
    }

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Result<Self> {
            let path = std::env::temp_dir().join(format!("gitadel-backup-test-{}", Uuid::new_v4()));
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

    fn settings_at(root: &Path) -> Settings {
        let mut settings = Settings::default();
        settings.database.url = format!("sqlite://{}?mode=rwc", root.join("gitadel.db").display());
        settings.storage.repository_root = root.join("repositories");
        settings.storage.lfs_root = root.join("lfs");
        settings.storage.registry_root = root.join("registry");
        settings.storage.actions_artifact_root = root.join("actions-artifacts");
        settings.ssh.host_key = root.join("ssh-host-key");
        settings
    }

    #[tokio::test]
    async fn backup_creation_uses_owner_only_mode() -> Result<()> {
        let directory = TestDirectory::new()?;
        let instance_root = directory.path().join("mode-test");
        fs::create_dir(&instance_root)?;
        let settings = settings_at(&instance_root);
        let backup = directory.path().join("private-backup.tar.zst");

        create_archive(&backup, &settings).await?;

        assert_eq!(fs::metadata(backup)?.permissions().mode() & 0o777, 0o600);
        Ok(())
    }

    #[tokio::test]
    async fn archive_round_trip_restores_complete_instance_state() -> Result<()> {
        let directory = TestDirectory::new()?;
        let source_root = directory.path().join("source");
        fs::create_dir(&source_root)?;
        let source_settings = settings_at(&source_root);
        let source_database = database::connect_and_migrate(&source_settings.database).await?;

        let now = Utc::now();
        user::ActiveModel {
            id: Set(Uuid::new_v4()),
            username: Set("backup-admin".to_owned()),
            password_hash: Set("stored-password-hash".to_owned()),
            is_admin: Set(true),
            default_repository_visibility: Set("private".to_owned()),
            theme_preference: Set("system".to_owned()),
            motion_preference: Set("system".to_owned()),
            disabled_at: Set(None),
            avatar_updated_at: Set(None),
            created_at: Set(now),
            updated_at: Set(now),
        }
        .insert(&source_database)
        .await?;
        let mut instance: instance::ActiveModel = instance::Entity::find_by_id(1)
            .one(&source_database)
            .await?
            .context("instance settings row is missing")?
            .into();
        instance.site_name = Set("Recovered Gitadel".to_owned());
        instance.site_description = Set(Some("Complete backup".to_owned()));
        instance.updated_at = Set(now);
        instance.update(&source_database).await?;
        source_database.close().await?;

        let repository_file = source_settings
            .storage
            .repository_root
            .join("repository.git/objects/pack/data");
        fs::create_dir_all(
            repository_file
                .parent()
                .context("repository file has no parent")?,
        )?;
        fs::write(&repository_file, b"repository-object")?;
        let lfs_payload = b"lfs-object";
        let lfs_digest = BlobDigest::from_bytes(Sha256::digest(lfs_payload).into());
        let lfs_relative = lfs_object_key(Uuid::new_v4(), &lfs_digest.to_hex())?;
        let lfs_file = source_settings.storage.lfs_root.join(lfs_relative.as_str());
        fs::create_dir_all(lfs_file.parent().context("LFS file has no parent")?)?;
        fs::write(&lfs_file, lfs_payload)?;
        let upload_file = source_settings
            .storage
            .lfs_root
            .join("uploads/issue-attachment");
        fs::create_dir_all(upload_file.parent().context("upload file has no parent")?)?;
        fs::write(&upload_file, b"issue-upload")?;
        let registry_file = source_settings
            .storage
            .registry_root
            .join("registry-payload");
        fs::create_dir_all(&source_settings.storage.registry_root)?;
        fs::write(&registry_file, b"registry-payload")?;
        let artifact_file = source_settings
            .storage
            .actions_artifact_root
            .join("workflow/artifact.zip");
        fs::create_dir_all(
            artifact_file
                .parent()
                .context("artifact file has no parent")?,
        )?;
        fs::write(&artifact_file, b"actions-artifact")?;
        fs::write(&source_settings.ssh.host_key, b"ssh-host-key")?;

        let backup = directory.path().join("complete.tar.zst");
        create_archive(&backup, &source_settings).await?;
        let inspection = inspect_backup(&backup)?;
        assert_eq!(
            inspection.gitadel_version.as_deref(),
            Some(env!("CARGO_PKG_VERSION"))
        );
        assert_eq!(inspection.backup_name, None);
        assert_eq!(inspection.version_warning, None);

        let restored_root = directory.path().join("restored");
        let restored_settings = settings_at(&restored_root);
        let restored_config = restored_root.join("gitadel.toml");
        restore_archive(&backup, &restored_settings, &restored_config)?;

        let restored_database = database::connect_and_migrate(&restored_settings.database).await?;
        let restored_user = user::Entity::find()
            .one(&restored_database)
            .await?
            .context("restored user is missing")?;
        let restored_instance = instance::Entity::find_by_id(1)
            .one(&restored_database)
            .await?
            .context("restored instance settings are missing")?;
        restored_database.close().await?;

        let restored = RestoredState {
            username: restored_user.username,
            site_name: restored_instance.site_name,
            site_description: restored_instance.site_description,
            repository: fs::read(
                restored_settings
                    .storage
                    .repository_root
                    .join("repository.git/objects/pack/data"),
            )?,
            lfs: fs::read(
                restored_settings
                    .storage
                    .lfs_root
                    .join(lfs_relative.as_str()),
            )?,
            registry: fs::read(
                restored_settings
                    .storage
                    .registry_root
                    .join("registry-payload"),
            )?,
            upload: fs::read(
                restored_settings
                    .storage
                    .lfs_root
                    .join("uploads/issue-attachment"),
            )?,
            actions_artifact: fs::read(
                restored_settings
                    .storage
                    .actions_artifact_root
                    .join("workflow/artifact.zip"),
            )?,
            host_key: fs::read(&restored_settings.ssh.host_key)?,
            effective_settings: fs::read_to_string(&restored_config)?,
        };
        let expected = RestoredState {
            username: "backup-admin".to_owned(),
            site_name: "Recovered Gitadel".to_owned(),
            site_description: Some("Complete backup".to_owned()),
            repository: b"repository-object".to_vec(),
            lfs: b"lfs-object".to_vec(),
            upload: b"issue-upload".to_vec(),
            registry: b"registry-payload".to_vec(),
            actions_artifact: b"actions-artifact".to_vec(),
            host_key: b"ssh-host-key".to_vec(),
            effective_settings: toml::to_string_pretty(&source_settings)?,
        };

        assert_eq!(restored, expected);
        Ok(())
    }

    /// Pulls `latest` of a repository's image after the registry has started
    /// on a restored instance.
    async fn restored_manifest(
        settings: &Settings,
        repository: &crate::entity::repository::Model,
    ) -> Result<Option<Vec<u8>>> {
        let database = database::connect_and_migrate(&settings.database).await?;
        let storage = crate::registry::storage::start(&database, &settings.storage).await?;
        let manifest = crate::registry::store::RegistryStore::new(
            database.clone(),
            &settings.storage.registry_root,
        )
        .image(repository, "", storage.store())
        .get_manifest("latest")
        .await
        .map_err(|error| anyhow::anyhow!("{error}"))?
        .map(|manifest| manifest.bytes);
        database.close().await?;
        Ok(manifest)
    }

    #[tokio::test]
    async fn restoring_a_0_13_archive_converts_registry_data_in_repositories() -> Result<()> {
        use crate::registry::test_support;

        let directory = TestDirectory::new()?;
        let source_root = directory.path().join("source");
        fs::create_dir(&source_root)?;
        let source_settings = settings_at(&source_root);
        let (source_database, owner) = test_support::database(&source_root).await;
        let repository = test_support::repository(&source_database, owner, "legacy").await;

        // Lay out a format 3 archive as 0.13.1 wrote it: registry files live
        // inside the bare repository and there is no registry root.
        let staging = directory.path().join("staging");
        for directory in ["lfs", "actions-artifacts"] {
            fs::create_dir_all(staging.join(directory))?;
        }
        let repository_dir = staging
            .join("repositories")
            .join(format!("{}.git", repository.storage_key));
        fs::create_dir_all(&repository_dir)?;
        fs::write(repository_dir.join("HEAD"), b"ref: refs/heads/main\n")?;
        let legacy = test_support::write_legacy_image(
            &test_support::legacy_image_dir(&repository_dir.join("gitadel-registry/images"), ""),
            repository.storage_key,
            "",
            None,
        )
        .await;
        snapshot_database(&source_database, &staging.join("database.sqlite")).await?;
        source_database.close().await?;
        prepare_restored_snapshot(&staging.join("database.sqlite")).await?;
        fs::write(
            staging.join(SETTINGS_NAME),
            toml::to_string_pretty(&source_settings)?,
        )?;
        let manifest = Manifest {
            format_version: 3,
            gitadel_version: Some("0.13.1".to_owned()),
            backup_name: None,
            created_at: Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
            host_key: false,
            files: manifest_files(&staging)?,
        };
        let backup = directory.path().join("gitadel-0.13.1.tar.zst");
        write_archive(&staging, &manifest, &backup)?;

        let restored_root = directory.path().join("restored");
        let restored_settings = settings_at(&restored_root);
        restore_archive(
            &backup,
            &restored_settings,
            &restored_root.join("gitadel.toml"),
        )?;
        assert_eq!(
            restored_manifest(&restored_settings, &repository).await?,
            Some(legacy.manifest.clone())
        );
        let restored_repository = restored_settings
            .storage
            .repository_root
            .join(format!("{}.git", repository.storage_key));
        assert!(restored_repository.join("HEAD").is_file());
        assert!(!restored_repository.join("gitadel-registry").exists());

        // A backup of the converted instance carries the registry root and
        // the metadata tables.
        let converted = directory.path().join("converted.tar.zst");
        create_archive(&converted, &restored_settings).await?;
        assert_eq!(inspect_backup(&converted)?.format_version, FORMAT_VERSION);
        let second_root = directory.path().join("second");
        let second_settings = settings_at(&second_root);
        restore_archive(
            &converted,
            &second_settings,
            &second_root.join("gitadel.toml"),
        )?;
        assert_eq!(
            restored_manifest(&second_settings, &repository).await?,
            Some(legacy.manifest)
        );
        Ok(())
    }

    #[tokio::test]
    async fn backup_schedule_persists_and_can_be_disabled() -> Result<()> {
        let directory = TestDirectory::new()?;
        let settings = settings_at(directory.path());
        let database = database::connect_and_migrate(&settings.database).await?;
        let provider = filesystem_provider(directory.path().join("backups"));
        backup_provider::save(&database, &provider).await?;

        let saved = save_backup_schedule(&database, provider.id, Some("*/15 * * * *")).await?;
        assert_eq!(load_backup_schedule(&database, provider.id).await?, saved);

        save_backup_schedule(&database, provider.id, None).await?;
        assert!(
            load_backup_schedule(&database, provider.id)
                .await?
                .is_none()
        );
        save_backup_schedule(&database, provider.id, Some("0 2 * * *")).await?;
        assert!(backup_provider::delete(&database, provider.id).await?);
        assert!(
            load_backup_schedule(&database, provider.id)
                .await?
                .is_none()
        );
        database.close().await?;
        Ok(())
    }

    #[tokio::test]
    async fn due_backup_schedule_is_claimed_only_once() -> Result<()> {
        let directory = TestDirectory::new()?;
        let settings = settings_at(directory.path());
        let database = database::connect_and_migrate(&settings.database).await?;
        let provider = filesystem_provider(directory.path().join("backups"));
        backup_provider::save(&database, &provider).await?;
        let now = Utc::now();
        backup_provider_schedule::ActiveModel {
            provider_id: Set(provider.id),
            schedule: Set("*/15 * * * *".to_owned()),
            next_backup_at: Set(now - ChronoDuration::minutes(1)),
            updated_at: Set(now),
        }
        .insert(&database)
        .await?;

        let claimed = claim_due_backup_schedule(&database, now)
            .await?
            .context("due schedule was not claimed")?;
        assert_eq!(claimed.provider_id, provider.id);
        assert!(claimed.next_backup_at > now);
        assert!(claim_due_backup_schedule(&database, now).await?.is_none());
        database.close().await?;
        Ok(())
    }
    #[test]
    fn generated_backup_keys_include_name_version_and_creation_time() -> Result<()> {
        let settings = S3Settings {
            endpoint: "https://s3.example.com".parse()?,
            bucket: "gitadel".to_owned(),
            access_key: "access".to_owned(),
            secret_key: "secret".to_owned(),
            region: "us-east-1".to_owned(),
            prefix: "snapshots".to_owned(),
        };

        let key = new_backup_key(
            &BackupProviderConfig::S3(settings),
            Some("Before platform upgrade"),
        )?;
        let filename = key
            .rsplit('/')
            .next()
            .context("backup key has no filename")?;
        assert!(key.starts_with("snapshots/gitadel-"));
        assert!(filename.contains(&format!("__v{}__", env!("CARGO_PKG_VERSION"))));
        assert!(filename.contains("__Before-platform-upgrade__"));
        assert!(filename.ends_with(".tar.zst"));
        let (name, version) = generated_backup_metadata(&key);
        assert_eq!(name.as_deref(), Some("Before platform upgrade"));
        assert_eq!(version.as_deref(), Some(env!("CARGO_PKG_VERSION")));
        Ok(())
    }
    #[test]
    fn backup_listing_is_sorted_by_creation_time_descending() {
        let mut backups = vec![
            BackupObject {
                key: "older".to_owned(),
                name: None,
                gitadel_version: None,
                size: 1,
                created_at: "2026-01-01T00:00:00Z".to_owned(),
            },
            BackupObject {
                key: "newer".to_owned(),
                name: None,
                gitadel_version: None,
                size: 1,
                created_at: "2026-08-27T00:00:00Z".to_owned(),
            },
        ];

        sort_backups(&mut backups);
        assert_eq!(backups[0].key, "newer");
        assert_eq!(backups[1].key, "older");
    }

    #[test]
    fn older_backup_version_requires_confirmation_warning() {
        assert!(backup_version_warning(Some("0.0.1")).is_some());
        assert_eq!(
            backup_version_warning(Some(env!("CARGO_PKG_VERSION"))),
            None
        );
    }
    #[test]
    fn managed_backup_keys_stay_inside_the_configured_prefix() -> Result<()> {
        let settings = S3Settings {
            endpoint: "https://s3.example.com".parse()?,
            bucket: "gitadel".to_owned(),
            access_key: "access".to_owned(),
            secret_key: "secret".to_owned(),
            region: "us-east-1".to_owned(),
            prefix: "snapshots".to_owned(),
        };

        validate_managed_s3_backup_key(&settings, "snapshots/valid.tar.zst")?;
        assert!(validate_managed_s3_backup_key(&settings, "other/valid.tar.zst").is_err());
        assert!(validate_managed_s3_backup_key(&settings, "snapshots/not-a-backup.txt").is_err());
        Ok(())
    }

    #[tokio::test]
    async fn filesystem_provider_creates_lists_and_deletes_backup() -> Result<()> {
        let directory = TestDirectory::new()?;
        let instance_root = directory.path().join("instance");
        fs::create_dir(&instance_root)?;
        let settings = settings_at(&instance_root);
        let database = database::connect_and_migrate(&settings.database).await?;
        database.close().await?;
        let provider = filesystem_provider(directory.path().join("backups"));
        let key = new_backup_key(&provider.config, Some("Filesystem proof"))?;
        let action = MaintenanceAction::Create {
            operation_id: Uuid::new_v4(),
            key: key.clone(),
            name: Some("Filesystem proof".to_owned()),
            provider: provider.clone(),
        };
        let reporter = MaintenanceProgressReporter::new(&action);

        perform_maintenance(action, &settings, &reporter).await?;
        let backups = list_backups(&provider.config, &settings).await?;
        assert_eq!(backups.len(), 1);
        assert_eq!(backups[0].key, key);

        delete_backup(&provider.config, &settings, &key).await?;
        assert!(list_backups(&provider.config, &settings).await?.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn maintenance_reporter_publishes_phases_and_upload_bytes() -> Result<()> {
        let operation_id = Uuid::new_v4();
        let action = MaintenanceAction::Create {
            operation_id,
            key: "snapshots/progress.tar.zst".to_owned(),
            name: None,
            provider: BackupProvider {
                id: Uuid::new_v4(),
                name: "S3".to_owned(),
                config: BackupProviderConfig::S3(S3Settings {
                    endpoint: "https://s3.example.com".parse()?,
                    bucket: "gitadel".to_owned(),
                    access_key: "access".to_owned(),
                    secret_key: "secret".to_owned(),
                    region: "us-east-1".to_owned(),
                    prefix: "snapshots".to_owned(),
                }),
                source: crate::backup_provider::BackupProviderSource::Stored,
                created_at: Utc::now(),
                updated_at: Utc::now(),
            },
        };
        let reporter = MaintenanceProgressReporter::new(&action);
        let mut receiver = reporter.subscribe();

        reporter.report(
            MaintenancePhase::CopyingRepositories,
            "Copying Git repositories.",
        );
        receiver.changed().await?;
        {
            let progress = receiver.borrow_and_update();
            assert_eq!(progress.operation_id, operation_id);
            assert!(matches!(
                progress.phase,
                MaintenancePhase::CopyingRepositories
            ));
            assert_eq!(progress.processed_bytes, None);
        }

        reporter.upload(3, 10);
        receiver.changed().await?;
        let progress = receiver.borrow_and_update();
        assert!(matches!(progress.phase, MaintenancePhase::Uploading));
        assert_eq!(progress.processed_bytes, Some(3));
        assert_eq!(progress.total_bytes, Some(10));
        Ok(())
    }

    fn filesystem_provider(path: PathBuf) -> BackupProvider {
        let now = Utc::now();
        BackupProvider {
            id: Uuid::new_v4(),
            name: "Filesystem".to_owned(),
            config: BackupProviderConfig::Filesystem(FilesystemSettings { path }),
            source: crate::backup_provider::BackupProviderSource::Stored,
            created_at: now,
            updated_at: now,
        }
    }
}
