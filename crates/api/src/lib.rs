#![cfg_attr(feature = "api-contract-export", allow(dead_code, unused_imports))]
//! Gitadel's HTTP application, registered REST contract and service composition.

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

pub use archive::{acquire_storage_lock, run as run_backup};
pub use identity::bootstrap_admin;
pub use server::{ServerExit, perform_maintenance, serve};

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

/// Initialize the logging configuration used by the server and offline commands.
pub fn init_tracing() {
    use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "gitadel=info,tower_http=info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();
}
