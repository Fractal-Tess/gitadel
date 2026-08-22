use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Stdio,
};

use axum::{
    Json,
    body::Body,
    extract::{Path as AxumPath, Query, State},
    http::{HeaderMap, HeaderValue, header},
    response::Response,
};
use axum_extra::extract::cookie::CookieJar;
use chrono::{DateTime, Duration, NaiveDate, Utc};
use comrak::{Options, markdown_to_html};
use serde::{Deserialize, Serialize};
use sley::{
    GitError, GitObjectType, ObjectId, ReachableCommitOptions, ReferenceTarget,
    Repository as GitRepository, StreamControl, TagQueryOptions,
};
use tokei::{Config as TokeiConfig, LanguageType};
use tokio::{process::Command, task::JoinSet};

use super::{
    Permission, RepositoryState,
    resources::{self, accessible_repositories},
};
use crate::{
    entity::repository,
    identity::{ApiError, SCOPE_READ},
};

const MAX_TEXT_BLOB_BYTES: usize = 2 * 1024 * 1024;
const MAX_DIFF_BYTES: usize = 5 * 1024 * 1024;
const DEFAULT_REPOSITORY_ACTIVITY_DAYS: u16 = 14;
const MAX_REPOSITORY_ACTIVITY_DAYS: u16 = 365;
const DEFAULT_OVERVIEW_PER_PAGE: usize = 20;
const MAX_OVERVIEW_PER_PAGE: usize = 50;

#[derive(Deserialize)]
pub struct BrowseQuery {
    rev: Option<String>,
    #[serde(default)]
    path: String,
}

#[derive(Deserialize)]
pub struct HistoryQuery {
    rev: Option<String>,
    #[serde(default = "default_page")]
    page: usize,
    #[serde(default = "default_per_page")]
    per_page: usize,
}

#[derive(Deserialize)]
pub struct OverviewQuery {
    #[serde(default = "default_page")]
    page: usize,
    #[serde(default = "default_overview_per_page")]
    per_page: usize,
}

#[derive(Deserialize)]
pub struct ActivityQuery {
    #[serde(default = "default_repository_activity_days")]
    days: u16,
}

#[derive(Serialize)]
pub struct RefResponse {
    name: String,
    oid: String,
}

#[derive(Serialize)]
pub struct RefsResponse {
    branches: Vec<RefResponse>,
    tags: Vec<RefResponse>,
    size_bytes: Option<u64>,
}

#[derive(Serialize)]
pub struct TreeResponse {
    revision: String,
    commit_oid: String,
    commit_count: Option<usize>,
    path: String,
    entries: Vec<TreeEntryResponse>,
}

#[derive(Serialize)]
pub struct TreeEntryResponse {
    name: String,
    path: String,
    oid: String,
    kind: &'static str,
    mode: u32,
    size: Option<u64>,
}

#[derive(Serialize)]
pub struct BlobResponse {
    revision: String,
    commit_oid: String,
    path: String,
    oid: String,
    size: usize,
    binary: bool,
    too_large: bool,
    content: Option<String>,
    rendered_html: Option<String>,
}

#[derive(Serialize)]
pub struct HistoryResponse {
    commits: Vec<CommitResponse>,
    page: usize,
    per_page: usize,
    has_next: bool,
}

#[derive(Serialize)]
pub struct CommitResponse {
    oid: String,
    short_oid: String,
    tree_oid: String,
    parents: Vec<String>,
    author: SignatureResponse,
    committer: SignatureResponse,
    title: String,
    message: String,
}

#[derive(Serialize)]
pub struct SignatureResponse {
    name: String,
    email: String,
    timestamp: i64,
    timezone_offset_minutes: i16,
}

