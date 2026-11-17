//! Repository services shared by HTTP, SSH, imports and background mirrors.
//!
//! Callers own authentication and authorization; these services receive only
//! repository paths, transport policy decisions and storage handles.

pub mod analysis;
pub mod archive;
pub mod branches;
pub mod browser;
pub mod files;
pub mod git_service;
pub mod markdown;
pub mod native_remote;
pub mod signature;

/// Repository failures which the adapter maps to its own protocol status.
#[derive(Debug)]
pub enum Error {
    InvalidInput(String),
    Conflict(String),
    Internal(String),
}

impl Error {
    pub fn invalid_input(message: impl std::fmt::Display) -> Self {
        Self::InvalidInput(message.to_string())
    }
    pub fn conflict(message: impl std::fmt::Display) -> Self {
        Self::Conflict(message.to_string())
    }
    pub fn internal(message: impl std::fmt::Display) -> Self {
        Self::Internal(message.to_string())
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidInput(message) | Self::Conflict(message) | Self::Internal(message) => {
                formatter.write_str(message)
            }
        }
    }
}

impl std::error::Error for Error {}
