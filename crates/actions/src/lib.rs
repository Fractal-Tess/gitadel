//! Actions workflow planning, runner operations, and run persistence.
//!
//! Transport parsing and event delivery belong to callers. Service operations
//! revalidate credentials and use the existing planning and persistence primitives;
//! filesystem maintenance receives its artifact root explicitly.

pub mod artifacts;
pub mod dispatch;
pub mod logs;
pub mod matrix;
pub mod rerun;
pub mod runners;
pub mod runs;
pub mod schedule;
pub mod secrets;
pub mod tokens;
pub mod workflow;

/// Runner protocol version supported by the scheduler.
pub const REQUIRED_RUNNER_VERSION: &str = "13.0.0";

#[cfg(test)]
mod test_support;

pub mod discovery;
mod service;
pub mod service_schedule;
pub use service::{ActionsService, RunEffects};
pub mod artifact_service;
pub mod runner_service;
pub mod service_dispatch;
pub mod tasks;
