use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use chrono::{DateTime, Utc};
use reqwest::header::{ACCEPT, AUTHORIZATION, HeaderMap, HeaderValue, USER_AGENT};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, IntoActiveModel, ModelTrait, QueryFilter, Set,
    TransactionTrait, sea_query::OnConflict,
};
use serde::{Deserialize, de::DeserializeOwned};
use url::Url;
use uuid::Uuid;

use super::RepositoryState;
use crate::{
    entity::{issue_comment, repository, repository_issue, repository_topic, topic},
    identity::ApiError,
};

const SOURCE: &str = "github";
const PAGE_SIZE: usize = 100;
const KNOWN_HOSTS_CACHE_LIFETIME: Duration = Duration::from_secs(60 * 60);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct GithubRepository {
    pub owner: String,
    pub name: String,
}

pub(super) fn identify(remote_url: &str) -> Option<GithubRepository> {
    if let Some(path) = remote_url.strip_prefix("git@github.com:") {
        return coordinates(path);
    }
    let url = Url::parse(remote_url).ok()?;
    if !url
        .host_str()
        .is_some_and(|host| host.eq_ignore_ascii_case("github.com"))
    {
        return None;
    }
    coordinates(url.path())
}

pub(super) async fn synchronize(
    state: &RepositoryState,
    repository: &repository::Model,
    source: &GithubRepository,
    api_token: Option<&str>,
) -> Result<(), ApiError> {
    let client = GithubClient::new(state.http_client(), api_token)?;
    let repository_metadata: GithubRepositoryResponse = client
        .get(&format!("/repos/{}/{}", source.owner, source.name))
        .await?;
    // The issues endpoint defaults to `state=open`; closed issues must be
    // fetched too or they would be dropped as stale on the next sync.
    let issues: Vec<GithubIssue> = client
        .get_all::<GithubIssue>(&format!(
            "/repos/{}/{}/issues?state=all",
            source.owner, source.name
        ))
        .await?
        .into_iter()
        .filter(|issue| issue.pull_request.is_none())
        .collect();
    let comments: Vec<GithubComment> = client
        .get_all(&format!(
            "/repos/{}/{}/issues/comments",
            source.owner, source.name
        ))
        .await?;

    persist(state, repository, repository_metadata, issues, comments).await
}

pub(super) async fn known_hosts(state: &RepositoryState) -> Result<String, ApiError> {
    let mut cache = state.github_known_hosts.lock().await;
    if let Some((known_hosts, fetched_at)) = cache.as_ref()
        && fetched_at.elapsed() < KNOWN_HOSTS_CACHE_LIFETIME
    {
        return Ok(known_hosts.clone());
    }

    let refreshed = fetch_known_hosts(state).await;
    match refreshed {
        Ok(known_hosts) => {
            *cache = Some((known_hosts.clone(), Instant::now()));
            Ok(known_hosts)
        }
        Err(error) => {
            if let Some((known_hosts, _)) = cache.as_ref() {
                tracing::warn!(%error, "could not refresh GitHub SSH host keys; using cached keys");
                Ok(known_hosts.clone())
            } else {
                Err(error)
            }
        }
    }
}

async fn fetch_known_hosts(state: &RepositoryState) -> Result<String, ApiError> {
    #[derive(Deserialize)]
    struct GithubMeta {
        ssh_keys: Vec<String>,
    }

    let meta: GithubMeta = GithubClient::new(state.http_client(), None)?
        .get("/meta")
        .await?;
    if meta.ssh_keys.is_empty() {
        return Err(ApiError::internal("GitHub returned no SSH host keys"));
    }
    let mut known_hosts = String::new();
    for key in meta.ssh_keys {
        known_hosts.push_str("github.com ");
        known_hosts.push_str(&key);
        known_hosts.push('\n');
    }
    Ok(known_hosts)
}

struct GithubClient<'a> {
    client: &'a reqwest::Client,
    headers: HeaderMap,
}

