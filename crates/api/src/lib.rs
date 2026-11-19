#![cfg_attr(feature = "api-contract-export", allow(dead_code, unused_imports))]
//! HTTP/protocol adapters and the registered REST contract.

mod actions;
mod api;
mod archive;
mod backup_provider;
mod identity;
mod integrations;
mod openapi;
mod registry;
mod repository;
mod server;

use gitadel_config as config;
use gitadel_core::{filesystem, network, schedule};
#[cfg(test)]
use gitadel_db as database;
use gitadel_db::entity;
#[cfg(test)]
use gitadel_db::migration;
use gitadel_mail as mail;
use gitadel_secrets as secrets;
use gitadel_storage::{blob_store, lfs as storage};

pub use actions::{ActionsState, bootstrap_system_runner};
pub use archive::{acquire_storage_lock, run as run_backup};
pub use identity::bootstrap_admin;
pub use identity::{ApiError, IdentityState};
pub use repository::{
    RepositoryState, load_ssh_host_key, purge_soft_deleted_repositories, serve_ssh,
    tick_repository_mirrors,
};
pub use server::{http_router, maintenance_router};

/// Cargo can unify a dependency's features independently of the binary's.
/// An exporter-only API build must never be used to start the service.
pub const CONTRACT_EXPORT_ONLY: bool = cfg!(feature = "api-contract-export");

/// Export the same registered contract served by `/api/v1/openapi.json`.
pub fn export_openapi() -> Result<String, serde_json::Error> {
    openapi::json()
}

/// Run the isolated image decoder before the server's Tokio runtime starts.
pub fn image_worker(path: &str, thumbnail: bool) -> anyhow::Result<()> {
    repository::image::worker(path, thumbnail)
}
