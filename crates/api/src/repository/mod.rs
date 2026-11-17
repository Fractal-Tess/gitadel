use crate::openapi::ContractRouterExt;

use utoipa_axum::router::UtoipaMethodRouterExt;
#[cfg(test)]
mod access_tests;
pub(crate) mod analysis;
pub(crate) mod browser;
pub(crate) mod code;
#[cfg(test)]
mod code_tests;
pub(crate) mod deploy_detect;
pub(crate) mod deploy_keys;
pub(crate) mod files;
mod git_http;
#[cfg(test)]
mod git_http_tests;
mod git_service;
pub(crate) mod gitea;
mod github_mirror;
#[cfg(test)]
mod http_test_support;
pub(crate) mod icon;
pub(crate) mod image;
mod import_metadata;
mod imports;
pub(crate) mod integrations;
mod integrity;
pub(crate) mod issues;
mod lfs;
#[cfg(test)]
mod lfs_tests;
mod maintenance;
mod markdown;
mod mirror_scheduler;
mod mirrors;
mod native_remote;
pub(crate) mod notifications;
pub(crate) mod profile;
pub(crate) mod protection;
pub(crate) mod registry;
pub(crate) mod releases;
pub(crate) mod resources;
mod signature;
pub(crate) mod source_archive;
mod ssh;
pub(crate) mod statuses;
pub(crate) mod topics;
pub(crate) mod webhook_events;
pub(crate) mod webhooks;
pub(crate) mod websites;
pub(crate) use browser::{read_git, render_markdown};

pub(crate) use git_http::GitHttpState;
pub(crate) use integrity::serve_integrity_scheduler;
pub(crate) use mirror_scheduler::serve_mirror_scheduler;
pub(crate) use resources::purge_soft_deleted_repositories;
pub(crate) use statuses::statuses_for_commits;

fn service_error(error: gitadel_repositories::Error) -> ApiError {
    match error {
        gitadel_repositories::Error::InvalidInput(message) => ApiError::bad_request(message),
        gitadel_repositories::Error::Conflict(message) => ApiError::conflict(message),
        gitadel_repositories::Error::Internal(message) => ApiError::internal(message),
    }
}

pub(crate) async fn recover_imports(state: &RepositoryState) -> Result<(), ApiError> {
    imports::recover_interrupted(state).await
}

