use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{Read as _, Write as _},
    path::{Component, Path, PathBuf},
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
use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    sync::watch,
};
use uuid::Uuid;
use walkdir::WalkDir;

use crate::{
    backup_provider::{self, BackupProvider, BackupProviderConfig, FilesystemSettings},
    blob_store::{BlobDigest, ObjectPrefix, targets},
    config::{BackupCommand, DatabaseSettings, S3Settings, Settings},
    database,
    entity::backup_provider_schedule,
    identity::IdentityState,
};

mod filesystem;
mod s3;

const FORMAT_VERSION: u32 = 2;
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

    pub(crate) fn report_progress(
        &self,
        phase: MaintenancePhase,
        message: impl Into<String>,
        processed_bytes: u64,
        total_bytes: u64,
    ) {
        let (operation_id, key, operation) = self.identity();
        self.sender.send_replace(MaintenanceProgress {
            operation_id,
            key,
            operation,
            phase,
            message: message.into(),
            processed_bytes: Some(processed_bytes),
            total_bytes: Some(total_bytes),
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

pub async fn load_s3_settings(
    database: &DatabaseConnection,
    fallback: Option<&S3Settings>,
) -> Result<Option<S3Settings>> {
    Ok(backup_provider::list(database, fallback)
        .await?
        .into_iter()
        .find_map(|provider| match provider.config {
            BackupProviderConfig::S3(settings) => Some(settings),
            BackupProviderConfig::Filesystem(_) => None,
        }))
}

pub async fn load_backup_schedule(
    database: &DatabaseConnection,
    provider_id: Uuid,
) -> Result<Option<BackupSchedule>> {
    Ok(backup_provider_schedule::Entity::find_by_id(provider_id)
        .one(database)
        .await?
        .map(|stored| BackupSchedule {
            schedule: stored.schedule,
            next_backup_at: stored.next_backup_at,
        }))
}

pub async fn list_backup_schedules(
    database: &DatabaseConnection,
) -> Result<BTreeMap<Uuid, BackupSchedule>> {
    Ok(backup_provider_schedule::Entity::find()
        .all(database)
        .await?
        .into_iter()
        .map(|stored| {
            (
                stored.provider_id,
                BackupSchedule {
                    schedule: stored.schedule,
                    next_backup_at: stored.next_backup_at,
                },
            )
        })
        .collect())
}

pub fn validate_backup_schedule(value: Option<&str>) -> Result<()> {
    prepare_backup_schedule(value, Utc::now()).map(drop)
}

fn prepare_backup_schedule(
    value: Option<&str>,
    after: DateTime<Utc>,
) -> Result<Option<BackupSchedule>> {
    let schedule = value.map(str::trim).filter(|value| !value.is_empty());
    let Some(schedule) = schedule else {
        return Ok(None);
    };
    ensure!(
        schedule.len() <= MAX_BACKUP_SCHEDULE_LENGTH,
        "backup schedules cannot exceed {MAX_BACKUP_SCHEDULE_LENGTH} characters"
    );
    let next_backup_at = crate::schedule::next_occurrence(schedule, after)
        .with_context(|| format!("backup schedule {schedule:?}"))?;
    Ok(Some(BackupSchedule {
        schedule: schedule.to_owned(),
        next_backup_at,
    }))
}

pub async fn save_backup_schedule(
    database: &DatabaseConnection,
    provider_id: Uuid,
    value: Option<&str>,
) -> Result<Option<BackupSchedule>> {
    let now = Utc::now();
    let Some(prepared) = prepare_backup_schedule(value, now)? else {
        backup_provider_schedule::Entity::delete_by_id(provider_id)
            .exec(database)
            .await?;
        return Ok(None);
    };
    let schedule = &prepared.schedule;
    let next_backup_at = prepared.next_backup_at;
    match backup_provider_schedule::Entity::find_by_id(provider_id)
        .one(database)
        .await?
    {
        Some(stored) => {
            let mut active: backup_provider_schedule::ActiveModel = stored.into();
            active.schedule = Set(schedule.clone());
            active.next_backup_at = Set(next_backup_at);
            active.updated_at = Set(now);
            active.update(database).await?;
        }
        None => {
            backup_provider_schedule::ActiveModel {
                provider_id: Set(provider_id),
                schedule: Set(schedule.clone()),
                next_backup_at: Set(next_backup_at),
                updated_at: Set(now),
            }
            .insert(database)
            .await?;
        }
    }
    Ok(Some(prepared))
}

pub async fn serve_backup_scheduler(state: IdentityState) -> Result<()> {
    let mut interval = tokio::time::interval(BACKUP_SCHEDULER_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        interval.tick().await;
        if let Err(error) = schedule_due_backup(&state).await {
            tracing::warn!(%error, "scheduled automatic backup could not start");
        }
    }
}

async fn schedule_due_backup(state: &IdentityState) -> Result<()> {
    let Some(claimed) = claim_due_backup_schedule(state.database(), Utc::now()).await? else {
        return Ok(());
    };
    let settings = state
        .runtime_settings()
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    let Some(provider) = backup_provider::load_with_storage(
        state.database(),
        settings.backup.s3.as_ref(),
        &settings.storage.lfs_root,
        claimed.provider_id,
    )
    .await?
    else {
        tracing::warn!(
            provider_id = %claimed.provider_id,
            schedule = %claimed.schedule,
            "automatic backup is due but its provider does not exist"
        );
        return Ok(());
    };
    let backup_name = "scheduled automatic backup";
    let key = new_backup_key(&provider.config, Some(backup_name))?;
    let operation_id = Uuid::new_v4();
    if let Err(error) = state
        .schedule_maintenance(MaintenanceAction::Create {
            operation_id,
            key: key.clone(),
            name: Some(backup_name.to_owned()),
            provider: provider.clone(),
        })
        .await
    {
        restore_claimed_backup_schedule(state.database(), &claimed).await?;
        bail!("could not reserve maintenance: {error}");
    }
    state
        .audit(None, "backup.create.automatic", Some(key.clone()))
        .await
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    tracing::info!(
        %operation_id,
        provider_id = %provider.id,
        %key,
        schedule = %claimed.schedule,
        next_backup_at = %claimed.next_backup_at,
        "scheduled automatic backup"
    );
    Ok(())
}

async fn claim_due_backup_schedule(
    database: &DatabaseConnection,
    now: DateTime<Utc>,
) -> Result<Option<ClaimedBackupSchedule>> {
    let Some(stored) = backup_provider_schedule::Entity::find()
        .filter(backup_provider_schedule::Column::NextBackupAt.lte(now))
        .order_by_asc(backup_provider_schedule::Column::NextBackupAt)
        .one(database)
        .await?
    else {
        return Ok(None);
    };
    let next_backup_at = crate::schedule::next_occurrence(&stored.schedule, now)
        .with_context(|| format!("stored backup schedule {:?}", stored.schedule))?;
    let updated = backup_provider_schedule::Entity::update_many()
        .col_expr(
            backup_provider_schedule::Column::NextBackupAt,
            Expr::value(next_backup_at),
        )
        .col_expr(
            backup_provider_schedule::Column::UpdatedAt,
            Expr::value(now),
        )
        .filter(backup_provider_schedule::Column::ProviderId.eq(stored.provider_id))
        .filter(backup_provider_schedule::Column::Schedule.eq(&stored.schedule))
        .filter(backup_provider_schedule::Column::NextBackupAt.eq(stored.next_backup_at))
        .exec(database)
        .await?;
    if updated.rows_affected == 0 {
        return Ok(None);
    }
    Ok(Some(ClaimedBackupSchedule {
        provider_id: stored.provider_id,
        schedule: stored.schedule,
        previous_next_backup_at: stored.next_backup_at,
        next_backup_at,
    }))
}

async fn restore_claimed_backup_schedule(
    database: &DatabaseConnection,
    claimed: &ClaimedBackupSchedule,
) -> Result<()> {
    backup_provider_schedule::Entity::update_many()
        .col_expr(
            backup_provider_schedule::Column::NextBackupAt,
            Expr::value(claimed.previous_next_backup_at),
        )
        .col_expr(
            backup_provider_schedule::Column::UpdatedAt,
            Expr::value(Utc::now()),
        )
        .filter(backup_provider_schedule::Column::ProviderId.eq(claimed.provider_id))
        .filter(backup_provider_schedule::Column::Schedule.eq(&claimed.schedule))
        .filter(backup_provider_schedule::Column::NextBackupAt.eq(claimed.next_backup_at))
        .exec(database)
        .await?;
    Ok(())
}

async fn resolve_s3_settings(settings: &Settings) -> Result<S3Settings> {
    let database_path = sqlite_path(&settings.database.url)?;
    if database_path.is_file() {
        let database = database::connect_and_migrate(&settings.database).await?;
        let resolved = load_s3_settings(&database, settings.backup.s3.as_ref()).await;
        database
            .close()
            .await
            .context("could not close backup settings connection")?;
        return resolved?.context("backup S3 storage is not configured");
    }
    settings
        .backup
        .s3
        .clone()
        .context("backup S3 storage is not configured")
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
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .with_context(|| format!("could not open storage lock {}", lock_path.display()))?;
    file.try_lock_exclusive().with_context(|| {
        format!(
            "could not lock {}; stop every Gitadel process using this database",
            lock_path.display()
        )
    })?;
    Ok(StorageLock { _file: file })
}

async fn create(output: &Path, settings: &Settings) -> Result<()> {
    create_archive(output, settings).await?;
    println!("Created backup {}.", output.display());
    Ok(())
}

async fn create_archive(output: &Path, settings: &Settings) -> Result<()> {
    let _lock = acquire_storage_lock(&settings.database)?;
    create_archive_locked(output, settings, None, None).await
}

async fn create_archive_locked(
    output: &Path,
    settings: &Settings,
    backup_name: Option<&str>,
    reporter: Option<&MaintenanceProgressReporter>,
) -> Result<()> {
    ensure!(
        !output.exists(),
        "backup output already exists: {}",
        output.display()
    );
    let output_parent = output.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(output_parent)
        .with_context(|| format!("could not create {}", output_parent.display()))?;
    let staging = output_parent.join(format!(".gitadel-backup-{}", Uuid::new_v4().simple()));
    let temporary_output = output_parent.join(format!(
        ".{}.{}.tmp",
        output
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("backup"),
        Uuid::new_v4().simple()
    ));
    let result =
        match create_staged_backup(&staging, &temporary_output, settings, backup_name, reporter)
            .await
        {
            Ok(()) => fs::rename(&temporary_output, output).with_context(|| {
                format!(
                    "could not publish backup {} as {}",
                    temporary_output.display(),
                    output.display()
                )
            }),
            Err(error) => Err(error),
        };
    let _ = fs::remove_dir_all(&staging);
    let _ = fs::remove_file(&temporary_output);
    result
}

async fn create_staged_backup(
    staging: &Path,
    temporary_output: &Path,
    settings: &Settings,
    backup_name: Option<&str>,
    reporter: Option<&MaintenanceProgressReporter>,
) -> Result<()> {
    fs::create_dir_all(staging)
        .with_context(|| format!("could not create {}", staging.display()))?;
    fs::create_dir(staging.join("repositories"))?;
    fs::create_dir(staging.join("lfs"))?;
    if let Some(reporter) = reporter {
        reporter.report(
            MaintenancePhase::SnapshottingDatabase,
            "Creating a consistent database snapshot.",
        );
    }

    let database = database::connect_and_migrate(&settings.database).await?;
    let lfs_storage = targets::load_active(&database, settings.storage.lfs_root.clone()).await?;
    let snapshot_path = staging.join("database.sqlite");
    snapshot_database(&database, &snapshot_path).await?;
    database
        .close()
        .await
        .context("could not close database snapshot connection")?;
    prepare_restored_snapshot(&snapshot_path).await?;

    if let Some(reporter) = reporter {
        reporter.report(
            MaintenancePhase::CopyingRepositories,
            "Copying Git repositories.",
        );
    }
    copy_tree(
        &settings.storage.repository_root,
        &staging.join("repositories"),
    )?;
    if let Some(reporter) = reporter {
        reporter.report(MaintenancePhase::CopyingLfs, "Copying Git LFS objects.");
    }
    materialize_lfs(lfs_storage.store.as_ref(), &staging.join("lfs")).await?;
    if let Some(reporter) = reporter {
        reporter.report(
            MaintenancePhase::WritingMetadata,
            "Writing settings, host key, and integrity manifest.",
        );
    }
    let host_key = settings.ssh.host_key.is_file();
    if host_key {
        fs::copy(&settings.ssh.host_key, staging.join("ssh-host-key")).with_context(|| {
            format!(
                "could not copy SSH host key {}",
                settings.ssh.host_key.display()
            )
        })?;
    }

    let serialized_settings =
        toml::to_string_pretty(settings).context("could not encode effective settings")?;
    let mut settings_file = File::create(staging.join(SETTINGS_NAME))?;
    settings_file.write_all(serialized_settings.as_bytes())?;
    settings_file.sync_all()?;

    let manifest = Manifest {
        format_version: FORMAT_VERSION,
        gitadel_version: Some(env!("CARGO_PKG_VERSION").to_owned()),
        backup_name: backup_name.map(str::to_owned),
        created_at: Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
        host_key,
        files: manifest_files(staging)?,
    };
    let manifest_bytes =
        serde_json::to_vec_pretty(&manifest).context("could not encode backup manifest")?;
    let mut manifest_file = File::create(staging.join(MANIFEST_NAME))?;
    manifest_file.write_all(&manifest_bytes)?;
    manifest_file.sync_all()?;
    if let Some(reporter) = reporter {
        reporter.report(
            MaintenancePhase::Compressing,
            "Compressing the backup archive.",
        );
    }

    let output = File::create(temporary_output)
        .with_context(|| format!("could not create {}", temporary_output.display()))?;
    let encoder = zstd::Encoder::new(output, 9).context("could not start Zstandard encoder")?;
    let mut archive = tar::Builder::new(encoder);
    archive
        .append_dir_all(ARCHIVE_ROOT, staging)
        .context("could not write backup archive")?;
    let encoder = archive
        .into_inner()
        .context("could not finish backup archive")?;
    let output = encoder
        .finish()
        .context("could not finish Zstandard stream")?;
    output.sync_all().context("could not sync backup archive")
}
async fn prepare_restored_snapshot(path: &Path) -> Result<()> {
    let mut options = ConnectOptions::new(format!("sqlite://{}?mode=rwc", path.display()));
    options.max_connections(1).sqlx_logging(false);
    let database = Database::connect(options)
        .await
        .context("could not open staged backup database")?;
    database
        .execute_unprepared(
            "PRAGMA foreign_keys = ON;\
             UPDATE lfs_storage_state SET active_target_id = NULL, updated_at = CURRENT_TIMESTAMP WHERE id = 1;\
             UPDATE lfs_objects SET storage_target_id = NULL;\
             DELETE FROM lfs_storage_migrations;\
             PRAGMA wal_checkpoint(TRUNCATE);",
        )
        .await
        .context("could not rebind staged backup to runtime-local LFS storage")?;
    database.close().await?;
    Ok(())
}

async fn materialize_lfs(
    store: &dyn crate::blob_store::BlobStore,
    destination: &Path,
) -> Result<()> {
    let objects = store.list(&ObjectPrefix::new("")?).await?;
    for metadata in objects {
        let parts = metadata.key.as_str().split('/').collect::<Vec<_>>();
        if parts.len() != 4
            || Uuid::parse_str(parts[0]).is_err()
            || parts[1].len() != 2
            || parts[2].len() != 2
            || parts[3].parse::<BlobDigest>().is_err()
        {
            continue;
        }
        let path = destination.join(metadata.key.as_str());
        let parent = path.parent().context("backup LFS object has no parent")?;
        tokio::fs::create_dir_all(parent).await?;
        let mut source = store.read(&metadata.key).await?;
        let mut output = tokio::fs::File::create(&path).await?;
        let mut digest = Sha256::new();
        let mut size = 0_u64;
        let mut buffer = vec![0_u8; 128 * 1024];
        loop {
            let read = source.read(&mut buffer).await?;
            if read == 0 {
                break;
            }
            digest.update(&buffer[..read]);
            output.write_all(&buffer[..read]).await?;
            size = size.saturating_add(read as u64);
        }
        output.flush().await?;
        output.sync_all().await?;
        ensure!(size == metadata.size, "backup LFS object size mismatch");
        let actual: [u8; 32] = digest.finalize().into();
        ensure!(
            actual == *metadata.digest.as_bytes(),
            "backup LFS object digest mismatch"
        );
    }
    Ok(())
}

async fn snapshot_database(database: &DatabaseConnection, destination: &Path) -> Result<()> {
    let destination = destination
        .to_str()
        .context("database snapshot path is not valid UTF-8")?
        .replace('\'', "''");
    database
        .execute_unprepared(&format!("VACUUM INTO '{destination}'"))
        .await
        .context("could not create consistent SQLite snapshot")?;
    Ok(())
}

async fn create_s3(key: Option<&str>, settings: &Settings, s3: &S3Settings) -> Result<()> {
    let key = s3_object_key(key, s3)?;
    let bucket = s3::bucket(s3)?;
    ensure!(
        !s3::object_exists(&bucket, &key).await?,
        "S3 backup object already exists: s3://{}/{}",
        s3.bucket,
        key
    );

    let temporary = temporary_archive_path(settings, "s3-upload")?;
    let result = async {
        create_archive(&temporary, settings).await?;
        s3::upload_archive(&bucket, &key, &temporary, None).await
    }
    .await;
    let _ = fs::remove_file(&temporary);
    result?;
    println!("Created S3 backup s3://{}/{}.", s3.bucket, key);
    Ok(())
}

async fn restore_s3(
    key: &str,
    settings: &Settings,
    config_path: &Path,
    s3: &S3Settings,
) -> Result<()> {
    s3::validate_key(key)?;
    let bucket = s3::bucket(s3)?;
    let temporary = temporary_archive_path(settings, "s3-download")?;
    let result = async {
        s3::download_archive(&bucket, key, &temporary).await?;
        restore_archive(&temporary, settings, config_path)
    }
    .await;
    let _ = fs::remove_file(&temporary);
    result?;
    println!("Restored S3 backup s3://{}/{}.", s3.bucket, key);
    Ok(())
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

async fn create_provider_locked(
    key: &str,
    settings: &Settings,
    provider: &BackupProviderConfig,
    backup_name: Option<&str>,
    reporter: &MaintenanceProgressReporter,
) -> Result<()> {
    match provider {
        BackupProviderConfig::Filesystem(filesystem) => {
            create_filesystem_locked(key, settings, filesystem, backup_name, reporter).await
        }
        BackupProviderConfig::S3(s3) => {
            create_s3_locked(key, settings, s3, backup_name, reporter).await
        }
    }
}

async fn create_filesystem_locked(
    key: &str,
    settings: &Settings,
    filesystem: &FilesystemSettings,
    backup_name: Option<&str>,
    reporter: &MaintenanceProgressReporter,
) -> Result<()> {
    filesystem::validate_filesystem_key(key)?;
    reporter.report(
        MaintenancePhase::CheckingDestination,
        "Checking the filesystem destination.",
    );
    let directory = filesystem::prepare_filesystem_directory(filesystem, settings)?;
    let output = directory.join(key);
    ensure!(
        !output.exists(),
        "backup file already exists: {}",
        output.display()
    );
    create_archive_locked(&output, settings, backup_name, Some(reporter)).await
}

async fn create_s3_locked(
    key: &str,
    settings: &Settings,
    s3_settings: &S3Settings,
    backup_name: Option<&str>,
    reporter: &MaintenanceProgressReporter,
) -> Result<()> {
    s3::validate_key(key)?;
    reporter.report(
        MaintenancePhase::CheckingDestination,
        "Checking the S3 destination.",
    );
    let bucket = s3::bucket(s3_settings)?;
    ensure!(
        !s3::object_exists(&bucket, key).await?,
        "S3 backup object already exists: s3://{}/{}",
        s3_settings.bucket,
        key
    );
    let temporary = temporary_archive_path(settings, "s3-upload")?;
    let result = async {
        create_archive_locked(&temporary, settings, backup_name, Some(reporter)).await?;
        s3::upload_archive(&bucket, key, &temporary, Some(reporter)).await
    }
    .await;
    let _ = fs::remove_file(&temporary);
    result
}

pub fn new_backup_key(
    provider: &BackupProviderConfig,
    backup_name: Option<&str>,
) -> Result<String> {
    let backup_name = validate_backup_name(backup_name)?;
    match provider {
        BackupProviderConfig::Filesystem(_) => {
            let key = generated_backup_filename(backup_name.as_deref());
            filesystem::validate_filesystem_key(&key)?;
            Ok(key)
        }
        BackupProviderConfig::S3(settings) => {
            generated_s3_object_key(settings, backup_name.as_deref())
        }
    }
}

pub fn validate_backup_name(backup_name: Option<&str>) -> Result<Option<String>> {
    let Some(name) = backup_name.map(str::trim).filter(|name| !name.is_empty()) else {
        return Ok(None);
    };
    ensure!(
        name.chars().count() <= 80,
        "backup name cannot exceed 80 characters"
    );
    ensure!(
        name.chars().all(|character| {
            character.is_ascii_alphanumeric()
                || character.is_ascii_whitespace()
                || matches!(character, '-' | '_' | '.')
        }),
        "backup name can contain only letters, numbers, spaces, hyphens, underscores, and periods"
    );
    Ok(Some(name.to_owned()))
}

fn generated_backup_filename(backup_name: Option<&str>) -> String {
    let name = backup_name
        .map(|name| name.split_ascii_whitespace().collect::<Vec<_>>().join("-"))
        .unwrap_or_else(|| "unnamed".to_owned());
    format!(
        "gitadel-{}__v{}__{}__{}.tar.zst",
        Utc::now().format("%Y%m%dT%H%M%SZ"),
        env!("CARGO_PKG_VERSION"),
        name,
        Uuid::new_v4().simple()
    )
}

fn generated_s3_object_key(settings: &S3Settings, backup_name: Option<&str>) -> Result<String> {
    let filename = generated_backup_filename(backup_name);
    let key = if settings.prefix.is_empty() {
        filename
    } else {
        format!("{}/{filename}", settings.prefix)
    };
    s3::validate_key(&key)?;
    Ok(key)
}

fn s3_object_key(key: Option<&str>, settings: &S3Settings) -> Result<String> {
    let key = match key {
        Some(key) => key.to_owned(),
        None => generated_s3_object_key(settings, None)?,
    };
    s3::validate_key(&key)?;
    Ok(key)
}

fn temporary_archive_path(settings: &Settings, operation: &str) -> Result<PathBuf> {
    let database_path = sqlite_path(&settings.database.url)?;
    let parent = database_path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).with_context(|| format!("could not create {}", parent.display()))?;
    Ok(parent.join(format!(
        ".gitadel-{operation}-{}.tar.zst",
        Uuid::new_v4().simple()
    )))
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

fn restore(input: &Path, settings: &Settings, config_path: &Path) -> Result<()> {
    restore_archive(input, settings, config_path)?;
    println!("Restored backup {}.", input.display());
    Ok(())
}

fn restore_archive(input: &Path, settings: &Settings, config_path: &Path) -> Result<()> {
    ensure!(
        input.is_file(),
        "backup archive does not exist: {}",
        input.display()
    );
    let _lock = acquire_storage_lock(&settings.database)?;
    let database_path = sqlite_path(&settings.database.url)?;
    ensure_restore_target(&database_path, false)?;
    ensure_restore_target(&settings.storage.repository_root, true)?;
    ensure_restore_target(&settings.storage.lfs_root, true)?;

    let parent = database_path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).with_context(|| format!("could not create {}", parent.display()))?;
    let staging = parent.join(format!(".gitadel-restore-{}", Uuid::new_v4().simple()));
    fs::create_dir(&staging).with_context(|| format!("could not create {}", staging.display()))?;
    let result = restore_from_staging(input, &staging, settings, &database_path, config_path);
    let _ = fs::remove_dir_all(&staging);
    result
}

fn restore_from_staging(
    input: &Path,
    staging: &Path,
    settings: &Settings,
    database_path: &Path,
    config_path: &Path,
) -> Result<()> {
    extract_archive(input, staging)?;
    let root = staging.join(ARCHIVE_ROOT);
    let manifest_path = root.join(MANIFEST_NAME);
    let manifest_metadata = fs::metadata(&manifest_path).context("backup manifest is missing")?;
    ensure!(
        manifest_metadata.len() <= 16 * 1024 * 1024,
        "backup manifest is too large"
    );
    let manifest: Manifest =
        serde_json::from_slice(&fs::read(&manifest_path)?).context("backup manifest is invalid")?;
    ensure!(
        (MIN_FORMAT_VERSION..=FORMAT_VERSION).contains(&manifest.format_version),
        "unsupported backup format version {}",
        manifest.format_version
    );
    verify_manifest(&root, &manifest)?;
    let settings_source = root.join(SETTINGS_NAME);
    ensure!(
        manifest.format_version < 2 || settings_source.is_file(),
        "backup settings are missing"
    );

    let suffix = Uuid::new_v4().simple().to_string();
    let repository_temp = sibling_temp(&settings.storage.repository_root, &suffix);
    let lfs_temp = sibling_temp(&settings.storage.lfs_root, &suffix);
    let database_temp = sibling_temp(database_path, &suffix);
    let host_key_temp = sibling_temp(&settings.ssh.host_key, &suffix);
    let settings_temp = sibling_temp(config_path, &suffix);
    let mut temporary_paths = vec![
        repository_temp.clone(),
        lfs_temp.clone(),
        database_temp.clone(),
    ];
    if manifest.host_key {
        temporary_paths.push(host_key_temp.clone());
    }
    if settings_source.is_file() {
        temporary_paths.push(settings_temp.clone());
    }

    let prepared = (|| -> Result<()> {
        copy_tree(&root.join("repositories"), &repository_temp)?;
        copy_tree(&root.join("lfs"), &lfs_temp)?;
        copy_file(&root.join("database.sqlite"), &database_temp)?;
        if manifest.host_key {
            ensure_restore_target(&settings.ssh.host_key, false)?;
            copy_file(&root.join("ssh-host-key"), &host_key_temp)?;
        }
        if settings_source.is_file() {
            copy_file(&settings_source, &settings_temp)?;
        }
        Ok(())
    })();
    if let Err(error) = prepared {
        remove_paths(&temporary_paths);
        return Err(error);
    }

    remove_empty_target(&settings.storage.repository_root)?;
    remove_empty_target(&settings.storage.lfs_root)?;
    rename_prepared(&repository_temp, &settings.storage.repository_root)?;
    rename_prepared(&lfs_temp, &settings.storage.lfs_root)?;
    rename_prepared(&database_temp, database_path)?;
    if manifest.host_key {
        rename_prepared(&host_key_temp, &settings.ssh.host_key)?;
    }
    if settings_source.is_file() {
        rename_prepared(&settings_temp, config_path)?;
    }
    Ok(())
}

fn replace_archive(input: &Path, settings: &Settings) -> Result<()> {
    let inspection = inspect_backup(input)?;
    let database_path = sqlite_path(&settings.database.url)?;
    let parent = database_path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).with_context(|| format!("could not create {}", parent.display()))?;
    let suffix = Uuid::new_v4().simple().to_string();
    let staging = parent.join(format!(".gitadel-replace-{suffix}"));
    let discard_config = parent.join(format!(".gitadel-restored-config-{suffix}.toml"));

    let mut targets = vec![
        database_path.clone(),
        settings.storage.repository_root.clone(),
        settings.storage.lfs_root.clone(),
        sqlite_sidecar(&database_path, "-wal"),
        sqlite_sidecar(&database_path, "-shm"),
    ];
    if inspection.includes_host_key {
        targets.push(settings.ssh.host_key.clone());
    }
    let rollback_paths = targets
        .iter()
        .map(|target| (target.clone(), pre_restore_path(target, &suffix)))
        .collect::<Vec<_>>();

    let mut moved = Vec::new();
    for (target, rollback) in &rollback_paths {
        if !target.exists() {
            continue;
        }
        let moved_path = (|| -> Result<()> {
            if let Some(parent) = rollback.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::rename(target, rollback).with_context(|| {
                format!(
                    "could not move {} out of the restore path",
                    target.display()
                )
            })
        })();
        if let Err(error) = moved_path {
            if let Err(rollback_error) = rollback_moved_paths(&moved) {
                return Err(error).context(format!(
                    "restore preparation failed and moved paths could not be rolled back: {rollback_error}"
                ));
            }
            return Err(error);
        }
        moved.push((target.clone(), rollback.clone()));
    }

    let restored = (|| -> Result<()> {
        fs::create_dir(&staging)
            .with_context(|| format!("could not create {}", staging.display()))?;
        restore_from_staging(input, &staging, settings, &database_path, &discard_config)
    })();
    let _ = fs::remove_dir_all(&staging);
    let _ = fs::remove_file(&discard_config);
    if let Err(error) = restored {
        for target in &targets {
            remove_path(target);
        }
        if let Err(rollback_error) = rollback_moved_paths(&moved) {
            return Err(error).context(format!(
                "restore failed and the previous instance could not be rolled back: {rollback_error}"
            ));
        }
        return Err(error);
    }

    for (_, rollback) in &moved {
        remove_path(rollback);
    }
    Ok(())
}

fn rollback_moved_paths(moved: &[(PathBuf, PathBuf)]) -> Result<()> {
    for (target, rollback) in moved.iter().rev() {
        if rollback.exists() {
            fs::rename(rollback, target).with_context(|| {
                format!(
                    "could not roll back {} to {}",
                    rollback.display(),
                    target.display()
                )
            })?;
        }
    }
    Ok(())
}

fn pre_restore_path(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_owned();
    value.push(format!(".pre-restore-{suffix}"));
    PathBuf::from(value)
}

fn sqlite_sidecar(database_path: &Path, suffix: &str) -> PathBuf {
    let mut value = database_path.as_os_str().to_owned();
    value.push(suffix);
    PathBuf::from(value)
}

fn remove_path(path: &Path) {
    let _ = if path.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    };
}

fn extract_archive(input: &Path, staging: &Path) -> Result<()> {
    let file = File::open(input).with_context(|| format!("could not open {}", input.display()))?;
    let decoder = zstd::Decoder::new(file).context("backup is not a valid Zstandard stream")?;
    let mut archive = tar::Archive::new(decoder);
    for entry in archive.entries().context("could not read backup archive")? {
        let mut entry = entry.context("could not read backup entry")?;
        let path = entry
            .path()
            .context("backup entry path is invalid")?
            .into_owned();
        ensure!(
            safe_archive_path(&path),
            "unsafe backup entry path: {}",
            path.display()
        );
        let kind = entry.header().entry_type();
        ensure!(
            kind.is_file() || kind.is_dir(),
            "unsupported backup entry type"
        );
        ensure!(
            entry.unpack_in(staging)?,
            "backup entry escaped the restore directory"
        );
    }
    Ok(())
}

fn verify_manifest(root: &Path, manifest: &Manifest) -> Result<()> {
    let expected: BTreeMap<&str, &ManifestFile> = manifest
        .files
        .iter()
        .map(|file| (file.path.as_str(), file))
        .collect();
    ensure!(
        expected.len() == manifest.files.len(),
        "backup manifest contains duplicate paths"
    );
    let actual = data_file_paths(root)?;
    let expected_paths: BTreeSet<String> = expected.keys().map(|path| (*path).to_owned()).collect();
    ensure!(
        actual == expected_paths,
        "backup contents do not match the manifest"
    );
    for file in &manifest.files {
        let path = root.join(&file.path);
        let metadata = fs::metadata(&path)?;
        ensure!(
            metadata.len() == file.size,
            "backup file size mismatch: {}",
            file.path
        );
        ensure!(
            sha256(&path)? == file.sha256,
            "backup checksum mismatch: {}",
            file.path
        );
    }
    Ok(())
}

fn manifest_files(root: &Path) -> Result<Vec<ManifestFile>> {
    let mut files = Vec::new();
    for path in data_file_paths(root)? {
        let full_path = root.join(&path);
        files.push(ManifestFile {
            path,
            size: fs::metadata(&full_path)?.len(),
            sha256: sha256(&full_path)?,
        });
    }
    Ok(files)
}

fn data_file_paths(root: &Path) -> Result<BTreeSet<String>> {
    let mut files = BTreeSet::new();
    for entry in WalkDir::new(root).follow_links(false) {
        let entry = entry?;
        if entry.file_type().is_symlink() {
            bail!(
                "symbolic links are not supported in backups: {}",
                entry.path().display()
            );
        }
        if !entry.file_type().is_file() || entry.path() == root.join(MANIFEST_NAME) {
            continue;
        }
        let relative = entry.path().strip_prefix(root)?;
        files.insert(path_string(relative)?);
    }
    Ok(files)
}

fn copy_tree(source: &Path, destination: &Path) -> Result<()> {
    fs::create_dir_all(destination)
        .with_context(|| format!("could not create {}", destination.display()))?;
    if !source.exists() {
        return Ok(());
    }
    for entry in WalkDir::new(source).follow_links(false) {
        let entry = entry?;
        let relative = entry.path().strip_prefix(source)?;
        let target = destination.join(relative);
        if entry.file_type().is_symlink() {
            bail!(
                "symbolic links are not supported: {}",
                entry.path().display()
            );
        }
        if entry.file_type().is_dir() {
            fs::create_dir_all(&target)?;
        } else if entry.file_type().is_file() {
            copy_file(entry.path(), &target)?;
        }
    }
    Ok(())
}

fn copy_file(source: &Path, destination: &Path) -> Result<()> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::copy(source, destination).with_context(|| {
        format!(
            "could not copy {} to {}",
            source.display(),
            destination.display()
        )
    })?;
    let output = File::open(destination)?;
    output.sync_all()?;
    Ok(())
}

