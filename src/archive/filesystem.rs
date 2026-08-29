use std::{
    fs::{self, OpenOptions},
    io::Write as _,
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use chrono::{DateTime, SecondsFormat, Utc};
use uuid::Uuid;

use crate::{backup_provider::FilesystemSettings, config::Settings};

use super::{BackupObject, generated_backup_metadata, sort_backups};

pub(super) fn test_filesystem_settings(
    filesystem: &FilesystemSettings,
    settings: &Settings,
) -> Result<()> {
    let directory = prepare_filesystem_directory(filesystem, settings)?;
    let probe = directory.join(format!(
        ".gitadel-backup-provider-test-{}",
        Uuid::new_v4().simple()
    ));
    let result = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
        .and_then(|mut file| file.write_all(b"gitadel backup provider test"))
        .with_context(|| {
            format!(
                "backup filesystem destination is not writable: {}",
                directory.display()
            )
        });
    let _ = fs::remove_file(probe);
    result
}

pub(super) fn prepare_filesystem_directory(
    filesystem: &FilesystemSettings,
    settings: &Settings,
) -> Result<PathBuf> {
    ensure!(
        filesystem.path.is_absolute(),
        "backup filesystem path must be absolute"
    );
    fs::create_dir_all(&filesystem.path).with_context(|| {
        format!(
            "could not create backup filesystem destination {}",
            filesystem.path.display()
        )
    })?;
    let directory = fs::canonicalize(&filesystem.path).with_context(|| {
        format!(
            "could not resolve backup filesystem destination {}",
            filesystem.path.display()
        )
    })?;
    for source in [
        &settings.storage.repository_root,
        &settings.storage.lfs_root,
    ] {
        let source = if source.exists() {
            fs::canonicalize(source)
                .with_context(|| format!("could not resolve {}", source.display()))?
        } else if source.is_absolute() {
            source.clone()
        } else {
            std::env::current_dir()?.join(source)
        };
        ensure!(
            directory != source && !directory.starts_with(&source),
            "backup filesystem destination {} must be outside managed storage {}",
            directory.display(),
            source.display()
        );
    }
    Ok(directory)
}

pub(super) async fn open_filesystem_backup(
    settings: &Settings,
    filesystem: &FilesystemSettings,
    key: &str,
) -> Result<(tokio::fs::File, u64)> {
    let path = filesystem_backup_path(filesystem, settings, key)?;
    let metadata = tokio::fs::symlink_metadata(&path)
        .await
        .with_context(|| format!("could not inspect filesystem backup {}", path.display()))?;
    ensure!(
        metadata.file_type().is_file(),
        "filesystem backup is not a regular file: {}",
        path.display()
    );
    let file = tokio::fs::File::open(&path)
        .await
        .with_context(|| format!("could not open filesystem backup {}", path.display()))?;
    Ok((file, metadata.len()))
}

pub(super) async fn delete_filesystem_backup(
    settings: &Settings,
    filesystem: &FilesystemSettings,
    key: &str,
) -> Result<()> {
    let path = filesystem_backup_path(filesystem, settings, key)?;
    let metadata = tokio::fs::symlink_metadata(&path)
        .await
        .with_context(|| format!("could not inspect filesystem backup {}", path.display()))?;
    ensure!(
        metadata.file_type().is_file(),
        "filesystem backup is not a regular file: {}",
        path.display()
    );
    tokio::fs::remove_file(&path)
        .await
        .with_context(|| format!("could not delete filesystem backup {}", path.display()))
}

pub(super) async fn list_filesystem_backups(
    filesystem: &FilesystemSettings,
    settings: &Settings,
) -> Result<Vec<BackupObject>> {
    let directory = prepare_filesystem_directory(filesystem, settings)?;
    let mut entries = tokio::fs::read_dir(&directory)
        .await
        .with_context(|| format!("could not read backup directory {}", directory.display()))?;
    let mut backups = Vec::new();
    while let Some(entry) = entries
        .next_entry()
        .await
        .with_context(|| format!("could not read backup directory {}", directory.display()))?
    {
        let file_type = entry
            .file_type()
            .await
            .with_context(|| format!("could not inspect {}", entry.path().display()))?;
        if !file_type.is_file() {
            continue;
        }
        let Some(key) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if validate_filesystem_key(&key).is_err() {
            continue;
        }
        let metadata = entry
            .metadata()
            .await
            .with_context(|| format!("could not inspect {}", entry.path().display()))?;
        let created_at = metadata
            .modified()
            .context("filesystem backup modification time is unavailable")?;
        let (name, gitadel_version) = generated_backup_metadata(&key);
        backups.push(BackupObject {
            key,
            name,
            gitadel_version,
            size: metadata.len(),
            created_at: DateTime::<Utc>::from(created_at)
                .to_rfc3339_opts(SecondsFormat::Secs, true),
        });
    }
    sort_backups(&mut backups);
    Ok(backups)
}

pub(super) fn filesystem_backup_path(
    filesystem: &FilesystemSettings,
    settings: &Settings,
    key: &str,
) -> Result<PathBuf> {
    validate_filesystem_key(key)?;
    Ok(prepare_filesystem_directory(filesystem, settings)?.join(key))
}

pub(super) async fn copy_backup(source: &Path, archive: &Path) -> Result<()> {
    tokio::fs::copy(source, archive).await.with_context(|| {
        format!(
            "could not copy filesystem backup {} for validation",
            source.display()
        )
    })?;
    Ok(())
}

pub(super) fn validate_filesystem_key(key: &str) -> Result<()> {
    let mut components = Path::new(key).components();
    ensure!(
        matches!(components.next(), Some(Component::Normal(_)))
            && components.next().is_none()
            && key.ends_with(".tar.zst")
            && !key.chars().any(char::is_control),
        "filesystem backup key must be a single .tar.zst filename"
    );
    Ok(())
}
