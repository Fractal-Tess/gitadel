//! One answer to "how full are my disks, and how much of that is Gitadel":
//! every place Gitadel keeps data, what each holds, and the volume it sits on.

use std::{
    collections::BTreeMap,
    os::unix::fs::MetadataExt as _,
    path::{Path, PathBuf},
};

use axum::{Json, extract::State, http::HeaderMap};
use axum_extra::extract::cookie::CookieJar;
use chrono::{DateTime, Utc};
use sea_orm::{ConnectionTrait as _, DatabaseBackend, Statement};
use serde::Serialize;

use super::super::{ApiError, IdentityState, SCOPE_READ, require_admin};
use crate::blob_store::targets::{self, StorageTargetView};

#[derive(Debug, Serialize)]
pub struct StorageOverview {
    volumes: Vec<Volume>,
    categories: Vec<Category>,
    measured_at: DateTime<Utc>,
}

/// A mounted filesystem holding at least one category. `used_bytes` counts
/// everything on it, Gitadel included; space the filesystem keeps back for
/// root is neither used nor available, so it is reported on its own instead
/// of making the other figures fail to add up.
#[derive(Debug, Serialize)]
struct Volume {
    id: String,
    mount_point: Option<String>,
    total_bytes: u64,
    used_bytes: u64,
    available_bytes: u64,
    reserved_bytes: u64,
    gitadel_bytes: u64,
}

#[derive(Debug, Serialize)]
struct Category {
    key: &'static str,
    label: &'static str,
    location: String,
    bytes: u64,
    /// `None` for object storage and external databases, which sit on no
    /// volume this host can see.
    volume_id: Option<String>,
}