impl<'a> GithubClient<'a> {
    fn new(client: &'a reqwest::Client, api_token: Option<&str>) -> Result<Self, ApiError> {
        let mut headers = HeaderMap::new();
        headers.insert(USER_AGENT, HeaderValue::from_static("gitadel-mirror"));
        headers.insert(
            ACCEPT,
            HeaderValue::from_static("application/vnd.github+json"),
        );
        headers.insert(
            "x-github-api-version",
            HeaderValue::from_static("2022-11-28"),
        );
        if let Some(token) = api_token {
            let value = HeaderValue::from_str(&format!("Bearer {token}"))
                .map_err(|_| ApiError::bad_request("The selected GitHub token is invalid."))?;
            headers.insert(AUTHORIZATION, value);
        }
        Ok(Self { client, headers })
    }

    async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, ApiError> {
        let response = self
            .client
            .get(format!("https://api.github.com{path}"))
            .headers(self.headers.clone())
            .send()
            .await
            .map_err(|error| ApiError::internal(format!("GitHub request failed: {error}")))?;
        let status = response.status();
        if !status.is_success() {
            return Err(ApiError::bad_request(format!(
                "GitHub metadata request failed with HTTP {status}."
            )));
        }
        response
            .json()
            .await
            .map_err(|error| ApiError::internal(format!("invalid GitHub response: {error}")))
    }

    async fn get_all<T: DeserializeOwned>(&self, path: &str) -> Result<Vec<T>, ApiError> {
        let mut page = 1_u64;
        let mut all = Vec::new();
        loop {
            let separator = if path.contains('?') { '&' } else { '?' };
            let batch: Vec<T> = self
                .get(&format!(
                    "{path}{separator}per_page={PAGE_SIZE}&page={page}"
                ))
                .await?;
            let complete = batch.len() < PAGE_SIZE;
            all.extend(batch);
            if complete {
                return Ok(all);
            }
            page = page
                .checked_add(1)
                .ok_or_else(|| ApiError::internal("GitHub pagination overflow"))?;
        }
    }
}

#[derive(Deserialize)]
struct GithubRepositoryResponse {
    homepage: Option<String>,
    #[serde(default)]
    topics: Vec<String>,
}

#[derive(Deserialize)]
struct GithubUser {
    login: String,
    html_url: String,
}

