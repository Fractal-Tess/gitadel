//! Container image catalog, manifests, tags, uploads and payload layout.

pub mod catalog;
mod registry;
pub use registry::{storage, store, usage, valid_image_path_components};

use gitadel_config as config;
use gitadel_core::filesystem;
#[cfg(test)]
use gitadel_db as database;
use gitadel_db::entity;
use gitadel_storage::blob_store;
