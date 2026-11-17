//! Actions workflow planning, runner operations, and run persistence.
//!
//! Transport authentication and event delivery belong to callers. Operations accept
//! explicit database handles; filesystem maintenance also accepts its artifact root.

pub mod artifacts;
pub mod dispatch;
pub mod matrix;
pub mod rerun;
pub mod runners;
pub mod runs;
pub mod schedule;
pub mod tokens;
pub mod workflow;

/// Runner protocol version supported by the scheduler.
pub const REQUIRED_RUNNER_VERSION: &str = "13.0.0";

#[cfg(test)]
mod test_support;
