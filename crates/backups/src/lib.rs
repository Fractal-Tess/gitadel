//! Backup archives, providers and schedule persistence, independent of HTTP.
pub mod archive;
pub mod backup_provider;
pub mod service;
use gitadel_config as config;
use gitadel_core::{filesystem, schedule};
use gitadel_db as database;
use gitadel_db::entity;
use gitadel_storage::{blob_store, lfs as storage, registry};
#[cfg(test)]
mod test_support;
