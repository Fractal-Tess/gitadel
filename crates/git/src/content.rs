//! Bounded-to-one-repository Git reads on a blocking thread.

use std::{path::PathBuf, sync::Arc};

use sley::{GitError, Repository as GitRepository};
use uuid::Uuid;

#[derive(Clone)]
pub struct RepositoryContent {
    root: Arc<PathBuf>,
}

impl RepositoryContent {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root: Arc::new(root),
        }
    }

    pub fn repository_path(&self, storage_key: Uuid) -> PathBuf {
        self.root.join(format!("{storage_key}.git"))
    }
}

#[derive(Debug)]
pub enum ReadError {
    NotFound,
    Internal(String),
}

/// Read one exact bare repository without blocking the async executor.
pub async fn read_git<T, F>(path: PathBuf, operation: F) -> Result<T, ReadError>
where
    T: Send + 'static,
    F: FnOnce(&GitRepository) -> sley::Result<T> + Send + 'static,
{
    tokio::task::spawn_blocking(move || {
        let repository = GitRepository::open_exact_bare(path)?;
        operation(&repository)
    })
    .await
    .map_err(|error| ReadError::Internal(error.to_string()))?
    .map_err(|error| match error {
        GitError::InvalidObjectId(_) | GitError::InvalidPath(_) | GitError::NotFound(_) => {
            ReadError::NotFound
        }
        error => ReadError::Internal(error.to_string()),
    })
}
