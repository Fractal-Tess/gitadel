//! Backup creation: staging a consistent snapshot of the database and data
//! roots, writing the archive, and handing it to a filesystem or S3 provider.

use super::*;

pub(super) async fn create(output: &Path, settings: &Settings) -> Result<()> {
    create_archive(output, settings).await?;
    println!("Created backup {}.", output.display());
    Ok(())
}

pub(super) async fn create_archive(output: &Path, settings: &Settings) -> Result<()> {
    let _lock = acquire_storage_lock(&settings.database)?;
    create_archive_locked(output, settings, None, None).await
}

pub(super) async fn create_archive_locked(
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

pub(super) async fn create_staged_backup(
    staging: &Path,
    temporary_output: &Path,
    settings: &Settings,
    backup_name: Option<&str>,
    reporter: Option<&MaintenanceProgressReporter>,
) -> Result<()> {
    create_private_directory(staging)
        .with_context(|| format!("could not create {}", staging.display()))?;
    fs::create_dir(staging.join("repositories"))?;
    fs::create_dir(staging.join("lfs"))?;
    fs::create_dir(staging.join("registry"))?;
    fs::create_dir(staging.join("actions-artifacts"))?;
    if let Some(reporter) = reporter {
        reporter.report(
            MaintenancePhase::SnapshottingDatabase,
            "Creating a consistent database snapshot.",
        );
    }

    let database = database::connect_and_migrate(&settings.database).await?;
    // Payloads on a storage target are materialized into the local layout, so
    // a restored instance starts on local storage.
    let lfs_storage = DomainStorage::load(
        &database,
        Arc::new(LfsDomain),
        LfsDomain.local_root(&settings.storage),
    )
    .await?;
    let lfs_objects = external_inventory(&database, &lfs_storage).await?;
    let registry_storage = DomainStorage::load(
        &database,
        Arc::new(RegistryDomain),
        RegistryDomain.local_root(&settings.storage),
    )
    .await?;
    let registry_objects = external_inventory(&database, &registry_storage).await?;
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
            "Copying Git repositories and container registry objects.",
        );
    }
    copy_tree(
        &settings.storage.repository_root,
        &staging.join("repositories"),
    )?;
    copy_tree(&settings.storage.registry_root, &staging.join("registry"))?;
    if let Some(objects) = registry_objects {
        materialize(&registry_storage, &objects, staging.join("registry")).await?;
    }
    if let Some(reporter) = reporter {
        reporter.report(
            MaintenancePhase::CopyingLfs,
            "Copying Git LFS objects and uploaded files.",
        );
    }
    copy_tree(&settings.storage.lfs_root, &staging.join("lfs"))?;
    if let Some(objects) = lfs_objects {
        materialize(&lfs_storage, &objects, staging.join("lfs")).await?;
    }
    copy_tree(
        &settings.storage.actions_artifact_root,
        &staging.join("actions-artifacts"),
    )?;
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
    if let Some(reporter) = reporter {
        reporter.report(
            MaintenancePhase::Compressing,
            "Compressing the backup archive.",
        );
    }
    write_archive(staging, &manifest, temporary_output)
}

/// Writes `manifest` into `staging` and compresses the directory into `destination`.
pub(super) fn write_archive(staging: &Path, manifest: &Manifest, destination: &Path) -> Result<()> {
    let manifest_bytes =
        serde_json::to_vec_pretty(manifest).context("could not encode backup manifest")?;
    let mut manifest_file = File::create(staging.join(MANIFEST_NAME))?;
    manifest_file.write_all(&manifest_bytes)?;
    manifest_file.sync_all()?;
    let output = open_private_file(destination, true)
        .with_context(|| format!("could not create {}", destination.display()))?;
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

pub(super) async fn prepare_restored_snapshot(path: &Path) -> Result<()> {
    let mut options = ConnectOptions::new(format!("sqlite://{}?mode=rwc", path.display()));
    options.max_connections(1).sqlx_logging(false);
    let database = Database::connect(options)
        .await
        .context("could not open staged backup database")?;
    database
        .execute_unprepared(
            "PRAGMA foreign_keys = ON;\
             UPDATE storage_domain_state SET active_target_id = NULL, updated_at = CURRENT_TIMESTAMP;\
             UPDATE lfs_objects SET storage_target_id = NULL;\
             DELETE FROM storage_migrations;\
             PRAGMA wal_checkpoint(TRUNCATE);",
        )
        .await
        .context("could not rebind staged backup to runtime-local object storage")?;
    database.close().await?;
    Ok(())
}

/// The objects to copy out of a domain's storage target, or `None` when the
/// domain uses its local root, which the directory copy already covers.
pub(super) async fn external_inventory(
    database: &DatabaseConnection,
    storage: &DomainStorage,
) -> Result<Option<Vec<BlobMetadata>>> {
    if storage.target_id().is_none() {
        return Ok(None);
    }
    storage.inventory(database).await.map(Some)
}

/// Writes `objects` into the domain's local layout under `root`.
pub(super) async fn materialize(
    storage: &DomainStorage,
    objects: &[BlobMetadata],
    root: PathBuf,
) -> Result<()> {
    let label = storage.domain().label();
    let destination = storage.domain().open_local(root).await?;
    let source = storage.store();
    for object in objects {
        // A stale local copy from before the target was selected is replaced.
        destination.delete(&object.key).await?;
        manager::copy_object(source.as_ref(), destination.as_ref(), object, label)
            .await
            .with_context(|| format!("could not back up {label} object {}", object.key))?;
    }
    Ok(())
}

pub(super) async fn snapshot_database(
    database: &DatabaseConnection,
    destination: &Path,
) -> Result<()> {
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

pub(super) async fn create_s3(
    key: Option<&str>,
    settings: &Settings,
    s3: &S3Settings,
) -> Result<()> {
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

pub(super) async fn restore_s3(
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

pub(super) async fn create_provider_locked(
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

pub(super) async fn create_filesystem_locked(
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

pub(super) async fn create_s3_locked(
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

pub(super) fn generated_backup_filename(backup_name: Option<&str>) -> String {
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

pub(super) fn generated_s3_object_key(
    settings: &S3Settings,
    backup_name: Option<&str>,
) -> Result<String> {
    let filename = generated_backup_filename(backup_name);
    let key = if settings.prefix.is_empty() {
        filename
    } else {
        format!("{}/{filename}", settings.prefix)
    };
    s3::validate_key(&key)?;
    Ok(key)
}

pub(super) fn s3_object_key(key: Option<&str>, settings: &S3Settings) -> Result<String> {
    let key = match key {
        Some(key) => key.to_owned(),
        None => generated_s3_object_key(settings, None)?,
    };
    s3::validate_key(&key)?;
    Ok(key)
}

pub(super) fn temporary_archive_path(settings: &Settings, operation: &str) -> Result<PathBuf> {
    let database_path = sqlite_path(&settings.database.url)?;
    let parent = database_path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).with_context(|| format!("could not create {}", parent.display()))?;
    Ok(parent.join(format!(
        ".gitadel-{operation}-{}.tar.zst",
        Uuid::new_v4().simple()
    )))
}