pub async fn storage_overview(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<StorageOverview>, ApiError> {
    require_admin(&state, &headers, &jar, SCOPE_READ).await?;
    let settings = state.runtime_settings()?;
    let storage = &settings.storage;
    let measured = state.measured_storage().await;
    let target_views = targets::list(state.database(), &storage.lfs_root, &measured)
        .await
        .map_err(ApiError::internal)?;

    let mut located = Vec::new();
    located.push((
        "repositories",
        "Git repositories",
        Location::Path(storage.repository_root.clone()),
        None,
    ));
    for view in &target_views {
        if view.active || view.usage.lfs_bytes > 0 {
            located.push((
                "lfs",
                "Git LFS",
                location_of(view),
                Some(view.usage.lfs_bytes),
            ));
        }
        if view.registry_active {
            located.push((
                "registry",
                "Container registry",
                location_of(view),
                Some(view.usage.registry_bytes),
            ));
        }
    }
    if !target_views.iter().any(|view| view.registry_active) {
        located.push((
            "registry",
            "Container registry",
            Location::Path(storage.registry_root.clone()),
            None,
        ));
    }
    located.push((
        "artifacts",
        "Workflow artifacts",
        Location::Path(storage.actions_artifact_root.clone()),
        None,
    ));

    let mut categories = Vec::with_capacity(located.len() + 1);
    if let Some(database) = database_category(&state, &settings.database.url).await? {
        categories.push(database);
    }
    let measured = tokio::task::spawn_blocking(move || {
        located
            .into_iter()
            .map(|(key, label, location, known)| {
                let bytes = match (&location, known) {
                    (_, Some(bytes)) => bytes,
                    (Location::Path(path), None) => directory_size(path),
                    (Location::Remote(_), None) => 0,
                };
                (key, label, location, bytes)
            })
            .collect::<Vec<_>>()
    })
    .await
    .map_err(ApiError::internal)?;
    for (key, label, location, bytes) in measured {
        categories.push(match location {
            Location::Path(path) => path_category(key, label, &path, bytes),
            Location::Remote(description) => Category {
                key,
                label,
                location: description,
                bytes,
                volume_id: None,
            },
        });
    }

    Ok(Json(StorageOverview {
        volumes: volumes(&categories),
        categories,
        measured_at: Utc::now(),
    }))
}

enum Location {
    Path(PathBuf),
    Remote(String),
}

fn location_of(view: &StorageTargetView) -> Location {
    let configuration = &view.configuration;
    if let Some(path) = configuration.get("path").and_then(|path| path.as_str()) {
        return Location::Path(PathBuf::from(path));
    }
    let s3 = &configuration["s3"];
    Location::Remote(format!(
        "{} · {}",
        s3["bucket"].as_str().unwrap_or_default(),
        s3["endpoint"].as_str().unwrap_or_default()
    ))
}

fn path_category(key: &'static str, label: &'static str, path: &Path, bytes: u64) -> Category {
    Category {
        key,
        label,
        location: absolute(path).display().to_string(),
        bytes,
        volume_id: device(path).map(|device| device.to_string()),
    }
}

/// SQLite is a file on one of the volumes; PostgreSQL reports its own size
/// but lives wherever its server does.
async fn database_category(
    state: &IdentityState,
    url: &str,
) -> Result<Option<Category>, ApiError> {
    let database = state.database();
    match database.get_database_backend() {
        DatabaseBackend::Sqlite => {
            let Some(path) = sqlite_path(url) else {
                return Ok(None);
            };
            let bytes = ["", "-wal", "-shm"]
                .iter()
                .filter_map(|suffix| {
                    let mut file = path.clone().into_os_string();
                    file.push(suffix);
                    std::fs::metadata(file).ok()
                })
                .map(|metadata| metadata.len())
                .sum();
            Ok(Some(path_category("database", "Database", &path, bytes)))
        }
        DatabaseBackend::Postgres => {
            let row = database
                .query_one_raw(Statement::from_string(
                    DatabaseBackend::Postgres,
                    "SELECT pg_database_size(current_database()) AS bytes",
                ))
                .await?;
            let bytes = row
                .map(|row| row.try_get::<i64>("", "bytes"))
                .transpose()?
                .unwrap_or(0);
            Ok(Some(Category {
                key: "database",
                label: "Database",
                location: "PostgreSQL server".to_owned(),
                bytes: u64::try_from(bytes).unwrap_or(0),
                volume_id: None,
            }))
        }
        _ => Ok(None),
    }
}

/// The file behind a `sqlite://` URL; in-memory databases have none.
fn sqlite_path(url: &str) -> Option<PathBuf> {
    let rest = url
        .strip_prefix("sqlite://")
        .or_else(|| url.strip_prefix("sqlite:"))?;
    let path = rest.split('?').next()?;
    (!path.is_empty() && path != ":memory:").then(|| PathBuf::from(path))
}

/// Groups the categories by the volume they sit on, in the order each volume
/// first appears.
fn volumes(categories: &[Category]) -> Vec<Volume> {
    let mut gitadel = BTreeMap::<&str, u64>::new();
    let mut order = Vec::new();
    for category in categories {
        let Some(id) = category.volume_id.as_deref() else {
            continue;
        };
        if !gitadel.contains_key(id) {
            order.push(category);
        }
        *gitadel.entry(id).or_default() += category.bytes;
    }
    order
        .into_iter()
        .filter_map(|category| {
            let id = category.volume_id.as_deref()?;
            let path = Path::new(&category.location);
            let probe = existing_ancestor(path)?;
            let total_bytes = fs2::total_space(&probe).ok()?;
            let free_bytes = fs2::free_space(&probe).ok()?;
            let available_bytes = fs2::available_space(&probe).ok()?.min(free_bytes);
            Some(Volume {
                id: id.to_owned(),
                mount_point: mount_point(&probe),
                total_bytes,
                used_bytes: total_bytes.saturating_sub(free_bytes),
                available_bytes,
                reserved_bytes: free_bytes - available_bytes,
                gitadel_bytes: gitadel[id],
            })
        })
        .collect()
}

fn absolute(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

/// A directory Gitadel has not created yet still lands on the volume of its
/// nearest existing parent.
fn existing_ancestor(path: &Path) -> Option<PathBuf> {
    absolute(path)
        .ancestors()
        .find(|candidate| candidate.exists())
        .map(Path::to_path_buf)
}

fn device(path: &Path) -> Option<u64> {
    std::fs::metadata(existing_ancestor(path)?)
        .ok()
        .map(|metadata| metadata.dev())
}

/// The longest mount point containing `path`, so the page can name the disk
/// rather than one of the directories on it.
fn mount_point(path: &Path) -> Option<String> {
    let path = path.canonicalize().ok()?;
    let mounts = std::fs::read_to_string("/proc/self/mounts").ok()?;
    mounts
        .lines()
        .filter_map(|line| line.split_whitespace().nth(1))
        .map(|mount| mount.replace("\\040", " "))
        .filter(|mount| path.starts_with(mount))
        .max_by_key(String::len)
}

/// Bytes in every file below `path`; a missing directory holds nothing.
fn directory_size(path: &Path) -> u64 {
    walkdir::WalkDir::new(path)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .filter_map(|entry| entry.metadata().ok())
        .map(|metadata| metadata.len())
        .sum()
}

#[cfg(test)]
mod tests {
    use super::sqlite_path;

    #[test]
    fn sqlite_path_reads_the_file_from_the_url() {
        assert_eq!(
            sqlite_path("sqlite://data/gitadel.db?mode=rwc").as_deref(),
            Some(std::path::Path::new("data/gitadel.db"))
        );
        assert_eq!(
            sqlite_path("sqlite:/var/lib/gitadel/gitadel.db").as_deref(),
            Some(std::path::Path::new("/var/lib/gitadel/gitadel.db"))
        );
        assert_eq!(sqlite_path("sqlite::memory:"), None);
        assert_eq!(sqlite_path("postgres://localhost/gitadel"), None);
    }
}