#[derive(Serialize)]
pub struct DiffResponse {
    patch: String,
    truncated: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct LanguageStatResponse {
    language: String,
    files: usize,
    code: usize,
    comments: usize,
    blanks: usize,
}

impl LanguageStatResponse {
    const fn non_blank_lines(&self) -> usize {
        self.code + self.comments
    }
}

#[derive(Serialize)]
pub struct RepositoryOverviewResponse {
    repositories: Vec<RepositoryOverviewItemResponse>,
    page: usize,
    per_page: usize,
    has_next: bool,
}

#[derive(Serialize)]
struct RepositoryOverviewItemResponse {
    #[serde(flatten)]
    repository: resources::RepositoryResponse,
    branch_count: usize,
    total_lines: usize,
    languages: Vec<OverviewLanguageResponse>,
    activity: ActivityResponse,
}

#[derive(Serialize)]
struct OverviewLanguageResponse {
    language: String,
    lines: usize,
}

#[derive(Serialize)]
pub struct ActivityResponse {
    start_date: NaiveDate,
    end_date: NaiveDate,
    total_commits: usize,
    days: Vec<ActivityDayResponse>,
}

#[derive(Serialize)]
struct ActivityDayResponse {
    date: NaiveDate,
    count: usize,
}

#[derive(Clone)]
pub(super) struct GitOverview {
    branch_count: usize,
    head: Option<(String, ObjectId)>,
    activity: BTreeMap<NaiveDate, usize>,
}

async fn repository_overview_item(
    state: &RepositoryState,
    repository: repository::Model,
    favorited: bool,
    can_manage: bool,
    start_date: NaiveDate,
    end_date: NaiveDate,
) -> Result<RepositoryOverviewItemResponse, ApiError> {
    let git_overview = read_repository_overview(state, &repository, start_date, end_date).await?;
    let activity = activity_response(start_date, end_date, git_overview.activity);
    let stats = match git_overview.head {
        Some((commit_oid, tree_oid)) => {
            let cache_key = format!("{}:{commit_oid}", repository.storage_key);
            if let Some(cached) = state.cached_stats(&cache_key).await {
                cached
            } else {
                let path = state.repository_path(&repository);
                let computed = read_git(path, move |git| compute_stats(git, tree_oid)).await?;
                state.cache_stats(cache_key, computed.clone()).await;
                computed
            }
        }
        None => Vec::new(),
    };
    let total_lines = stats
        .iter()
        .map(LanguageStatResponse::non_blank_lines)
        .sum();
    let mut languages = stats
        .into_iter()
        .map(|stat| {
            let lines = stat.non_blank_lines();
            OverviewLanguageResponse {
                language: stat.language,
                lines,
            }
        })
        .collect::<Vec<_>>();
    languages.sort_unstable_by(|left, right| {
        right
            .lines
            .cmp(&left.lines)
            .then_with(|| left.language.cmp(&right.language))
    });
    languages.truncate(3);

    Ok(RepositoryOverviewItemResponse {
        activity,
        branch_count: git_overview.branch_count,
        total_lines,
        languages,
        repository: resources::RepositoryResponse::new(repository, state, favorited, can_manage),
    })
}

pub async fn overview(
    State(state): State<RepositoryState>,
    headers: HeaderMap,
    jar: CookieJar,
    Query(query): Query<OverviewQuery>,
) -> Result<Json<RepositoryOverviewResponse>, ApiError> {
    let accessible = accessible_repositories(&state, &headers, &jar).await?;
    let page = query.page.max(1);
    let per_page = query.per_page.clamp(1, MAX_OVERVIEW_PER_PAGE);
    let offset = (page - 1).saturating_mul(per_page);
    let mut page_repositories = accessible
        .repositories
        .into_iter()
        .skip(offset)
        .take(per_page + 1)
        .collect::<Vec<_>>();
    let has_next = page_repositories.len() > per_page;
    page_repositories.truncate(per_page);

    let end_date = Utc::now().date_naive();
    let start_date = activity_start_date(end_date, DEFAULT_REPOSITORY_ACTIVITY_DAYS)?;
    let repository_count = page_repositories.len();
    let mut pending = JoinSet::new();
    for (index, repository) in page_repositories.into_iter().enumerate() {
        let favorited = accessible.favorite_ids.contains(&repository.id);
        let can_manage = accessible.manageable_ids.contains(&repository.id);
        let state = state.clone();
        let slots = state.analysis_slots.clone();
        pending.spawn(async move {
            let _permit = slots.acquire_owned().await.map_err(ApiError::internal)?;
            let item = repository_overview_item(
                &state, repository, favorited, can_manage, start_date, end_date,
            )
            .await?;
            Ok::<_, ApiError>((index, item))
        });
    }

    let mut repositories = std::iter::repeat_with(|| None)
        .take(repository_count)
        .collect::<Vec<_>>();
    while let Some(result) = pending.join_next().await {
        let (index, item) = result.map_err(ApiError::internal)??;
        repositories[index] = Some(item);
    }
    let repositories = repositories
        .into_iter()
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| ApiError::internal("repository overview task did not return a result"))?;

