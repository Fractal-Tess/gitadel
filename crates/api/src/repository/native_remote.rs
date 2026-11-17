//! Adapter from repository state to the native remote service.
use super::RepositoryState;
use crate::identity::ApiError;
pub(crate) use gitadel_repositories::native_remote::{NativeAuthentication, NativeInitialization};
use std::path::Path;
use uuid::Uuid;

pub(super) async fn initialize(
    state: &RepositoryState,
    path: &Path,
    remote_url: &str,
    authentication: NativeAuthentication,
) -> Result<NativeInitialization, ApiError> {
    gitadel_repositories::native_remote::initialize(
        state.http_client(),
        path,
        remote_url,
        authentication,
    )
    .await
    .map_err(super::service_error)
}

pub(super) async fn synchronize(
    state: &RepositoryState,
    path: &Path,
    remote_url: &str,
    authentication: NativeAuthentication,
) -> Result<(), ApiError> {
    gitadel_repositories::native_remote::synchronize(
        state.http_client(),
        path,
        remote_url,
        authentication,
    )
    .await
    .map_err(super::service_error)
}

pub(super) async fn fetch_lfs(
    state: &RepositoryState,
    storage_key: Uuid,
    repository_path: &Path,
    remote_url: &str,
    authentication: NativeAuthentication,
) -> Result<(), ApiError> {
    gitadel_repositories::native_remote::fetch_lfs(
        state.http_client(),
        state.lfs_storage(),
        storage_key,
        repository_path,
        remote_url,
        authentication,
    )
    .await
    .map_err(super::service_error)
}
