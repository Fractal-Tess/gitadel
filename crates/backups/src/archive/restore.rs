//! Backup restoration: extracting and verifying an archive, swapping it into
//! the configured storage paths, and the file helpers both directions share.

use super::*;

pub(super) fn restore(input: &Path, settings: &Settings, config_path: &Path) -> Result<()> {
    restore_archive(input, settings, config_path)?;
    println!("Restored backup {}.", input.display());
    Ok(())
}

pub(super) fn restore_archive(input: &Path, settings: &Settings, config_path: &Path) -> Result<()> {
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
    ensure_restore_target(&settings.storage.registry_root, true)?;

    let parent = database_path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).with_context(|| format!("could not create {}", parent.display()))?;
    let staging = parent.join(format!(".gitadel-restore-{}", Uuid::new_v4().simple()));
    fs::create_dir(&staging).with_context(|| format!("could not create {}", staging.display()))?;
    let result = restore_from_staging(input, &staging, settings, &database_path, config_path);
    let _ = fs::remove_dir_all(&staging);
    result
}

pub(super) fn restore_from_staging(
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
    let includes_actions_artifacts = manifest.format_version >= 3;
    let includes_registry = manifest.format_version >= 4;
    if includes_registry {
        ensure!(
            root.join("registry").is_dir(),
            "backup registry data is missing"
        );
    }
    if includes_actions_artifacts {
        ensure!(
            root.join("actions-artifacts").is_dir(),
            "backup Actions artifacts are missing"
        );
        ensure_restore_target(&settings.storage.actions_artifact_root, true)?;
    }

    let suffix = Uuid::new_v4().simple().to_string();
    let repository_temp = sibling_temp(&settings.storage.repository_root, &suffix);
    let lfs_temp = sibling_temp(&settings.storage.lfs_root, &suffix);
    let registry_temp = sibling_temp(&settings.storage.registry_root, &suffix);
    let actions_artifact_temp = sibling_temp(&settings.storage.actions_artifact_root, &suffix);
    let database_temp = sibling_temp(database_path, &suffix);
    let host_key_temp = sibling_temp(&settings.ssh.host_key, &suffix);
    let settings_temp = sibling_temp(config_path, &suffix);
    let mut temporary_paths = vec![
        repository_temp.clone(),
        lfs_temp.clone(),
        database_temp.clone(),
    ];
    if includes_actions_artifacts {
        temporary_paths.push(actions_artifact_temp.clone());
    }
    if includes_registry {
        temporary_paths.push(registry_temp.clone());
    }
    if manifest.host_key {
        temporary_paths.push(host_key_temp.clone());
    }
    if settings_source.is_file() {
        temporary_paths.push(settings_temp.clone());
    }

    let prepared = (|| -> Result<()> {
        copy_tree(&root.join("repositories"), &repository_temp)?;
        copy_tree(&root.join("lfs"), &lfs_temp)?;
        if includes_registry {
            copy_tree(&root.join("registry"), &registry_temp)?;
        }
        if includes_actions_artifacts {
            copy_tree(&root.join("actions-artifacts"), &actions_artifact_temp)?;
        }
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
    if includes_registry {
        remove_empty_target(&settings.storage.registry_root)?;
        rename_prepared(&registry_temp, &settings.storage.registry_root)?;
    }
    if includes_actions_artifacts {
        remove_empty_target(&settings.storage.actions_artifact_root)?;
        rename_prepared(
            &actions_artifact_temp,
            &settings.storage.actions_artifact_root,
        )?;
    }
    rename_prepared(&database_temp, database_path)?;
    if manifest.host_key {
        rename_prepared(&host_key_temp, &settings.ssh.host_key)?;
    }
    if settings_source.is_file() {
        rename_prepared(&settings_temp, config_path)?;
    }
    Ok(())
}

pub(super) fn replace_archive(input: &Path, settings: &Settings) -> Result<()> {
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
        settings.storage.registry_root.clone(),
        sqlite_sidecar(&database_path, "-wal"),
        sqlite_sidecar(&database_path, "-shm"),
    ];
    if inspection.format_version >= 3 {
        targets.push(settings.storage.actions_artifact_root.clone());
    }
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

pub(super) fn rollback_moved_paths(moved: &[(PathBuf, PathBuf)]) -> Result<()> {
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

pub(super) fn pre_restore_path(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_owned();
    value.push(format!(".pre-restore-{suffix}"));
    PathBuf::from(value)
}

pub(super) fn sqlite_sidecar(database_path: &Path, suffix: &str) -> PathBuf {
    let mut value = database_path.as_os_str().to_owned();
    value.push(suffix);
    PathBuf::from(value)
}

pub(super) fn remove_path(path: &Path) {
    let _ = if path.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    };
}

pub(super) fn extract_archive(input: &Path, staging: &Path) -> Result<()> {
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

pub(super) fn verify_manifest(root: &Path, manifest: &Manifest) -> Result<()> {
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

pub(super) fn manifest_files(root: &Path) -> Result<Vec<ManifestFile>> {
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

pub(super) fn data_file_paths(root: &Path) -> Result<BTreeSet<String>> {
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

pub(super) fn copy_tree(source: &Path, destination: &Path) -> Result<()> {
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

pub(super) fn copy_file(source: &Path, destination: &Path) -> Result<()> {
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

pub(super) fn sha256(path: &Path) -> Result<String> {
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
    Ok(hex::encode(digest.finalize()))
}

pub(super) fn sqlite_path(url: &str) -> Result<PathBuf> {
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

pub(super) fn lock_path(database_path: &Path) -> PathBuf {
    let mut value = database_path.as_os_str().to_owned();
    value.push(".lock");
    PathBuf::from(value)
}

pub(super) fn sibling_temp(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_owned();
    value.push(format!(".restore-{suffix}"));
    PathBuf::from(value)
}

pub(super) fn safe_archive_path(path: &Path) -> bool {
    let mut components = path.components();
    matches!(components.next(), Some(Component::Normal(root)) if root == ARCHIVE_ROOT)
        && components.all(|component| matches!(component, Component::Normal(_)))
}

pub(super) fn path_string(path: &Path) -> Result<String> {
    let value = path.to_str().context("backup path is not valid UTF-8")?;
    Ok(value.replace(std::path::MAIN_SEPARATOR, "/"))
}

pub(super) fn ensure_restore_target(path: &Path, allow_empty_directory: bool) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    if allow_empty_directory && path.is_dir() && fs::read_dir(path)?.next().is_none() {
        return Ok(());
    }
    bail!("restore target is not empty: {}", path.display())
}

pub(super) fn remove_empty_target(path: &Path) -> Result<()> {
    if path.is_dir() {
        fs::remove_dir(path).with_context(|| format!("could not remove {}", path.display()))?;
    }
    Ok(())
}

pub(super) fn rename_prepared(source: &Path, destination: &Path) -> Result<()> {
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

pub(super) fn remove_paths(paths: &[PathBuf]) {
    for path in paths {
        let _ = if path.is_dir() {
            fs::remove_dir_all(path)
        } else {
            fs::remove_file(path)
        };
    }
}