    Ok(Json(RepositoryOverviewResponse {
        repositories,
        page,
        per_page,
        has_next,
    }))
}

pub async fn activity(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name)): AxumPath<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
    Query(query): Query<ActivityQuery>,
) -> Result<Json<ActivityResponse>, ApiError> {
    let repository = readable_repository(&state, &headers, &jar, &namespace, &name).await?;
    let end_date = Utc::now().date_naive();
    let start_date = activity_start_date(end_date, query.days)?;
    let activity = read_repository_overview(&state, &repository, start_date, end_date)
        .await?
        .activity;
    Ok(Json(activity_response(start_date, end_date, activity)))
}

pub async fn refs(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name)): AxumPath<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<RefsResponse>, ApiError> {
    let repository = readable_repository(&state, &headers, &jar, &namespace, &name).await?;
    let size_bytes = state.repository_size(&repository).await;
    let path = state.repository_path(&repository);
    let response = read_git(path, move |git| {
        let mut branches = git
            .references()
            .list_refs_with_prefix("refs/heads/")?
            .into_iter()
            .filter_map(|reference| {
                let ReferenceTarget::Direct(oid) = reference.target else {
                    return None;
                };
                Some(RefResponse {
                    name: reference.name.strip_prefix("refs/heads/")?.to_owned(),
                    oid: oid.to_hex(),
                })
            })
            .collect::<Vec<_>>();
        branches.sort_unstable_by(|left, right| left.name.cmp(&right.name));

        let mut tags = git
            .query_tags(TagQueryOptions::new())
            .map_err(|error| GitError::Command(error.to_string()))?
            .entries
            .into_iter()
            .filter_map(|entry| {
                let ReferenceTarget::Direct(oid) = entry.reference.target else {
                    return None;
                };
                Some(RefResponse {
                    name: entry.name,
                    oid: oid.to_hex(),
                })
            })
            .collect::<Vec<_>>();
        tags.sort_unstable_by(|left, right| left.name.cmp(&right.name));
        Ok(RefsResponse {
            branches,
            tags,
            size_bytes,
        })
    })
    .await?;
    Ok(Json(response))
}

pub async fn tree(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name)): AxumPath<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
    Query(query): Query<BrowseQuery>,
) -> Result<Json<TreeResponse>, ApiError> {
    let repository = readable_repository(&state, &headers, &jar, &namespace, &name).await?;
    let revision = query
        .rev
        .unwrap_or_else(|| repository.default_branch.clone());
    let requested_path = normalize_browse_path(&query.path)?;
    let include_commit_count = requested_path.is_empty();
    let path = state.repository_path(&repository);
    let count_path = path.clone();
    let (mut response, commit_oid) = read_git(path, move |git| {
        let commit_oid = git.peel_to_commit_oid(git.rev_parse(&revision)?)?;
        let resolved = git.resolve_path(&revision, &requested_path)?;
        if resolved.object_type != GitObjectType::Tree {
            return Err(GitError::InvalidPath(requested_path));
        }
        let tree = git.read_tree(&resolved.oid)?;
        let mut entries = Vec::with_capacity(tree.entries.len());
        for entry in tree.entries {
            let name = String::from_utf8_lossy(entry.name.as_bytes()).into_owned();
            let entry_path = if requested_path.is_empty() {
                name.clone()
            } else {
                format!("{requested_path}/{name}")
            };
            let kind = if entry.is_tree() {
                "tree"
            } else if entry.is_gitlink() {
                "submodule"
            } else if entry.is_symlink() {
                "symlink"
            } else {
                "blob"
            };
            let size = if kind == "blob" || kind == "symlink" {
                git.read_object_header(&entry.oid)
                    .ok()
                    .flatten()
                    .map(|(_, size)| size)
            } else {
                None
            };
            entries.push(TreeEntryResponse {
                name,
                path: entry_path,
                oid: entry.oid.to_hex(),
                kind,
                mode: entry.mode,
                size,
            });
        }
        entries.sort_unstable_by(|left, right| {
            let left_file = left.kind != "tree";
            let right_file = right.kind != "tree";
            left_file
                .cmp(&right_file)
                .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
        });
        Ok((
            TreeResponse {
                revision,
                commit_oid: commit_oid.to_hex(),
                commit_count: None,
                path: requested_path,
                entries,
            },
            commit_oid,
        ))
    })
    .await?;

    if include_commit_count {
        let cache_key = format!("{}:{}", repository.storage_key, commit_oid.to_hex());
        if let Some(count) = state.cached_commit_count(&cache_key).await {
            response.commit_count = Some(count);
        } else {
            let mut refreshing = state.commit_count_refreshing.lock().await;
            if refreshing.insert(cache_key.clone()) {
                drop(refreshing);
                let state = state.clone();
                tokio::spawn(async move {
                    let result = async {
                        let _permit = state
                            .commit_count_slots
                            .acquire()
                            .await
                            .map_err(ApiError::internal)?;
                        if state.cached_commit_count(&cache_key).await.is_none() {
                            let count = read_git(count_path, move |git| {
                                let mut count = 0;
                                git.rev_graph().stream_reachable_commits(
                                    [commit_oid],
                                    ReachableCommitOptions::new(),
                                    |_| {
                                        count += 1;
                                        Ok(StreamControl::Continue)
                                    },
                                )?;
                                Ok(count)
                            })
                            .await?;
                            state.cache_commit_count(cache_key.clone(), count).await;
                        }
                        Ok::<_, ApiError>(())
                    }
                    .await;
                    if let Err(error) = result {
                        tracing::warn!(%error, "could not refresh repository commit count");
                    }
                    state
                        .commit_count_refreshing
                        .lock()
                        .await
                        .remove(&cache_key);
                });
            }
        }
    }
    Ok(Json(response))
}