fn sha256(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn sqlite_path(url: &str) -> Result<PathBuf> {
    let raw = url
        .strip_prefix("sqlite://")
        .or_else(|| url.strip_prefix("sqlite:"))
        .context("backups require a SQLite database URL")?;
    let raw = raw.split('?').next().unwrap_or(raw);
    ensure!(
        !raw.is_empty() && raw != ":memory:",
        "backups require a file-backed SQLite database"
    );
    Ok(PathBuf::from(raw))
}

fn lock_path(database_path: &Path) -> PathBuf {
    let mut value = database_path.as_os_str().to_owned();
    value.push(".lock");
    PathBuf::from(value)
}

fn sibling_temp(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_owned();
    value.push(format!(".restore-{suffix}"));
    PathBuf::from(value)
}

fn safe_archive_path(path: &Path) -> bool {
    let mut components = path.components();
    matches!(components.next(), Some(Component::Normal(root)) if root == ARCHIVE_ROOT)
        && components.all(|component| matches!(component, Component::Normal(_)))
}

fn path_string(path: &Path) -> Result<String> {
    let value = path.to_str().context("backup path is not valid UTF-8")?;
    Ok(value.replace(std::path::MAIN_SEPARATOR, "/"))
}

fn ensure_restore_target(path: &Path, allow_empty_directory: bool) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    if allow_empty_directory && path.is_dir() && fs::read_dir(path)?.next().is_none() {
        return Ok(());
    }
    bail!("restore target is not empty: {}", path.display())
}

