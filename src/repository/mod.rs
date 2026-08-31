mod browser;
mod git_http;
mod gitea;
mod github_mirror;
mod import_metadata;
mod imports;
mod integrations;
mod issues;
mod lfs;
mod mirror_scheduler;
mod mirrors;
mod releases;
mod resources;
mod ssh;
mod topics;
mod webhooks;
pub(crate) use browser::{read_git, render_markdown};

pub(crate) use git_http::GitHttpState;
pub(crate) use mirror_scheduler::serve_mirror_scheduler;
pub(crate) fn outbound_http_client() -> Result<reqwest::Client, reqwest::Error> {
    webhooks::webhook_client()
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

use axum::{
    Router,
    routing::{delete, get},
};
use axum_extra::extract::cookie::CookieJar;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use tokio::{
    fs,
    sync::{Mutex, RwLock, Semaphore},
};
use tokio_util::task::TaskTracker;
use url::Url;
use uuid::Uuid;

use crate::{
    blob_store::{BlobStore, ObjectKey, ObjectPrefix, lfs_object_key, targets},
    config::StorageSettings,
    entity::{
        namespace, organization_member, repository, repository_alias, repository_collaborator, user,
    },
    identity::{ApiError, AuthenticatedUser, IdentityState},
};

#[derive(Clone)]
pub struct RepositoryState {
    identity: IdentityState,
    repository_root: Arc<PathBuf>,
    actions_artifact_root: Arc<PathBuf>,
    analysis_slots: Arc<Semaphore>,
    overview_cache: Arc<RwLock<HashMap<String, browser::GitOverview>>>,
    stats_cache: Arc<RwLock<HashMap<String, Vec<browser::LanguageStatResponse>>>>,
    commit_count_cache: Arc<RwLock<HashMap<String, usize>>>,
    commit_count_refreshing: Arc<Mutex<HashSet<String>>>,
    commit_count_slots: Arc<Semaphore>,
    size_cache: Arc<RwLock<HashMap<Uuid, CachedRepositorySize>>>,
    size_generations: Arc<RwLock<HashMap<Uuid, u64>>>,
    size_refreshing: Arc<Mutex<HashSet<Uuid>>>,
    size_measurement_slots: Arc<Semaphore>,
    mirror_slots: Arc<Semaphore>,
    mirror_syncing: Arc<Mutex<HashSet<Uuid>>>,
    github_known_hosts: Arc<Mutex<Option<(String, Instant)>>>,
    lfs_root: Arc<PathBuf>,
    lfs_store: Arc<dyn BlobStore>,
    lfs_target_id: Option<Uuid>,
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

#[derive(Clone, Copy)]
struct CachedRepositorySize {
    bytes: u64,
    measured_at: Instant,
}

const ANALYSIS_CACHE_CAPACITY: usize = 4_096;
const ANALYSIS_CONCURRENCY: usize = 4;
const COMMIT_COUNT_CACHE_CAPACITY: usize = 4_096;
const COMMIT_COUNT_CONCURRENCY: usize = 2;
const REPOSITORY_SIZE_CACHE_LIFETIME: Duration = Duration::from_secs(5 * 60);
const REPOSITORY_SIZE_MEASUREMENT_CONCURRENCY: usize = 2;
const MIRROR_SYNC_CONCURRENCY: usize = 2;

impl RepositoryState {
    pub async fn new(
        identity: IdentityState,
        settings: StorageSettings,
        public_url: Url,
        ssh_port: u16,
    ) -> Result<Self, anyhow::Error> {
        fs::create_dir_all(&settings.repository_root).await?;
        mirrors::cleanup_temporary_files(&settings.repository_root).await?;
        let active_storage =
            targets::load_active(identity.database(), settings.lfs_root.clone()).await?;
        mirrors::cleanup_lfs_staging(&settings.lfs_root).await?;
        fs::create_dir_all(&settings.actions_artifact_root).await?;
        Ok(Self {
            identity,
            repository_root: Arc::new(settings.repository_root),
            actions_artifact_root: Arc::new(settings.actions_artifact_root),
            analysis_slots: Arc::new(Semaphore::new(ANALYSIS_CONCURRENCY)),
            overview_cache: Arc::new(RwLock::new(HashMap::new())),
            stats_cache: Arc::new(RwLock::new(HashMap::new())),
            commit_count_cache: Arc::new(RwLock::new(HashMap::new())),
            commit_count_refreshing: Arc::new(Mutex::new(HashSet::new())),
            commit_count_slots: Arc::new(Semaphore::new(COMMIT_COUNT_CONCURRENCY)),
            size_cache: Arc::new(RwLock::new(HashMap::new())),
            size_generations: Arc::new(RwLock::new(HashMap::new())),
            size_refreshing: Arc::new(Mutex::new(HashSet::new())),
            size_measurement_slots: Arc::new(Semaphore::new(
                REPOSITORY_SIZE_MEASUREMENT_CONCURRENCY,
            )),
            mirror_slots: Arc::new(Semaphore::new(MIRROR_SYNC_CONCURRENCY)),
            mirror_syncing: Arc::new(Mutex::new(HashSet::new())),
            github_known_hosts: Arc::new(Mutex::new(None)),
            lfs_root: Arc::new(settings.lfs_root),
            lfs_store: active_storage.store,
            lfs_target_id: active_storage.target_id,
            public_url: Arc::new(public_url),
            ssh_port,
            lfs_tokens: Arc::new(RwLock::new(HashMap::new())),
            webhook_client: webhooks::webhook_client()?,
            tasks: TaskTracker::new(),
        })
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

    pub(crate) fn actions_artifact_root(&self) -> &Path {
        self.actions_artifact_root.as_ref()
    }

    pub(super) fn local_lfs_root(&self) -> &Path {
        self.lfs_root.as_ref()
    }

    pub(super) fn lfs_repository_path(&self, repository: &repository::Model) -> PathBuf {
        self.lfs_root_path(repository.storage_key)
    }

    pub(super) fn lfs_root_path(&self, storage_key: Uuid) -> PathBuf {
        self.lfs_root.join(storage_key.to_string())
    }

    pub(super) fn lfs_import_staging_path(&self) -> PathBuf {
        self.lfs_root
            .join(format!(".gitadel-import-{}", Uuid::new_v4().simple()))
    }

    pub(super) fn lfs_object_key(
        &self,
        repository: &repository::Model,
        oid: &str,
    ) -> Result<ObjectKey, ApiError> {
        lfs_object_key(repository.storage_key, oid).map_err(ApiError::internal)
    }

    pub(super) fn lfs_store(&self) -> &dyn BlobStore {
        self.lfs_store.as_ref()
    }

    pub(super) fn lfs_target_id(&self) -> Option<Uuid> {
        self.lfs_target_id
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

    pub(crate) fn webhook_client(&self) -> &reqwest::Client {
        &self.webhook_client
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
        (authorization.expires_at > now
            && authorization.repository_id == repository_id
            && (permission == LfsPermission::Read
                || authorization.permission == LfsPermission::Write))
            .then_some(authorization.user_id)
    }

    async fn cached_overview(&self, key: &str) -> Option<browser::GitOverview> {
        self.overview_cache.read().await.get(key).cloned()
    }

    async fn cache_overview(&self, key: String, overview: browser::GitOverview) {
        let mut cache = self.overview_cache.write().await;
        if cache.len() >= ANALYSIS_CACHE_CAPACITY {
            cache.clear();
        }
        cache.insert(key, overview);
    }

    async fn cached_stats(&self, key: &str) -> Option<Vec<browser::LanguageStatResponse>> {
        self.stats_cache.read().await.get(key).cloned()
    }

    async fn cache_stats(&self, key: String, stats: Vec<browser::LanguageStatResponse>) {
        let mut cache = self.stats_cache.write().await;
        if cache.len() >= ANALYSIS_CACHE_CAPACITY {
            cache.clear();
        }
        cache.insert(key, stats);
    }

    async fn cached_commit_count(&self, key: &str) -> Option<usize> {
        self.commit_count_cache.read().await.get(key).copied()
    }

    async fn cache_commit_count(&self, key: String, count: usize) {
        let mut cache = self.commit_count_cache.write().await;
        if cache.len() >= COMMIT_COUNT_CACHE_CAPACITY {
            cache.clear();
        }
        cache.insert(key, count);
    }

    async fn repository_size(&self, repository: &repository::Model) -> Option<u64> {
        let cached = self.size_cache.read().await.get(&repository.id).copied();
        let should_refresh = cached
            .is_none_or(|entry| entry.measured_at.elapsed() >= REPOSITORY_SIZE_CACHE_LIFETIME);
        if should_refresh {
            self.queue_repository_size_refresh(repository.clone()).await;
        }
        cached.map(|entry| entry.bytes)
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
            self.size_cache.write().await.insert(
                repository.id,
                CachedRepositorySize {
                    bytes,
                    measured_at: Instant::now(),
                },
            );
            let generation_after_insert = self
                .size_generations
                .read()
                .await
                .get(&repository.id)
                .copied()
                .unwrap_or_default();
            if generation_after_insert != generation {
                self.size_cache.write().await.remove(&repository.id);
            }
        }
        Ok(())
    }

    async fn invalidate_repository_size(&self, repository_id: Uuid) {
        let mut generations = self.size_generations.write().await;
        let generation = generations.entry(repository_id).or_default();
        *generation = generation.wrapping_add(1);
        drop(generations);
        self.size_cache.write().await.remove(&repository_id);
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
        if permission == Permission::Write && repository.archived_at.is_some() {
            return Err(ApiError::forbidden("Archived repositories are read-only."));
        }
        if permission == Permission::Write && repository.mirrored {
            return Err(ApiError::forbidden("Mirrored repositories are read-only."));
        }
        if self.can_access(repository, user_id, permission).await? {
            Ok(())
        } else {
            Err(ApiError::not_found())
        }
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
    Router::new()
        .route(
            "/repositories",
            get(resources::list_repositories).post(resources::create_repository),
        )
        .route("/repositories/overview", get(browser::overview))
        .route(
            "/repository-imports/discover",
            axum::routing::post(imports::discover),
        )
        .route(
            "/repository-imports",
            axum::routing::post(imports::create_import),
        )
        .route(
            "/repository-imports/direct",
            axum::routing::post(imports::create_direct_import),
        )
        .route(
            "/repository-imports/{id}",
            get(imports::get_import).delete(imports::cancel_import),
        )
        .route(
            "/repository-imports/{id}/retry",
            axum::routing::post(imports::retry_import),
        )
        .route(
            "/repositories/{namespace}/{name}",
            get(resources::get_repository),
        )
        .route(
            "/repositories/{namespace}/{name}/control",
            axum::routing::patch(resources::update_repository_control),
        )
        .route(
            "/repositories/{namespace}/{name}/archive",
            axum::routing::post(resources::archive_repository)
                .delete(resources::unarchive_repository),
        )
        .route(
            "/repositories/{namespace}/{name}/delete",
            axum::routing::post(resources::soft_delete_repository),
        )
        .route(
            "/repositories/{namespace}/{name}/restore",
            axum::routing::post(resources::restore_repository),
        )
        .route(
            "/repositories/{namespace}/{name}/purge",
            delete(resources::purge_repository),
        )
        .route(
            "/repositories/{namespace}/{name}/favorite",
            delete(resources::unfavorite_repository).put(resources::favorite_repository),
        )
        .route(
            "/repositories/{namespace}/{name}/topics",
            get(topics::list_topics).put(topics::replace_topics),
        )
        .route("/topics", get(topics::suggest_topics))
        .route("/repositories/{namespace}/{name}/refs", get(browser::refs))
        .route(
            "/repositories/{namespace}/{name}/mirror",
            get(mirrors::get_mirror)
                .patch(mirrors::update_mirror)
                .delete(mirrors::convert_mirror),
        )
        .route(
            "/repositories/{namespace}/{name}/mirror/sync",
            axum::routing::post(mirrors::sync_mirror),
        )
        .route(
            "/repositories/{namespace}/{name}/releases",
            get(releases::list_releases).post(releases::create_release),
        )
        .route(
            "/repositories/{namespace}/{name}/releases/{id}",
            get(releases::get_release)
                .patch(releases::update_release)
                .delete(releases::delete_release),
        )
        .route(
            "/repositories/{namespace}/{name}/releases/{id}/assets",
            axum::routing::put(releases::upload_asset)
                .layer(axum::extract::DefaultBodyLimit::disable()),
        )
        .route(
            "/repositories/{namespace}/{name}/releases/{release_id}/assets/{asset_id}",
            get(releases::download_asset).delete(releases::delete_asset),
        )
        .route(
            "/repositories/{namespace}/{name}/issues",
            get(issues::list_issues).post(issues::create_issue),
        )
        .route(
            "/repositories/{namespace}/{name}/issues/{number}",
            get(issues::get_issue)
                .patch(issues::update_issue)
                .delete(issues::delete_issue),
        )
        .route(
            "/repositories/{namespace}/{name}/issues/{number}/comments",
            get(issues::list_comments).post(issues::create_comment),
        )
        .route(
            "/repositories/{namespace}/{name}/issues/{number}/comments/{id}",
            axum::routing::patch(issues::update_comment).delete(issues::delete_comment),
        )
        .route(
            "/repositories/{namespace}/{name}/issue-labels",
            get(issues::list_labels).post(issues::create_label),
        )
        .route(
            "/repositories/{namespace}/{name}/issue-labels/{id}",
            axum::routing::patch(issues::update_label).delete(issues::delete_label),
        )
        .route(
            "/repositories/{namespace}/{name}/assignable-users",
            get(issues::list_assignable_users),
        )
        .route(
            "/repositories/{namespace}/{name}/markdown-preview",
            axum::routing::post(issues::preview_markdown),
        )
        .route(
            "/repositories/{namespace}/{name}/issue-attachments",
            axum::routing::put(issues::upload_attachment)
                .layer(axum::extract::DefaultBodyLimit::disable()),
        )
        .route(
            "/repositories/{namespace}/{name}/issue-attachments/{attachment_id}",
            get(issues::download_attachment),
        )
        .route(
            "/repositories/{namespace}/{name}/activity",
            get(browser::activity),
        )
        .route("/repositories/{namespace}/{name}/tree", get(browser::tree))
        .route("/repositories/{namespace}/{name}/blob", get(browser::blob))
        .route("/repositories/{namespace}/{name}/raw", get(browser::raw))
        .route(
            "/repositories/{namespace}/{name}/history",
            get(browser::history),
        )
        .route(
            "/repositories/{namespace}/{name}/stats",
            get(browser::stats),
        )
        .route(
            "/repositories/{namespace}/{name}/commits/{revision}",
            get(browser::commit),
        )
        .route(
            "/repositories/{namespace}/{name}/commits/{revision}/diff",
            get(browser::diff),
        )
        .route(
            "/repositories/{namespace}/{name}/collaborators",
            get(resources::list_collaborators).post(resources::add_collaborator),
        )
        .route(
            "/repositories/{namespace}/{name}/collaborators/{username}",
            delete(resources::remove_collaborator),
        )
        .route(
            "/repos/{namespace}/{name}/integrations",
            get(integrations::list_integrations).post(integrations::create_integration),
        )
        .route(
            "/repos/{namespace}/{name}/integrations/{id}",
            axum::routing::put(integrations::update_integration)
                .get(integrations::get_integration)
                .delete(integrations::delete_integration),
        )
        .route(
            "/repos/{namespace}/{name}/integrations/{id}/resources",
            get(integrations::list_remote_resources).post(integrations::create_remote_resource),
        )
        .route(
            "/repos/{namespace}/{name}/integrations/{id}/resources/link",
            axum::routing::post(integrations::link_remote_resource),
        )
        .route(
            "/repos/{namespace}/{name}/integrations/{id}/projects",
            axum::routing::post(integrations::create_remote_project),
        )
        .route(
            "/repos/{namespace}/{name}/integrations/{id}/environments",
            axum::routing::post(integrations::create_remote_environment),
        )
        .route(
            "/repos/{namespace}/{name}/integrations/{id}/deploy",
            axum::routing::post(integrations::deploy_integration),
        )
        .route(
            "/repos/{namespace}/{name}/hooks",
            get(webhooks::list_webhooks).post(webhooks::create_webhook),
        )
        .route(
            "/repos/{namespace}/{name}/hooks/{id}",
            get(webhooks::get_webhook)
                .patch(webhooks::update_webhook)
                .delete(webhooks::delete_webhook),
        )
        .route(
            "/repos/{namespace}/{name}/hooks/{id}/pings",
            axum::routing::post(webhooks::ping_webhook),
        )
        .route(
            "/repos/{namespace}/{name}/hooks/{id}/deliveries",
            get(webhooks::list_webhook_deliveries),
        )
        .route(
            "/repos/{namespace}/{name}/hooks/{id}/deliveries/{delivery_id}",
            get(webhooks::get_webhook_delivery),
        )
        .route(
            "/repos/{namespace}/{name}/hooks/{id}/deliveries/{delivery_id}/attempts",
            axum::routing::post(webhooks::redeliver_webhook_delivery),
        )
        .route("/user/repos", get(gitea::list_user_repositories))
        .route(
            "/repos/{namespace}/{name}/branches",
            get(gitea::list_branches),
        )
}

pub fn git_http_router() -> Router<git_http::GitHttpState> {
    git_http::router().merge(lfs::router())
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
