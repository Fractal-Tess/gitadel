//! Commit statuses reported by external CI, compatible with the GitHub and
//! Gitea status APIs.
//!
//! Each (commit, context) pair keeps its latest state; posting again for the
//! same context replaces it, which is what the combined status reports.

use std::collections::HashMap;

use axum::{
    Json,
    extract::{Path as AxumPath, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use axum_extra::extract::cookie::CookieJar;
use chrono::{DateTime, Utc};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, PaginatorTrait, QueryFilter,
    QueryOrder, Set, TransactionTrait,
};
use serde::{Deserialize, Serialize};
use url::Url;
use uuid::Uuid;

use super::{Permission, RepositoryState, browser::read_git};
use crate::{
    actions::tokens,
    entity::{repository, repository_commit_status, user},
    identity::{ApiError, SCOPE_READ, SCOPE_WRITE},
};

const STATES: [&str; 4] = ["pending", "success", "error", "failure"];
const MAX_CONTEXT_BYTES: usize = 255;
const MAX_DESCRIPTION_BYTES: usize = 1024;
const MAX_TARGET_URL_BYTES: usize = 2048;
const MAX_CONTEXTS_PER_COMMIT: u64 = 1000;

#[derive(Deserialize)]
pub struct CreateStatusRequest {
    /// GitHub names the field `state`; Gitea also accepts it.
    state: String,
    target_url: Option<String>,
    description: Option<String>,
    context: Option<String>,
}

#[derive(Serialize)]
pub struct StatusCreator {
    id: Uuid,
    login: String,
    username: String,
}

#[derive(Serialize)]
pub struct StatusResponse {
    id: i64,
    url: String,
    /// GitHub's field name.
    state: String,
    /// Gitea's field name for the same value.
    status: String,
    context: String,
    description: Option<String>,
    target_url: Option<String>,
    creator: Option<StatusCreator>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(Serialize)]
pub struct CombinedStatusResponse {
    state: &'static str,
    sha: String,
    total_count: usize,
    statuses: Vec<StatusResponse>,
    commit_url: String,
    url: String,
}

pub async fn create_status(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name, revision)): AxumPath<(String, String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<CreateStatusRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let (repository, creator_id) =
        writable_repository(&state, &headers, &jar, &namespace, &name).await?;
    let status_state = request.state.trim().to_ascii_lowercase();
    if !STATES.contains(&status_state.as_str()) {
        return Err(ApiError::bad_request(
            "Commit status state must be pending, success, error, or failure.",
        ));
    }
    let context = validate_context(request.context.as_deref())?;
    let description = request
        .description
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    if description
        .as_ref()
        .is_some_and(|value| value.len() > MAX_DESCRIPTION_BYTES)
    {
        return Err(ApiError::bad_request(
            "Commit status descriptions are limited to 1024 bytes.",
        ));
    }
    let target_url = validate_target_url(request.target_url.as_deref())?;
    let sha = resolve_commit(&state, &repository, &revision).await?;

    let database = state.identity().database();
    let transaction = database.begin().await?;
    let existing = repository_commit_status::Entity::find()
        .filter(repository_commit_status::Column::RepositoryId.eq(repository.id))
        .filter(repository_commit_status::Column::Sha.eq(&sha))
        .filter(repository_commit_status::Column::Context.eq(&context))
        .one(&transaction)
        .await?;
    let now = Utc::now();
    let row = match existing {
        Some(existing) => {
            let mut active: repository_commit_status::ActiveModel = existing.into();
            active.state = Set(status_state);
            active.description = Set(description);
            active.target_url = Set(target_url);
            active.creator_id = Set(Some(creator_id));
            active.updated_at = Set(now);
            active.update(&transaction).await?
        }
        None => {
            let contexts = repository_commit_status::Entity::find()
                .filter(repository_commit_status::Column::RepositoryId.eq(repository.id))
                .filter(repository_commit_status::Column::Sha.eq(&sha))
                .count(&transaction)
                .await?;
            if contexts >= MAX_CONTEXTS_PER_COMMIT {
                return Err(ApiError::bad_request(
                    "A commit can have at most 1000 status contexts.",
                ));
            }
            repository_commit_status::ActiveModel {
                id: sea_orm::ActiveValue::NotSet,
                repository_id: Set(repository.id),
                sha: Set(sha),
                context: Set(context),
                state: Set(status_state),
                description: Set(description),
                target_url: Set(target_url),
                creator_id: Set(Some(creator_id)),
                created_at: Set(now),
                updated_at: Set(now),
            }
            .insert(&transaction)
            .await?
        }
    };
    transaction.commit().await?;
    let mut responses = status_responses(&state, &repository, vec![row]).await?;
    let response = responses
        .pop()
        .ok_or_else(|| ApiError::internal("created commit status vanished"))?;
    Ok((StatusCode::CREATED, Json(response)))
}

