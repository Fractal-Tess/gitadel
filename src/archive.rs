use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{Read as _, Write as _},
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result, bail, ensure};
use chrono::{SecondsFormat, Utc};
use fs2::FileExt as _;
use sea_orm::{ConnectionTrait as _, DatabaseConnection};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use uuid::Uuid;
use walkdir::WalkDir;

use crate::{
    config::{BackupCommand, DatabaseSettings, Settings},
    database,
};

const FORMAT_VERSION: u32 = 1;
const ARCHIVE_ROOT: &str = "backup";
const MANIFEST_NAME: &str = "manifest.json";

pub struct StorageLock {
    _file: File,
}

#[derive(Serialize, Deserialize)]
struct Manifest {
    format_version: u32,
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

pub async fn run(command: &BackupCommand, settings: &Settings) -> Result<()> {
    match command {
        BackupCommand::Create { output } => create(output, settings).await,
        BackupCommand::Restore { input } => restore(input, settings),
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
    ensure!(
        !output.exists(),
        "backup output already exists: {}",
        output.display()
    );
    let output_parent = output.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(output_parent)
        .with_context(|| format!("could not create {}", output_parent.display()))?;
    let _lock = acquire_storage_lock(&settings.database)?;
    let staging = output_parent.join(format!(".gitadel-backup-{}", Uuid::new_v4().simple()));
    let temporary_output = output_parent.join(format!(
        ".{}.{}.tmp",
        output
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("backup"),
        Uuid::new_v4().simple()
    ));
    let result = match create_staged_backup(&staging, &temporary_output, settings).await {
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
    result?;
    println!("Created backup {}.", output.display());
    Ok(())
}

async fn create_staged_backup(
    staging: &Path,
    temporary_output: &Path,
    settings: &Settings,
) -> Result<()> {
    fs::create_dir_all(staging)
        .with_context(|| format!("could not create {}", staging.display()))?;
    fs::create_dir(staging.join("repositories"))?;
    fs::create_dir(staging.join("lfs"))?;

    let database = database::connect_and_migrate(&settings.database).await?;
    snapshot_database(&database, &staging.join("database.sqlite")).await?;
    database
        .close()
        .await
        .context("could not close database snapshot connection")?;

    copy_tree(
        &settings.storage.repository_root,
        &staging.join("repositories"),
    )?;
    copy_tree(&settings.storage.lfs_root, &staging.join("lfs"))?;
    let host_key = settings.ssh.host_key.is_file();
    if host_key {
        fs::copy(&settings.ssh.host_key, staging.join("ssh-host-key")).with_context(|| {
            format!(
                "could not copy SSH host key {}",
                settings.ssh.host_key.display()
            )
        })?;
    }

    let manifest = Manifest {
        format_version: FORMAT_VERSION,
        created_at: Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
        host_key,
        files: manifest_files(staging)?,
    };
    let manifest_bytes =
        serde_json::to_vec_pretty(&manifest).context("could not encode backup manifest")?;
    let mut manifest_file = File::create(staging.join(MANIFEST_NAME))?;
    manifest_file.write_all(&manifest_bytes)?;
    manifest_file.sync_all()?;

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

fn restore(input: &Path, settings: &Settings) -> Result<()> {
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
    let result = restore_from_staging(input, &staging, settings, &database_path);
    let _ = fs::remove_dir_all(&staging);
    result?;
    println!("Restored backup {}.", input.display());
    Ok(())
}

fn restore_from_staging(
    input: &Path,
    staging: &Path,
    settings: &Settings,
    database_path: &Path,
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
        manifest.format_version == FORMAT_VERSION,
        "unsupported backup format version {}",
        manifest.format_version
    );
    verify_manifest(&root, &manifest)?;

    let suffix = Uuid::new_v4().simple().to_string();
    let repository_temp = sibling_temp(&settings.storage.repository_root, &suffix);
    let lfs_temp = sibling_temp(&settings.storage.lfs_root, &suffix);
    let database_temp = sibling_temp(database_path, &suffix);
    let host_key_temp = sibling_temp(&settings.ssh.host_key, &suffix);
    let mut temporary_paths = vec![
        repository_temp.clone(),
        lfs_temp.clone(),
        database_temp.clone(),
    ];
    if manifest.host_key {
        temporary_paths.push(host_key_temp.clone());
    }

    let prepared = (|| -> Result<()> {
        copy_tree(&root.join("repositories"), &repository_temp)?;
        copy_tree(&root.join("lfs"), &lfs_temp)?;
        copy_file(&root.join("database.sqlite"), &database_temp)?;
        if manifest.host_key {
            ensure_restore_target(&settings.ssh.host_key, false)?;
            copy_file(&root.join("ssh-host-key"), &host_key_temp)?;
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
    Ok(())
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
