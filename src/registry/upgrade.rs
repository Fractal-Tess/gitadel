//! One-time startup conversions of registry data written by earlier versions.
//! Each step is idempotent and crash-safe, and records its completion in
//! `registry_upgrades` so later starts skip it.

use std::{
    fs::{self, File},
    io::{self, Read as _, Write as _},
    os::unix::fs::DirBuilderExt as _,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use chrono::Utc;
use sea_orm::{ConnectionTrait as _, DatabaseConnection, Statement};
use sha2::{Digest as _, Sha256};
use uuid::Uuid;

use crate::{config::StorageSettings, filesystem::create_private_directory_async};

/// Moves registry data out of repository directories into the registry root.
pub(crate) const RELOCATE: &str = "relocate_repository_registry";

/// Where 0.13 and earlier kept a repository's registry data, relative to its
/// bare repository directory.
const LEGACY_DIRECTORY: &str = "gitadel-registry";

pub(crate) async fn completed(database: &DatabaseConnection, step: &str) -> Result<bool> {
    Ok(database
        .query_one_raw(Statement::from_sql_and_values(
            database.get_database_backend(),
            "SELECT step FROM registry_upgrades WHERE step = ?",
            [step.into()],
        ))
        .await?
        .is_some())
}

pub(crate) async fn mark_completed(database: &DatabaseConnection, step: &str) -> Result<()> {
    database
        .execute_raw(Statement::from_sql_and_values(
            database.get_database_backend(),
            "INSERT INTO registry_upgrades (step, completed_at) VALUES (?, ?) \
             ON CONFLICT (step) DO NOTHING",
            [step.into(), Utc::now().into()],
        ))
        .await?;
    Ok(())
}

/// What moving one repository's registry directory did.
#[derive(Debug, Default, PartialEq, Eq)]
struct Relocation {
    /// The whole directory was renamed into place.
    renamed: bool,
    /// Files copied, verified, and removed from the source.
    copied_files: u64,
    copied_bytes: u64,
    /// Source files whose verified copy already existed.
    already_present: u64,
    /// Source files left in place because a different file is at their
    /// destination.
    conflicts: u64,
}

/// Moves every `<repository_root>/<storage-key>.git/gitadel-registry/images/`
/// to `<registry_root>/<storage-key>/`. A rename is used when both sit on one
/// filesystem; otherwise each file is copied, its size and SHA-256 verified,
/// and only then removed from the source, so an interrupted run resumes safely.
pub(crate) async fn relocate(
    database: &DatabaseConnection,
    settings: &StorageSettings,
) -> Result<()> {
    if completed(database, RELOCATE).await? {
        return Ok(());
    }
    create_private_directory_async(&settings.registry_root)
        .await
        .with_context(|| format!("could not create {}", settings.registry_root.display()))?;
    let repository_root = settings.repository_root.clone();
    let registry_root = settings.registry_root.clone();
    tokio::task::spawn_blocking(move || relocate_all(&repository_root, &registry_root)).await??;
    mark_completed(database, RELOCATE).await
}

fn relocate_all(repository_root: &Path, registry_root: &Path) -> Result<()> {
    for (storage_key, legacy) in legacy_directories(repository_root)? {
        let destination = registry_root.join(storage_key.to_string());
        let relocation = relocate_repository(&legacy, &destination).with_context(|| {
            format!(
                "could not move registry data from {} to {}",
                legacy.display(),
                destination.display()
            )
        })?;
        tracing::info!(
            %storage_key,
            source = %legacy.display(),
            destination = %destination.display(),
            renamed = relocation.renamed,
            copied_files = relocation.copied_files,
            copied_bytes = relocation.copied_bytes,
            already_present = relocation.already_present,
            "moved container registry data out of the repository directory"
        );
        if relocation.conflicts > 0 {
            tracing::warn!(
                %storage_key,
                conflicts = relocation.conflicts,
                source = %legacy.display(),
                "left registry files whose destination already holds different content"
            );
        }
    }
    Ok(())
}

/// Registry directories inside bare repositories, by repository storage key.
fn legacy_directories(repository_root: &Path) -> Result<Vec<(Uuid, PathBuf)>> {
    let entries = match fs::read_dir(repository_root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut result = Vec::new();
    for entry in entries {
        let entry = entry?;
        let Some(storage_key) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.strip_suffix(".git"))
            .and_then(|name| Uuid::parse_str(name).ok())
        else {
            continue;
        };
        let legacy = entry.path().join(LEGACY_DIRECTORY);
        if fs::symlink_metadata(&legacy).is_ok_and(|metadata| metadata.is_dir()) {
            result.push((storage_key, legacy));
        }
    }
    result.sort();
    Ok(result)
}

fn relocate_repository(legacy: &Path, destination: &Path) -> Result<Relocation> {
    let images = legacy.join("images");
    let mut relocation = Relocation::default();
    if fs::symlink_metadata(&images).is_ok_and(|metadata| metadata.is_dir()) {
        let renamed = match fs::symlink_metadata(destination) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                match fs::rename(&images, destination) {
                    Ok(()) => true,
                    Err(error) if error.kind() == io::ErrorKind::CrossesDevices => false,
                    Err(error) => return Err(error.into()),
                }
            }
            _ => false,
        };
        if renamed {
            sync_parent(destination)?;
            relocation.renamed = true;
        } else {
            merge_tree(&images, destination, &mut relocation)?;
        }
    }
    remove_empty_directories(legacy)?;
    Ok(relocation)
}

