//! Database connections, entities, and schema migrations.

mod database;
pub mod entity;
pub mod migration;

pub use database::{connect_and_migrate, connect_reader};

#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
