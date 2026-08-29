use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use axum::{
    Json,
    extract::{Path as AxumPath, State},
    http::{HeaderMap, StatusCode},
};
use axum_extra::extract::cookie::CookieJar;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use chrono::{DateTime, Utc};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, IntoActiveModel, QueryFilter, Set,
    TransactionTrait, sea_query::Query,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use tokio::{
    fs,
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    net::{TcpListener, TcpStream, lookup_host},
    process::Command,
    task::JoinHandle,
};
use url::{Host, Url};
use uuid::Uuid;

use super::{Permission, RepositoryState, github_mirror};
use crate::{
    entity::{
        issue_comment, namespace_mirror_identity, repository, repository_issue, repository_mirror,
        repository_topic,
    },
    identity::{ApiError, SCOPE_WRITE, load_mirror_identity_secret, mark_repository_identity_used},
    network::is_public_ip,
    schedule,
};

const MAX_REMOTE_URL_LENGTH: usize = 2_048;
const MAX_SCHEDULE_LENGTH: usize = 255;
const MAX_ERROR_LENGTH: usize = 2_048;
const GIT_COMMAND_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const TEMPORARY_FILE_MAX_AGE: Duration = Duration::from_secs(12 * 60);
const LFS_IMPORT_STAGING_PREFIX: &str = ".gitadel-import-";

#[derive(Deserialize)]
pub(super) struct CreateMirrorRequest {
    remote_url: String,
    identity_id: Option<Uuid>,
    schedule: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct UpdateMirrorRequest {
    identity_id: Option<Uuid>,
    schedule: Option<String>,
}

#[derive(Serialize)]
pub struct MirrorResponse {
    remote_url: String,
    identity_id: Option<Uuid>,
    credential_configured: bool,
    schedule: Option<String>,
    last_attempted_at: Option<DateTime<Utc>>,
    last_synced_at: Option<DateTime<Utc>>,
    last_error: Option<String>,
    metadata_last_synced_at: Option<DateTime<Utc>>,
    metadata_error: Option<String>,
    next_sync_at: Option<DateTime<Utc>>,
    syncing: bool,
}

pub(super) struct PreparedMirror {
    remote_url: String,
    namespace: String,
    identity_id: Option<Uuid>,
    schedule: Option<String>,
    next_sync_at: Option<DateTime<Utc>>,
    github: Option<github_mirror::GithubRepository>,
    authentication: GitAuthentication,
}

pub(super) struct MirrorInitialization {
    pub(super) object_format: String,
    pub(super) default_branch: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RemoteTransport {
    Https,
    Ssh,
}

struct GitAuthentication {
    basic: Option<(String, String)>,
    ssh_private_key: Option<String>,
    api_token: Option<String>,
}

#[derive(Clone)]
pub(super) struct ImportAuthentication {
    pub(super) username: Option<String>,
    pub(super) secret: Option<String>,
    pub(super) ssh_private_key: Option<String>,
}

impl PreparedMirror {
    pub(super) async fn prepare(
        state: &RepositoryState,
        namespace: &str,
        request: CreateMirrorRequest,
    ) -> Result<Self, ApiError> {
        let (remote_url, transport) = validate_remote_url(&request.remote_url)?;
        if transport != RemoteTransport::Https {
            return Err(ApiError::bad_request(
                "Mirrors must use an HTTPS clone URL.",
            ));
        }
        let github = github_mirror::identify(&remote_url);
        let authentication = authentication(
            state,
            namespace,
            &remote_url,
            request.identity_id,
            github.is_some(),
        )
        .await?;
        let (schedule, next_sync_at) = normalize_schedule(request.schedule, Utc::now())?;
        Ok(Self {
            remote_url,
            namespace: namespace.to_owned(),
            identity_id: request.identity_id,
            schedule,
            next_sync_at,
            github,
            authentication,
        })
    }
}

impl MirrorResponse {
    async fn new(mirror: repository_mirror::Model, state: &RepositoryState) -> Self {
        let syncing = state
            .mirror_syncing
            .lock()
            .await
            .contains(&mirror.repository_id);
        Self {
            remote_url: mirror.remote_url,
            identity_id: mirror.identity_id,
            credential_configured: mirror.identity_id.is_some(),
            schedule: mirror.schedule,
            last_attempted_at: mirror.last_attempted_at,
            last_synced_at: mirror.last_synced_at,
            last_error: mirror.last_error,
            metadata_last_synced_at: mirror.metadata_last_synced_at,
            metadata_error: mirror.metadata_error,
            next_sync_at: mirror.next_sync_at,
            syncing,
        }
    }
}

pub(super) async fn initialize(
    state: &RepositoryState,
    path: &Path,
    mirror: &PreparedMirror,
) -> Result<MirrorInitialization, ApiError> {
    let _permit = state
        .mirror_slots
        .acquire()
        .await
        .map_err(|_| ApiError::internal("mirror synchronization is unavailable"))?;
    let mut command = Command::new("git");
    command
        .args(["clone", "--mirror", "--no-local"])
        .arg(&mirror.remote_url)
        .arg(path)
        .stdin(Stdio::null());
    let _temporary_files = apply_git_environment(
        state,
        &mut command,
        path,
        &mirror.remote_url,
        &mirror.authentication,
        true,
        None,
        None,
    )
    .await?;
    if let Some(id) = mirror.identity_id {
        mark_repository_identity_used(state.identity(), &mirror.namespace, id, Utc::now()).await?;
    }
    run_git(&mut command).await.map_err(|error| {
        ApiError::bad_request(format!("Could not clone the source repository: {error}"))
    })?;

    let object_format = git_output(path, &["rev-parse", "--show-object-format"])
        .await
        .map_err(ApiError::internal)?;
    if object_format != "sha1" && object_format != "sha256" {
        return Err(ApiError::internal(format!(
            "Git returned unsupported object format {object_format}"
        )));
    }
    let default_branch = match git_output(path, &["symbolic-ref", "--short", "HEAD"]).await {
        Ok(branch) if !branch.is_empty() => branch,
        _ => {
            set_symbolic_head(path, "main").await?;
            "main".to_owned()
        }
    };
    Ok(MirrorInitialization {
        object_format,
        default_branch,
    })
}

pub(super) async fn initialize_import(
    state: &RepositoryState,
    path: &Path,
    storage_key: Uuid,
    source_instance_url: Option<&str>,
    remote_url: &str,
    authentication: &ImportAuthentication,
) -> Result<MirrorInitialization, ApiError> {
    let (remote_url, transport) = validate_direct_import_remote(
        source_instance_url,
        remote_url,
        authentication.ssh_private_key.is_some(),
    )?;
    let _permit = state
        .mirror_slots
        .acquire()
        .await
        .map_err(|_| ApiError::internal("repository importing is unavailable"))?;
    let authentication = GitAuthentication {
        basic: match (&authentication.username, &authentication.secret) {
            (Some(username), Some(secret)) => Some((username.clone(), secret.clone())),
            _ => None,
        },
        ssh_private_key: authentication.ssh_private_key.clone(),
        api_token: None,
    };
    match transport {
        RemoteTransport::Https if authentication.basic.is_none() => {
            return Err(ApiError::bad_request(
                "HTTPS imports require a token or username and password identity.",
            ));
        }
        RemoteTransport::Ssh if authentication.ssh_private_key.is_none() => {
            return Err(ApiError::bad_request(
                "SSH imports require an SSH identity.",
            ));
        }
        _ => {}
    }
    let mut command = Command::new("git");
    command
        .args(["clone", "--mirror", "--no-local"])
        .arg(&remote_url)
        .arg(path)
        .stdin(Stdio::null());
    let _temporary_files = apply_git_environment(
        state,
        &mut command,
        path,
        &remote_url,
        &authentication,
        false,
        None,
        None,
    )
    .await?;
    run_git(&mut command).await.map_err(|error| {
        ApiError::bad_request(format!("Could not import the source repository: {error}"))
    })?;

    let staging = state.lfs_import_staging_path();
    let lfs_path = state.lfs_root_path(storage_key);
    let lfs_result = fetch_and_promote_lfs(
        state,
        path,
        &lfs_path,
        &staging,
        &remote_url,
        &authentication,
    )
    .await;
    if let Err(error) = lfs_result {
        cleanup_lfs_path(&lfs_path).await;
        return Err(error);
    }

    let object_format = git_output(path, &["rev-parse", "--show-object-format"])
        .await
        .map_err(ApiError::internal)?;
    if object_format != "sha1" && object_format != "sha256" {
        return Err(ApiError::internal(format!(
            "Git returned unsupported object format {object_format}"
        )));
    }
    let default_branch = match git_output(path, &["symbolic-ref", "--short", "HEAD"]).await {
        Ok(branch) if !branch.is_empty() => branch,
        _ => {
            set_symbolic_head(path, "main").await?;
            "main".to_owned()
        }
    };
    Ok(MirrorInitialization {
        object_format,
        default_branch,
    })
}

pub(super) async fn insert(
    connection: &impl sea_orm::ConnectionTrait,
    repository_id: Uuid,
    namespace: &str,
    mirror: PreparedMirror,
) -> Result<(), ApiError> {
    let now = Utc::now();
    ensure_identity_exists(connection, namespace, mirror.identity_id).await?;
    let (github_owner, github_repository) = mirror
        .github
        .map(|github| (Some(github.owner), Some(github.name)))
        .unwrap_or_default();
    repository_mirror::ActiveModel {
        repository_id: Set(repository_id),
        remote_url: Set(mirror.remote_url),
        identity_id: Set(mirror.identity_id),
        github_owner: Set(github_owner),
        github_repository: Set(github_repository),
        schedule: Set(mirror.schedule),
        last_attempted_at: Set(Some(now)),
        last_synced_at: Set(Some(now)),
        last_error: Set(None),
        metadata_last_synced_at: Set(None),
        metadata_error: Set(None),
        next_sync_at: Set(mirror.next_sync_at),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(connection)
    .await?;
    Ok(())
}
pub(super) async fn import_initial_metadata(
    state: &RepositoryState,
    repository: &repository::Model,
) -> Result<repository::Model, ApiError> {
    let mirror = find_mirror(repository.id, state.identity().database()).await?;
    let source = match (&mirror.github_owner, &mirror.github_repository) {
        (Some(owner), Some(name)) => github_mirror::GithubRepository {
            owner: owner.clone(),
            name: name.clone(),
        },
        // Mirrors created before the github_owner/github_repository columns
        // existed (or restored from an older export) identify from the URL.
        _ => match github_mirror::identify(&mirror.remote_url) {
            Some(github) => github,
            None => return Ok(repository.clone()),
        },
    };
    validate_remote_url(&mirror.remote_url)?;
    let authentication = authentication(
        state,
        &repository.namespace,
        &mirror.remote_url,
        mirror.identity_id,
        true,
    )
    .await?;
    let result = github_mirror::synchronize(
        state,
        repository,
        &source,
        authentication.api_token.as_deref(),
    )
    .await;
    let now = Utc::now();
    let mut active = mirror.into_active_model();
    match result {
        Ok(()) => {
            active.metadata_last_synced_at = Set(Some(now));
            active.metadata_error = Set(None);
        }
        Err(error) => {
            active.metadata_error = Set(Some(truncate(&error.to_string(), MAX_ERROR_LENGTH)));
        }
    }
    active.updated_at = Set(now);
    active.update(state.identity().database()).await?;
    repository::Entity::find_by_id(repository.id)
        .one(state.identity().database())
        .await?
        .ok_or_else(ApiError::not_found)
}

pub async fn get_mirror(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name)): AxumPath<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<MirrorResponse>, ApiError> {
    let (_, repository) = state
        .authenticated_repository(
            &headers,
            &jar,
            &namespace,
            &name,
            Permission::Manage,
            SCOPE_WRITE,
        )
        .await?;
    let mirror = find_mirror(repository.id, state.identity().database()).await?;
    Ok(Json(MirrorResponse::new(mirror, &state).await))
}

pub async fn update_mirror(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name)): AxumPath<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<UpdateMirrorRequest>,
) -> Result<Json<MirrorResponse>, ApiError> {
    let (actor, repository) = state
        .authenticated_repository(
            &headers,
            &jar,
            &namespace,
            &name,
            Permission::Manage,
            SCOPE_WRITE,
        )
        .await?;
    let mirror = find_mirror(repository.id, state.identity().database()).await?;
    validate_remote_url(&mirror.remote_url)?;
    let github = github_mirror::identify(&mirror.remote_url).is_some();
    authentication(
        &state,
        &repository.namespace,
        &mirror.remote_url,
        request.identity_id,
        github,
    )
    .await?;
    let now = Utc::now();
    let (schedule, next_sync_at) = normalize_schedule(request.schedule, now)?;
    let mut active = mirror.into_active_model();
    active.identity_id = Set(request.identity_id);
    active.schedule = Set(schedule);
    active.next_sync_at = Set(next_sync_at);
    active.updated_at = Set(now);

    let transaction = state.identity().database().begin().await?;
    ensure_identity_exists(&transaction, &repository.namespace, request.identity_id).await?;
    let mirror = active.update(&transaction).await?;
    state
        .identity()
        .audit_on(
            &transaction,
            Some(actor.user.id),
            "repository.mirror.update",
            Some(format!("{namespace}/{name}")),
        )
        .await?;
    transaction.commit().await?;
    Ok(Json(MirrorResponse::new(mirror, &state).await))
}