pub async fn blob(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name)): AxumPath<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
    Query(query): Query<BrowseQuery>,
) -> Result<Json<BlobResponse>, ApiError> {
    let repository = readable_repository(&state, &headers, &jar, &namespace, &name).await?;
    let revision = query
        .rev
        .unwrap_or_else(|| repository.default_branch.clone());
    let requested_path = normalize_browse_path(&query.path)?;
    if requested_path.is_empty() {
        return Err(ApiError::bad_request("A file path is required."));
    }
    let path = state.repository_path(&repository);
    let response = read_git(path, move |git| {
        let commit_oid = git.peel_to_commit_oid(git.rev_parse(&revision)?)?;
        let resolved = git.resolve_path(&revision, &requested_path)?;
        if resolved.object_type != GitObjectType::Blob {
            return Err(GitError::InvalidPath(requested_path));
        }
        let content = git.blobs().read(resolved.oid)?;
        let size = content.len();
        let too_large = size > MAX_TEXT_BLOB_BYTES;
        let binary = content.iter().take(8192).any(|byte| *byte == 0)
            || (!too_large && std::str::from_utf8(&content).is_err());
        let text = (!too_large && !binary).then(|| String::from_utf8_lossy(&content).into_owned());
        let rendered_html = text
            .as_deref()
            .and_then(|text| is_markdown_path(&requested_path).then(|| render_markdown(text)));
        Ok(BlobResponse {
            revision,
            commit_oid: commit_oid.to_hex(),
            path: requested_path,
            oid: resolved.oid.to_hex(),
            size,
            binary,
            too_large,
            content: text,
            rendered_html,
        })
    })
    .await?;
    Ok(Json(response))
}

pub async fn raw(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name)): AxumPath<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
    Query(query): Query<BrowseQuery>,
) -> Result<Response, ApiError> {
    let repository = readable_repository(&state, &headers, &jar, &namespace, &name).await?;
    let revision = query
        .rev
        .unwrap_or_else(|| repository.default_branch.clone());
    let requested_path = normalize_browse_path(&query.path)?;
    if requested_path.is_empty() {
        return Err(ApiError::bad_request("A file path is required."));
    }
    let repository_path = state.repository_path(&repository);
    let mime_path = requested_path.clone();
    let content = read_git(repository_path, move |git| {
        let resolved = git.resolve_path(&revision, &requested_path)?;
        if resolved.object_type != GitObjectType::Blob {
            return Err(GitError::InvalidPath(requested_path));
        }
        git.blobs().read(resolved.oid)
    })
    .await?;
    let content_type = mime_guess::from_path(mime_path)
        .first_or_octet_stream()
        .to_string();
    let mut response = Response::new(Body::from(content));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(&content_type).map_err(ApiError::internal)?,
    );
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-cache"),
    );
    Ok(response)
}

