//! Request metadata passed explicitly to domain operations.

use std::net::IpAddr;

/// Audit metadata shared by HTTP, SSH and noninteractive callers.
#[derive(Clone, Copy, Debug, Default)]
pub struct AuditContext {
    pub remote_address: Option<IpAddr>,
}