pub async fn sync_mirror(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name)): AxumPath<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<MirrorResponse>, ApiError> {
    let (actor, repository) = state
        .authenticated_repository(
            &headers,
            &jar,
            &namespace,
            &name,
            Permission::Manage,
            SCOPE_WRITE,
        )
        .await?;
    let mirror = synchronize(&state, &repository, Some(actor.user.id)).await?;
    Ok(Json(MirrorResponse::new(mirror, &state).await))
}

pub async fn convert_mirror(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name)): AxumPath<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<StatusCode, ApiError> {
    let (actor, repository) = state
        .authenticated_repository(
            &headers,
            &jar,
            &namespace,
            &name,
            Permission::Manage,
            SCOPE_WRITE,
        )
        .await?;
    convert(&state, &repository, Some(actor.user.id)).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Converts a mirrored repository into a standard, writable repository: the
/// mirror row is removed and imported metadata is detached while the refs,
/// topics, issues, comments, labels, website, and description are preserved.
async fn convert(
    state: &RepositoryState,
    repository: &repository::Model,
    actor_user_id: Option<Uuid>,
) -> Result<(), ApiError> {
    // Standard repositories have no mirror row and fail through the same
    // lookup the other mirror endpoints use.
    let _mirror = find_mirror(repository.id, state.identity().database()).await?;
    {
        let mut syncing = state.mirror_syncing.lock().await;
        if !syncing.insert(repository.id) {
            return Err(ApiError::conflict(
                "The repository is currently synchronizing.",
            ));
        }
    }
    let result = convert_inner(state, repository, actor_user_id).await;
    state.mirror_syncing.lock().await.remove(&repository.id);
    result
}

async fn convert_inner(
    state: &RepositoryState,
    repository: &repository::Model,
    actor_user_id: Option<Uuid>,
) -> Result<(), ApiError> {
    let transaction = state.identity().database().begin().await?;
    repository_mirror::Entity::delete_by_id(repository.id)
        .exec(&transaction)
        .await?;
    // Imported issues and comments become local: external linkage is cleared
    // while the original author attribution remains as provenance.
    repository_issue::Entity::update_many()
        .col_expr(
            repository_issue::Column::ExternalSource,
            sea_orm::sea_query::Expr::value(None::<String>),
        )
        .col_expr(
            repository_issue::Column::ExternalId,
            sea_orm::sea_query::Expr::value(None::<String>),
        )
        .col_expr(
            repository_issue::Column::ExternalUrl,
            sea_orm::sea_query::Expr::value(None::<String>),
        )
        .col_expr(
            repository_issue::Column::ExternalUpdatedAt,
            sea_orm::sea_query::Expr::value(None::<DateTime<Utc>>),
        )
        .filter(repository_issue::Column::RepositoryId.eq(repository.id))
        .exec(&transaction)
        .await?;
    issue_comment::Entity::update_many()
        .col_expr(
            issue_comment::Column::ExternalSource,
            sea_orm::sea_query::Expr::value(None::<String>),
        )
        .col_expr(
            issue_comment::Column::ExternalId,
            sea_orm::sea_query::Expr::value(None::<String>),
        )
        .col_expr(
            issue_comment::Column::ExternalUrl,
            sea_orm::sea_query::Expr::value(None::<String>),
        )
        .col_expr(
            issue_comment::Column::ExternalUpdatedAt,
            sea_orm::sea_query::Expr::value(None::<DateTime<Utc>>),
        )
        .filter(
            issue_comment::Column::IssueId.in_subquery(
                Query::select()
                    .column(repository_issue::Column::Id)
                    .from(repository_issue::Entity)
                    .and_where(repository_issue::Column::RepositoryId.eq(repository.id))
                    .to_owned(),
            ),
        )
        .exec(&transaction)
        .await?;
    repository_topic::Entity::update_many()
        .col_expr(
            repository_topic::Column::ExternalSource,
            sea_orm::sea_query::Expr::value(None::<String>),
        )
        .filter(repository_topic::Column::RepositoryId.eq(repository.id))
        .exec(&transaction)
        .await?;
    let now = Utc::now();
    let mut active_repository = repository.clone().into_active_model();
    active_repository.mirrored = Set(false);
    active_repository.updated_at = Set(now);
    active_repository.update(&transaction).await?;
    state
        .identity()
        .audit_on(
            &transaction,
            actor_user_id,
            "repository.mirror.convert",
            Some(format!("{}/{}", repository.namespace, repository.name)),
        )
        .await?;
    transaction.commit().await?;
    Ok(())
}

pub(super) async fn synchronize(
    state: &RepositoryState,
    repository: &repository::Model,
    actor_user_id: Option<Uuid>,
) -> Result<repository_mirror::Model, ApiError> {
    let _permit = state
        .mirror_slots
        .acquire()
        .await
        .map_err(ApiError::internal)?;
    {
        let mut syncing = state.mirror_syncing.lock().await;
        if !syncing.insert(repository.id) {
            return Err(ApiError::conflict(
                "Mirror synchronization is already running.",
            ));
        }
    }
    let result = synchronize_inner(state, repository, actor_user_id).await;
    state.mirror_syncing.lock().await.remove(&repository.id);
    result
}

async fn synchronize_inner(
    state: &RepositoryState,
    repository: &repository::Model,
    actor_user_id: Option<Uuid>,
) -> Result<repository_mirror::Model, ApiError> {
    let mirror = find_mirror(repository.id, state.identity().database()).await?;
    let attempted_at = Utc::now();
    let path = state.repository_path(repository);
    validate_remote_url(&mirror.remote_url)?;
    let github = match (&mirror.github_owner, &mirror.github_repository) {
        (Some(owner), Some(name)) => Some(github_mirror::GithubRepository {
            owner: owner.clone(),
            name: name.clone(),
        }),
        _ => github_mirror::identify(&mirror.remote_url),
    };
    let authentication = authentication(
        state,
        &repository.namespace,
        &mirror.remote_url,
        mirror.identity_id,
        github.is_some(),
    )
    .await?;
    let mut command = Command::new("git");
    command
        .arg("--git-dir")
        .arg(&path)
        .args([
            "fetch",
            "--prune",
            "--prune-tags",
            "origin",
            "+refs/*:refs/*",
        ])
        .stdin(Stdio::null());
    let _temporary_files = apply_git_environment(
        state,
        &mut command,
        &path,
        &mirror.remote_url,
        &authentication,
        true,
        None,
        None,
    )
    .await?;
    if let Some(id) = mirror.identity_id {
        mark_repository_identity_used(state.identity(), &repository.namespace, id, attempted_at)
            .await?;
    }
    let sync_result = run_git(&mut command).await;
    let metadata_result = match (&sync_result, github.as_ref()) {
        (Ok(()), Some(github)) => Some(
            github_mirror::synchronize(
                state,
                repository,
                github,
                authentication.api_token.as_deref(),
            )
            .await,
        ),
        _ => None,
    };
    let completed_at = Utc::now();
    let next_sync_at = mirror
        .schedule
        .as_deref()
        .map(|expression| {
            schedule::next_occurrence(expression, completed_at)
                .map_err(|error| ApiError::bad_request(format!("Mirror schedule {error}.")))
        })
        .transpose()?;
    let mut active = mirror.into_active_model();
    active.last_attempted_at = Set(Some(attempted_at));
    active.next_sync_at = Set(next_sync_at);
    active.updated_at = Set(completed_at);
    match &sync_result {
        Ok(()) => {
            active.last_synced_at = Set(Some(completed_at));
            active.last_error = Set(None);
        }
        Err(error) => {
            active.last_error = Set(Some(truncate(error, MAX_ERROR_LENGTH)));
        }
    }
    if let Some(metadata_result) = &metadata_result {
        match metadata_result {
            Ok(()) => {
                active.metadata_last_synced_at = Set(Some(completed_at));
                active.metadata_error = Set(None);
            }
            Err(error) => {
                active.metadata_error = Set(Some(truncate(&error.to_string(), MAX_ERROR_LENGTH)));
            }
        }
    }

    let transaction = state.identity().database().begin().await?;
    let mirror = active.update(&transaction).await?;
    if sync_result.is_ok() {
        repository::Entity::update_many()
            .col_expr(
                repository::Column::UpdatedAt,
                sea_orm::sea_query::Expr::value(completed_at),
            )
            .filter(repository::Column::Id.eq(repository.id))
            .exec(&transaction)
            .await?;
        state
            .identity()
            .audit_on(
                &transaction,
                actor_user_id,
                "repository.mirror.sync",
                Some(format!("{}/{}", repository.namespace, repository.name)),
            )
            .await?;
    }
    transaction.commit().await?;
    state.invalidate_repository_size(repository.id).await;
    sync_result.map_err(|error| ApiError::bad_request(format!("Mirror sync failed: {error}")))?;
    Ok(mirror)
}

async fn find_mirror(
    repository_id: Uuid,
    connection: &impl sea_orm::ConnectionTrait,
) -> Result<repository_mirror::Model, ApiError> {
    repository_mirror::Entity::find_by_id(repository_id)
        .one(connection)
        .await?
        .ok_or_else(ApiError::not_found)
}

fn validate_remote_url(value: &str) -> Result<(String, RemoteTransport), ApiError> {
    let value = value.trim();
    if value.is_empty() || value.len() > MAX_REMOTE_URL_LENGTH {
        return Err(ApiError::bad_request(
            "Remote URLs must contain 1 to 2048 characters.",
        ));
    }
    let normalized = if let Some(target) = value.strip_prefix("git@") {
        let (host, path) = target.split_once(':').ok_or_else(|| {
            ApiError::bad_request("SSH URLs must identify a host and repository path.")
        })?;
        if host.is_empty() || path.trim_matches('/').is_empty() {
            return Err(ApiError::bad_request(
                "SSH URLs must identify a host and repository path.",
            ));
        }
        format!("ssh://git@{host}/{}", path.trim_start_matches('/'))
    } else {
        value.to_owned()
    };
    let url = Url::parse(&normalized)
        .map_err(|_| ApiError::bad_request("Remote URL must use HTTPS or canonical SSH syntax."))?;
    match url.scheme() {
        "https" => {
            if url.host_str().is_none() {
                return Err(ApiError::bad_request("Remote URL must include a host."));
            }
            if !url.username().is_empty() || url.password().is_some() {
                return Err(ApiError::bad_request(
                    "Choose an identity instead of putting credentials in the URL.",
                ));
            }
            if url.query().is_some() || url.fragment().is_some() {
                return Err(ApiError::bad_request(
                    "Remote URL must not contain a query or fragment.",
                ));
            }
            Ok((url.to_string(), RemoteTransport::Https))
        }
        "http" => Err(ApiError::bad_request(
            "Remote URLs must use HTTPS so credentials are never sent in the clear.",
        )),
        "ssh"
            if url.host_str().is_some()
                && url.username() == "git"
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none()
                && !url.path().trim_matches('/').is_empty() =>
        {
            Ok((url.to_string(), RemoteTransport::Ssh))
        }
        _ => Err(ApiError::bad_request(
            "SSH URLs must use git@host:path or ssh://git@host/path syntax.",
        )),
    }
}

fn validate_import_remote_url(source_instance_url: &str, value: &str) -> Result<String, ApiError> {
    let value = value.trim();
    if value.is_empty() || value.len() > MAX_REMOTE_URL_LENGTH {
        return Err(ApiError::bad_request(
            "Import URLs must contain 1 to 2048 characters.",
        ));
    }
    let source = Url::parse(source_instance_url)
        .map_err(|_| ApiError::internal("source instance URL is invalid"))?;
    let mut url = Url::parse(value).map_err(|_| {
        ApiError::bad_request("Import URL must be an HTTP or HTTPS URL without credentials.")
    })?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(ApiError::bad_request(
            "Import URL must be an HTTP or HTTPS URL without credentials.",
        ));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(ApiError::bad_request(
            "Import URL must not contain credentials.",
        ));
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(ApiError::bad_request(
            "Import URL must not contain a query or fragment.",
        ));
    }
    let github_api = source.scheme() == "https"
        && source.host_str() == Some("api.github.com")
        && url.scheme() == "https"
        && url.host_str() == Some("github.com")
        && url.port().is_none();
    let same_origin = source.scheme() == url.scheme()
        && source
            .host_str()
            .zip(url.host_str())
            .is_some_and(|(source, clone)| source.eq_ignore_ascii_case(clone))
        && source.port_or_known_default() == url.port_or_known_default();
    if !github_api && !same_origin {
        return Err(ApiError::bad_request(
            "The repository clone URL must use the source instance origin.",
        ));
    }
    url.set_query(None);
    url.set_fragment(None);
    Ok(url.to_string())
}