pub async fn history(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name)): AxumPath<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
    Query(query): Query<HistoryQuery>,
) -> Result<Json<HistoryResponse>, ApiError> {
    let repository = readable_repository(&state, &headers, &jar, &namespace, &name).await?;
    let revision = query
        .rev
        .unwrap_or_else(|| repository.default_branch.clone());
    let page = query.page.max(1);
    let per_page = query.per_page.clamp(1, 100);
    let path = state.repository_path(&repository);
    let response = read_git(path, move |git| {
        let tip = git.peel_to_commit_oid(git.rev_parse(&revision)?)?;
        let start = (page - 1).saturating_mul(per_page);
        let end = start.saturating_add(per_page + 1);
        let mut selected = Vec::with_capacity(per_page + 1);
        let mut seen = 0usize;
        git.rev_graph().stream_reachable_commits(
            [tip],
            ReachableCommitOptions::new(),
            |metadata| {
                if seen >= start && seen < end {
                    selected.push(metadata.oid);
                }
                seen += 1;
                Ok(if seen >= end {
                    StreamControl::Stop
                } else {
                    StreamControl::Continue
                })
            },
        )?;
        let has_next = selected.len() > per_page;
        selected.truncate(per_page);
        let commits = selected
            .into_iter()
            .map(|oid| commit_response(git, oid))
            .collect::<sley::Result<Vec<_>>>()?;
        Ok(HistoryResponse {
            commits,
            page,
            per_page,
            has_next,
        })
    })
    .await?;
    Ok(Json(response))
}

pub async fn commit(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name, revision)): AxumPath<(String, String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<CommitResponse>, ApiError> {
    let repository = readable_repository(&state, &headers, &jar, &namespace, &name).await?;
    let path = state.repository_path(&repository);
    let response = read_git(path, move |git| {
        let oid = git.peel_to_commit_oid(git.rev_parse(&revision)?)?;
        commit_response(git, oid)
    })
    .await?;
    Ok(Json(response))
}

pub async fn diff(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name, revision)): AxumPath<(String, String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<DiffResponse>, ApiError> {
    let repository = readable_repository(&state, &headers, &jar, &namespace, &name).await?;
    let repository_path = state.repository_path(&repository);
    let oid = read_git(repository_path.clone(), move |git| {
        git.peel_to_commit_oid(git.rev_parse(&revision)?)
            .map(|oid| oid.to_hex())
    })
    .await?;
    let output = Command::new("git")
        .arg("--git-dir")
        .arg(repository_path)
        .args([
            "show",
            "--format=",
            "--no-ext-diff",
            "--no-color",
            "--find-renames",
            "--unified=3",
            &oid,
            "--",
        ])
        .stdin(Stdio::null())
        .output()
        .await
        .map_err(ApiError::internal)?;
    if !output.status.success() {
        return Err(ApiError::internal(String::from_utf8_lossy(&output.stderr)));
    }
    let truncated = output.stdout.len() > MAX_DIFF_BYTES;
    let bytes = if truncated {
        &output.stdout[..MAX_DIFF_BYTES]
    } else {
        &output.stdout
    };
    Ok(Json(DiffResponse {
        patch: String::from_utf8_lossy(bytes).into_owned(),
        truncated,
    }))
}

pub async fn stats(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name)): AxumPath<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
    Query(query): Query<BrowseQuery>,
) -> Result<Json<Vec<LanguageStatResponse>>, ApiError> {
    let repository = readable_repository(&state, &headers, &jar, &namespace, &name).await?;
    let revision = query
        .rev
        .unwrap_or_else(|| repository.default_branch.clone());
    let path = state.repository_path(&repository);
    let (commit_oid, tree_oid) = read_git(path.clone(), move |git| {
        let commit_oid = git.peel_to_commit_oid(git.rev_parse(&revision)?)?;
        let commit = git.read_commit(&commit_oid)?;
        Ok((commit_oid.to_hex(), commit.tree))
    })
    .await?;
    let cache_key = format!("{}:{commit_oid}", repository.storage_key);
    if let Some(cached) = state.cached_stats(&cache_key).await {
        return Ok(Json(cached));
    }
    let computed = read_git(path, move |git| compute_stats(git, tree_oid)).await?;
    state.cache_stats(cache_key, computed.clone()).await;
    Ok(Json(computed))
}

