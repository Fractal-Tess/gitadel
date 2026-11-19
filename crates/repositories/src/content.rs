//! Narrow read handle for consumers of committed repository content.
pub use gitadel_git::content::RepositoryContent;
use gitadel_identity::IdentityError;
use sley::Repository as GitRepository;
use std::path::PathBuf;
/// Run a finite read against one exact bare repository on a blocking thread.
pub async fn read_git<T, F>(path: PathBuf, operation: F) -> Result<T, IdentityError>
where
    T: Send + 'static,
    F: FnOnce(&GitRepository) -> sley::Result<T> + Send + 'static,
{
    gitadel_git::content::read_git(path, operation)
        .await
        .map_err(|error| match error {
            gitadel_git::content::ReadError::NotFound => IdentityError::not_found(),
            gitadel_git::content::ReadError::Internal(message) => IdentityError::internal(message),
        })
}