fn validate_direct_import_remote(
    source_instance_url: Option<&str>,
    value: &str,
    uses_ssh: bool,
) -> Result<(String, RemoteTransport), ApiError> {
    if uses_ssh {
        let (remote_url, transport) = validate_remote_url(value)?;
        if transport != RemoteTransport::Ssh {
            return Err(ApiError::bad_request(
                "Select a password or token identity for an HTTP or HTTPS import URL.",
            ));
        }
        return Ok((remote_url, transport));
    }
    if let Some(source_instance_url) = source_instance_url {
        return Ok((
            validate_import_remote_url(source_instance_url, value)?,
            RemoteTransport::Https,
        ));
    }
    validate_remote_url(value)
}

pub(super) fn validate_direct_import_identity(
    source_instance_url: Option<&str>,
    value: &str,
    identity_kind: &str,
) -> Result<String, ApiError> {
    let (remote_url, transport) =
        validate_direct_import_remote(source_instance_url, value, identity_kind == "ssh")?;
    if transport == RemoteTransport::Ssh && identity_kind != "ssh" {
        return Err(ApiError::bad_request(
            "Select an SSH identity for an SSH import URL.",
        ));
    }
    Ok(remote_url)
}

fn import_lfs_endpoint(remote_url: &str) -> Result<String, ApiError> {
    let mut url = Url::parse(remote_url)
        .map_err(|_| ApiError::internal("import remote URL is not a valid URL"))?;
    if url.scheme() == "ssh" {
        url.set_scheme("https")
            .map_err(|_| ApiError::internal("could not derive the Git LFS endpoint"))?;
        url.set_username("")
            .map_err(|_| ApiError::internal("could not derive the Git LFS endpoint"))?;
    }
    let path = url.path().trim_end_matches('/');
    url.set_path(&format!("{path}/info/lfs"));
    url.set_query(None);
    url.set_fragment(None);
    Ok(url.to_string())
}