async fn read_repository_overview(
    state: &RepositoryState,
    repository: &repository::Model,
    start_date: NaiveDate,
    end_date: NaiveDate,
) -> Result<GitOverview, ApiError> {
    let cache_key = format!(
        "{}:{}:{}:{start_date}:{end_date}",
        repository.storage_key, repository.updated_at, repository.default_branch
    );
    if let Some(cached) = state.cached_overview(&cache_key).await {
        return Ok(cached);
    }

    let path = state.repository_path(repository);
    let default_reference = format!("refs/heads/{}", repository.default_branch);
    let overview = read_git(path, move |git| {
        let references = git.references().list_refs_with_prefix("refs/heads/")?;
        let branch_count = references.len();
        let mut roots = Vec::with_capacity(branch_count);
        let mut default_index = None;
        for reference in references {
            let ReferenceTarget::Direct(target) = reference.target else {
                continue;
            };
            let commit_oid = git.peel_to_commit_oid(target)?;
            if reference.name == default_reference {
                default_index = Some(roots.len());
            }
            roots.push(commit_oid);
        }
        let head = if let Some(index) = default_index {
            let commit_oid = &roots[index];
            let commit = git.read_commit(commit_oid)?;
            Some((commit_oid.to_hex(), commit.tree))
        } else {
            None
        };
        let mut activity = BTreeMap::new();
        if !roots.is_empty() {
            git.rev_graph().stream_reachable_commits(
                roots,
                ReachableCommitOptions::new(),
                |metadata| {
                    let commit = git.read_commit(&metadata.oid)?;
                    if let Some(signature) = commit.author_signature()
                        && let Some(timestamp) =
                            DateTime::<Utc>::from_timestamp(signature.time.seconds, 0)
                    {
                        let date = (timestamp
                            + Duration::minutes(i64::from(signature.time.timezone_offset_minutes)))
                        .date_naive();
                        if date >= start_date && date <= end_date {
                            *activity.entry(date).or_default() += 1;
                        }
                    }
                    Ok(StreamControl::Continue)
                },
            )?;
        }
        Ok(GitOverview {
            branch_count,
            head,
            activity,
        })
    })
    .await?;
    state.cache_overview(cache_key, overview.clone()).await;
    Ok(overview)
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
        .map(|account| account.id);
    state
        .authorize(&repository, user_id, Permission::Read)
        .await?;
    Ok(repository)
}

pub(super) async fn read_git<T, F>(path: PathBuf, operation: F) -> Result<T, ApiError>
where
    T: Send + 'static,
    F: FnOnce(&GitRepository) -> sley::Result<T> + Send + 'static,
{
    tokio::task::spawn_blocking(move || {
        let repository = GitRepository::open_exact_bare(path)?;
        operation(&repository)
    })
    .await
    .map_err(ApiError::internal)?
    .map_err(map_git_error)
}

fn map_git_error(error: GitError) -> ApiError {
    match error {
        GitError::InvalidObjectId(_) | GitError::InvalidPath(_) | GitError::NotFound(_) => {
            ApiError::not_found()
        }
        error => ApiError::internal(error),
    }
}

fn normalize_browse_path(path: &str) -> Result<String, ApiError> {
    let normalized = path.trim_matches('/');
    if normalized.is_empty() {
        return Ok(String::new());
    }
    if normalized
        .split('/')
        .any(|component| component.is_empty() || component == "." || component == "..")
    {
        return Err(ApiError::bad_request("The repository path is invalid."));
    }
    Ok(normalized.to_owned())
}

fn commit_response(repository: &GitRepository, oid: ObjectId) -> sley::Result<CommitResponse> {
    let commit = repository.read_commit(&oid)?;
    let author = signature_response(commit.author_signature(), &commit.author);
    let committer = signature_response(commit.committer_signature(), &commit.committer);
    let message = String::from_utf8_lossy(&commit.message).trim().to_owned();
    let title = message.lines().next().unwrap_or_default().to_owned();
    let oid_hex = oid.to_hex();
    Ok(CommitResponse {
        short_oid: oid_hex[..12.min(oid_hex.len())].to_owned(),
        oid: oid_hex,
        tree_oid: commit.tree.to_hex(),
        parents: commit
            .parents
            .into_iter()
            .map(|parent| parent.to_hex())
            .collect(),
        author,
        committer,
        title,
        message,
    })
}