pub async fn list_statuses(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name, revision)): AxumPath<(String, String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<Vec<StatusResponse>>, ApiError> {
    let repository = readable_repository(&state, &headers, &jar, &namespace, &name).await?;
    let sha = resolve_commit(&state, &repository, &revision).await?;
    let rows = statuses_for(state.identity().database(), repository.id, &sha).await?;
    status_responses(&state, &repository, rows).await.map(Json)
}

pub async fn combined_status(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name, revision)): AxumPath<(String, String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<CombinedStatusResponse>, ApiError> {
    let repository = readable_repository(&state, &headers, &jar, &namespace, &name).await?;
    let sha = resolve_commit(&state, &repository, &revision).await?;
    let rows = statuses_for(state.identity().database(), repository.id, &sha).await?;
    let combined = combined_state(rows.iter().map(|row| row.state.as_str()));
    let statuses = status_responses(&state, &repository, rows).await?;
    let base = api_base(&state, &repository);
    Ok(Json(CombinedStatusResponse {
        state: combined,
        total_count: statuses.len(),
        statuses,
        commit_url: format!("{base}/commits/{sha}"),
        url: format!("{base}/commits/{sha}/status"),
        sha,
    }))
}

/// The latest status per context for each commit, newest first.
pub(crate) async fn statuses_for_commits<C: ConnectionTrait>(
    database: &C,
    repository_id: Uuid,
    shas: &[&str],
) -> Result<HashMap<String, Vec<repository_commit_status::Model>>, ApiError> {
    let rows = repository_commit_status::Entity::find()
        .filter(repository_commit_status::Column::RepositoryId.eq(repository_id))
        .filter(repository_commit_status::Column::Sha.is_in(shas.iter().copied()))
        .order_by_desc(repository_commit_status::Column::UpdatedAt)
        .all(database)
        .await?;
    let mut statuses: HashMap<String, Vec<_>> = HashMap::new();
    for row in rows {
        statuses.entry(row.sha.clone()).or_default().push(row);
    }
    Ok(statuses)
}

async fn statuses_for<C: ConnectionTrait>(
    database: &C,
    repository_id: Uuid,
    sha: &str,
) -> Result<Vec<repository_commit_status::Model>, ApiError> {
    Ok(statuses_for_commits(database, repository_id, &[sha])
        .await?
        .remove(sha)
        .unwrap_or_default())
}

/// GitHub's combined state: any error or failure fails, any pending (or no
/// status at all) is pending, and otherwise every context succeeded.
pub(crate) fn combined_state<'a>(states: impl Iterator<Item = &'a str>) -> &'static str {
    let mut any = false;
    let mut pending = false;
    for state in states {
        any = true;
        match state {
            "error" | "failure" => return "failure",
            "pending" => pending = true,
            _ => {}
        }
    }
    if pending || !any {
        "pending"
    } else {
        "success"
    }
}

fn validate_context(value: Option<&str>) -> Result<String, ApiError> {
    let value = value.map(str::trim).filter(|value| !value.is_empty());
    let value = value.unwrap_or("default");
    if value.len() > MAX_CONTEXT_BYTES || value.chars().any(char::is_control) {
        return Err(ApiError::bad_request(
            "Commit status contexts must be up to 255 printable characters.",
        ));
    }
    Ok(value.to_owned())
}

fn validate_target_url(value: Option<&str>) -> Result<Option<String>, ApiError> {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let valid = value.len() <= MAX_TARGET_URL_BYTES
        && Url::parse(value).is_ok_and(|url| matches!(url.scheme(), "http" | "https"));
    if !valid {
        return Err(ApiError::bad_request(
            "Commit status target URLs must be http(s) URLs up to 2048 characters.",
        ));
    }
    Ok(Some(value.to_owned()))
}