async fn fetch_and_promote_lfs(
    state: &RepositoryState,
    repository_path: &Path,
    lfs_path: &Path,
    staging: &Path,
    remote_url: &str,
    authentication: &GitAuthentication,
) -> Result<(), ApiError> {
    let result = async {
        fs::create_dir_all(staging)
            .await
            .map_err(ApiError::internal)?;
        let endpoint = import_lfs_endpoint(remote_url)?;
        let mut command = Command::new("git");
        command
            .arg("--git-dir")
            .arg(repository_path)
            .args(["lfs", "fetch", "--all", "origin"])
            .stdin(Stdio::null());
        let _temporary_files = apply_git_environment(
            state,
            &mut command,
            repository_path,
            remote_url,
            authentication,
            false,
            Some(&endpoint),
            Some(staging),
        )
        .await?;
        run_git(&mut command).await.map_err(|error| {
            ApiError::bad_request(format!("Could not import Git LFS objects: {error}"))
        })?;
        promote_lfs_objects(staging, lfs_path).await
    }
    .await;
    cleanup_lfs_path(staging).await;
    result
}

async fn promote_lfs_objects(staging: &Path, destination: &Path) -> Result<(), ApiError> {
    let objects = staging.join("objects");
    if !fs::try_exists(&objects).await.map_err(ApiError::internal)? {
        return Ok(());
    }

    let mut first_level = fs::read_dir(&objects).await.map_err(ApiError::internal)?;
    while let Some(first) = first_level.next_entry().await.map_err(ApiError::internal)? {
        let first_name = first.file_name();
        let Some(first_name) = first_name.to_str() else {
            return Err(ApiError::internal(
                "Git LFS produced an invalid object path",
            ));
        };
        if !is_hex_component(first_name) {
            return Err(ApiError::internal(
                "Git LFS produced an invalid object path",
            ));
        }
        if !first
            .file_type()
            .await
            .map_err(ApiError::internal)?
            .is_dir()
        {
            return Err(ApiError::internal(
                "Git LFS produced an invalid object path",
            ));
        }

        let mut second_level = fs::read_dir(first.path())
            .await
            .map_err(ApiError::internal)?;
        while let Some(second) = second_level
            .next_entry()
            .await
            .map_err(ApiError::internal)?
        {
            let second_name = second.file_name();
            let Some(second_name) = second_name.to_str() else {
                return Err(ApiError::internal(
                    "Git LFS produced an invalid object path",
                ));
            };
            if !is_hex_component(second_name)
                || !second
                    .file_type()
                    .await
                    .map_err(ApiError::internal)?
                    .is_dir()
            {
                return Err(ApiError::internal(
                    "Git LFS produced an invalid object path",
                ));
            }

            let mut objects_level = fs::read_dir(second.path())
                .await
                .map_err(ApiError::internal)?;
            while let Some(object) = objects_level
                .next_entry()
                .await
                .map_err(ApiError::internal)?
            {
                let oid = object.file_name();
                let Some(oid) = oid.to_str() else {
                    return Err(ApiError::internal(
                        "Git LFS produced an invalid object path",
                    ));
                };
                if !is_lfs_oid(oid)
                    || !object
                        .file_type()
                        .await
                        .map_err(ApiError::internal)?
                        .is_file()
                {
                    return Err(ApiError::internal(
                        "Git LFS produced an invalid object path",
                    ));
                }
                if &oid[..2] != first_name || &oid[2..4] != second_name {
                    return Err(ApiError::internal(
                        "Git LFS produced an invalid object path",
                    ));
                }

                let source = object.path();
                let target = destination.join(first_name).join(second_name).join(oid);
                let parent = target
                    .parent()
                    .ok_or_else(|| ApiError::internal("Git LFS object path has no parent"))?;
                fs::create_dir_all(parent)
                    .await
                    .map_err(ApiError::internal)?;
                if sha256_file(&source).await? != oid {
                    return Err(ApiError::internal(
                        "A fetched Git LFS object does not match its identifier",
                    ));
                }
                if fs::try_exists(&target).await.map_err(ApiError::internal)? {
                    if sha256_file(&target).await? != oid {
                        return Err(ApiError::internal(
                            "An existing Git LFS object does not match its identifier",
                        ));
                    }
                    fs::remove_file(source).await.map_err(ApiError::internal)?;
                } else {
                    fs::rename(source, target)
                        .await
                        .map_err(ApiError::internal)?;
                }
            }
        }
    }
    Ok(())
}

async fn sha256_file(path: &Path) -> Result<String, ApiError> {
    let mut file = fs::File::open(path).await.map_err(ApiError::internal)?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 128 * 1024];
    loop {
        let read = file.read(&mut buffer).await.map_err(ApiError::internal)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    let digest = digest.finalize();
    let mut oid = String::with_capacity(64);
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for byte in digest {
        oid.push(HEX[(byte >> 4) as usize] as char);
        oid.push(HEX[(byte & 0x0f) as usize] as char);
    }
    Ok(oid)
}

