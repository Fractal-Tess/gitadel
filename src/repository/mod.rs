mod browser;
mod git_http;
mod gitea;
mod lfs;
mod resources;
mod ssh;
mod topics;
mod webhooks;

pub(crate) use browser::render_markdown;

use std::{
    collections::HashMap,
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
    sync::{RwLock, Semaphore},
};
use url::Url;
use uuid::Uuid;

use crate::{
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
    stats_cache: Arc<RwLock<HashMap<String, Vec<browser::LanguageStatResponse>>>>,
    commit_count_cache: Arc<RwLock<HashMap<String, usize>>>,
    commit_count_slots: Arc<Semaphore>,
    size_cache: Arc<RwLock<HashMap<Uuid, CachedRepositorySize>>>,
    size_generations: Arc<RwLock<HashMap<Uuid, u64>>>,
    size_measurement_slots: Arc<Semaphore>,
    lfs_root: Arc<PathBuf>,
    public_url: Arc<Url>,
    ssh_port: u16,
    lfs_tokens: Arc<RwLock<HashMap<String, LfsAuthorization>>>,
    webhook_client: reqwest::Client,
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

const COMMIT_COUNT_CACHE_CAPACITY: usize = 4_096;
const COMMIT_COUNT_CONCURRENCY: usize = 2;
const REPOSITORY_SIZE_CACHE_LIFETIME: Duration = Duration::from_secs(5 * 60);
const REPOSITORY_SIZE_MEASUREMENT_CONCURRENCY: usize = 2;

impl RepositoryState {
    pub async fn new(
        identity: IdentityState,
        settings: StorageSettings,
        public_url: Url,
        ssh_port: u16,
    ) -> Result<Self, anyhow::Error> {
        fs::create_dir_all(&settings.repository_root).await?;
        fs::create_dir_all(&settings.lfs_root).await?;
        Ok(Self {
            identity,
            repository_root: Arc::new(settings.repository_root),
            stats_cache: Arc::new(RwLock::new(HashMap::new())),
            commit_count_cache: Arc::new(RwLock::new(HashMap::new())),
            commit_count_slots: Arc::new(Semaphore::new(COMMIT_COUNT_CONCURRENCY)),
            size_cache: Arc::new(RwLock::new(HashMap::new())),
            size_generations: Arc::new(RwLock::new(HashMap::new())),
            size_measurement_slots: Arc::new(Semaphore::new(
                REPOSITORY_SIZE_MEASUREMENT_CONCURRENCY,
            )),
            lfs_root: Arc::new(settings.lfs_root),
            public_url: Arc::new(public_url),
            ssh_port,
            lfs_tokens: Arc::new(RwLock::new(HashMap::new())),
            webhook_client: webhooks::webhook_client()?,
        })
    }

    pub fn identity(&self) -> &IdentityState {
        &self.identity
    }

    pub fn repository_path(&self, repository: &repository::Model) -> PathBuf {
        self.repository_root
            .join(format!("{}.git", repository.storage_key))
    }

    pub(super) fn lfs_repository_path(&self, repository: &repository::Model) -> PathBuf {
        self.lfs_root.join(repository.storage_key.to_string())
    }

    pub(super) fn lfs_object_path(&self, repository: &repository::Model, oid: &str) -> PathBuf {
        self.lfs_repository_path(repository)
            .join(&oid[..2])
            .join(&oid[2..4])
            .join(oid)
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

    pub(super) fn webhook_client(&self) -> &reqwest::Client {
        &self.webhook_client
    }

    pub(super) fn http_clone_url(&self, repository: &repository::Model) -> String {
        let mut endpoint = self.public_url.as_ref().clone();
        endpoint.set_path(&format!(
            "/{}/{}.git",
            repository.namespace, repository.name
        ));
        endpoint.set_query(None);
        endpoint.set_fragment(None);
        endpoint.to_string()
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

    async fn cached_stats(&self, key: &str) -> Option<Vec<browser::LanguageStatResponse>> {
        self.stats_cache.read().await.get(key).cloned()
    }

    async fn cache_stats(&self, key: String, stats: Vec<browser::LanguageStatResponse>) {
        self.stats_cache.write().await.insert(key, stats);
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

    async fn repository_size(&self, repository: &repository::Model) -> Result<u64, ApiError> {
        if let Some(cached) = self.fresh_cached_repository_size(repository.id).await {
            return Ok(cached);
        }

        let _permit = self
            .size_measurement_slots
            .acquire()
            .await
            .map_err(ApiError::internal)?;
        if let Some(cached) = self.fresh_cached_repository_size(repository.id).await {
            return Ok(cached);
        }

        let generation = self
            .size_generations
            .read()
            .await
            .get(&repository.id)
            .copied()
            .unwrap_or_default();
        let repository_path = self.repository_path(repository);
        let lfs_path = self.lfs_repository_path(repository);
        let bytes = tokio::task::spawn_blocking(move || {
            let git_bytes = directory_size(&repository_path)?;
            let lfs_bytes = directory_size(&lfs_path)?;
            Ok::<_, anyhow::Error>(git_bytes.saturating_add(lfs_bytes))
        })
        .await
        .map_err(ApiError::internal)?
        .map_err(ApiError::internal)?;
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
        Ok(bytes)
    }

    async fn fresh_cached_repository_size(&self, repository_id: Uuid) -> Option<u64> {
        self.size_cache
            .read()
            .await
            .get(&repository_id)
            .filter(|cached| cached.measured_at.elapsed() < REPOSITORY_SIZE_CACHE_LIFETIME)
            .map(|cached| cached.bytes)
    }

    async fn invalidate_repository_size(&self, repository_id: Uuid) {
        self.size_cache.write().await.remove(&repository_id);
        let mut generations = self.size_generations.write().await;
        let generation = generations.entry(repository_id).or_default();
        *generation = generation.wrapping_add(1);
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
        .route("/user/repos", get(gitea::list_user_repositories))
        .route(
            "/repos/{namespace}/{name}/branches",
            get(gitea::list_branches),
        )
}

pub fn git_http_router() -> Router<RepositoryState> {
    git_http::router().merge(lfs::router())
}

pub async fn serve_ssh(
    settings: crate::config::SshSettings,
    state: RepositoryState,
) -> Result<(), anyhow::Error> {
    ssh::serve(settings, state).await
}
pub fn validate_repository_name(value: &str) -> Result<String, ApiError> {
    let name = value.trim();

    let valid_length = (1..=100).contains(&name.len());
    let valid_chars = name
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'));
    let reserved = matches!(name, "." | "..")
        || name.contains("..")
        || name.to_ascii_lowercase().ends_with(".git");
    if !valid_length || !valid_chars || reserved {
        return Err(ApiError::bad_request(
            "Repository names may contain letters, numbers, periods, underscores, and hyphens.",
        ));
    }
    Ok(name.to_owned())
}