async fn resolve_commit(
    state: &RepositoryState,
    repository: &repository::Model,
    revision: &str,
) -> Result<String, ApiError> {
    if revision.is_empty() || revision.len() > 255 || revision.chars().any(char::is_control) {
        return Err(ApiError::not_found());
    }
    let revision = revision.to_owned();
    read_git(state.repository_path(repository), move |git| {
        let oid = git.peel_to_commit_oid(git.rev_parse(&revision)?)?;
        Ok(oid.to_hex())
    })
    .await
    .map_err(|_| ApiError::not_found())
}

async fn status_responses(
    state: &RepositoryState,
    repository: &repository::Model,
    rows: Vec<repository_commit_status::Model>,
) -> Result<Vec<StatusResponse>, ApiError> {
    let creator_ids = rows.iter().filter_map(|row| row.creator_id);
    let creators = user::Entity::find()
        .filter(user::Column::Id.is_in(creator_ids))
        .all(state.identity().database())
        .await?
        .into_iter()
        .map(|account| (account.id, account.username))
        .collect::<HashMap<_, _>>();
    let base = api_base(state, repository);
    Ok(rows
        .into_iter()
        .map(|row| StatusResponse {
            id: row.id,
            url: format!("{base}/statuses/{}", row.sha),
            status: row.state.clone(),
            state: row.state,
            context: row.context,
            description: row.description,
            target_url: row.target_url,
            creator: row.creator_id.and_then(|id| {
                creators.get(&id).map(|username| StatusCreator {
                    id,
                    login: username.clone(),
                    username: username.clone(),
                })
            }),
            created_at: row.created_at,
            updated_at: row.updated_at,
        })
        .collect())
}

fn api_base(state: &RepositoryState, repository: &repository::Model) -> String {
    format!(
        "{}/api/v1/repos/{}/{}",
        state.public_url().as_str().trim_end_matches('/'),
        repository.namespace,
        repository.name
    )
}

/// Posting requires write access through a write-scoped token or session,
/// or an Actions job token for this repository. Mirrors accept statuses so
/// CI can report on mirrored code; archived repositories do not.
async fn writable_repository(
    state: &RepositoryState,
    headers: &HeaderMap,
    jar: &CookieJar,
    namespace: &str,
    name: &str,
) -> Result<(repository::Model, Uuid), ApiError> {
    let repository = state.find(namespace, name).await?;
    let actor_id = if let Some(job) =
        tokens::authenticate_job_api_token(state.identity().database(), headers).await?
    {
        if job.repository.id != repository.id {
            return Err(ApiError::forbidden(
                "The Actions token belongs to another repository.",
            ));
        }
        job.run
            .actor_id
            .ok_or_else(|| ApiError::forbidden("The workflow run has no actor."))?
    } else {
        let actor = state
            .identity()
            .authenticate(headers, jar, SCOPE_WRITE)
            .await?;
        if !state
            .can_access(&repository, Some(actor.user.id), Permission::Write)
            .await?
        {
            return Err(ApiError::not_found());
        }
        actor.user.id
    };
    if repository.archived_at.is_some() {
        return Err(ApiError::forbidden("Archived repositories are read-only."));
    }
    Ok((repository, actor_id))
}

async fn readable_repository(
    state: &RepositoryState,
    headers: &HeaderMap,
    jar: &CookieJar,
    namespace: &str,
    name: &str,
) -> Result<repository::Model, ApiError> {
    let repository = state.find(namespace, name).await?;
    let user_id = state
        .identity()
        .optional_user(headers, jar, SCOPE_READ)
        .await?
        .map(|actor| actor.id);
    state
        .authorize(&repository, user_id, Permission::Read)
        .await?;
    Ok(repository)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn combined_state_follows_github_precedence() {
        assert_eq!(combined_state([].into_iter()), "pending");
        assert_eq!(combined_state(["success"].into_iter()), "success");
        assert_eq!(
            combined_state(["success", "pending"].into_iter()),
            "pending"
        );
        assert_eq!(
            combined_state(["pending", "error", "success"].into_iter()),
            "failure"
        );
        assert_eq!(
            combined_state(["success", "failure"].into_iter()),
            "failure"
        );
    }

    #[test]
    fn status_inputs_are_validated() {
        assert_eq!(validate_context(None).unwrap(), "default");
        assert_eq!(validate_context(Some("  ci/build ")).unwrap(), "ci/build");
        assert!(validate_context(Some("bad\ncontext")).is_err());
        assert_eq!(validate_target_url(Some("")).unwrap(), None);
        assert!(validate_target_url(Some("https://ci.example/run/1")).is_ok());
        assert!(validate_target_url(Some("javascript:alert(1)")).is_err());
    }
}