fn is_hex_component(value: &str) -> bool {
    value.len() == 2
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn is_lfs_oid(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

async fn cleanup_lfs_path(path: &Path) {
    if let Err(error) = fs::remove_dir_all(path).await
        && error.kind() != std::io::ErrorKind::NotFound
    {
        tracing::error!(%error, path = %path.display(), "could not clean up Git LFS directory");
    }
}

async fn authentication(
    state: &RepositoryState,
    namespace: &str,
    remote_url: &str,
    identity_id: Option<Uuid>,
    github: bool,
) -> Result<GitAuthentication, ApiError> {
    let Some(id) = identity_id else {
        return Ok(GitAuthentication {
            basic: None,
            ssh_private_key: None,
            api_token: None,
        });
    };
    let identity = load_mirror_identity_secret(state.identity(), namespace, id).await?;
    if identity.kind != "token" {
        return Err(ApiError::bad_request(
            "Mirrors require an access token identity.",
        ));
    }
    let provider = identity
        .provider
        .as_deref()
        .ok_or_else(|| ApiError::bad_request("Mirror identity provider is missing."))?;
    let server_url = identity
        .instance_url
        .as_deref()
        .ok_or_else(|| ApiError::bad_request("Mirror identity server URL is missing."))?;
    if !same_origin(server_url, remote_url) {
        return Err(ApiError::bad_request(
            "The selected identity belongs to a different git server.",
        ));
    }
    let username = match provider {
        "github" => "x-access-token",
        "gitlab" => "oauth2",
        "gitea" | "forgejo" => "git",
        _ => {
            return Err(ApiError::bad_request(
                "Mirror identity provider is invalid.",
            ));
        }
    };
    let api_token = (github && provider == "github").then(|| identity.secret.clone());
    Ok(GitAuthentication {
        basic: Some((username.to_owned(), identity.secret)),
        ssh_private_key: None,
        api_token,
    })
}

fn same_origin(server_url: &str, remote_url: &str) -> bool {
    let Ok(server) = Url::parse(server_url) else {
        return false;
    };
    let Ok(remote) = Url::parse(remote_url) else {
        return false;
    };
    server.scheme() == remote.scheme()
        && server
            .host_str()
            .zip(remote.host_str())
            .is_some_and(|(server, remote)| server.eq_ignore_ascii_case(remote))
        && server.port_or_known_default() == remote.port_or_known_default()
}

async fn ensure_identity_exists(
    connection: &impl sea_orm::ConnectionTrait,
    namespace: &str,
    identity_id: Option<Uuid>,
) -> Result<(), ApiError> {
    let Some(identity_id) = identity_id else {
        return Ok(());
    };
    let exists = namespace_mirror_identity::Entity::find_by_id(identity_id)
        .filter(namespace_mirror_identity::Column::Namespace.eq(namespace))
        .filter(namespace_mirror_identity::Column::Kind.eq("token"))
        .one(connection)
        .await?
        .is_some();
    if exists {
        Ok(())
    } else {
        Err(ApiError::bad_request(
            "The selected mirror identity does not exist in this namespace.",
        ))
    }
}

fn normalize_schedule(
    value: Option<String>,
    after: DateTime<Utc>,
) -> Result<(Option<String>, Option<DateTime<Utc>>), ApiError> {
    let schedule = value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty() && value != "manual");
    let Some(schedule) = schedule else {
        return Ok((None, None));
    };
    if schedule.len() > MAX_SCHEDULE_LENGTH {
        return Err(ApiError::bad_request(
            "Mirror schedules cannot exceed 255 characters.",
        ));
    }
    let next = schedule::next_occurrence(&schedule, after)
        .map_err(|error| ApiError::bad_request(format!("Mirror schedule {error}.")))?;
    Ok((Some(schedule), Some(next)))
}

struct TemporaryGitFiles {
    paths: Vec<PathBuf>,
    proxy: Option<JoinHandle<()>>,
}

impl Drop for TemporaryGitFiles {
    fn drop(&mut self) {
        for path in &self.paths {
            if let Err(error) = std::fs::remove_file(path)
                && error.kind() != std::io::ErrorKind::NotFound
            {
                tracing::warn!(%error, path = %path.display(), "could not remove temporary mirror credential");
            }
        }
        if let Some(proxy) = self.proxy.take() {
            proxy.abort();
        }
    }
}
pub(super) async fn cleanup_temporary_files(directory: &Path) -> Result<(), std::io::Error> {
    let mut entries = tokio::fs::read_dir(directory).await?;
    while let Some(entry) = entries.next_entry().await? {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let is_mirror_credential = name.starts_with(".gitadel-mirror-")
            && (name.ends_with(".key") || name.ends_with(".known-hosts"));
        let metadata = entry.metadata().await?;
        let is_stale = metadata
            .modified()?
            .elapsed()
            .is_ok_and(|age| age > TEMPORARY_FILE_MAX_AGE);
        if is_mirror_credential && metadata.is_file() && is_stale {
            tokio::fs::remove_file(entry.path()).await?;
        }
    }
    Ok(())
}

pub(super) async fn cleanup_lfs_staging(directory: &Path) -> Result<(), std::io::Error> {
    let mut entries = fs::read_dir(directory).await?;
    while let Some(entry) = entries.next_entry().await? {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !name.starts_with(LFS_IMPORT_STAGING_PREFIX) {
            continue;
        }
        let metadata = entry.metadata().await?;
        let is_stale = metadata
            .modified()?
            .elapsed()
            .is_ok_and(|age| age > TEMPORARY_FILE_MAX_AGE);
        if metadata.is_dir() && is_stale {
            fs::remove_dir_all(entry.path()).await?;
        }
    }
    Ok(())
}

/// Builds the git config key that scopes `http.extraHeader` to the mirror's
/// own remote origin (for example `http.https://github.com/.extraHeader`), so
/// stored credentials are never sent to any other host git may contact.
/// An explicit non-default port is serialized into the key; without it git's
/// urlmatch would not apply the header to `https://host:8443/...` remotes.
fn basic_auth_header_key(remote_url: &str) -> Result<String, ApiError> {
    let url = Url::parse(remote_url)
        .map_err(|_| ApiError::internal("mirror remote URL is not a valid URL"))?;
    let host = url
        .host_str()
        .ok_or_else(|| ApiError::internal("mirror remote URL has no host"))?;
    let origin = match url.port() {
        Some(port) => format!("{}://{}:{}", url.scheme(), host, port),
        None => format!("{}://{}", url.scheme(), host),
    };
    Ok(format!("http.{origin}/.extraHeader"))
}
async fn resolve_https_remote(remote_url: &str) -> Result<Option<HttpsTarget>, ApiError> {
    if remote_url.starts_with("git@github.com:") {
        return Ok(None);
    }
    let url = Url::parse(remote_url)
        .map_err(|_| ApiError::internal("mirror remote URL is not a valid URL"))?;
    if url.scheme() != "https" {
        return Ok(None);
    }
    let host = match url.host() {
        Some(Host::Domain(host)) => host,
        _ => {
            return Err(ApiError::bad_request(
                "Mirror HTTPS URLs must use a public DNS hostname.",
            ));
        }
    };
    let normalized_host = host.trim_end_matches('.');
    if normalized_host.eq_ignore_ascii_case("localhost")
        || normalized_host.to_ascii_lowercase().ends_with(".localhost")
    {
        return Err(ApiError::bad_request(
            "Mirror HTTPS URLs must use a public DNS hostname.",
        ));
    }
    let port = url.port_or_known_default().unwrap_or(443);
    let resolved = lookup_host((host, port)).await.map_err(|error| {
        ApiError::bad_request(format!("Could not resolve mirror host: {error}"))
    })?;
    let mut addresses = Vec::new();
    for address in resolved {
        if !is_public_ip(address.ip()) {
            return Err(ApiError::bad_request(
                "Mirror hosts must not resolve to private or reserved network addresses.",
            ));
        }
        if !addresses.contains(&address) {
            addresses.push(address);
        }
    }
    if addresses.is_empty() {
        return Err(ApiError::bad_request(
            "Mirror host did not resolve to a network address.",
        ));
    }
    Ok(Some(HttpsTarget {
        host: normalized_host.to_ascii_lowercase(),
        port,
        addresses,
    }))
}

#[derive(Clone)]
struct HttpsTarget {
    host: String,
    port: u16,
    addresses: Vec<SocketAddr>,
}

async fn start_https_proxy(target: HttpsTarget) -> Result<(String, JoinHandle<()>), ApiError> {
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .map_err(|error| ApiError::internal(format!("could not bind mirror proxy: {error}")))?;
    let address = listener.local_addr().map_err(|error| {
        ApiError::internal(format!("could not read mirror proxy address: {error}"))
    })?;
    let task = tokio::spawn(async move {
        loop {
            let (connection, _) = match listener.accept().await {
                Ok(accepted) => accepted,
                Err(error) => {
                    tracing::warn!(%error, "mirror HTTPS proxy stopped accepting connections");
                    return;
                }
            };
            let target = target.clone();
            tokio::spawn(async move {
                if let Err(error) = proxy_https_connection(connection, target).await {
                    tracing::debug!(%error, "mirror HTTPS proxy rejected a connection");
                }
            });
        }
    });
    Ok((format!("http://{address}"), task))
}

async fn proxy_https_connection(mut client: TcpStream, target: HttpsTarget) -> Result<(), String> {
    let request = tokio::time::timeout(Duration::from_secs(10), read_proxy_request(&mut client))
        .await
        .map_err(|_| "proxy request timed out".to_owned())??;
    let request_line = request
        .split_once("\r\n")
        .map(|(line, _)| line)
        .unwrap_or(request.as_str());
    let mut parts = request_line.split_whitespace();
    let method = parts.next();
    let authority = parts.next();
    let version = parts.next();
    if method != Some("CONNECT")
        || !version.is_some_and(|version| version.starts_with("HTTP/"))
        || parts.next().is_some()
    {
        let _ = client
            .write_all(b"HTTP/1.1 403 Forbidden\r\nConnection: close\r\n\r\n")
            .await;
        return Err("only HTTPS CONNECT requests are allowed".to_owned());
    }
    let (host, port) = authority
        .and_then(|authority| authority.rsplit_once(':'))
        .and_then(|(host, port)| port.parse::<u16>().ok().map(|port| (host, port)))
        .ok_or_else(|| "proxy CONNECT authority is invalid".to_owned())?;
    if !host
        .trim_end_matches('.')
        .eq_ignore_ascii_case(&target.host)
        || port != target.port
    {
        let _ = client
            .write_all(b"HTTP/1.1 403 Forbidden\r\nConnection: close\r\n\r\n")
            .await;
        return Err("proxy CONNECT target is not the mirror origin".to_owned());
    }

    let mut upstream = tokio::time::timeout(Duration::from_secs(10), async {
        for address in target.addresses {
            if let Ok(upstream) = TcpStream::connect(address).await {
                return Some(upstream);
            }
        }
        None
    })
    .await
    .map_err(|_| "proxy upstream connection timed out".to_owned())?
    .ok_or_else(|| "proxy could not connect to the mirror origin".to_owned())?;
    client
        .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
        .await
        .map_err(|error| error.to_string())?;
    tokio::io::copy_bidirectional(&mut client, &mut upstream)
        .await
        .map_err(|error| error.to_string())?;
    Ok(())
}

async fn read_proxy_request(stream: &mut TcpStream) -> Result<String, String> {
    const MAX_PROXY_REQUEST_LENGTH: usize = 16 * 1024;
    let mut request = Vec::with_capacity(1024);
    loop {
        let mut chunk = [0_u8; 1024];
        let read = stream
            .read(&mut chunk)
            .await
            .map_err(|error| error.to_string())?;
        if read == 0 {
            return Err("proxy client closed before sending a request".to_owned());
        }
        request.extend_from_slice(&chunk[..read]);
        if request.len() > MAX_PROXY_REQUEST_LENGTH {
            return Err("proxy request headers are too large".to_owned());
        }
        if request.windows(4).any(|window| window == b"\r\n\r\n") {
            return String::from_utf8(request)
                .map_err(|_| "proxy request headers are not UTF-8".to_owned());
        }
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "arguments describe one concrete Git process environment"
)]
async fn apply_git_environment(
    state: &RepositoryState,
    command: &mut Command,
    repository_path: &Path,
    remote_url: &str,
    authentication: &GitAuthentication,
    enforce_public_https: bool,
    lfs_endpoint: Option<&str>,
    lfs_storage: Option<&Path>,
) -> Result<TemporaryGitFiles, ApiError> {
    command.env("GIT_TERMINAL_PROMPT", "0");
    for variable in [
        "ALL_PROXY",
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "NO_PROXY",
        "all_proxy",
        "http_proxy",
        "https_proxy",
        "no_proxy",
    ] {
        command.env_remove(variable);
    }

    let mut temporary_files = TemporaryGitFiles {
        paths: Vec::new(),
        proxy: None,
    };
    let mut git_config = vec![
        ("credential.helper".to_owned(), String::new()),
        ("http.followRedirects".to_owned(), "false".to_owned()),
    ];
    if enforce_public_https {
        if let Some(target) = resolve_https_remote(remote_url).await? {
            let (proxy, task) = start_https_proxy(target).await?;
            temporary_files.proxy = Some(task);
            git_config.push(("http.proxy".to_owned(), proxy));
        } else {
            git_config.push(("http.proxy".to_owned(), String::new()));
        }
    } else {
        git_config.push(("http.proxy".to_owned(), String::new()));
    }
    if let Some(lfs_endpoint) = lfs_endpoint {
        git_config.push(("lfs.url".to_owned(), lfs_endpoint.to_owned()));
        git_config.push(("remote.origin.lfsurl".to_owned(), lfs_endpoint.to_owned()));
        git_config.push(("lfs.basictransfersonly".to_owned(), "true".to_owned()));
    }
    if let Some(lfs_storage) = lfs_storage {
        let lfs_storage = lfs_storage
            .to_str()
            .ok_or_else(|| ApiError::internal("Git LFS staging path is not valid UTF-8"))?;
        git_config.push(("lfs.storage".to_owned(), lfs_storage.to_owned()));
    }
    if let Some((username, secret)) = authentication.basic.as_ref() {
        let credentials = STANDARD.encode(format!("{username}:{secret}"));
        git_config.push((
            basic_auth_header_key(remote_url)?,
            format!("Authorization: Basic {credentials}"),
        ));
    }
    command.env("GIT_CONFIG_COUNT", git_config.len().to_string());
    for (index, (key, value)) in git_config.into_iter().enumerate() {
        command.env(format!("GIT_CONFIG_KEY_{index}"), key);
        command.env(format!("GIT_CONFIG_VALUE_{index}"), value);
    }

    if let Some(private_key) = authentication.ssh_private_key.as_deref() {
        let directory = repository_path
            .parent()
            .ok_or_else(|| ApiError::internal("mirror repository path has no parent"))?;
        let nonce = Uuid::new_v4();
        let key_path = directory.join(format!(".gitadel-mirror-{nonce}.key"));
        let known_hosts_path = directory.join(format!(".gitadel-mirror-{nonce}.known-hosts"));
        write_private_file(&key_path, private_key.as_bytes()).await?;
        temporary_files.paths.push(key_path.clone());
        let known_hosts = github_mirror::known_hosts(state).await?;
        write_private_file(&known_hosts_path, known_hosts.as_bytes()).await?;
        temporary_files.paths.push(known_hosts_path.clone());
        let key = shlex::try_quote(
            key_path
                .to_str()
                .ok_or_else(|| ApiError::internal("SSH key path is not valid UTF-8"))?,
        )
        .map_err(ApiError::internal)?;
        let known_hosts = shlex::try_quote(
            known_hosts_path
                .to_str()
                .ok_or_else(|| ApiError::internal("known-hosts path is not valid UTF-8"))?,
        )
        .map_err(ApiError::internal)?;
        command.env(
            "GIT_SSH_COMMAND",
            format!(
                "ssh -i {key} -o BatchMode=yes -o IdentitiesOnly=yes -o PasswordAuthentication=no -o StrictHostKeyChecking=yes -o UserKnownHostsFile={known_hosts}"
            ),
        );
    }
    Ok(temporary_files)
}

async fn write_private_file(path: &Path, content: &[u8]) -> Result<(), ApiError> {
    let mut options = tokio::fs::OpenOptions::new();
    options.write(true).create_new(true).mode(0o600);
    let mut file = options.open(path).await.map_err(|error| {
        ApiError::internal(format!("could not create {}: {error}", path.display()))
    })?;
    file.write_all(content).await.map_err(|error| {
        ApiError::internal(format!("could not write {}: {error}", path.display()))
    })?;
    file.flush()
        .await
        .map_err(|error| ApiError::internal(format!("could not flush {}: {error}", path.display())))
}

async fn run_git(command: &mut Command) -> Result<(), String> {
    command.kill_on_drop(true);
    let output = tokio::time::timeout(GIT_COMMAND_TIMEOUT, command.output())
        .await
        .map_err(|_| "Git command timed out after 10 minutes.".to_owned())?
        .map_err(|error| error.to_string())?;
    if output.status.success() {
        Ok(())
    } else {
        let message = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        Err(if message.is_empty() {
            format!("Git exited with status {}.", output.status)
        } else {
            truncate(&message, MAX_ERROR_LENGTH)
        })
    }
}

async fn git_output(path: &std::path::Path, arguments: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .arg("--git-dir")
        .arg(path)
        .args(arguments)
        .stdin(Stdio::null())
        .output()
        .await
        .map_err(|error| error.to_string())?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_owned())
    }
}