use std::{
    collections::{HashMap, HashSet},
    future::Future,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use axum::Router;
use axum_extra::extract::cookie::CookieJar;
use sea_orm::{
    ActiveValue::Set, ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter, QueryOrder,
    sea_query::OnConflict,
};
use serde::{Serialize, de::DeserializeOwned};
use tokio::sync::{Mutex, OwnedSemaphorePermit, RwLock, Semaphore};
use tokio_util::task::TaskTracker;
use url::Url;
use uuid::Uuid;

use crate::{
    blob_store::{BlobStore, ObjectKey, ObjectPrefix, lfs_object_key},
    config::StorageSettings,
    entity::{
        namespace, organization_member, repository, repository_alias, repository_cache_entry,
        repository_collaborator, user,
    },
    filesystem::create_private_directory_async,
    identity::{ApiError, AuthenticatedUser, IdentityState},
};

#[derive(Clone)]
pub struct RepositoryState {
    identity: IdentityState,
    repository_root: Arc<PathBuf>,
    archive_cache_root: Arc<PathBuf>,
    archive_slots: Arc<Semaphore>,
    actions_artifact_root: Arc<PathBuf>,
    analysis_slots: Arc<Semaphore>,
    analysis_refreshing: Arc<Mutex<HashMap<Uuid, bool>>>,
    commit_count_refreshing: Arc<Mutex<HashSet<String>>>,
    commit_count_slots: Arc<Semaphore>,
    size_generations: Arc<RwLock<HashMap<Uuid, u64>>>,
    size_refreshing: Arc<Mutex<HashSet<Uuid>>>,
    size_measurement_slots: Arc<Semaphore>,
    mirror_slots: Arc<Semaphore>,
    mirror_syncing: mirrors::MirrorBusySet,
    github_known_hosts: Arc<Mutex<Option<(String, Instant)>>>,
    lfs_root: Arc<PathBuf>,
    registry_root: Arc<PathBuf>,
    lfs_storage: Arc<crate::blob_store::DomainStorage>,
    registry_storage: Arc<crate::blob_store::DomainStorage>,
    public_url: Arc<Url>,
    ssh_port: u16,
    lfs_tokens: Arc<RwLock<HashMap<String, LfsAuthorization>>>,
    webhook_client: reqwest::Client,
    tasks: TaskTracker,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Permission {
    Read,
    Write,
    Manage,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LfsPermission {
    Read,
    Write,
}

#[derive(Clone, Copy)]
struct LfsAuthorization {
    repository_id: Uuid,
    user_id: Uuid,
    permission: LfsPermission,
    expires_at: Instant,
}

#[derive(Clone, Copy, Serialize, serde::Deserialize)]
struct CachedRepositorySize {
    bytes: u64,
    lfs_bytes: u64,
}

const ARCHIVE_GENERATION_CONCURRENCY: usize = 2;
const ANALYSIS_CONCURRENCY: usize = 4;
const COMMIT_COUNT_CONCURRENCY: usize = 2;
const ANALYSIS_CACHE_LIFETIME: Duration = Duration::from_secs(30 * 24 * 60 * 60);
const OVERVIEW_CACHE_LIFETIME: Duration = ANALYSIS_CACHE_LIFETIME;
const REPOSITORY_SIZE_CACHE_LIFETIME: Duration = Duration::from_secs(5 * 60);
const REPOSITORY_SIZE_MEASUREMENT_CONCURRENCY: usize = 2;
const CACHE_KIND_OVERVIEW: &str = "overview";
const CACHE_KIND_STATS: &str = "language_stats";
const CACHE_KIND_COMMIT_COUNT: &str = "commit_count";
const CACHE_KIND_SIZE: &str = "size";
/// The commits authored on one day, keyed by branch digest and date.
const CACHE_KIND_DAY_COMMITS: &str = "day_commits";
/// What one commit changed against its first parent, keyed by commit id.
const CACHE_KIND_COMMIT_STATS: &str = "commit_stats";
const MIRROR_SYNC_CONCURRENCY: usize = 2;

impl RepositoryState {
    pub async fn new(
        identity: IdentityState,
        settings: StorageSettings,
        public_url: Url,
        ssh_port: u16,
    ) -> Result<Self, anyhow::Error> {
        create_private_directory_async(&settings.repository_root).await?;
        let archive_cache_root = settings.repository_root.join(".cache/source-archives");
        create_private_directory_async(&archive_cache_root).await?;
        mirrors::cleanup_temporary_files(&settings.repository_root).await?;
        let lfs_storage = match identity.lfs_storage().await {
            Some(manager) => manager,
            None => crate::storage::start(identity.database(), &settings).await?,
        };
        let registry_storage = match identity.registry_storage().await {
            Some(manager) => manager,
            None => crate::registry::storage::start(identity.database(), &settings).await?,
        };
        mirrors::cleanup_lfs_staging(&settings.lfs_root).await?;
        create_private_directory_async(&settings.registry_root).await?;
        create_private_directory_async(&settings.actions_artifact_root).await?;
        let state = Self {
            identity,
            repository_root: Arc::new(settings.repository_root),
            archive_cache_root: Arc::new(archive_cache_root),
            archive_slots: Arc::new(Semaphore::new(ARCHIVE_GENERATION_CONCURRENCY)),
            actions_artifact_root: Arc::new(settings.actions_artifact_root),
            analysis_slots: Arc::new(Semaphore::new(ANALYSIS_CONCURRENCY)),
            analysis_refreshing: Arc::new(Mutex::new(HashMap::new())),
            commit_count_refreshing: Arc::new(Mutex::new(HashSet::new())),
            commit_count_slots: Arc::new(Semaphore::new(COMMIT_COUNT_CONCURRENCY)),
            size_generations: Arc::new(RwLock::new(HashMap::new())),
            size_refreshing: Arc::new(Mutex::new(HashSet::new())),
            size_measurement_slots: Arc::new(Semaphore::new(
                REPOSITORY_SIZE_MEASUREMENT_CONCURRENCY,
            )),
            mirror_slots: Arc::new(Semaphore::new(MIRROR_SYNC_CONCURRENCY)),
            mirror_syncing: mirrors::MirrorBusySet::default(),
            github_known_hosts: Arc::new(Mutex::new(None)),
            registry_storage,
            lfs_root: Arc::new(settings.lfs_root),
            registry_root: Arc::new(settings.registry_root),
            lfs_storage,
            public_url: Arc::new(public_url),
            ssh_port,
            lfs_tokens: Arc::new(RwLock::new(HashMap::new())),
            webhook_client: webhooks::webhook_client()?,
            tasks: TaskTracker::new(),
        };
        resources::backfill_legacy_empty_defaults(&state).await?;
        let background = state.clone();
        state.spawn_task(async move {
            let result = async {
                let mut pages = repository::Entity::find()
                    .filter(repository::Column::DeletedAt.is_null())
                    .order_by_asc(repository::Column::Id)
                    .paginate(background.identity.database(), 50);
                while let Some(repositories) = pages.fetch_and_next().await? {
                    for repository in repositories {
                        if let Err(error) = browser::warm_repository_analysis(&background, &repository).await {
                            tracing::warn!(%error, repository_id = %repository.id, "could not backfill repository analysis");
                        }
                    }
                }
                Ok::<_, ApiError>(())
            }.await;
            if let Err(error) = result {
                tracing::warn!(%error, "repository analysis backfill failed");
            }
        });
        Ok(state)
    }

    pub(crate) fn spawn_task<F>(&self, task: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        self.tasks.spawn(task);
    }

    pub(crate) fn close_tasks(&self) {
        self.tasks.close();
    }

    pub(crate) async fn wait_for_tasks(&self) {
        self.tasks.wait().await;
    }

    pub fn identity(&self) -> &IdentityState {
        &self.identity
    }
    pub(super) fn http_client(&self) -> &reqwest::Client {
        &self.webhook_client
    }

    pub fn repository_path(&self, repository: &repository::Model) -> PathBuf {
        self.repository_root
            .join(format!("{}.git", repository.storage_key))
    }

    pub(crate) fn registry_root(&self) -> &Path {
        self.registry_root.as_ref()
    }

    /// Where the repository's registry payloads and upload sessions live on
    /// local disk.
    pub(crate) fn registry_paths(&self, repository: &repository::Model) -> [PathBuf; 2] {
        let storage_key = repository.storage_key.to_string();
        [
            self.registry_root.join(&storage_key),
            crate::registry::store::uploads_root(&self.registry_root).join(storage_key),
        ]
    }

    pub(super) fn source_archive_cache_path(
        &self,
        repository: &repository::Model,
        commit: &str,
        extension: &str,
    ) -> PathBuf {
        self.archive_cache_root
            .join(repository.storage_key.to_string())
            .join(format!("{commit}.{extension}"))
    }

    pub(super) fn source_archive_cache_directory(&self, repository: &repository::Model) -> PathBuf {
        self.archive_cache_root
            .join(repository.storage_key.to_string())
    }

    pub(super) fn try_archive_generation(&self) -> Result<OwnedSemaphorePermit, ApiError> {
        Arc::clone(&self.archive_slots)
            .try_acquire_owned()
            .map_err(|_| {
                ApiError::too_many_requests(
                    "Source archive capacity is busy. Try the request again later.",
                )
            })
    }

    pub(crate) fn actions_artifact_root(&self) -> &Path {
        self.actions_artifact_root.as_ref()
    }

    pub(super) fn lfs_storage(&self) -> &Arc<crate::blob_store::DomainStorage> {
        &self.lfs_storage
    }

    pub(super) fn lfs_repository_path(&self, repository: &repository::Model) -> PathBuf {
        self.lfs_root_path(repository.storage_key)
    }

    pub(super) fn lfs_root_path(&self, storage_key: Uuid) -> PathBuf {
        self.lfs_root.join(storage_key.to_string())
    }

    pub(super) fn lfs_object_key(
        &self,
        repository: &repository::Model,
        oid: &str,
    ) -> Result<ObjectKey, ApiError> {
        lfs_object_key(repository.storage_key, oid).map_err(ApiError::internal)
    }

    pub(super) fn lfs_store(&self) -> Arc<dyn BlobStore> {
        self.lfs_storage.store()
    }

    pub(super) fn lfs_target_id(&self) -> Option<Uuid> {
        self.lfs_storage.target_id()
    }

    pub(super) async fn lfs_operation_guard(&self) -> tokio::sync::RwLockReadGuard<'_, ()> {
        self.lfs_storage.lock_operation().await
    }

    pub(crate) fn registry_storage(&self) -> &Arc<crate::blob_store::DomainStorage> {
        &self.registry_storage
    }
    pub(super) fn lfs_endpoint(&self, repository: &repository::Model) -> String {
        let mut endpoint = self.public_url.as_ref().clone();
        endpoint.set_path(&format!(
            "/{}/{}.git/info/lfs",
            repository.namespace, repository.name
        ));
        endpoint.set_query(None);
        endpoint.set_fragment(None);
        endpoint.to_string().trim_end_matches('/').to_owned()
    }

    pub(crate) fn http_clone_url(&self, repository: &repository::Model) -> String {
        let mut endpoint = self.public_url.as_ref().clone();
        endpoint.set_path(&format!(
            "/{}/{}.git",
            repository.namespace, repository.name
        ));
        endpoint.set_query(None);
        endpoint.set_fragment(None);
        endpoint.to_string()
    }

    pub(crate) fn public_url(&self) -> &Url {
        &self.public_url
    }

    pub(super) fn ssh_clone_url(&self, repository: &repository::Model) -> String {
        let mut endpoint = Url::parse("ssh://git@localhost").expect("static SSH URL is valid");
        endpoint
            .set_host(self.public_url.host_str())
            .expect("configured public URL has a valid host");
        endpoint
            .set_port(Some(self.ssh_port))
            .expect("SSH URLs support explicit ports");
        endpoint.set_path(&format!(
            "/{}/{}.git",
            repository.namespace, repository.name
        ));
        endpoint.to_string()
    }

    pub(super) async fn issue_lfs_token(
        &self,
        repository_id: Uuid,
        user_id: Uuid,
        permission: LfsPermission,
    ) -> String {
        let token = format!("{}.{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
        let key = lfs_token_key(&token);
        let now = Instant::now();
        let mut tokens = self.lfs_tokens.write().await;
        tokens.retain(|_, authorization| authorization.expires_at > now);
        tokens.insert(
            key,
            LfsAuthorization {
                repository_id,
                user_id,
                permission,
                expires_at: now + Duration::from_secs(15 * 60),
            },
        );
        token
    }

    pub(super) async fn authenticate_lfs_token(
        &self,
        token: &str,
        repository_id: Uuid,
        permission: LfsPermission,
    ) -> Option<Uuid> {
        let key = lfs_token_key(token);
        let now = Instant::now();
        let authorization = self.lfs_tokens.read().await.get(&key).copied()?;
        if authorization.expires_at <= now
            || authorization.repository_id != repository_id
            || (permission == LfsPermission::Write
                && authorization.permission != LfsPermission::Write)
        {
            return None;
        }
        // The token outlives the request that issued it, so an account
        // disabled since then must not keep using it.
        match self.identity.user_is_enabled(authorization.user_id).await {
            Ok(true) => Some(authorization.user_id),
            Ok(false) => None,
            Err(error) => {
                tracing::error!(%error, "could not verify the account behind an LFS token");
                None
            }
        }
    }

    async fn cached_value<T>(
        &self,
        repository_id: Uuid,
        kind: &str,
        cache_key: &str,
    ) -> Result<Option<T>, ApiError>
    where
        T: DeserializeOwned,
    {
        let id = (repository_id, kind.to_owned(), cache_key.to_owned());
        let Some(entry) = repository_cache_entry::Entity::find_by_id(id.clone())
            .one(self.identity.read_database())
            .await?
        else {
            return Ok(None);
        };
        if entry.expires_at <= chrono::Utc::now() {
            repository_cache_entry::Entity::delete_by_id(id)
                .exec(self.identity.database())
                .await?;
            return Ok(None);
        }
        match serde_json::from_str(&entry.payload_json) {
            Ok(value) => Ok(Some(value)),
            Err(error) => {
                tracing::warn!(
                    %error,
                    %repository_id,
                    cache_kind = kind,
                    "discarding invalid repository cache entry"
                );
                repository_cache_entry::Entity::delete_by_id(id)
                    .exec(self.identity.database())
                    .await?;
                Ok(None)
            }
        }
    }

    async fn cache_value<T>(
        &self,
        repository_id: Uuid,
        kind: &str,
        cache_key: &str,
        value: &T,
        lifetime: Duration,
    ) -> Result<(), ApiError>
    where
        T: Serialize + ?Sized,
    {
        let now = chrono::Utc::now();
        let expires_at = now + chrono::Duration::from_std(lifetime).map_err(ApiError::internal)?;
        let entry = repository_cache_entry::ActiveModel {
            repository_id: Set(repository_id),
            kind: Set(kind.to_owned()),
            cache_key: Set(cache_key.to_owned()),
            payload_json: Set(serde_json::to_string(value).map_err(ApiError::internal)?),
            expires_at: Set(expires_at),
            created_at: Set(now),
        };
        repository_cache_entry::Entity::insert(entry)
            .on_conflict(
                OnConflict::columns([
                    repository_cache_entry::Column::RepositoryId,
                    repository_cache_entry::Column::Kind,
                    repository_cache_entry::Column::CacheKey,
                ])
                .update_columns([
                    repository_cache_entry::Column::PayloadJson,
                    repository_cache_entry::Column::ExpiresAt,
                    repository_cache_entry::Column::CreatedAt,
                ])
                .to_owned(),
            )
            .exec(self.identity.database())
            .await?;
        repository_cache_entry::Entity::delete_many()
            .filter(repository_cache_entry::Column::ExpiresAt.lte(now))
            .exec(self.identity.database())
            .await?;
        Ok(())
    }

    async fn cached_overview(
        &self,
        repository_id: Uuid,
        key: &str,
    ) -> Result<Option<browser::GitOverview>, ApiError> {
        self.cached_value(repository_id, CACHE_KIND_OVERVIEW, key)
            .await
    }

    async fn cache_overview(
        &self,
        repository_id: Uuid,
        key: &str,
        overview: &browser::GitOverview,
    ) -> Result<(), ApiError> {
        self.cache_value(
            repository_id,
            CACHE_KIND_OVERVIEW,
            key,
            overview,
            OVERVIEW_CACHE_LIFETIME,
        )
        .await
    }

    async fn cached_stats(
        &self,
        repository_id: Uuid,
        key: &str,
    ) -> Result<Option<Vec<browser::LanguageStatResponse>>, ApiError> {
        self.cached_value(repository_id, CACHE_KIND_STATS, key)
            .await
    }

    async fn cache_stats(
        &self,
        repository_id: Uuid,
        key: &str,
        stats: &[browser::LanguageStatResponse],
    ) -> Result<(), ApiError> {
        self.cache_value(
            repository_id,
            CACHE_KIND_STATS,
            key,
            stats,
            ANALYSIS_CACHE_LIFETIME,
        )
        .await
    }

    async fn cached_commit_count(
        &self,
        repository_id: Uuid,
        key: &str,
    ) -> Result<Option<usize>, ApiError> {
        self.cached_value(repository_id, CACHE_KIND_COMMIT_COUNT, key)
            .await
    }

    async fn cache_commit_count(
        &self,
        repository_id: Uuid,
        key: &str,
        count: usize,
    ) -> Result<(), ApiError> {
        self.cache_value(
            repository_id,
            CACHE_KIND_COMMIT_COUNT,
            key,
            &count,
            ANALYSIS_CACHE_LIFETIME,
        )
        .await
    }

    pub(super) async fn queue_repository_analysis(&self, repository_id: Uuid) {
        let mut refreshing = self.analysis_refreshing.lock().await;
        if let Some(dirty) = refreshing.get_mut(&repository_id) {
            *dirty = true;
            return;
        }
        refreshing.insert(repository_id, false);
        drop(refreshing);

        let task_state = self.clone();
        let state = self.clone();
        task_state.spawn_task(async move {
            loop {
                let result = async {
                    let repository = repository::Entity::find_by_id(repository_id)
                        .one(state.identity.database())
                        .await?
                        .filter(|repository| repository.deleted_at.is_none());
                    if let Some(repository) = repository {
                        browser::warm_repository_analysis(&state, &repository).await?;
                    }
                    Ok::<_, ApiError>(())
                }
                .await;
                if let Err(error) = result {
                    tracing::warn!(
                        %error,
                        %repository_id,
                        "could not precompute repository analysis"
                    );
                }

                let mut refreshing = state.analysis_refreshing.lock().await;
                if refreshing.get(&repository_id) == Some(&true) {
                    refreshing.insert(repository_id, false);
                    drop(refreshing);
                    continue;
                }
                refreshing.remove(&repository_id);
                break;
            }
        });
    }

    async fn repository_size(
        &self,
        repository: &repository::Model,
    ) -> Result<Option<CachedRepositorySize>, ApiError> {
        let cached = self
            .cached_value::<CachedRepositorySize>(repository.id, CACHE_KIND_SIZE, "current")
            .await?;
        if cached.is_none() {
            self.queue_repository_size_refresh(repository.clone()).await;
        }
        Ok(cached)
    }

    async fn queue_repository_size_refresh(&self, repository: repository::Model) {
        let mut refreshing = self.size_refreshing.lock().await;
        if !refreshing.insert(repository.id) {
            return;
        }
        drop(refreshing);

        let task_state = self.clone();
        let state = self.clone();
        task_state.spawn_task(async move {
            if let Err(error) = state.measure_repository_size(&repository).await {
                tracing::warn!(
                    %error,
                    repository_id = %repository.id,
                    "could not refresh repository size"
                );
            }
            state.size_refreshing.lock().await.remove(&repository.id);
        });
    }

    async fn measure_repository_size(
        &self,
        repository: &repository::Model,
    ) -> Result<(), ApiError> {
        let _permit = self
            .size_measurement_slots
            .acquire()
            .await
            .map_err(ApiError::internal)?;
        let generation = self
            .size_generations
            .read()
            .await
            .get(&repository.id)
            .copied()
            .unwrap_or_default();
        let repository_path = self.repository_path(repository);
        let git_bytes = tokio::task::spawn_blocking(move || directory_size(&repository_path))
            .await
            .map_err(ApiError::internal)?
            .map_err(ApiError::internal)?;
        let lfs_prefix =
            ObjectPrefix::new(repository.storage_key.to_string()).map_err(ApiError::internal)?;
        let lfs_bytes = self
            .lfs_store()
            .list(&lfs_prefix)
            .await
            .map_err(ApiError::internal)?
            .into_iter()
            .fold(0_u64, |total, object| total.saturating_add(object.size));
        let bytes = git_bytes.saturating_add(lfs_bytes);
        let current_generation = self
            .size_generations
            .read()
            .await
            .get(&repository.id)
            .copied()
            .unwrap_or_default();
        if current_generation == generation {
            self.cache_value(
                repository.id,
                CACHE_KIND_SIZE,
                "current",
                &CachedRepositorySize { bytes, lfs_bytes },
                REPOSITORY_SIZE_CACHE_LIFETIME,
            )
            .await?;
            let generation_after_insert = self
                .size_generations
                .read()
                .await
                .get(&repository.id)
                .copied()
                .unwrap_or_default();
            if generation_after_insert != generation {
                repository_cache_entry::Entity::delete_by_id((
                    repository.id,
                    CACHE_KIND_SIZE.to_owned(),
                    "current".to_owned(),
                ))
                .exec(self.identity.database())
                .await?;
            }
        }
        Ok(())
    }

    async fn invalidate_repository_size(&self, repository_id: Uuid) {
        let mut generations = self.size_generations.write().await;
        let generation = generations.entry(repository_id).or_default();
        *generation = generation.wrapping_add(1);
        drop(generations);
        if let Err(error) = repository_cache_entry::Entity::delete_by_id((
            repository_id,
            CACHE_KIND_SIZE.to_owned(),
            "current".to_owned(),
        ))
        .exec(self.identity.database())
        .await
        {
            tracing::warn!(%error, %repository_id, "could not invalidate repository size cache");
        }
    }

    pub async fn find(&self, namespace: &str, name: &str) -> Result<repository::Model, ApiError> {
        let repository = self.find_including_deleted(namespace, name).await?;
        if repository.deleted_at.is_some() {
            return Err(ApiError::not_found());
        }
        Ok(repository)
    }

    pub async fn find_including_deleted(
        &self,
        namespace: &str,
        name: &str,
    ) -> Result<repository::Model, ApiError> {
        if let Some(repository) = repository::Entity::find()
            .filter(repository::Column::Namespace.eq(namespace))
            .filter(repository::Column::Name.eq(name))
            .one(self.identity.database())
            .await?
        {
            return Ok(repository);
        }
        let alias = repository_alias::Entity::find_by_id((namespace.to_owned(), name.to_owned()))
            .one(self.identity.database())
            .await?
            .ok_or_else(ApiError::not_found)?;
        repository::Entity::find_by_id(alias.repository_id)
            .one(self.identity.database())
            .await?
            .ok_or_else(ApiError::not_found)
    }

    pub async fn authorize(
        &self,
        repository: &repository::Model,
        user_id: Option<Uuid>,
        permission: Permission,
    ) -> Result<(), ApiError> {
        // Access comes first: the read-only answers below would otherwise
        // tell a caller without access that a private repository exists.
        if !self.can_access(repository, user_id, permission).await? {
            return Err(ApiError::not_found());
        }
        if permission == Permission::Write && repository.archived_at.is_some() {
            return Err(ApiError::forbidden("Archived repositories are read-only."));
        }
        if permission == Permission::Write && repository.mirrored {
            return Err(ApiError::forbidden("Mirrored repositories are read-only."));
        }
        Ok(())
    }

    pub async fn can_access(
        &self,
        repository: &repository::Model,
        user_id: Option<Uuid>,
        permission: Permission,
    ) -> Result<bool, ApiError> {
        if repository.deleted_at.is_some() {
            return Ok(false);
        }
        if permission == Permission::Read && repository.visibility == "public" {
            return Ok(true);
        }
        let Some(user_id) = user_id else {
            return Ok(false);
        };
        let namespace = namespace::Entity::find_by_id(&repository.namespace)
            .one(self.identity.database())
            .await?
            .ok_or_else(ApiError::not_found)?;

        match namespace.kind.as_str() {
            "user" if namespace.user_id == Some(user_id) => Ok(true),
            "user" if permission == Permission::Manage => Ok(false),
            "user" => Ok(
                repository_collaborator::Entity::find_by_id((repository.id, user_id))
                    .one(self.identity.database())
                    .await?
                    .is_some_and(|collaborator| {
                        permission == Permission::Read || collaborator.role == "write"
                    }),
            ),
            "organization" => {
                let Some(organization_id) = namespace.organization_id else {
                    return Ok(false);
                };
                Ok(
                    organization_member::Entity::find_by_id((organization_id, user_id))
                        .one(self.identity.database())
                        .await?
                        .is_some_and(|membership| {
                            permission != Permission::Manage || membership.role == "owner"
                        }),
                )
            }
            _ => Ok(false),
        }
    }

    pub async fn authenticated_repository(
        &self,
        headers: &axum::http::HeaderMap,
        jar: &CookieJar,
        namespace: &str,
        name: &str,
        permission: Permission,
        scope: i32,
    ) -> Result<(AuthenticatedUser, repository::Model), ApiError> {
        let actor = self.identity.authenticate(headers, jar, scope).await?;
        let repository = self.find(namespace, name).await?;
        self.authorize(&repository, Some(actor.user.id), permission)
            .await?;
        Ok((actor, repository))
    }

    pub async fn personal_owner(
        &self,
        repository: &repository::Model,
        actor: &user::Model,
    ) -> Result<(), ApiError> {
        let namespace = namespace::Entity::find_by_id(&repository.namespace)
            .one(self.identity.database())
            .await?
            .ok_or_else(ApiError::not_found)?;
        if namespace.kind != "user" || namespace.user_id != Some(actor.id) {
            return Err(ApiError::not_found());
        }
        Ok(())
    }
}

fn directory_size(path: &Path) -> Result<u64, anyhow::Error> {
    if !path.try_exists()? {
        return Ok(0);
    }

    let mut bytes = 0u64;
    for entry in walkdir::WalkDir::new(path).follow_links(false) {
        let entry = entry?;
        if entry.file_type().is_file() {
            bytes = bytes.saturating_add(entry.metadata()?.len());
        }
    }
    Ok(bytes)
}

fn lfs_token_key(token: &str) -> String {
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use sha2::{Digest as _, Sha256};

    URL_SAFE_NO_PAD.encode(Sha256::digest(token.as_bytes()))
}

pub fn router() -> Router<RepositoryState> {
    contract_router().into()
}

pub fn contract_router() -> utoipa_axum::router::OpenApiRouter<RepositoryState> {
    utoipa_axum::router::OpenApiRouter::new()
        .api_routes(crate::openapi::routes!(
            crate::repository::resources::list_repositories,
            crate::repository::resources::create_repository
        ))
        .api_routes(crate::openapi::routes!(
            crate::repository::analysis::overview
        ))
        .api_routes(crate::openapi::routes!(
            crate::repository::imports::discover
        ))
        .api_routes(crate::openapi::routes!(
            crate::repository::imports::create_import
        ))
        .api_routes(crate::openapi::routes!(
            crate::repository::imports::create_direct_import
        ))
        .api_routes(crate::openapi::routes!(
            crate::repository::imports::get_import,
            crate::repository::imports::cancel_import
        ))
        .api_routes(crate::openapi::routes!(
            crate::repository::imports::retry_import
        ))
        .api_routes(crate::openapi::routes!(
            crate::repository::resources::get_repository
        ))
        .api_routes(crate::openapi::routes!(
            crate::repository::resources::update_repository_control
        ))
        .api_routes(crate::openapi::routes!(
            crate::repository::resources::archive_repository,
            crate::repository::resources::unarchive_repository
        ))
        .api_routes(crate::openapi::routes!(
            crate::repository::resources::delete_repository
        ))
        .api_routes(crate::openapi::routes!(
            crate::repository::resources::unfavorite_repository,
            crate::repository::resources::favorite_repository
        ))
        .api_routes(crate::openapi::routes!(
            crate::repository::topics::list_topics,
            crate::repository::topics::replace_topics
        ))
        .api_routes(crate::openapi::routes!(
            crate::repository::topics::suggest_topics
        ))
        .api_routes(crate::openapi::routes!(
            crate::repository::icon::icon_candidates
        ))
        .api_routes(crate::openapi::routes!(
            crate::repository::icon::select_icon
        ))
        .api_routes(
            crate::openapi::routes!(
                crate::repository::icon::public_icon,
                crate::repository::icon::update_icon,
                crate::repository::icon::delete_icon
            )
            .layer(axum::extract::DefaultBodyLimit::max(
                icon::MAX_ICON_REQUEST_BYTES,
            )),
        )
        .api_routes(crate::openapi::routes!(crate::repository::registry::browse))
        .api_routes(crate::openapi::routes!(crate::repository::browser::refs))
        .api_routes(crate::openapi::routes!(
            crate::repository::mirrors::get_mirror,
            crate::repository::mirrors::update_mirror,
            crate::repository::mirrors::convert_mirror
        ))
        .api_routes(crate::openapi::routes!(
            crate::repository::mirrors::sync_mirror
        ))
        .api_routes(crate::openapi::routes!(
            crate::repository::releases::list_releases,
            crate::repository::releases::create_release
        ))
        .api_routes(crate::openapi::routes!(
            crate::repository::releases::get_release,
            crate::repository::releases::update_release,
            crate::repository::releases::delete_release
        ))
        .api_routes(
            crate::openapi::routes!(crate::repository::releases::upload_asset)
                .layer(axum::extract::DefaultBodyLimit::disable()),
        )
        .api_routes(crate::openapi::routes!(
            crate::repository::releases::download_asset,
            crate::repository::releases::delete_asset
        ))
        .api_routes(crate::openapi::routes!(
            crate::repository::releases::forgejo_get_tag
        ))
        .api_routes(crate::openapi::alias(
            "/repos/{namespace}/{name}/tags/{tag}",
            crate::openapi::routes!(crate::repository::releases::forgejo_get_tag),
        ))
        .api_routes(crate::openapi::routes!(
            crate::repository::releases::forgejo_create_release
        ))
        .api_routes(crate::openapi::routes!(
            crate::repository::releases::forgejo_get_release_by_tag
        ))
        .api_routes(crate::openapi::routes!(
            crate::repository::releases::forgejo_update_release
        ))
        .api_routes(
            crate::openapi::routes!(crate::repository::releases::forgejo_upload_asset)
                .layer(axum::extract::DefaultBodyLimit::disable()),
        )
        .api_routes(crate::openapi::routes!(
            crate::repository::issues::list_issues,
            crate::repository::issues::create_issue
        ))
        .api_routes(crate::openapi::routes!(
            crate::repository::issues::get_issue,
            crate::repository::issues::update_issue,
            crate::repository::issues::delete_issue
        ))
        .api_routes(crate::openapi::routes!(
            crate::repository::issues::list_comments,
            crate::repository::issues::create_comment
        ))
        .api_routes(crate::openapi::routes!(
            crate::repository::issues::update_comment,
            crate::repository::issues::delete_comment
        ))
        .api_routes(crate::openapi::routes!(
            crate::repository::issues::list_labels,
            crate::repository::issues::create_label
        ))
        .api_routes(crate::openapi::routes!(
            crate::repository::issues::update_label,
            crate::repository::issues::delete_label
        ))
        .api_routes(crate::openapi::routes!(
            crate::repository::issues::list_assignable_users
        ))
        .api_routes(crate::openapi::routes!(
            crate::repository::issues::preview_markdown
        ))
        .api_routes(
            crate::openapi::routes!(crate::repository::issues::upload_attachment)
                .layer(axum::extract::DefaultBodyLimit::disable()),
        )
        .api_routes(crate::openapi::routes!(
            crate::repository::issues::download_attachment,
            crate::repository::issues::delete_attachment
        ))
        .api_routes(crate::openapi::routes!(
            crate::repository::analysis::activity
        ))
        .api_routes(crate::openapi::routes!(
            crate::repository::profile::activity
        ))
        .api_routes(crate::openapi::routes!(
            crate::repository::profile::activity_day
        ))
        .api_routes(crate::openapi::routes!(
            crate::repository::profile::list_pins,
            crate::repository::profile::set_pins
        ))
        .api_routes(crate::openapi::routes!(crate::repository::browser::tree))
        .api_routes(crate::openapi::routes!(
            crate::repository::websites::list,
            crate::repository::websites::update
        ))
        .api_routes(crate::openapi::routes!(crate::repository::browser::blob))
        .api_routes(crate::openapi::routes!(crate::repository::browser::raw))
        .api_routes(crate::openapi::routes!(crate::repository::code::snippet))
        .api_routes(crate::openapi::routes!(crate::repository::code::symbols))
        .api_routes(crate::openapi::routes!(crate::repository::code::blame))
        .api_routes(crate::openapi::routes!(crate::repository::image::preview))
        .api_routes(
            crate::openapi::routes!(
                crate::repository::files::create_file,
                crate::repository::files::update_file
            )
            .layer(axum::extract::DefaultBodyLimit::max(
                files::MAX_FILE_REQUEST_BYTES,
            )),
        )
        .api_routes(crate::openapi::routes!(
            crate::repository::source_archive::source_archive
        ))
        .api_routes(crate::openapi::routes!(crate::repository::browser::history))
        .api_routes(crate::openapi::routes!(crate::repository::analysis::stats))
        .api_routes(crate::openapi::routes!(crate::repository::browser::commit))
        .api_routes(crate::openapi::routes!(crate::repository::browser::diff))
        .api_routes(crate::openapi::routes!(
            crate::repository::resources::list_collaborators,
            crate::repository::resources::add_collaborator
        ))
        .api_routes(crate::openapi::routes!(
            crate::repository::resources::remove_collaborator
        ))
        .api_routes(crate::openapi::routes!(
            crate::repository::deploy_keys::list_deploy_keys,
            crate::repository::deploy_keys::create_deploy_key
        ))
        .api_routes(crate::openapi::routes!(
            crate::repository::deploy_keys::delete_deploy_key
        ))
        .api_routes(crate::openapi::routes!(
            crate::repository::protection::list_rules,
            crate::repository::protection::create_rule
        ))
        .api_routes(crate::openapi::routes!(
            crate::repository::protection::update_rule,
            crate::repository::protection::delete_rule
        ))
        .api_routes(crate::openapi::routes!(
            crate::repository::integrations::list_integrations,
            crate::repository::integrations::create_integration
        ))
        .api_routes(crate::openapi::routes!(
            crate::repository::integrations::update_integration,
            crate::repository::integrations::get_integration,
            crate::repository::integrations::delete_integration
        ))
        .api_routes(crate::openapi::routes!(
            crate::repository::integrations::list_remote_resources,
            crate::repository::integrations::create_remote_resource
        ))
        .api_routes(crate::openapi::routes!(
            crate::repository::integrations::link_remote_resource
        ))
        .api_routes(crate::openapi::routes!(
            crate::repository::integrations::create_remote_project
        ))
        .api_routes(crate::openapi::routes!(
            crate::repository::integrations::create_remote_environment
        ))
        .api_routes(crate::openapi::routes!(
            crate::repository::integrations::deploy_integration
        ))
        .api_routes(crate::openapi::routes!(
            crate::repository::integrations::repository_deployment_suggestions
        ))
        .api_routes(crate::openapi::routes!(
            crate::repository::integrations::deployment_suggestions
        ))
        .api_routes(crate::openapi::routes!(
            crate::repository::integrations::setup_deployment
        ))
        .api_routes(crate::openapi::routes!(
            crate::repository::integrations::deployment_status
        ))
        .api_routes(crate::openapi::routes!(
            crate::repository::webhooks::list_webhooks,
            crate::repository::webhooks::create_webhook
        ))
        .api_routes(crate::openapi::routes!(
            crate::repository::webhooks::get_webhook,
            crate::repository::webhooks::update_webhook,
            crate::repository::webhooks::delete_webhook
        ))
        .api_routes(crate::openapi::routes!(
            crate::repository::webhooks::ping_webhook
        ))
        .api_routes(crate::openapi::routes!(
            crate::repository::webhooks::list_webhook_deliveries
        ))
        .api_routes(crate::openapi::routes!(
            crate::repository::webhooks::get_webhook_delivery
        ))
        .api_routes(crate::openapi::routes!(
            crate::repository::webhooks::redeliver_webhook_delivery
        ))
        .api_routes(crate::openapi::routes!(
            crate::repository::releases::forgejo_current_user
        ))
        .api_routes(crate::openapi::routes!(
            crate::repository::gitea::list_user_repositories
        ))
        .api_routes(crate::openapi::routes!(
            crate::repository::gitea::list_branches
        ))
        .api_routes(crate::openapi::routes!(
            crate::repository::statuses::list_statuses,
            crate::repository::statuses::create_status
        ))
        .api_routes(crate::openapi::alias(
            "/repos/{namespace}/{name}/commits/{revision}/statuses",
            crate::openapi::routes!(crate::repository::statuses::list_statuses),
        ))
        .api_routes(crate::openapi::routes!(
            crate::repository::statuses::combined_status
        ))
}

pub fn git_http_router() -> Router<git_http::GitHttpState> {
    git_http::router().merge(lfs::router())
}

/// Loads the SSH host key, creating it on first start.
pub fn load_ssh_host_key(path: &std::path::Path) -> Result<russh::keys::PrivateKey, anyhow::Error> {
    ssh::load_or_create_host_key(path)
}

pub async fn serve_ssh(
    settings: crate::config::SshSettings,
    state: RepositoryState,
    actions: crate::actions::ActionsState,
) -> Result<(), anyhow::Error> {
    ssh::serve(settings, state, actions).await
}
const RESERVED_REPOSITORY_NAMES: &[&str] = &[
    "integrations",
    "members",
    "mirror-credentials",
    "runners",
    "settings",
];

pub fn validate_repository_name(value: &str) -> Result<String, ApiError> {
    let name = value.trim();

    let valid_length = (1..=100).contains(&name.len());
    let valid_chars = name
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'));
    let lower_name = name.to_ascii_lowercase();
    let reserved =
        matches!(name, "." | "..") || name.contains("..") || lower_name.ends_with(".git");
    if RESERVED_REPOSITORY_NAMES.contains(&lower_name.as_str()) {
        return Err(ApiError::bad_request(format!(
            "The repository name '{name}' is reserved for namespace management.",
        )));
    }
    if !valid_length || !valid_chars || reserved {
        return Err(ApiError::bad_request(
            "Repository names may contain letters, numbers, periods, underscores, and hyphens.",
        ));
    }
    Ok(name.to_owned())
}

#[cfg(test)]
mod tests {
    use super::validate_repository_name;

    #[test]
    fn repository_names_reject_namespace_management_routes() {
        for name in [
            "members",
            "Runners",
            "integrations",
            "mirror-credentials",
            "settings",
        ] {
            assert!(validate_repository_name(name).is_err(), "{name}");
        }
    }

    #[test]
    fn repository_names_allow_non_route_names() {
        assert_eq!(
            validate_repository_name("member-service").unwrap(),
            "member-service",
        );
    }
}
