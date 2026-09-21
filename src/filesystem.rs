use std::{
    fs::{self, OpenOptions},
    io,
    os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _},
    path::Path,
};

const PRIVATE_DIRECTORY_MODE: u32 = 0o700;
const PRIVATE_FILE_MODE: u32 = 0o600;

pub(crate) fn create_private_directory(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(PRIVATE_DIRECTORY_MODE))
}

pub(crate) async fn create_private_directory_async(path: &Path) -> io::Result<()> {
    tokio::fs::create_dir_all(path).await?;
    tokio::fs::set_permissions(path, fs::Permissions::from_mode(PRIVATE_DIRECTORY_MODE)).await
}

pub(crate) fn open_private_file(path: &Path, create_new: bool) -> io::Result<fs::File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).mode(PRIVATE_FILE_MODE);
    if create_new {
        options.create_new(true);
    } else {
        options.create(true);
    }
    let file = options.open(path)?;
    file.set_permissions(fs::Permissions::from_mode(PRIVATE_FILE_MODE))?;
    Ok(file)
}

pub(crate) fn protect_file_sync(path: &Path) -> io::Result<()> {
    fs::set_permissions(path, fs::Permissions::from_mode(PRIVATE_FILE_MODE))
}

pub(crate) async fn protect_file(path: &Path) -> io::Result<()> {
    tokio::fs::set_permissions(path, fs::Permissions::from_mode(PRIVATE_FILE_MODE)).await
}