fn signature_response(signature: Option<sley::Signature>, raw: &[u8]) -> SignatureResponse {
    if let Some(signature) = signature {
        SignatureResponse {
            name: String::from_utf8_lossy(signature.name.as_bytes()).into_owned(),
            email: String::from_utf8_lossy(signature.email.as_bytes()).into_owned(),
            timestamp: signature.time.seconds,
            timezone_offset_minutes: signature.time.timezone_offset_minutes,
        }
    } else {
        SignatureResponse {
            name: String::from_utf8_lossy(raw).into_owned(),
            email: String::new(),
            timestamp: 0,
            timezone_offset_minutes: 0,
        }
    }
}

fn is_markdown_path(path: &str) -> bool {
    Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "md" | "markdown" | "mdown" | "mkd"
            )
        })
}

pub(crate) fn render_markdown(markdown: &str) -> String {
    let mut options = Options::default();
    options.extension.strikethrough = true;
    options.extension.tagfilter = true;
    options.extension.table = true;
    options.extension.autolink = true;
    options.extension.tasklist = true;
    options.render.r#unsafe = true;

    let rendered = markdown_to_html(markdown, &options);
    let mut sanitizer = ammonia::Builder::default();
    sanitizer
        .add_generic_attributes(&["align", "class", "id"])
        .add_tags(&["input"])
        .add_tag_attributes("input", &["checked", "disabled", "type"])
        .set_tag_attribute_value("input", "disabled", "")
        .set_tag_attribute_value("input", "type", "checkbox");
    sanitizer.clean(&rendered).to_string()
}

fn compute_stats(
    repository: &GitRepository,
    root: ObjectId,
) -> sley::Result<Vec<LanguageStatResponse>> {
    let mut totals = BTreeMap::<String, LanguageStatResponse>::new();
    let config = TokeiConfig::default();
    collect_tree_stats(repository, root, "", &config, &mut totals)?;
    let mut response = totals.into_values().collect::<Vec<_>>();
    response.sort_unstable_by(|left, right| {
        right
            .code
            .cmp(&left.code)
            .then_with(|| left.language.cmp(&right.language))
    });
    Ok(response)
}

fn collect_tree_stats(
    repository: &GitRepository,
    tree_oid: ObjectId,
    prefix: &str,
    config: &TokeiConfig,
    totals: &mut BTreeMap<String, LanguageStatResponse>,
) -> sley::Result<()> {
    for entry in repository.read_tree(&tree_oid)?.entries {
        let name = String::from_utf8_lossy(entry.name.as_bytes());
        let path = if prefix.is_empty() {
            name.into_owned()
        } else {
            format!("{prefix}/{name}")
        };
        if entry.is_tree() {
            collect_tree_stats(repository, entry.oid, &path, config, totals)?;
            continue;
        }
        if entry.is_gitlink() {
            continue;
        }
        let Some(language) = LanguageType::from_path(&path, config) else {
            continue;
        };
        let content = repository.blobs().read(entry.oid)?;
        let stats = language.parse_from_slice(&content, config).summarise();
        let language_name = language.to_string();
        let total = totals
            .entry(language_name.clone())
            .or_insert_with(|| LanguageStatResponse {
                language: language_name,
                files: 0,
                code: 0,
                comments: 0,
                blanks: 0,
            });
        total.files += 1;
        total.code += stats.code;
        total.comments += stats.comments;
        total.blanks += stats.blanks;
    }
    Ok(())
}

const fn default_repository_activity_days() -> u16 {
    DEFAULT_REPOSITORY_ACTIVITY_DAYS
}

const fn default_overview_per_page() -> usize {
    DEFAULT_OVERVIEW_PER_PAGE
}

fn activity_start_date(end_date: NaiveDate, days: u16) -> Result<NaiveDate, ApiError> {
    if !(1..=MAX_REPOSITORY_ACTIVITY_DAYS).contains(&days) {
        return Err(ApiError::bad_request(format!(
            "Activity window must be between 1 and {MAX_REPOSITORY_ACTIVITY_DAYS} days."
        )));
    }
    Ok(end_date - Duration::days(i64::from(days - 1)))
}

fn activity_response(
    start_date: NaiveDate,
    end_date: NaiveDate,
    activity: BTreeMap<NaiveDate, usize>,
) -> ActivityResponse {
    let total_commits = activity.values().sum();
    let days = activity
        .into_iter()
        .map(|(date, count)| ActivityDayResponse { date, count })
        .collect();
    ActivityResponse {
        start_date,
        end_date,
        total_commits,
        days,
    }
}

const fn default_page() -> usize {
    1
}

const fn default_per_page() -> usize {
    30
}
