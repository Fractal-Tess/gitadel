use std::{path::Path, time::Duration};

use axum::{
    Json,
    extract::{Path as AxumPath, State},
    http::{HeaderMap, StatusCode},
};
use axum_extra::extract::cookie::CookieJar;
use chrono::{DateTime, Utc};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, IntoActiveModel, QueryFilter, Set,
    TransactionTrait, sea_query::Query,
};
use serde::{Deserialize, Serialize};
use tokio::fs;
use url::Url;
use uuid::Uuid;

use super::{Permission, RepositoryState, github_mirror, native_remote};
use crate::{
    blob_store::ObjectPrefix,
    entity::{
        issue_comment, namespace_mirror_identity, repository, repository_issue, repository_mirror,
        repository_topic,
    },
    identity::{ApiError, SCOPE_WRITE, load_mirror_identity_secret, mark_repository_identity_used},
    schedule,
};

const MAX_REMOTE_URL_LENGTH: usize = 2_048;
const MAX_SCHEDULE_LENGTH: usize = 255;
const MAX_ERROR_LENGTH: usize = 2_048;
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
    let native_auth = native_remote::NativeAuthentication {
        username: mirror
            .authentication
            .basic
            .as_ref()
            .map(|value| value.0.clone()),
        secret: mirror
            .authentication
            .basic
            .as_ref()
            .map(|value| value.1.clone()),
        ssh_private_key: mirror.authentication.ssh_private_key.clone(),
        ssh_known_hosts: None,
    };
    if let Some(id) = mirror.identity_id {
        mark_repository_identity_used(state.identity(), &mirror.namespace, id, Utc::now()).await?;
    }
    let initialized =
        native_remote::initialize(state, path, &mirror.remote_url, native_auth).await?;
    Ok(MirrorInitialization {
        object_format: initialized.object_format,
        default_branch: initialized.default_branch,
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
    let native_auth = native_remote::NativeAuthentication {
        username: authentication.username.clone(),
        secret: authentication.secret.clone(),
        ssh_private_key: authentication.ssh_private_key.clone(),
        ssh_known_hosts: if transport == RemoteTransport::Ssh
            && github_mirror::identify(&remote_url).is_some()
        {
            Some(github_mirror::known_hosts(state).await?)
        } else {
            None
        },
    };
    match transport {
        RemoteTransport::Https
            if authentication.username.is_none() || authentication.secret.is_none() =>
        {
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
    let initialized =
        native_remote::initialize(state, path, &remote_url, native_auth.clone()).await?;
    if let Err(error) =
        native_remote::fetch_lfs(state, storage_key, path, &remote_url, native_auth).await
    {
        cleanup_imported_lfs(state, storage_key).await;
        return Err(error);
    }
    Ok(MirrorInitialization {
        object_format: initialized.object_format,
        default_branch: initialized.default_branch,
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
    let native_auth = native_remote::NativeAuthentication {
        username: authentication.basic.as_ref().map(|value| value.0.clone()),
        secret: authentication.basic.as_ref().map(|value| value.1.clone()),
        ssh_private_key: authentication.ssh_private_key.clone(),
        ssh_known_hosts: None,
    };
    if let Some(id) = mirror.identity_id {
        mark_repository_identity_used(state.identity(), &repository.namespace, id, attempted_at)
            .await?;
    }
    let sync_result =
        native_remote::synchronize(state, &path, &mirror.remote_url, native_auth).await;
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
            active.last_error = Set(Some(truncate(&error.to_string(), MAX_ERROR_LENGTH)));
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
    state.queue_repository_analysis(repository.id).await;
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

async fn cleanup_imported_lfs(state: &RepositoryState, storage_key: Uuid) {
    let _operation_guard = state.lfs_operation_guard().await;
    let prefix = match ObjectPrefix::new(storage_key.to_string()) {
        Ok(prefix) => prefix,
        Err(error) => {
            tracing::error!(%error, "could not construct imported LFS cleanup prefix");
            return;
        }
    };
    let objects = match state.lfs_store().list(&prefix).await {
        Ok(objects) => objects,
        Err(error) => {
            tracing::error!(%error, "could not list imported LFS objects for cleanup");
            return;
        }
    };
    for object in objects {
        if let Err(error) = state.lfs_store().delete(&object.key).await {
            tracing::error!(%error, key = %object.key, "could not clean up imported LFS object");
        }
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
    use crate::network::is_public_ip;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    use super::{
        RemoteTransport, same_origin, validate_direct_import_identity, validate_import_remote_url,
        validate_remote_url,
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
                icon_updated_at: Set(None),
                icon_source: Set(None),
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