#[derive(Deserialize)]
struct GithubIssue {
    id: i64,
    number: i64,
    title: String,
    body: Option<String>,
    state: String,
    html_url: String,
    user: Option<GithubUser>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    closed_at: Option<DateTime<Utc>>,
    pull_request: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct GithubComment {
    id: i64,
    issue_url: String,
    html_url: String,
    body: Option<String>,
    user: Option<GithubUser>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

async fn persist(
    state: &RepositoryState,
    repository: &repository::Model,
    metadata: GithubRepositoryResponse,
    issues: Vec<GithubIssue>,
    comments: Vec<GithubComment>,
) -> Result<(), ApiError> {
    let transaction = state.identity().database().begin().await?;
    let mut active_repository = repository.clone().into_active_model();
    active_repository.website_url = Set(normalize_homepage(metadata.homepage));
    active_repository.issue_counter = Set(issues
        .iter()
        .map(|issue| issue.number)
        .max()
        .unwrap_or(repository.issue_counter)
        .max(repository.issue_counter));
    active_repository.updated_at = Set(Utc::now());
    active_repository.update(&transaction).await?;

    synchronize_topics(repository.id, metadata.topics, &transaction).await?;
    let issue_ids = synchronize_issues(repository, issues, &transaction).await?;
    synchronize_comments(repository, issue_ids, comments, &transaction).await?;
    transaction.commit().await?;
    Ok(())
}

async fn synchronize_topics(
    repository_id: Uuid,
    topic_names: Vec<String>,
    connection: &impl sea_orm::ConnectionTrait,
) -> Result<(), ApiError> {
    repository_topic::Entity::delete_many()
        .filter(repository_topic::Column::RepositoryId.eq(repository_id))
        .filter(repository_topic::Column::ExternalSource.eq(SOURCE))
        .exec(connection)
        .await?;
    let now = Utc::now();
    for name in topic_names {
        let name = name.trim().to_ascii_lowercase();
        if name.is_empty() {
            continue;
        }
        topic::Entity::insert(topic::ActiveModel {
            id: Set(Uuid::new_v4()),
            name: Set(name.clone()),
            created_at: Set(now),
        })
        .on_conflict(
            OnConflict::column(topic::Column::Name)
                .do_nothing()
                .to_owned(),
        )
        .exec(connection)
        .await?;
        let topic = topic::Entity::find()
            .filter(topic::Column::Name.eq(name))
            .one(connection)
            .await?
            .ok_or_else(|| ApiError::internal("GitHub topic was not persisted"))?;
        repository_topic::Entity::insert(repository_topic::ActiveModel {
            repository_id: Set(repository_id),
            topic_id: Set(topic.id),
            created_at: Set(now),
            external_source: Set(Some(SOURCE.to_owned())),
        })
        .on_conflict(
            OnConflict::columns([
                repository_topic::Column::RepositoryId,
                repository_topic::Column::TopicId,
            ])
            .do_nothing()
            .to_owned(),
        )
        .exec(connection)
        .await?;
    }
    Ok(())
}

async fn synchronize_issues(
    repository: &repository::Model,
    issues: Vec<GithubIssue>,
    connection: &impl sea_orm::ConnectionTrait,
) -> Result<HashMap<i64, Uuid>, ApiError> {
    let existing = repository_issue::Entity::find()
        .filter(repository_issue::Column::RepositoryId.eq(repository.id))
        .filter(repository_issue::Column::ExternalSource.eq(SOURCE))
        .all(connection)
        .await?;
    let mut existing_by_id: HashMap<String, repository_issue::Model> = existing
        .into_iter()
        .filter_map(|issue| issue.external_id.clone().map(|id| (id, issue)))
        .collect();
    let mut issue_ids = HashMap::new();

    for issue in issues {
        let external_id = issue.id.to_string();
        let author = issue.user.as_ref();
        let saved = if let Some(current) = existing_by_id.remove(&external_id) {
            let mut active = current.into_active_model();
            active.number = Set(issue.number);
            active.title = Set(issue.title);
            active.body = Set(issue.body.unwrap_or_default());
            active.state = Set(issue.state);
            active.updated_at = Set(issue.updated_at);
            active.closed_at = Set(issue.closed_at);
            active.external_url = Set(Some(issue.html_url));
            active.external_author = Set(author.map(|user| user.login.clone()));
            active.external_author_url = Set(author.map(|user| user.html_url.clone()));
            active.external_updated_at = Set(Some(issue.updated_at));
            active.update(connection).await?
        } else {
            if repository_issue::Entity::find()
                .filter(repository_issue::Column::RepositoryId.eq(repository.id))
                .filter(repository_issue::Column::Number.eq(issue.number))
                .one(connection)
                .await?
                .is_some()
            {
                return Err(ApiError::conflict(format!(
                    "GitHub issue #{} conflicts with an existing local issue.",
                    issue.number
                )));
            }
            repository_issue::ActiveModel {
                id: Set(Uuid::new_v4()),
                repository_id: Set(repository.id),
                number: Set(issue.number),
                author_user_id: Set(repository.created_by),
                title: Set(issue.title),
                body: Set(issue.body.unwrap_or_default()),
                state: Set(issue.state),
                assignee_user_id: Set(None),
                created_at: Set(issue.created_at),
                updated_at: Set(issue.updated_at),
                closed_at: Set(issue.closed_at),
                external_source: Set(Some(SOURCE.to_owned())),
                external_id: Set(Some(external_id)),
                external_url: Set(Some(issue.html_url)),
                external_author: Set(author.map(|user| user.login.clone())),
                external_author_url: Set(author.map(|user| user.html_url.clone())),
                external_updated_at: Set(Some(issue.updated_at)),
            }
            .insert(connection)
            .await?
        };
        issue_ids.insert(issue.number, saved.id);
    }

    for stale in existing_by_id.into_values() {
        stale.delete(connection).await?;
    }
    Ok(issue_ids)
}

async fn synchronize_comments(
    repository: &repository::Model,
    issue_ids: HashMap<i64, Uuid>,
    comments: Vec<GithubComment>,
    connection: &impl sea_orm::ConnectionTrait,
) -> Result<(), ApiError> {
    let imported_issue_ids: Vec<Uuid> = issue_ids.values().copied().collect();
    let existing = if imported_issue_ids.is_empty() {
        Vec::new()
    } else {
        issue_comment::Entity::find()
            .filter(issue_comment::Column::IssueId.is_in(imported_issue_ids))
            .filter(issue_comment::Column::ExternalSource.eq(SOURCE))
            .all(connection)
            .await?
    };
    let mut existing_by_id: HashMap<String, issue_comment::Model> = existing
        .into_iter()
        .filter_map(|comment| comment.external_id.clone().map(|id| (id, comment)))
        .collect();

    for comment in comments {
        let Some(issue_number) = issue_number(&comment.issue_url) else {
            continue;
        };
        let Some(issue_id) = issue_ids.get(&issue_number).copied() else {
            continue;
        };
        let external_id = comment.id.to_string();
        let author = comment.user.as_ref();
        if let Some(current) = existing_by_id.remove(&external_id) {
            let mut active = current.into_active_model();
            active.issue_id = Set(issue_id);
            active.body = Set(comment.body.unwrap_or_default());
            active.updated_at = Set(comment.updated_at);
            active.external_url = Set(Some(comment.html_url));
            active.external_author = Set(author.map(|user| user.login.clone()));
            active.external_author_url = Set(author.map(|user| user.html_url.clone()));
            active.external_updated_at = Set(Some(comment.updated_at));
            active.update(connection).await?;
        } else {
            issue_comment::ActiveModel {
                id: Set(Uuid::new_v4()),
                issue_id: Set(issue_id),
                author_user_id: Set(repository.created_by),
                body: Set(comment.body.unwrap_or_default()),
                created_at: Set(comment.created_at),
                updated_at: Set(comment.updated_at),
                external_source: Set(Some(SOURCE.to_owned())),
                external_id: Set(Some(external_id)),
                external_url: Set(Some(comment.html_url)),
                external_author: Set(author.map(|user| user.login.clone())),
                external_author_url: Set(author.map(|user| user.html_url.clone())),
                external_updated_at: Set(Some(comment.updated_at)),
            }
            .insert(connection)
            .await?;
        }
    }
    for stale in existing_by_id.into_values() {
        stale.delete(connection).await?;
    }
    Ok(())
}

fn coordinates(path: &str) -> Option<GithubRepository> {
    let path = path.trim_start_matches('/').trim_end_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let mut parts = path.split('/');
    let owner = parts.next()?.trim();
    let name = parts.next()?.trim();
    if owner.is_empty() || name.is_empty() || parts.next().is_some() {
        return None;
    }
    Some(GithubRepository {
        owner: owner.to_owned(),
        name: name.to_owned(),
    })
}

fn issue_number(issue_url: &str) -> Option<i64> {
    issue_url.rsplit('/').next()?.parse().ok()
}

fn normalize_homepage(homepage: Option<String>) -> Option<String> {
    homepage
        .map(|homepage| homepage.trim().to_owned())
        .filter(|homepage| !homepage.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_github_https_and_ssh_urls() {
        let expected = GithubRepository {
            owner: "Fractal-Tess".to_owned(),
            name: "gitadel".to_owned(),
        };
        assert_eq!(
            identify("https://github.com/Fractal-Tess/gitadel.git"),
            Some(expected.clone())
        );
        assert_eq!(
            identify("git@github.com:Fractal-Tess/gitadel.git"),
            Some(expected.clone())
        );
        assert_eq!(
            identify("ssh://git@github.com/Fractal-Tess/gitadel.git"),
            Some(expected)
        );
        assert_eq!(
            identify("https://gitlab.com/Fractal-Tess/gitadel.git"),
            None
        );
    }
}