async fn set_symbolic_head(path: &std::path::Path, branch: &str) -> Result<(), ApiError> {
    let mut command = Command::new("git");
    command
        .arg("--git-dir")
        .arg(path)
        .args(["symbolic-ref", "HEAD", &format!("refs/heads/{branch}")])
        .stdin(Stdio::null());
    run_git(&mut command).await.map_err(ApiError::internal)
}

fn truncate(value: &str, maximum: usize) -> String {
    if value.len() <= maximum {
        return value.to_owned();
    }
    let mut end = maximum;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    use tokio::{
        fs,
        io::{AsyncReadExt as _, AsyncWriteExt as _},
        net::TcpStream,
    };
    use uuid::Uuid;

    use super::{
        HttpsTarget, RemoteTransport, import_lfs_endpoint, is_public_ip, promote_lfs_objects,
        resolve_https_remote, same_origin, start_https_proxy, validate_direct_import_identity,
        validate_import_remote_url, validate_remote_url,
    };

    #[test]
    fn validate_remote_url_rejects_executable_git_transport() {
        let result = validate_remote_url("ext::sh -c touch /tmp/owned");

        assert!(result.is_err());
    }

    #[test]
    fn validate_remote_url_rejects_embedded_credentials() {
        let result = validate_remote_url("https://user:secret@example.com/repository.git");

        assert!(result.is_err());
    }

    #[test]
    fn validate_remote_url_rejects_http_for_scheduled_mirrors() {
        let result = validate_remote_url("http://source.example/repository.git");

        assert!(result.is_err());
    }

    #[test]
    fn validate_remote_url_allows_canonical_github_ssh() {
        let result = validate_remote_url("git@github.com:octocat/hello-world.git").unwrap();

        assert_eq!(
            result,
            (
                "ssh://git@github.com/octocat/hello-world.git".to_owned(),
                RemoteTransport::Ssh,
            ),
        );
    }

    #[test]
    fn mirror_identity_origin_matches_repository_on_same_server() {
        assert!(same_origin(
            "https://git.example.com",
            "https://git.example.com/team/project.git",
        ));
    }

    #[test]
    fn mirror_identity_origin_rejects_different_server() {
        assert!(!same_origin(
            "https://git.example.com",
            "https://attacker.example.com/team/project.git",
        ));
    }

    #[test]
    fn direct_import_requires_ssh_identity_for_ssh_url() {
        let result = validate_direct_import_identity(
            Some("https://github.com"),
            "git@github.com:octocat/hello-world.git",
            "token",
        );

        assert!(result.is_err());
    }

    #[test]
    fn direct_import_rejects_ssh_identity_for_https_url() {
        let result = validate_direct_import_identity(None, "https://example.com/source.git", "ssh");

        assert!(result.is_err());
    }

    #[test]
    fn validate_import_remote_url_allows_http_and_private_hosts() {
        let result =
            validate_import_remote_url("http://127.0.0.1:8080", "http://127.0.0.1:8080/source.git");

        assert!(result.is_ok());
    }

    #[test]
    fn validate_import_remote_url_rejects_embedded_credentials() {
        let result = validate_import_remote_url(
            "https://example.com",
            "https://user:secret@example.com/source.git",
        );

        assert!(result.is_err());
    }

    #[test]
    fn validate_import_remote_url_rejects_a_different_origin() {
        let result = validate_import_remote_url(
            "https://forge.example",
            "https://attacker.example/source.git",
        );

        assert!(result.is_err());
    }

    #[test]
    fn import_lfs_endpoint_appends_info_lfs_to_clone_path() {
        let result = import_lfs_endpoint("https://source.example/team/source.git");

        assert_eq!(
            result.unwrap(),
            "https://source.example/team/source.git/info/lfs"
        );
    }

    #[tokio::test]
    async fn promote_lfs_objects_uses_gitadel_object_layout() {
        let root = std::env::temp_dir().join(format!("gitadel-lfs-promote-{}", Uuid::new_v4()));
        let oid = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        let source = root.join("staging/objects/e3/b0");
        let destination = root.join("lfs/repository");
        fs::create_dir_all(&source).await.unwrap();
        fs::write(source.join(oid), &[] as &[u8]).await.unwrap();

        promote_lfs_objects(&root.join("staging"), &destination)
            .await
            .unwrap();

        assert!(destination.join("e3/b0").join(oid).try_exists().unwrap());
        fs::remove_dir_all(root).await.unwrap();
    }

    #[test]
    fn public_ip_filter_rejects_internal_and_reserved_networks() {
        for address in [
            IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
            IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)),
            IpAddr::V4(Ipv4Addr::new(169, 254, 1, 1)),
            IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)),
            IpAddr::V6(Ipv6Addr::LOCALHOST),
            IpAddr::V6("fc00::1".parse().unwrap()),
            IpAddr::V6("2001:db8::1".parse().unwrap()),
        ] {
            assert!(!is_public_ip(address), "{address} must not be public");
        }
        assert!(is_public_ip(IpAddr::V4(Ipv4Addr::new(140, 82, 114, 3))));
        assert!(is_public_ip(IpAddr::V6(
            "2606:50c0:8000::153".parse().unwrap()
        )));
    }

    #[tokio::test]
    async fn https_mirror_rejects_ip_literals_before_git_runs() {
        let result = resolve_https_remote("https://127.0.0.1/repository.git").await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn https_proxy_rejects_secondary_origins() {
        let target = HttpsTarget {
            host: "allowed.example".to_owned(),
            port: 443,
            addresses: Vec::new(),
        };
        let (proxy, task) = start_https_proxy(target).await.unwrap();
        let mut client = TcpStream::connect(proxy.strip_prefix("http://").unwrap())
            .await
            .unwrap();
        client
            .write_all(b"CONNECT internal.example:443 HTTP/1.1\r\n\r\n")
            .await
            .unwrap();
        let mut response = [0_u8; 128];
        let read = client.read(&mut response).await.unwrap();

        assert!(response[..read].starts_with(b"HTTP/1.1 403 Forbidden"));
        task.abort();
    }

    mod conversion {
        use axum::{
            extract::{Path as AxumPath, State},
            http::{HeaderMap, StatusCode, header},
            response::IntoResponse,
        };
        use axum_extra::extract::cookie::CookieJar;
        use chrono::Utc;
        use sea_orm::{
            ActiveModelTrait, ColumnTrait, ConnectOptions, Database, DatabaseConnection,
            EntityTrait, QueryFilter, Set,
        };
        use sea_orm_migration::MigratorTrait;
        use uuid::Uuid;

        use crate::{
            config::{AuthSettings, StorageSettings},
            entity::{
                api_token, audit_event, issue_comment, namespace, repository, repository_issue,
                repository_mirror, repository_topic, topic, user,
            },
            identity::{IdentityState, SCOPE_WRITE, hash_secret},
            migration::Migrator,
            repository::{RepositoryState, mirrors::convert_mirror},
        };

        const TOKEN: &str = "conversion-test-token";

        struct Environment {
            _root: std::path::PathBuf,
            database: DatabaseConnection,
            state: RepositoryState,
        }

        async fn environment() -> Environment {
            let root =
                std::env::temp_dir().join(format!("gitadel-mirror-conversion-{}", Uuid::new_v4()));
            let mut options = ConnectOptions::new("sqlite::memory:");
            options.max_connections(1);
            options.sqlx_logging(false);
            let database = Database::connect(options).await.unwrap();
            Migrator::up(&database, None).await.unwrap();

            let public_url = url::Url::parse("https://gitadel.test").unwrap();
            let identity = IdentityState::new(
                database.clone(),
                AuthSettings {
                    session_lifetime_hours: 24,
                    invitation_lifetime_hours: 24,
                },
                public_url.clone(),
            )
            .unwrap();
            let state = RepositoryState::new(
                identity,
                StorageSettings {
                    repository_root: root.join("repositories"),
                    lfs_root: root.join("lfs"),
                    actions_artifact_root: root.join("actions-artifacts"),
                },
                public_url,
                22,
            )
            .await
            .unwrap();
            Environment {
                _root: root,
                database,
                state,
            }
        }

        async fn seed_account(database: &DatabaseConnection) -> user::Model {
            let now = Utc::now();
            let account = user::ActiveModel {
                id: Set(Uuid::new_v4()),
                username: Set("alice".to_owned()),
                password_hash: Set("unused".to_owned()),
                is_admin: Set(true),
                default_repository_visibility: Set("private".to_owned()),
                theme_preference: Set("system".to_owned()),
                disabled_at: Set(None),
                avatar_updated_at: Set(None),
                created_at: Set(now),
                updated_at: Set(now),
            }
            .insert(database)
            .await
            .unwrap();
            namespace::ActiveModel {
                slug: Set("alice".to_owned()),
                kind: Set("user".to_owned()),
                user_id: Set(Some(account.id)),
                organization_id: Set(None),
                created_at: Set(now),
            }
            .insert(database)
            .await
            .unwrap();
            api_token::ActiveModel {
                id: Set(Uuid::new_v4()),
                user_id: Set(account.id),
                name: Set("conversion test".to_owned()),
                token_hash: Set(hash_secret(TOKEN)),
                scopes: Set(SCOPE_WRITE),
                expires_at: Set(None),
                created_at: Set(now),
                last_used_at: Set(None),
                revoked_at: Set(None),
            }
            .insert(database)
            .await
            .unwrap();
            account
        }

        async fn seed_repository(
            database: &DatabaseConnection,
            account: &user::Model,
            name: &str,
            mirrored: bool,
        ) -> repository::Model {
            let now = Utc::now();
            repository::ActiveModel {
                id: Set(Uuid::new_v4()),
                namespace: Set("alice".to_owned()),
                name: Set(name.to_owned()),
                description: Set(Some("mirrored repository".to_owned())),
                website_url: Set(Some("https://github.com/octocat/hello-world".to_owned())),
                visibility: Set("private".to_owned()),
                object_format: Set("sha1".to_owned()),
                mirrored: Set(mirrored),
                default_branch: Set("main".to_owned()),
                issue_counter: Set(1),
                storage_key: Set(Uuid::new_v4()),
                created_by: Set(account.id),
                archived_at: Set(None),
                deleted_at: Set(None),
                created_at: Set(now),
                updated_at: Set(now),
            }
            .insert(database)
            .await
            .unwrap()
        }

        async fn seed_mirror(
            database: &DatabaseConnection,
            repository: &repository::Model,
            account: &user::Model,
        ) -> repository_issue::Model {
            let now = Utc::now();
            repository_mirror::ActiveModel {
                repository_id: Set(repository.id),
                remote_url: Set("https://github.com/octocat/hello-world.git".to_owned()),
                identity_id: Set(None),
                github_owner: Set(Some("octocat".to_owned())),
                github_repository: Set(Some("hello-world".to_owned())),
                schedule: Set(Some("0 * * * *".to_owned())),
                last_attempted_at: Set(Some(now)),
                last_synced_at: Set(Some(now)),
                last_error: Set(None),
                metadata_last_synced_at: Set(Some(now)),
                metadata_error: Set(None),
                next_sync_at: Set(Some(now)),
                created_at: Set(now),
                updated_at: Set(now),
            }
            .insert(database)
            .await
            .unwrap();
            let issue = repository_issue::ActiveModel {
                id: Set(Uuid::new_v4()),
                repository_id: Set(repository.id),
                number: Set(1),
                author_user_id: Set(account.id),
                title: Set("Imported issue".to_owned()),
                body: Set("Kept body".to_owned()),
                state: Set("open".to_owned()),
                assignee_user_id: Set(None),
                created_at: Set(now),
                updated_at: Set(now),
                closed_at: Set(None),
                external_source: Set(Some("github".to_owned())),
                external_id: Set(Some("42".to_owned())),
                external_url: Set(Some(
                    "https://github.com/octocat/hello-world/issues/42".to_owned(),
                )),
                external_author: Set(Some("octocat".to_owned())),
                external_author_url: Set(Some("https://github.com/octocat".to_owned())),
                external_updated_at: Set(Some(now)),
            }
            .insert(database)
            .await
            .unwrap();
            issue_comment::ActiveModel {
                id: Set(Uuid::new_v4()),
                issue_id: Set(issue.id),
                author_user_id: Set(account.id),
                body: Set("Kept comment".to_owned()),
                created_at: Set(now),
                updated_at: Set(now),
                external_source: Set(Some("github".to_owned())),
                external_id: Set(Some("7".to_owned())),
                external_url: Set(Some(
                    "https://github.com/octocat/hello-world/issues/42#issuecomment-7".to_owned(),
                )),
                external_author: Set(Some("octocat".to_owned())),
                external_author_url: Set(Some("https://github.com/octocat".to_owned())),
                external_updated_at: Set(Some(now)),
            }
            .insert(database)
            .await
            .unwrap();
            let imported_topic = topic::ActiveModel {
                id: Set(Uuid::new_v4()),
                name: Set("hacktoberfest".to_owned()),
                created_at: Set(now),
            }
            .insert(database)
            .await
            .unwrap();
            repository_topic::ActiveModel {
                repository_id: Set(repository.id),
                topic_id: Set(imported_topic.id),
                created_at: Set(now),
                external_source: Set(Some("github".to_owned())),
            }
            .insert(database)
            .await
            .unwrap();
            issue
        }

        fn bearer_headers() -> HeaderMap {
            let mut headers = HeaderMap::new();
            headers.insert(
                header::AUTHORIZATION,
                format!("Bearer {TOKEN}").parse().unwrap(),
            );
            headers
        }

        async fn request_conversion(
            state: &RepositoryState,
            name: &str,
        ) -> Result<StatusCode, crate::identity::ApiError> {
            convert_mirror(
                State(state.clone()),
                AxumPath(("alice".to_owned(), name.to_owned())),
                bearer_headers(),
                CookieJar::new(),
            )
            .await
        }

        #[tokio::test]
        async fn convert_removes_mirror_row_and_detaches_imported_metadata() {
            let environment = environment().await;
            let account = seed_account(&environment.database).await;
            let repository =
                seed_repository(&environment.database, &account, "upstream", true).await;
            let issue = seed_mirror(&environment.database, &repository, &account).await;

            let response = request_conversion(&environment.state, "upstream").await;

            assert_eq!(response.unwrap(), StatusCode::NO_CONTENT);

            let mirror = repository_mirror::Entity::find_by_id(repository.id)
                .one(&environment.database)
                .await
                .unwrap();
            assert!(mirror.is_none(), "the mirror row must be removed");

            let converted = repository::Entity::find_by_id(repository.id)
                .one(&environment.database)
                .await
                .unwrap()
                .unwrap();
            assert!(!converted.mirrored, "the repository must become writable");
            assert_eq!(
                converted.description.as_deref(),
                Some("mirrored repository")
            );
            assert_eq!(
                converted.website_url.as_deref(),
                Some("https://github.com/octocat/hello-world")
            );

            let detached = repository_issue::Entity::find_by_id(issue.id)
                .one(&environment.database)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(detached.title, "Imported issue");
            assert_eq!(detached.body, "Kept body");
            assert_eq!(detached.external_source, None);
            assert_eq!(detached.external_id, None);
            assert_eq!(detached.external_url, None);
            assert_eq!(detached.external_updated_at, None);
            assert_eq!(
                detached.external_author.as_deref(),
                Some("octocat"),
                "external author attribution is preserved as provenance"
            );
            assert_eq!(
                detached.external_author_url.as_deref(),
                Some("https://github.com/octocat")
            );

            let comments = issue_comment::Entity::find()
                .filter(issue_comment::Column::IssueId.eq(issue.id))
                .all(&environment.database)
                .await
                .unwrap();
            assert_eq!(comments.len(), 1);
            assert_eq!(comments[0].body, "Kept comment");
            assert_eq!(comments[0].external_source, None);
            assert_eq!(comments[0].external_id, None);
            assert_eq!(comments[0].external_url, None);
            assert_eq!(comments[0].external_updated_at, None);
            assert_eq!(
                comments[0].external_author.as_deref(),
                Some("octocat"),
                "comment author attribution is preserved as provenance"
            );

            let topic_rows = repository_topic::Entity::find()
                .filter(repository_topic::Column::RepositoryId.eq(repository.id))
                .all(&environment.database)
                .await
                .unwrap();
            assert_eq!(topic_rows.len(), 1);
            assert_eq!(topic_rows[0].external_source, None);
            assert!(
                topic::Entity::find_by_id(topic_rows[0].topic_id)
                    .one(&environment.database)
                    .await
                    .unwrap()
                    .is_some(),
                "the topic itself remains"
            );

            let events = audit_event::Entity::find()
                .filter(audit_event::Column::Action.eq("repository.mirror.convert"))
                .all(&environment.database)
                .await
                .unwrap();
            assert_eq!(events.len(), 1);

            tokio::fs::remove_dir_all(&environment._root).await.ok();
        }

        #[tokio::test]
        async fn convert_conflicts_while_the_repository_is_synchronizing() {
            let environment = environment().await;
            let account = seed_account(&environment.database).await;
            let repository =
                seed_repository(&environment.database, &account, "upstream", true).await;
            seed_mirror(&environment.database, &repository, &account).await;
            environment
                .state
                .mirror_syncing
                .lock()
                .await
                .insert(repository.id);

            let error = request_conversion(&environment.state, "upstream")
                .await
                .unwrap_err();

            assert_eq!(error.into_response().status(), StatusCode::CONFLICT);
            let mirror = repository_mirror::Entity::find_by_id(repository.id)
                .one(&environment.database)
                .await
                .unwrap();
            assert!(
                mirror.is_some(),
                "conversion must not proceed while synchronizing"
            );

            tokio::fs::remove_dir_all(&environment._root).await.ok();
        }

        #[tokio::test]
        async fn convert_returns_not_found_for_a_standard_repository() {
            let environment = environment().await;
            let account = seed_account(&environment.database).await;
            let standard = seed_repository(&environment.database, &account, "local", false).await;

            let error = request_conversion(&environment.state, "local")
                .await
                .unwrap_err();

            assert_eq!(error.into_response().status(), StatusCode::NOT_FOUND);
            assert!(
                repository_mirror::Entity::find_by_id(standard.id)
                    .one(&environment.database)
                    .await
                    .unwrap()
                    .is_none()
            );

            tokio::fs::remove_dir_all(&environment._root).await.ok();
        }
    }
}
