//! Narrow read handle for consumers of committed repository content.
use gitadel_db::entity::repository;
use gitadel_identity::IdentityError;
use sley::{GitError, Repository as GitRepository};
use std::{path::PathBuf, sync::Arc};

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
    pub fn repository_path(&self, repository: &repository::Model) -> PathBuf {
        self.root.join(format!("{}.git", repository.storage_key))
    }
}
/// Run a finite read against one exact bare repository on a blocking thread.
pub async fn read_git<T, F>(path: PathBuf, operation: F) -> Result<T, IdentityError>
where
    T: Send + 'static,
    F: FnOnce(&GitRepository) -> sley::Result<T> + Send + 'static,
{
    tokio::task::spawn_blocking(move || {
        let repository = GitRepository::open_exact_bare(path)?;
        operation(&repository)
    })
    .await
    .map_err(IdentityError::internal)?
    .map_err(|error| match error {
        GitError::InvalidObjectId(_) | GitError::InvalidPath(_) | GitError::NotFound(_) => {
            IdentityError::not_found()
        }
        error => IdentityError::internal(error),
    })
}