fn remove_empty_target(path: &Path) -> Result<()> {
    if path.is_dir() {
        fs::remove_dir(path).with_context(|| format!("could not remove {}", path.display()))?;
    }
    Ok(())
}

fn rename_prepared(source: &Path, destination: &Path) -> Result<()> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::rename(source, destination).with_context(|| {
        format!(
            "could not install {} at {}",
            source.display(),
            destination.display()
        )
    })
}

fn remove_paths(paths: &[PathBuf]) {
    for path in paths {
        let _ = if path.is_dir() {
            fs::remove_dir_all(path)
        } else {
            fs::remove_file(path)
        };
    }
}

#[cfg(test)]
mod tests {
    use chrono::Duration as ChronoDuration;
    use sea_orm::{ActiveModelTrait as _, EntityTrait as _, Set};

    use super::s3::validate_managed_key as validate_managed_s3_backup_key;
    use super::*;
    use crate::{
        blob_store::lfs_object_key,
        entity::{backup_provider_schedule, instance, user},
    };

    #[derive(Debug, PartialEq, Eq)]
    struct RestoredState {
        username: String,
        site_name: String,
        site_description: Option<String>,
        repository: Vec<u8>,
        lfs: Vec<u8>,
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
        settings.ssh.host_key = root.join("ssh-host-key");
        settings
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
            host_key: fs::read(&restored_settings.ssh.host_key)?,
            effective_settings: fs::read_to_string(&restored_config)?,
        };
        let expected = RestoredState {
            username: "backup-admin".to_owned(),
            site_name: "Recovered Gitadel".to_owned(),
            site_description: Some("Complete backup".to_owned()),
            repository: b"repository-object".to_vec(),
            lfs: b"lfs-object".to_vec(),
            host_key: b"ssh-host-key".to_vec(),
            effective_settings: toml::to_string_pretty(&source_settings)?,
        };

        assert_eq!(restored, expected);
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