/// Moves every file of `source` to the same relative path under
/// `destination`, never removing a source file before its copy is verified.
fn merge_tree(source: &Path, destination: &Path, relocation: &mut Relocation) -> Result<()> {
    // Copies an earlier run could not finish are discarded and redone.
    for entry in walkdir::WalkDir::new(destination).follow_links(false) {
        let entry = entry?;
        if entry.file_type().is_file()
            && entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with(".relocate-"))
        {
            fs::remove_file(entry.path())?;
        }
    }
    for entry in walkdir::WalkDir::new(source).follow_links(false) {
        let entry = entry?;
        if !entry.file_type().is_file() {
            continue;
        }
        let relative = entry.path().strip_prefix(source)?;
        let target = destination.join(relative);
        match fs::symlink_metadata(&target) {
            Ok(metadata) if metadata.is_file() => {
                if fingerprint(entry.path())? == fingerprint(&target)? {
                    fs::remove_file(entry.path())?;
                    relocation.already_present += 1;
                } else {
                    relocation.conflicts += 1;
                }
            }
            Ok(_) => relocation.conflicts += 1,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let size = copy_verified(entry.path(), &target)?;
                fs::remove_file(entry.path())?;
                relocation.copied_files += 1;
                relocation.copied_bytes += size;
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

/// Copies `source` beside `target`, verifies the copy's size and SHA-256
/// against the source, and renames it into place.
fn copy_verified(source: &Path, target: &Path) -> Result<u64> {
    let parent = target.parent().context("registry file has no parent")?;
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(parent)?;
    let temporary = parent.join(format!(".relocate-{}", Uuid::new_v4().simple()));
    let result = (|| -> Result<u64> {
        let mut input = File::open(source)?;
        let mut output = File::create_new(&temporary)?;
        let mut hasher = Sha256::new();
        let mut size = 0_u64;
        let mut buffer = vec![0_u8; 64 * 1024];
        loop {
            let read = input.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
            output.write_all(&buffer[..read])?;
            size += read as u64;
        }
        output.sync_all()?;
        drop(output);
        let expected = (size, <[u8; 32]>::from(hasher.finalize()));
        anyhow::ensure!(
            fingerprint(&temporary)? == expected,
            "copy of {} did not verify",
            source.display()
        );
        fs::rename(&temporary, target)?;
        sync_parent(target)?;
        Ok(size)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

/// Size and SHA-256 of a file.
fn fingerprint(path: &Path) -> Result<(u64, [u8; 32])> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut size = 0_u64;
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        size += read as u64;
    }
    Ok((size, hasher.finalize().into()))
}

fn sync_parent(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        File::open(parent)?.sync_all()?;
    }
    Ok(())
}

/// Removes `root` and every directory under it that holds no files.
fn remove_empty_directories(root: &Path) -> Result<()> {
    for entry in walkdir::WalkDir::new(root)
        .follow_links(false)
        .contents_first(true)
    {
        let entry = entry?;
        if entry.file_type().is_dir() {
            match fs::remove_dir(entry.path()) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::DirectoryNotEmpty => {}
                Err(error) => return Err(error.into()),
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database;

    struct Fixture {
        root: PathBuf,
        settings: StorageSettings,
        database: DatabaseConnection,
    }

    impl Fixture {
        async fn new() -> Self {
            let root =
                std::env::temp_dir().join(format!("gitadel-registry-upgrade-{}", Uuid::new_v4()));
            fs::create_dir_all(&root).unwrap();
            let database = database::connect_and_migrate(&crate::config::DatabaseSettings {
                url: format!("sqlite://{}?mode=rwc", root.join("gitadel.db").display()),
            })
            .await
            .unwrap();
            let settings = StorageSettings {
                repository_root: root.join("repositories"),
                lfs_root: root.join("lfs"),
                registry_root: root.join("registry"),
                actions_artifact_root: root.join("actions-artifacts"),
            };
            Self {
                root,
                settings,
                database,
            }
        }

        fn legacy(&self, storage_key: Uuid) -> PathBuf {
            self.settings
                .repository_root
                .join(format!("{storage_key}.git"))
                .join(LEGACY_DIRECTORY)
                .join("images")
        }

        /// Writes an old-layout image with one blob and a tag.
        fn write_legacy(&self, storage_key: Uuid) -> Vec<(String, Vec<u8>)> {
            let image = hex::encode(Sha256::digest(b""));
            let blob = b"legacy layer".to_vec();
            let digest = hex::encode(Sha256::digest(&blob));
            let files = vec![
                (format!("{image}/suffix"), Vec::new()),
                (
                    format!("{image}/blobs/{}/{}/{digest}", &digest[..2], &digest[2..4]),
                    blob,
                ),
                (format!("{image}/refs/tag.json"), b"{}".to_vec()),
            ];
            for (relative, bytes) in &files {
                let path = self.legacy(storage_key).join(relative);
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(path, bytes).unwrap();
            }
            files
        }

        fn assert_relocated(&self, storage_key: Uuid, files: &[(String, Vec<u8>)]) {
            for (relative, bytes) in files {
                let path = self
                    .settings
                    .registry_root
                    .join(storage_key.to_string())
                    .join(relative);
                assert_eq!(&fs::read(&path).unwrap(), bytes, "{}", path.display());
            }
            assert!(
                !self
                    .settings
                    .repository_root
                    .join(format!("{storage_key}.git"))
                    .join(LEGACY_DIRECTORY)
                    .exists()
            );
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[tokio::test]
    async fn relocation_moves_repository_registry_data_once() {
        let fixture = Fixture::new().await;
        let storage_key = Uuid::new_v4();
        let files = fixture.write_legacy(storage_key);

        relocate(&fixture.database, &fixture.settings)
            .await
            .unwrap();
        fixture.assert_relocated(storage_key, &files);
        assert!(completed(&fixture.database, RELOCATE).await.unwrap());

        // A completed relocation does not rescan repository directories.
        let late = Uuid::new_v4();
        fixture.write_legacy(late);
        relocate(&fixture.database, &fixture.settings)
            .await
            .unwrap();
        assert!(fixture.legacy(late).exists());
    }

    #[tokio::test]
    async fn interrupted_relocation_resumes_without_losing_files() {
        let fixture = Fixture::new().await;
        let storage_key = Uuid::new_v4();
        let files = fixture.write_legacy(storage_key);
        // An earlier run copied one file and stopped before removing its source
        // or touching the others.
        let destination = fixture.settings.registry_root.join(storage_key.to_string());
        let (first, bytes) = &files[1];
        fs::create_dir_all(destination.join(first).parent().unwrap()).unwrap();
        fs::write(destination.join(first), bytes).unwrap();
        // A copy that never finished is left as a temporary file.
        fs::write(destination.join(".relocate-interrupted"), b"partial").unwrap();

        relocate(&fixture.database, &fixture.settings)
            .await
            .unwrap();
        fixture.assert_relocated(storage_key, &files);
        assert!(!destination.join(".relocate-interrupted").exists());
    }

    #[test]
    fn conflicting_destination_keeps_the_source() {
        let root = std::env::temp_dir().join(format!("gitadel-registry-merge-{}", Uuid::new_v4()));
        let source = root.join("source");
        let destination = root.join("destination");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&destination).unwrap();
        fs::write(source.join("suffix"), b"worker").unwrap();
        fs::write(destination.join("suffix"), b"other").unwrap();
        let mut relocation = Relocation::default();
        merge_tree(&source, &destination, &mut relocation).unwrap();
        assert_eq!(relocation.conflicts, 1);
        assert_eq!(fs::read(source.join("suffix")).unwrap(), b"worker");
        fs::remove_dir_all(root).unwrap();
    }
}
