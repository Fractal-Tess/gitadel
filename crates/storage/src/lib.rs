//! Shared blob stores, Git LFS storage, and container registry persistence.
//!
//! Storage paths, verified migrations, and registry metadata are owned here;
//! HTTP routing and authentication belong to the API crate.

pub mod blob_store;
pub mod lfs;

use gitadel_config as config;
use gitadel_core::filesystem;
use gitadel_db::{self as database, entity};

#[cfg(test)]
use gitadel_db::test_support;
pub mod domains;
pub mod overview;
pub mod service;
