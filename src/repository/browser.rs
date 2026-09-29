use std::{
    cmp::Ordering,
    collections::{BTreeMap, HashMap, HashSet},
    fs::File,
    io::{self, Write},
    path::{Path, PathBuf},
    time::{Duration as StdDuration, Instant},
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
use russh::keys::ssh_key::{HashAlg, PublicKey, SshSig};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder};
use serde::{Deserialize, Deserializer, Serialize};
use sley::{
    GitError, GitObjectType, ObjectId, ReachableCommitOptions, ReferenceTarget,
    Repository as GitRepository, StreamControl, TagQueryOptions,
};
use sley_archive::{
    ArchiveConvert, ArchiveExtras, TarArchiveOptions, ZipArchiveOptions, write_tar_gz_archive_full,
    write_zip_archive_full,
};
use sley_diff_merge::porcelain::{
    DiffRenderOptions, LineStats, SubmoduleDiffFormat, collect_diff_stat_entries,
    write_diff_patch_entry,
};
use sley_diff_merge::render::LineIndicators;
use sley_diff_merge::{
    DiffAlgorithm, DiffNameStatusOptions, WsIgnore, diff_name_status_empty_tree_with_options,
    diff_name_status_trees_with_options,
};
use tokei::{Config as TokeiConfig, LanguageType};
use tokio::task::JoinSet;

use super::{
    Permission, RepositoryState,
    resources::{self, accessible_repositories},
};
use crate::{
    entity::{repository, repository_release, ssh_key as ssh_key_entity, user},
    filesystem::{create_private_directory_async, open_private_file},
    identity::{ApiError, SCOPE_READ},
};

const MAX_TEXT_BLOB_BYTES: usize = 2 * 1024 * 1024;
const MAX_LFS_POINTER_BYTES: u64 = 1024;
const MAX_DIFF_BYTES: usize = 5 * 1024 * 1024;
const MAX_GITMODULES_BYTES: usize = 128 * 1024;
const DEFAULT_REPOSITORY_ACTIVITY_DAYS: u16 = 14;
pub(super) const MAX_REPOSITORY_ACTIVITY_DAYS: u16 = 365;
const DEFAULT_OVERVIEW_PER_PAGE: usize = 20;
const MAX_OVERVIEW_PER_PAGE: usize = 50;

/// A blank `rev=` means "no revision given" and falls back to the default
/// branch, rather than reaching the revision parser as an empty spec.
fn blank_as_none<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(Option::<String>::deserialize(deserializer)?.filter(|value| !value.trim().is_empty()))
}

#[derive(Deserialize)]
pub struct BrowseQuery {
    #[serde(default, deserialize_with = "blank_as_none")]
    pub(super) rev: Option<String>,
    #[serde(default)]
    pub(super) path: String,
}

#[derive(Clone, Copy, Deserialize)]
pub enum SourceArchiveFormat {
    #[serde(rename = "zip")]
    Zip,
    #[serde(rename = "tar.gz")]
    TarGz,
}

impl SourceArchiveFormat {
    fn extension(self) -> &'static str {
        match self {
            Self::Zip => "zip",
            Self::TarGz => "tar.gz",
        }
    }

    fn content_type(self) -> &'static str {
        match self {
            Self::Zip => "application/zip",
            Self::TarGz => "application/gzip",
        }
    }
}

#[derive(Deserialize)]
pub struct SourceArchiveQuery {
    #[serde(default, deserialize_with = "blank_as_none")]
    rev: Option<String>,
    format: SourceArchiveFormat,
}

#[derive(Deserialize)]
pub struct HistoryQuery {
    #[serde(default, deserialize_with = "blank_as_none")]
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
    namespace: Option<String>,
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
    /// The commit the ref resolves to. For an annotated tag this differs from
    /// `oid`, which is the tag object, so decorating commits needs this one.
    commit_oid: String,
}

#[derive(Serialize)]
pub struct RefsResponse {
    branches: Vec<RefResponse>,
    tags: Vec<RefResponse>,
    size_bytes: Option<u64>,
    lfs_size_bytes: Option<u64>,
}

#[derive(Serialize)]
pub struct TreeResponse {
    revision: String,
    commit_oid: String,
    commit_timestamp: i64,
    commit_count: Option<usize>,
    path: String,
    entries: Vec<TreeEntryResponse>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SubmoduleResponse {
    name: String,
    url: Option<String>,
    branch: Option<String>,
}

#[derive(Serialize)]
pub struct TreeEntryResponse {
    name: String,
    path: String,
    oid: String,
    kind: &'static str,
    mode: u32,
    size: Option<u64>,
    lfs_size: Option<u64>,
    submodule: Option<SubmoduleResponse>,
}

#[derive(Serialize)]
pub struct LfsMetadata {
    oid: String,
    size: String,
}

#[derive(Serialize)]
pub struct BlobResponse {
    revision: String,
    commit_oid: String,
    path: String,
    oid: String,
    size: usize,
    lfs: Option<LfsMetadata>,
    binary: bool,
    too_large: bool,
    content: Option<String>,
    rendered_html: Option<String>,
    image: Option<super::image::ImageMetadata>,
    image_error: Option<String>,
}

#[derive(Serialize)]
pub struct HistoryResponse {
    commits: Vec<CommitResponse>,
    page: usize,
    per_page: usize,
    has_next: bool,
}

/// A tag or release pointing at a commit, for decorating history rows.
#[derive(Serialize, Clone)]
pub struct CommitRefResponse {
    kind: &'static str,
    name: String,
    prerelease: bool,
    latest: bool,
    published_at: Option<DateTime<Utc>>,
    /// Commits between the previous release and this one, for releases whose
    /// predecessor is known.
    commits_since_previous: Option<usize>,
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
    insertions: usize,
    deletions: usize,
    refs: Vec<CommitRefResponse>,
    verification: Option<CommitVerificationResponse>,
    #[serde(skip)]
    signing_fingerprint: Option<String>,
}

#[derive(Serialize)]
pub struct CommitVerificationResponse {
    verified: bool,
    reason: &'static str,
    signer: Option<String>,
    fingerprint: Option<String>,
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

#[derive(Clone, Debug, Deserialize, Serialize)]
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
pub(super) struct RepositoryOverviewItemResponse {
    #[serde(flatten)]
    repository: resources::RepositoryResponse,
    branch_count: usize,
    commit_count: usize,
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

#[derive(Clone, Deserialize, Serialize)]
pub(super) struct GitOverview {
    branch_count: usize,
    head: Option<String>,
    pub(super) activity: BTreeMap<NaiveDate, usize>,
}

pub(super) async fn repository_overview_item(
    state: &RepositoryState,
    repository: repository::Model,
    favorited: bool,
    can_manage: bool,
    can_write: bool,
    start_date: NaiveDate,
    end_date: NaiveDate,
) -> Result<RepositoryOverviewItemResponse, ApiError> {
    let git_overview = read_repository_overview(state, &repository, start_date, end_date).await?;
    let activity = activity_response(start_date, end_date, git_overview.activity);
    let commit_count = match git_overview.head.as_deref() {
        Some(commit_oid) => {
            if let Some(count) = state.cached_commit_count(repository.id, commit_oid).await? {
                count
            } else {
                let path = state.repository_path(&repository);
                let revision = commit_oid.to_owned();
                let count = read_git(path, move |git| {
                    let oid = git.peel_to_commit_oid(git.rev_parse(&revision)?)?;
                    count_reachable_commits(git, oid)
                })
                .await?;
                state
                    .cache_commit_count(repository.id, commit_oid, count)
                    .await?;
                count
            }
        }
        None => 0,
    };
    let stats = match git_overview.head {
        Some(commit_oid) => {
            if let Some(cached) = state.cached_stats(repository.id, &commit_oid).await? {
                cached
            } else {
                let path = state.repository_path(&repository);
                let revision = commit_oid.clone();
                let computed = read_git(path, move |git| {
                    let commit_oid = git.peel_to_commit_oid(git.rev_parse(&revision)?)?;
                    let tree_oid = git.read_commit(&commit_oid)?.tree;
                    compute_stats(git, tree_oid)
                })
                .await?;
                state
                    .cache_stats(repository.id, &commit_oid, &computed)
                    .await?;
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
        commit_count,
        branch_count: git_overview.branch_count,
        total_lines,
        languages,
        repository: resources::RepositoryResponse::new(
            repository, state, favorited, can_manage, can_write,
        ),
    })
}

/// Overview cards for `repositories`, in order, analysed a few at a time so a
/// long page cannot occupy every blocking thread.
pub(super) async fn repository_overview_items(
    state: &RepositoryState,
    repositories: Vec<repository::Model>,
    favorite_ids: &HashSet<uuid::Uuid>,
    manageable_ids: &HashSet<uuid::Uuid>,
    writable_ids: &HashSet<uuid::Uuid>,
) -> Result<Vec<RepositoryOverviewItemResponse>, ApiError> {
    let end_date = Utc::now().date_naive();
    let start_date = activity_start_date(end_date, DEFAULT_REPOSITORY_ACTIVITY_DAYS)?;
    let repository_count = repositories.len();
    let mut pending = JoinSet::new();
    for (index, repository) in repositories.into_iter().enumerate() {
        let favorited = favorite_ids.contains(&repository.id);
        let can_manage = manageable_ids.contains(&repository.id);
        let can_write = writable_ids.contains(&repository.id);
        let state = state.clone();
        let slots = state.analysis_slots.clone();
        pending.spawn(async move {
            let _permit = slots.acquire_owned().await.map_err(ApiError::internal)?;
            let item = repository_overview_item(
                &state, repository, favorited, can_manage, can_write, start_date, end_date,
            )
            .await?;
            Ok::<_, ApiError>((index, item))
        });
    }

    let mut items = std::iter::repeat_with(|| None)
        .take(repository_count)
        .collect::<Vec<_>>();
    while let Some(result) = pending.join_next().await {
        let (index, item) = result.map_err(ApiError::internal)??;
        items[index] = Some(item);
    }
    items
        .into_iter()
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| ApiError::internal("repository overview task did not return a result"))
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
        .filter(|repository| {
            query
                .namespace
                .as_deref()
                .is_none_or(|namespace| repository.namespace == namespace)
        })
        .skip(offset)
        .take(per_page + 1)
        .collect::<Vec<_>>();
    let has_next = page_repositories.len() > per_page;
    page_repositories.truncate(per_page);

    let repositories = repository_overview_items(
        &state,
        page_repositories,
        &accessible.favorite_ids,
        &accessible.manageable_ids,
        &accessible.writable_ids,
    )
    .await?;

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
    let size = state.repository_size(&repository).await?;
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
                    commit_oid: oid.to_hex(),
                    oid: oid.to_hex(),
                })
            })
            .collect::<Vec<_>>();
        branches.sort_unstable_by(|left, right| compare_ref_names(&left.name, &right.name));

        let mut tags = tag_refs(git)?;
        tags.sort_unstable_by(|left, right| compare_ref_names(&left.name, &right.name));
        Ok(RefsResponse {
            branches,
            tags,
            size_bytes: size.map(|size| size.bytes),
            lfs_size_bytes: size.map(|size| size.lfs_bytes),
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
        .or_else(|| repository.default_branch.clone())
        .ok_or_else(ApiError::not_found)?;
    let requested_path = normalize_browse_path(&query.path)?;
    let include_commit_count = requested_path.is_empty();
    let path = state.repository_path(&repository);
    let count_path = path.clone();
    let (mut response, commit_oid) = read_git(path, move |git| {
        let commit_oid = git.peel_to_commit_oid(git.rev_parse(&revision)?)?;
        let commit = git.read_commit(&commit_oid)?;
        let commit_timestamp = commit
            .committer_signature()
            .map_or(0, |signature| signature.time.seconds);
        let commit_revision = commit_oid.to_hex();
        let resolved = git.resolve_path(&commit_revision, &requested_path)?;
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
            // Only small regular blobs can be pointers; never load large files or symlinks.
            let lfs_size =
                if kind == "blob" && size.is_some_and(|size| size < MAX_LFS_POINTER_BYTES) {
                    lfs_pointer_metadata(&git.blobs().read(entry.oid)?).map(|pointer| pointer.size)
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
                lfs_size,
                submodule: None,
            });
        }
        if entries.iter().any(|entry| entry.kind == "submodule") {
            let mut submodules = read_submodule_metadata(git, commit.tree)?;
            for entry in &mut entries {
                if entry.kind == "submodule" {
                    entry.submodule = submodules.remove(&entry.path);
                }
            }
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
                commit_timestamp,
                commit_count: None,
                path: requested_path,
                entries,
            },
            commit_oid,
        ))
    })
    .await?;

    if include_commit_count {
        let commit_key = commit_oid.to_hex();
        if let Some(count) = state
            .cached_commit_count(repository.id, &commit_key)
            .await?
        {
            response.commit_count = Some(count);
        } else {
            let refresh_key = format!("{}:{commit_key}", repository.id);
            let mut refreshing = state.commit_count_refreshing.lock().await;
            if refreshing.insert(refresh_key.clone()) {
                drop(refreshing);
                let task_state = state.clone();
                let state = state.clone();
                let repository_id = repository.id;
                task_state.spawn_task(async move {
                    let result = async {
                        let _permit = state
                            .commit_count_slots
                            .acquire()
                            .await
                            .map_err(ApiError::internal)?;
                        if state
                            .cached_commit_count(repository_id, &commit_key)
                            .await?
                            .is_none()
                        {
                            let count = read_git(count_path, move |git| {
                                count_reachable_commits(git, commit_oid)
                            })
                            .await?;
                            state
                                .cache_commit_count(repository_id, &commit_key, count)
                                .await?;
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
                        .remove(&refresh_key);
                });
            }
        }
    }
    Ok(Json(response))
}

fn read_submodule_metadata(
    git: &GitRepository,
    commit_tree_oid: ObjectId,
) -> Result<HashMap<String, SubmoduleResponse>, GitError> {
    let root_tree = git.read_tree(&commit_tree_oid)?;
    let Some(config_entry) = root_tree.entries.into_iter().find(|entry| {
        entry.name.as_bytes() == b".gitmodules" && matches!(entry.mode, 0o100644 | 0o100755)
    }) else {
        return Ok(HashMap::new());
    };
    let Some((object_type, size)) = git.read_object_header(&config_entry.oid)? else {
        return Err(GitError::InvalidObject(format!(
            "missing .gitmodules object {}",
            config_entry.oid
        )));
    };
    if object_type != GitObjectType::Blob {
        return Err(GitError::InvalidObject(format!(
            ".gitmodules object {} is not a blob",
            config_entry.oid
        )));
    }
    if size > MAX_GITMODULES_BYTES as u64 {
        return Ok(HashMap::new());
    }
    let bytes = git.blobs().read(config_entry.oid)?;
    Ok(parse_submodule_metadata(&bytes))
}

fn parse_submodule_metadata(bytes: &[u8]) -> HashMap<String, SubmoduleResponse> {
    let Ok(config) = sley_config::GitConfig::parse(bytes) else {
        return HashMap::new();
    };

    let mut submodules = HashMap::new();
    for section in &config.sections {
        if !section.name.eq_ignore_ascii_case("submodule") {
            continue;
        }
        let Some(name) = section.subsection.as_ref().filter(|name| !name.is_empty()) else {
            continue;
        };
        let path = config
            .get("submodule", Some(name.as_str()), "path")
            .map(str::to_owned)
            .filter(|path| !path.is_empty());
        let Some(path) = path else {
            continue;
        };
        submodules.insert(
            path,
            SubmoduleResponse {
                name: name.clone(),
                url: config
                    .get("submodule", Some(name.as_str()), "url")
                    .map(str::to_owned)
                    .filter(|value| !value.is_empty()),
                branch: config
                    .get("submodule", Some(name.as_str()), "branch")
                    .map(str::to_owned)
                    .filter(|value| !value.is_empty()),
            },
        );
    }
    submodules
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct LfsPointerMetadata<'a> {
    oid: &'a str,
    size: u64,
}

fn lfs_pointer_metadata(content: &[u8]) -> Option<LfsPointerMetadata<'_>> {
    let text = std::str::from_utf8(content).ok()?.strip_suffix('\n')?;
    let mut lines = text.split('\n');
    match lines.next()? {
        "version https://git-lfs.github.com/spec/v1"
        | "version https://hawser.github.com/spec/v1" => {}
        _ => return None,
    }
    let mut previous_key = "";
    let mut oid = None;
    let mut size = None;
    for line in lines {
        let (key, value) = line.split_once(' ')?;
        if key.is_empty()
            || key <= previous_key
            || !key.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'-')
            })
            || value.is_empty()
            || value.contains('\r')
        {
            return None;
        }
        previous_key = key;
        match key {
            "oid" => {
                let hash = value.strip_prefix("sha256:")?;
                if hash.len() != 64
                    || !hash
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                {
                    return None;
                }
                oid = Some(hash);
            }
            "size" => {
                if !value.bytes().all(|byte| byte.is_ascii_digit()) {
                    return None;
                }
                size = Some(value.parse().ok()?);
            }
            "version" => return None,
            _ => {}
        }
    }
    Some(LfsPointerMetadata {
        oid: oid?,
        size: size?,
    })
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
        .or_else(|| repository.default_branch.clone())
        .ok_or_else(ApiError::not_found)?;
    let requested_path = normalize_browse_path(&query.path)?;
    if requested_path.is_empty() {
        return Err(ApiError::bad_request("A file path is required."));
    }
    let path = state.repository_path(&repository);
    let (mut response, image_content) = read_git(path, move |git| {
        let commit_oid = git.peel_to_commit_oid(git.rev_parse(&revision)?)?;
        let resolved = git.resolve_path(&commit_oid.to_hex(), &requested_path)?;
        if resolved.object_type != GitObjectType::Blob {
            return Err(GitError::InvalidPath(requested_path));
        }
        let size = git
            .read_object_header(&resolved.oid)?
            .map_or(usize::MAX, |(_, size)| {
                usize::try_from(size).unwrap_or(usize::MAX)
            });
        let image_path = super::image::is_image_path(&requested_path);
        let limit = if image_path {
            super::image::MAX_IMAGE_BYTES
        } else {
            MAX_TEXT_BLOB_BYTES
        };
        let content = if size <= limit {
            Some(git.blobs().read(resolved.oid)?)
        } else {
            None
        };
        let too_large = size > MAX_TEXT_BLOB_BYTES;
        let binary = content.as_ref().is_some_and(|content| {
            content.iter().take(8192).any(|byte| *byte == 0)
                || (!too_large && std::str::from_utf8(content).is_err())
        });
        let lfs = content
            .as_deref()
            .filter(|_| size < MAX_LFS_POINTER_BYTES as usize)
            .and_then(lfs_pointer_metadata)
            .map(|pointer| LfsMetadata {
                oid: pointer.oid.to_owned(),
                size: pointer.size.to_string(),
            });
        let text = content
            .as_ref()
            .filter(|_| !too_large && !binary)
            .map(|content| String::from_utf8_lossy(content).into_owned());
        let rendered_html = text
            .as_deref()
            .and_then(|text| is_markdown_path(&requested_path).then(|| render_markdown(text)));
        let has_lfs = lfs.is_some();
        let image_content = if image_path && !has_lfs {
            content
        } else {
            None
        };
        let response = BlobResponse {
            revision,
            commit_oid: commit_oid.to_hex(),
            path: requested_path,
            oid: resolved.oid.to_hex(),
            size,
            lfs,
            binary,
            too_large,
            content: text,
            rendered_html,
            image: None,
            image_error: (image_path && image_content.is_none() && !has_lfs).then(|| {
                "Image previews are limited to 16 MiB. Download the original instead.".to_owned()
            }),
        };
        Ok((response, image_content))
    })
    .await?;
    if let Some(content) = image_content {
        match super::image::resolve_image_content(&state, &repository, content).await {
            Ok(content) => {
                response.size = content.len();
                if response.path.to_ascii_lowercase().ends_with(".svg") {
                    response.content = (content.len() <= MAX_TEXT_BLOB_BYTES)
                        .then(|| std::str::from_utf8(&content).ok().map(str::to_owned))
                        .flatten();
                }
                match super::image::cached_image(content, response.path.clone()).await {
                    Ok(image) => response.image = Some(image.metadata),
                    Err(error) => response.image_error = Some(error.to_string()),
                }
            }
            Err(error) => response.image_error = Some(error.to_string()),
        }
    }
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
        .or_else(|| repository.default_branch.clone())
        .ok_or_else(ApiError::not_found)?;
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
    let content_type = mime_guess::from_path(&mime_path)
        .first_or_octet_stream()
        .to_string();
    let body = if super::image::is_image_path(&mime_path)
        && let Some(oid) = super::image::lfs_image_oid(&content)?
    {
        let key = state.lfs_object_key(&repository, oid)?;
        let store = state.lfs_store();
        store
            .stat(&key)
            .await
            .map_err(ApiError::internal)?
            .ok_or_else(|| ApiError::bad_request("The LFS image has not been uploaded."))?;
        let reader = store.read(&key).await.map_err(ApiError::internal)?;
        Body::from_stream(tokio_util::io::ReaderStream::new(reader))
    } else {
        Body::from(content)
    };
    let mut response = Response::new(body);
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(&content_type).map_err(ApiError::internal)?,
    );
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-cache"),
    );
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response.headers_mut().insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("default-src 'none'; sandbox"),
    );
    if content_type == "image/svg+xml" {
        let filename = std::path::Path::new(&mime_path)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("image.svg");
        let encoded: String = url::form_urlencoded::byte_serialize(filename.as_bytes()).collect();
        response.headers_mut().insert(
            header::CONTENT_DISPOSITION,
            HeaderValue::from_str(&format!(
                "attachment; filename*=UTF-8''{}",
                encoded.replace('+', "%20")
            ))
            .map_err(ApiError::internal)?,
        );
    }
    Ok(response)
}

pub async fn source_archive(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name)): AxumPath<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
    Query(query): Query<SourceArchiveQuery>,
) -> Result<Response, ApiError> {
    let repository = readable_repository(&state, &headers, &jar, &namespace, &name).await?;
    let revision = query
        .rev
        .or_else(|| repository.default_branch.clone())
        .ok_or_else(ApiError::not_found)?;
    let repository_path = state.repository_path(&repository);
    let (commit_oid, tree_oid, mtime) = read_git(repository_path.clone(), move |git| {
        let commit_oid = git.peel_to_commit_oid(git.rev_parse(&revision)?)?;
        let commit = git.read_commit(&commit_oid)?;
        let mtime = commit.committer_signature().map_or(0, |signature| {
            u64::try_from(signature.time.seconds).unwrap_or_default()
        });
        Ok((commit_oid, commit.tree, mtime))
    })
    .await?;
    let commit_hex = commit_oid.to_hex();
    let short_oid = &commit_hex[..commit_hex.len().min(8)];
    let archive_name = format!(
        "{}-{short_oid}.{}",
        repository.name,
        query.format.extension()
    );
    let cache_path =
        state.source_archive_cache_path(&repository, &commit_hex, query.format.extension());
    if !tokio::fs::try_exists(&cache_path)
        .await
        .map_err(ApiError::internal)?
    {
        let permit = state.try_archive_generation()?;
        if !tokio::fs::try_exists(&cache_path)
            .await
            .map_err(ApiError::internal)?
        {
            let cache_directory = state.source_archive_cache_directory(&repository);
            create_private_directory_async(&cache_directory)
                .await
                .map_err(ApiError::internal)?;
            let temporary_path = cache_directory.join(format!(
                ".{}.{}.tmp",
                commit_hex,
                uuid::Uuid::new_v4().simple()
            ));
            let prefix = format!("{}-{short_oid}/", repository.name);
            let format = query.format;
            let published_path = cache_path.clone();
            tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
                let _permit = permit;
                let result = write_archive_file(
                    &repository_path,
                    &temporary_path,
                    format,
                    commit_oid,
                    tree_oid,
                    mtime,
                    prefix,
                )
                .and_then(|()| {
                    if published_path.exists() {
                        Ok(())
                    } else {
                        std::fs::rename(&temporary_path, &published_path).map_err(Into::into)
                    }
                })
                .and_then(|()| prune_archive_cache(&cache_directory));
                let _ = std::fs::remove_file(&temporary_path);
                result
            })
            .await
            .map_err(ApiError::internal)?
            .map_err(ApiError::internal)?;
        }
    }
    source_archive_response(&cache_path, &archive_name, query.format).await
}

const MAX_SOURCE_ARCHIVE_BYTES: u64 = 512 * 1024 * 1024;
const SOURCE_ARCHIVE_GENERATION_TIMEOUT: StdDuration = StdDuration::from_secs(2 * 60);
const MAX_CACHED_ARCHIVES_PER_REPOSITORY: usize = 8;

struct ArchiveFileWriter {
    file: File,
    bytes_written: u64,
    deadline: Instant,
}

impl Write for ArchiveFileWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if Instant::now() >= self.deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "source archive generation timed out",
            ));
        }
        if self.bytes_written.saturating_add(bytes.len() as u64) > MAX_SOURCE_ARCHIVE_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::FileTooLarge,
                "source archive exceeds the output size limit",
            ));
        }
        let written = self.file.write(bytes)?;
        self.bytes_written = self.bytes_written.saturating_add(written as u64);
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

fn write_archive_file(
    repository_path: &Path,
    output_path: &Path,
    archive_format: SourceArchiveFormat,
    commit_oid: ObjectId,
    tree_oid: ObjectId,
    mtime: u64,
    prefix: String,
) -> anyhow::Result<()> {
    let repository = GitRepository::open_exact_bare(repository_path)?;
    let config = repository.config()?;
    let converter = ArchiveConvert::from_tree(
        repository_path,
        repository_path,
        &config,
        repository.object_database(),
        repository.object_format(),
        &tree_oid,
    )?;
    let mut writer = ArchiveFileWriter {
        file: open_private_file(output_path, true)?,
        bytes_written: 0,
        deadline: Instant::now() + SOURCE_ARCHIVE_GENERATION_TIMEOUT,
    };
    match archive_format {
        SourceArchiveFormat::Zip => write_zip_archive_full(
            &mut writer,
            repository.object_database(),
            repository.object_format(),
            &tree_oid,
            ZipArchiveOptions {
                prefix: prefix.into_bytes(),
                mtime,
                commit_id: Some(commit_oid),
                ..ZipArchiveOptions::default()
            },
            &converter,
            &ArchiveExtras::default(),
        )?,
        SourceArchiveFormat::TarGz => write_tar_gz_archive_full(
            &mut writer,
            repository.object_database(),
            repository.object_format(),
            &tree_oid,
            TarArchiveOptions {
                prefix: prefix.into_bytes(),
                mtime,
                commit_id: Some(commit_oid),
                ..TarArchiveOptions::default()
            },
            &converter,
            &ArchiveExtras::default(),
            6,
        )?,
    }
    writer.flush()?;
    writer.file.sync_all()?;
    Ok(())
}

fn prune_archive_cache(directory: &Path) -> anyhow::Result<()> {
    let mut entries = std::fs::read_dir(directory)?
        .filter_map(Result::ok)
        .filter(|entry| !entry.file_name().to_string_lossy().ends_with(".tmp"))
        .filter_map(|entry| {
            let metadata = entry.metadata().ok()?;
            metadata
                .is_file()
                .then(|| (metadata.modified().ok(), entry.path()))
        })
        .collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.0);
    let remove = entries
        .len()
        .saturating_sub(MAX_CACHED_ARCHIVES_PER_REPOSITORY);
    for (_, path) in entries.into_iter().take(remove) {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

async fn source_archive_response(
    path: &Path,
    archive_name: &str,
    format: SourceArchiveFormat,
) -> Result<Response, ApiError> {
    let file = tokio::fs::File::open(path)
        .await
        .map_err(ApiError::internal)?;
    let size = file.metadata().await.map_err(ApiError::internal)?.len();
    let mut response = Response::new(Body::from_stream(tokio_util::io::ReaderStream::new(file)));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(format.content_type()),
    );
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!("attachment; filename=\"{archive_name}\""))
            .map_err(ApiError::internal)?,
    );
    response.headers_mut().insert(
        header::CONTENT_LENGTH,
        HeaderValue::from_str(&size.to_string()).map_err(ApiError::internal)?,
    );
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=31536000, immutable"),
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
        .or_else(|| repository.default_branch.clone())
        .ok_or_else(ApiError::not_found)?;
    let page = query.page.max(1);
    let per_page = query.per_page.clamp(1, 100);
    let path = state.repository_path(&repository);
    let mut response = read_git(path.clone(), move |git| {
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
        let stats = commit_line_stats(git, &selected);
        let tags = tag_decorations(git)?;
        let commits = selected
            .into_iter()
            .map(|oid| {
                let (insertions, deletions) = stats.get(&oid.to_hex()).copied().unwrap_or_default();
                commit_response(git, oid).map(|mut response| {
                    response.insertions = insertions;
                    response.deletions = deletions;
                    if let Some(found) = tags.get(&response.oid) {
                        response.refs = found.clone();
                    }
                    response
                })
            })
            .collect::<sley::Result<Vec<_>>>()?;
        Ok(HistoryResponse {
            commits,
            page,
            per_page,
            has_next,
        })
    })
    .await?;
    attach_release_refs(&state, &repository, &mut response.commits).await?;
    attach_commit_verifications(&state, &mut response.commits).await?;
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
    let mut response = read_git(path, move |git| {
        let oid = git.peel_to_commit_oid(git.rev_parse(&revision)?)?;
        let tags = tag_decorations(git)?;
        commit_response(git, oid).map(|mut response| {
            if let Some(found) = tags.get(&response.oid) {
                response.refs = found.clone();
            }
            response
        })
    })
    .await?;
    attach_release_refs(&state, &repository, std::slice::from_mut(&mut response)).await?;
    attach_commit_verifications(&state, std::slice::from_mut(&mut response)).await?;
    Ok(Json(response))
}

pub async fn diff(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name, revision)): AxumPath<(String, String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<DiffResponse>, ApiError> {
    let repository = readable_repository(&state, &headers, &jar, &namespace, &name).await?;
    let path = state.repository_path(&repository);
    let response = read_git(path, move |git| {
        let oid = git.peel_to_commit_oid(git.rev_parse(&revision)?)?;
        let commit = git.read_commit(&oid)?;
        let options = DiffNameStatusOptions {
            detect_renames: true,
            ..DiffNameStatusOptions::default()
        };
        let entries = if let Some(parent) = commit.parents.first().copied() {
            let parent_tree = git.read_commit(&parent)?.tree;
            diff_name_status_trees_with_options(
                git.object_database(),
                git.object_format(),
                &parent_tree,
                &commit.tree,
                options,
            )?
        } else {
            diff_name_status_empty_tree_with_options(
                git.object_database(),
                git.object_format(),
                &commit.tree,
                options,
            )?
        };
        let mut output = LimitedDiffWriter::new(MAX_DIFF_BYTES);
        let render_options = DiffRenderOptions {
            db: git.object_database(),
            lazy_fetch: None,
            worktree_root: None,
            use_worktree_new: false,
            format: git.object_format(),
            abbrev: 40,
            src_prefix: "a/",
            dst_prefix: "b/",
            context: 3,
            userdiff: None,
            funcname: None,
            colors: None,
            word_diff: None,
            line_indicators: LineIndicators::default(),
            suppress_blank_empty: false,
            no_index_contents: None,
            submodule_format: SubmoduleDiffFormat::Short,
            submodule_dirt: None,
            ws_error: None,
            color_moved: None,
            interhunk: 0,
            ws_ignore: WsIgnore::default(),
            diff_algorithm: DiffAlgorithm::Myers,
            ignore_blank_lines: false,
            ignore_regexes: &[],
            line_ranges: None,
            indent_heuristic: true,
            binary: false,
            anchors: &[],
            allow_textconv: false,
            big_file_threshold: 512 * 1024 * 1024,
            submodule_render: None,
        };
        for entry in &entries {
            write_diff_patch_entry(&mut output, entry, render_options)?;
        }
        Ok(DiffResponse {
            patch: String::from_utf8_lossy(output.as_bytes()).into_owned(),
            truncated: output.truncated(),
        })
    })
    .await?;
    Ok(Json(response))
}

struct LimitedDiffWriter {
    bytes: Vec<u8>,
    limit: usize,
    truncated: bool,
}

impl LimitedDiffWriter {
    fn new(limit: usize) -> Self {
        Self {
            bytes: Vec::with_capacity(limit.min(64 * 1024)),
            limit,
            truncated: false,
        }
    }

    fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    fn truncated(&self) -> bool {
        self.truncated
    }
}

impl Write for LimitedDiffWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let remaining = self.limit.saturating_sub(self.bytes.len());
        let copied = bytes.len().min(remaining);
        self.bytes.extend_from_slice(&bytes[..copied]);
        if copied != bytes.len() {
            self.truncated = true;
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
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
        .or_else(|| repository.default_branch.clone())
        .ok_or_else(ApiError::not_found)?;
    let path = state.repository_path(&repository);
    let (commit_oid, tree_oid) = read_git(path.clone(), move |git| {
        let commit_oid = git.peel_to_commit_oid(git.rev_parse(&revision)?)?;
        let commit = git.read_commit(&commit_oid)?;
        Ok((commit_oid.to_hex(), commit.tree))
    })
    .await?;
    if let Some(cached) = state.cached_stats(repository.id, &commit_oid).await? {
        return Ok(Json(cached));
    }
    let computed = read_git(path, move |git| compute_stats(git, tree_oid)).await?;
    state
        .cache_stats(repository.id, &commit_oid, &computed)
        .await?;
    Ok(Json(computed))
}

pub(super) async fn read_repository_overview(
    state: &RepositoryState,
    repository: &repository::Model,
    start_date: NaiveDate,
    end_date: NaiveDate,
) -> Result<GitOverview, ApiError> {
    let cache_key = format!(
        "{}:{}:{}:{start_date}:{end_date}",
        repository.storage_key,
        repository.updated_at,
        repository.default_branch.as_deref().unwrap_or("")
    );
    if let Some(cached) = state.cached_overview(repository.id, &cache_key).await? {
        return Ok(cached);
    }

    let path = state.repository_path(repository);
    let default_reference = repository
        .default_branch
        .as_ref()
        .map(|branch| format!("refs/heads/{branch}"));
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
            if default_reference.as_deref() == Some(reference.name.as_str()) {
                default_index = Some(roots.len());
            }
            roots.push(commit_oid);
        }
        let head = default_index.map(|index| roots[index].to_hex());
        let mut activity = BTreeMap::new();
        if !roots.is_empty() {
            git.rev_graph().stream_reachable_commits(
                roots,
                ReachableCommitOptions::new(),
                |metadata| {
                    let commit = git.read_commit(&metadata.oid)?;
                    if let Some(date) = commit.author_signature().as_ref().and_then(authored_on)
                        && date >= start_date
                        && date <= end_date
                    {
                        *activity.entry(date).or_default() += 1;
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
    state
        .cache_overview(repository.id, &cache_key, &overview)
        .await?;
    Ok(overview)
}

/// The calendar day a commit was authored on, in the author's own time zone,
/// which is the day the activity graph counts it under.
fn authored_on(signature: &sley::Signature) -> Option<NaiveDate> {
    let timestamp = DateTime::<Utc>::from_timestamp(signature.time.seconds, 0)?;
    Some(
        (timestamp + Duration::minutes(i64::from(signature.time.timezone_offset_minutes)))
            .date_naive(),
    )
}

/// A commit listed under one day of a namespace's activity graph.
#[derive(Serialize)]
pub struct DayCommitResponse {
    oid: String,
    short_oid: String,
    title: String,
    author_name: String,
    timestamp: i64,
}

/// Commits reachable from any branch that were authored on `date`, newest
/// first: the ones the activity graph counted for that day.
pub(super) async fn read_commits_authored_on(
    state: &RepositoryState,
    repository: &repository::Model,
    date: NaiveDate,
) -> Result<Vec<DayCommitResponse>, ApiError> {
    let path = state.repository_path(repository);
    let mut commits = read_git(path, move |git| {
        let mut roots = Vec::new();
        for reference in git.references().list_refs_with_prefix("refs/heads/")? {
            if let ReferenceTarget::Direct(target) = reference.target {
                roots.push(git.peel_to_commit_oid(target)?);
            }
        }
        let mut commits = Vec::new();
        if roots.is_empty() {
            return Ok(commits);
        }
        git.rev_graph().stream_reachable_commits(
            roots,
            ReachableCommitOptions::new(),
            |metadata| {
                let commit = git.read_commit(&metadata.oid)?;
                let signature = commit.author_signature();
                if signature.as_ref().and_then(authored_on) == Some(date) {
                    let author = signature_response(signature, &commit.author);
                    let message = String::from_utf8_lossy(&commit.message);
                    let oid = metadata.oid.to_hex();
                    commits.push(DayCommitResponse {
                        short_oid: oid[..12.min(oid.len())].to_owned(),
                        oid,
                        title: message.trim().lines().next().unwrap_or_default().to_owned(),
                        author_name: author.name,
                        timestamp: author.timestamp,
                    });
                }
                Ok(StreamControl::Continue)
            },
        )?;
        Ok(commits)
    })
    .await?;
    commits.sort_by_key(|commit| std::cmp::Reverse(commit.timestamp));
    Ok(commits)
}

pub(super) async fn warm_repository_analysis(
    state: &RepositoryState,
    repository: &repository::Model,
) -> Result<(), ApiError> {
    let _permit = state
        .analysis_slots
        .clone()
        .acquire_owned()
        .await
        .map_err(ApiError::internal)?;
    let end_date = Utc::now().date_naive();
    let start_date = activity_start_date(end_date, DEFAULT_REPOSITORY_ACTIVITY_DAYS)?;
    let overview = read_repository_overview(state, repository, start_date, end_date).await?;
    let Some(commit_oid) = overview.head else {
        return Ok(());
    };

    // Runs ahead of the cache check below: the stats may already be warm while
    // the push that triggered this pass was the one that added the logo.
    if let Err(error) = super::icon::detect_icon(state, repository).await {
        tracing::warn!(
            %error,
            repository_id = %repository.id,
            "could not detect a repository icon"
        );
    }

    let need_stats = state
        .cached_stats(repository.id, &commit_oid)
        .await?
        .is_none();
    let need_commit_count = state
        .cached_commit_count(repository.id, &commit_oid)
        .await?
        .is_none();
    if !need_stats && !need_commit_count {
        return Ok(());
    }

    let path = state.repository_path(repository);
    let revision = commit_oid.clone();
    let (stats, commit_count) = read_git(path, move |git| {
        let oid = git.peel_to_commit_oid(git.rev_parse(&revision)?)?;
        let stats = if need_stats {
            Some(compute_stats(git, git.read_commit(&oid)?.tree)?)
        } else {
            None
        };
        let commit_count = if need_commit_count {
            Some(count_reachable_commits(git, oid)?)
        } else {
            None
        };
        Ok((stats, commit_count))
    })
    .await?;
    if let Some(stats) = stats {
        state
            .cache_stats(repository.id, &commit_oid, &stats)
            .await?;
    }
    if let Some(commit_count) = commit_count {
        state
            .cache_commit_count(repository.id, &commit_oid, commit_count)
            .await?;
    }
    Ok(())
}

pub(super) async fn readable_repository(
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

fn count_reachable_commits(repository: &GitRepository, tip: ObjectId) -> Result<usize, GitError> {
    let mut count = 0;
    repository.rev_graph().stream_reachable_commits(
        [tip],
        ReachableCommitOptions::new(),
        |_| {
            count += 1;
            Ok(StreamControl::Continue)
        },
    )?;
    Ok(count)
}

pub(crate) async fn read_git<T, F>(path: PathBuf, operation: F) -> Result<T, ApiError>
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

pub(super) fn normalize_browse_path(path: &str) -> Result<String, ApiError> {
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

fn commit_line_stats(
    repository: &GitRepository,
    wanted: &[ObjectId],
) -> BTreeMap<String, (usize, usize)> {
    wanted
        .iter()
        .filter_map(|oid| diff_tree_stats(repository, *oid).map(|stats| (oid.to_hex(), stats)))
        .collect()
}

fn diff_tree_stats(repository: &GitRepository, oid: ObjectId) -> Option<(usize, usize)> {
    let commit = repository.read_commit(&oid).ok()?;
    let options = DiffNameStatusOptions {
        detect_renames: true,
        ..DiffNameStatusOptions::default()
    };
    let entries = if let Some(parent) = commit.parents.first().copied() {
        let parent_tree = repository.read_commit(&parent).ok()?.tree;
        diff_name_status_trees_with_options(
            repository.object_database(),
            repository.object_format(),
            &parent_tree,
            &commit.tree,
            options,
        )
        .ok()?
    } else {
        diff_name_status_empty_tree_with_options(
            repository.object_database(),
            repository.object_format(),
            &commit.tree,
            options,
        )
        .ok()?
    };
    let stats =
        collect_diff_stat_entries(&entries, repository.object_database(), None, false, None)
            .ok()?;
    let mut insertions = 0;
    let mut deletions = 0;
    for stat in stats {
        if let LineStats::Text { inserted, deleted } = stat.stats {
            insertions += inserted;
            deletions += deleted;
        }
    }
    Some((insertions, deletions))
}

fn commit_response(repository: &GitRepository, oid: ObjectId) -> sley::Result<CommitResponse> {
    let commit = repository.read_commit(&oid)?;
    let author = signature_response(commit.author_signature(), &commit.author);
    let committer = signature_response(commit.committer_signature(), &commit.committer);
    let message = String::from_utf8_lossy(&commit.message).trim().to_owned();
    let title = message.lines().next().unwrap_or_default().to_owned();
    let oid_hex = oid.to_hex();
    let (verification, signing_fingerprint) =
        match verify_ssh_commit_signature(&repository.read_object(&oid)?.body) {
            SshCommitSignature::Unsigned => (None, None),
            SshCommitSignature::Invalid => (
                Some(CommitVerificationResponse {
                    verified: false,
                    reason: "invalid",
                    signer: None,
                    fingerprint: None,
                }),
                None,
            ),
            SshCommitSignature::Valid(fingerprint) => (
                Some(CommitVerificationResponse {
                    verified: false,
                    reason: "unknown_key",
                    signer: None,
                    fingerprint: Some(fingerprint.clone()),
                }),
                Some(fingerprint),
            ),
        };
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
        insertions: 0,
        deletions: 0,
        refs: Vec::new(),
        verification,
        signing_fingerprint,
    })
}

enum SshCommitSignature {
    Unsigned,
    Invalid,
    Valid(String),
}

fn verify_ssh_commit_signature(body: &[u8]) -> SshCommitSignature {
    let Some((payload, pem)) = ssh_signed_payload(body) else {
        return SshCommitSignature::Unsigned;
    };
    if !pem.starts_with(b"-----BEGIN SSH SIGNATURE-----") {
        return SshCommitSignature::Unsigned;
    }
    let Ok(signature) = SshSig::from_pem(&pem) else {
        return SshCommitSignature::Invalid;
    };
    let public_key = PublicKey::new(signature.public_key().clone(), "");
    if public_key.verify("git", &payload, &signature).is_err() {
        return SshCommitSignature::Invalid;
    }
    SshCommitSignature::Valid(public_key.fingerprint(HashAlg::Sha256).to_string())
}

fn ssh_signed_payload(body: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
    let header_end = body.windows(2).position(|bytes| bytes == b"\n\n")?;
    let mut cursor = 0usize;
    while cursor <= header_end {
        let line_end = body[cursor..=header_end]
            .iter()
            .position(|byte| *byte == b'\n')
            .map(|offset| cursor + offset)?;
        let line = &body[cursor..line_end];
        let value = line
            .strip_prefix(b"gpgsig ")
            .or_else(|| line.strip_prefix(b"gpgsig-sha256 "));
        let Some(first_line) = value else {
            cursor = line_end + 1;
            continue;
        };

        let mut pem = Vec::with_capacity(512);
        pem.extend_from_slice(first_line);
        pem.push(b'\n');
        let mut field_end = line_end + 1;
        while field_end <= header_end && body[field_end] == b' ' {
            let continuation_end = body[field_end..=header_end]
                .iter()
                .position(|byte| *byte == b'\n')
                .map(|offset| field_end + offset)?;
            pem.extend_from_slice(&body[field_end + 1..continuation_end]);
            pem.push(b'\n');
            field_end = continuation_end + 1;
        }

        let mut payload = Vec::with_capacity(body.len() - (field_end - cursor));
        payload.extend_from_slice(&body[..cursor]);
        payload.extend_from_slice(&body[field_end..]);
        return Some((payload, pem));
    }
    None
}

/// Every tag, with annotated tags peeled to the commit they release.
fn tag_refs(repository: &GitRepository) -> sley::Result<Vec<RefResponse>> {
    Ok(repository
        .query_tags(TagQueryOptions::new())
        .map_err(|error| GitError::Command(error.to_string()))?
        .entries
        .into_iter()
        .filter_map(|entry| {
            let ReferenceTarget::Direct(oid) = entry.reference.target else {
                return None;
            };
            // An annotated tag points at a tag object, so the commit it
            // releases only comes out of peeling it.
            let commit_oid = repository.peel_to_commit_oid(oid).ok()?;
            Some(RefResponse {
                name: entry.name,
                oid: oid.to_hex(),
                commit_oid: commit_oid.to_hex(),
            })
        })
        .collect())
}

/// Tag decorations keyed by the commit hex they point at.
fn tag_decorations(
    repository: &GitRepository,
) -> sley::Result<BTreeMap<String, Vec<CommitRefResponse>>> {
    let mut decorations: BTreeMap<String, Vec<CommitRefResponse>> = BTreeMap::new();
    for tag in tag_refs(repository)? {
        decorations
            .entry(tag.commit_oid)
            .or_default()
            .push(CommitRefResponse {
                kind: "tag",
                name: tag.name,
                prerelease: false,
                latest: false,
                published_at: None,
                commits_since_previous: None,
            });
    }
    for tags in decorations.values_mut() {
        tags.sort_unstable_by(|left, right| compare_ref_names(&left.name, &right.name));
    }
    Ok(decorations)
}

/// Associate valid SSH signatures with the account that registered the key.
async fn attach_commit_verifications(
    state: &RepositoryState,
    commits: &mut [CommitResponse],
) -> Result<(), ApiError> {
    let fingerprints = commits
        .iter()
        .filter_map(|commit| commit.signing_fingerprint.clone())
        .collect::<HashSet<_>>();
    if fingerprints.is_empty() {
        return Ok(());
    }

    let keys = ssh_key_entity::Entity::find()
        .filter(ssh_key_entity::Column::Fingerprint.is_in(fingerprints))
        .all(state.identity().database())
        .await?;
    let user_ids = keys.iter().map(|key| key.user_id).collect::<HashSet<_>>();
    let usernames = user::Entity::find()
        .filter(user::Column::Id.is_in(user_ids))
        .all(state.identity().database())
        .await?
        .into_iter()
        .map(|owner| (owner.id, owner.username))
        .collect::<HashMap<_, _>>();
    let signers = keys
        .into_iter()
        .filter_map(|key| {
            usernames
                .get(&key.user_id)
                .cloned()
                .map(|username| (key.fingerprint, username))
        })
        .collect::<HashMap<_, _>>();

    for commit in commits {
        let Some(fingerprint) = &commit.signing_fingerprint else {
            continue;
        };
        let Some(signer) = signers.get(fingerprint) else {
            continue;
        };
        let Some(verification) = &mut commit.verification else {
            continue;
        };
        verification.verified = true;
        verification.reason = "verified";
        verification.signer = Some(signer.clone());
    }
    Ok(())
}

/// Overlay releases published for these commits and calculate predecessor counts.
async fn attach_release_refs(
    state: &RepositoryState,
    repository: &repository::Model,
    commits: &mut [CommitResponse],
) -> Result<(), ApiError> {
    if commits.is_empty() {
        return Ok(());
    }
    let releases = repository_release::Entity::find()
        .filter(repository_release::Column::RepositoryId.eq(repository.id))
        .order_by_desc(repository_release::Column::PublishedAt)
        .all(state.identity().database())
        .await?;
    let latest_id = releases
        .iter()
        .find(|release| !release.prerelease)
        .map(|release| release.id);
    let path = state.repository_path(repository);
    let mut decorated = false;
    for (index, release) in releases.iter().enumerate() {
        let Some(commit) = commits
            .iter_mut()
            .find(|commit| commit.oid == release.target_oid)
        else {
            continue;
        };
        // Only releases on this page cost a count, and the predecessor is the
        // next one published before it.
        let commits_since_previous = match releases.get(index + 1) {
            Some(previous) => {
                commits_between(&path, &previous.target_oid, &release.target_oid).await
            }
            None => None,
        };
        commit.refs.push(CommitRefResponse {
            kind: "release",
            name: release.title.clone(),
            prerelease: release.prerelease,
            latest: Some(release.id) == latest_id,
            published_at: Some(release.published_at),
            commits_since_previous,
        });
        decorated = true;
    }
    if decorated {
        for commit in commits {
            // Releases lead, tags follow, each already in name order.
            commit
                .refs
                .sort_by_key(|reference| u8::from(reference.kind == "tag"));
        }
    }
    Ok(())
}

/// Commits reachable from `to` but not `from`, or `None` when either revision
/// cannot be resolved.
async fn commits_between(path: &Path, from: &str, to: &str) -> Option<usize> {
    let path = path.to_owned();
    let from = from.to_owned();
    let to = to.to_owned();
    read_git(path, move |repository| {
        let mut selection = sley_rev::RevisionSelection::new();
        selection.add_spec(format!("{from}..{to}"))?;
        let resolved = selection.resolve(
            repository.git_dir(),
            repository.object_format(),
            repository.object_database(),
        )?;
        Ok(resolved
            .selected_commit_oids(
                repository.git_dir(),
                repository.object_format(),
                repository.object_database(),
                false,
            )?
            .len())
    })
    .await
    .ok()
}

/// Order ref names so embedded numbers compare numerically: `v0.9.0` sorts
/// before `v0.10.0`, which a plain string compare gets backwards.
fn compare_ref_names(left: &str, right: &str) -> Ordering {
    let mut left_parts = ref_name_parts(left);
    let mut right_parts = ref_name_parts(right);
    loop {
        match (left_parts.next(), right_parts.next()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(left), Some(right)) => {
                let ordering = match (left.parse::<u64>(), right.parse::<u64>()) {
                    (Ok(left), Ok(right)) => left.cmp(&right),
                    _ => left.cmp(right),
                };
                if ordering != Ordering::Equal {
                    return ordering;
                }
            }
        }
    }
}

/// Split a ref name into alternating digit and non-digit runs.
fn ref_name_parts(name: &str) -> impl Iterator<Item = &str> {
    let mut rest = name;
    std::iter::from_fn(move || {
        if rest.is_empty() {
            return None;
        }
        let numeric = rest.starts_with(|character: char| character.is_ascii_digit());
        let end = rest
            .find(|character: char| character.is_ascii_digit() != numeric)
            .unwrap_or(rest.len());
        let (part, tail) = rest.split_at(end);
        rest = tail;
        Some(part)
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

pub(super) fn activity_start_date(end_date: NaiveDate, days: u16) -> Result<NaiveDate, ApiError> {
    if !(1..=MAX_REPOSITORY_ACTIVITY_DAYS).contains(&days) {
        return Err(ApiError::bad_request(format!(
            "Activity window must be between 1 and {MAX_REPOSITORY_ACTIVITY_DAYS} days."
        )));
    }
    Ok(end_date - Duration::days(i64::from(days - 1)))
}

pub(super) fn activity_response(
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

#[cfg(test)]
mod tests {
    use std::{
        fs,
        io::Write as _,
        path::PathBuf,
        process::Command,
        time::{Duration, Instant},
    };

    use russh::keys::ssh_key::{HashAlg, LineEnding, PrivateKey, private::Ed25519Keypair};
    use sley::{
        BString, CommitObject, GitObjectType, ObjectId, Repository as GitRepository, TreeEditor,
    };
    use uuid::Uuid;

    use super::{
        ArchiveFileWriter, MAX_SOURCE_ARCHIVE_BYTES, SshCommitSignature, count_reachable_commits,
        read_submodule_metadata, verify_ssh_commit_signature,
    };

    fn native_repository() -> (PathBuf, GitRepository) {
        let path = std::env::temp_dir().join(format!("gitadel-submodule-test-{}", Uuid::new_v4()));
        let repository = GitRepository::init(&path).expect("initialize native repository");
        (path, repository)
    }

    fn native_tree(repository: &GitRepository, config_mode: u32, config: &[u8]) -> ObjectId {
        let config_oid = repository
            .write_blob(config.to_vec())
            .expect("write gitmodules blob");
        let mut tree = TreeEditor::new();
        tree.upsert_raw(BString::from(b".gitmodules"), config_mode, config_oid);
        repository.write_tree(tree).expect("write repository tree")
    }

    fn native_commit(
        repository: &GitRepository,
        tree: ObjectId,
        parent: Option<ObjectId>,
    ) -> ObjectId {
        let commit = CommitObject {
            tree,
            parents: parent.into_iter().collect(),
            author: b"Gitadel Test <gitadel@example.test> 1700000000 +0000".to_vec(),
            committer: b"Gitadel Test <gitadel@example.test> 1700000000 +0000".to_vec(),
            encoding: None,
            message: b"submodule test\n".to_vec(),
        };
        repository
            .write_raw_object(GitObjectType::Commit, commit.write())
            .expect("write repository commit")
    }

    #[test]
    fn source_archive_writer_rejects_output_over_limit() {
        let path =
            std::env::temp_dir().join(format!("gitadel-archive-limit-test-{}", Uuid::new_v4()));
        let mut writer = ArchiveFileWriter {
            file: fs::File::create(&path).expect("create archive output"),
            bytes_written: MAX_SOURCE_ARCHIVE_BYTES,
            deadline: Instant::now() + Duration::from_secs(1),
        };

        let error = writer
            .write_all(&[0])
            .expect_err("reject oversized archive");
        fs::remove_file(path).expect("remove archive output");

        assert_eq!(error.kind(), std::io::ErrorKind::FileTooLarge);
    }

    #[test]
    fn source_archive_writer_rejects_expired_generation() {
        let path =
            std::env::temp_dir().join(format!("gitadel-archive-timeout-test-{}", Uuid::new_v4()));
        let mut writer = ArchiveFileWriter {
            file: fs::File::create(&path).expect("create archive output"),
            bytes_written: 0,
            deadline: Instant::now(),
        };

        let error = writer.write_all(&[0]).expect_err("reject expired archive");
        fs::remove_file(path).expect("remove archive output");

        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
    }

    #[test]
    fn submodule_metadata_uses_the_selected_commit_and_full_paths() {
        let (path, repository) = native_repository();
        let old_tree = native_tree(
            &repository,
            0o100644,
            b"[submodule \"Chorus\"]\n\tpath = vendor/Chorus\n\turl = https://example.test/chorus-old.git\n\tbranch = old\n[submodule \"Nested tools\"]\n\tpath = vendor/tools/nested\n\turl = https://example.test/tools.git\n",
        );
        let old_commit = native_commit(&repository, old_tree, None);
        let new_tree = native_tree(
            &repository,
            0o100644,
            b"[submodule \"Chorus\"]\n\tpath = vendor/Chorus\n\turl = https://example.test/chorus-old.git\n[submodule \"Chorus\"]\n\turl = https://example.test/chorus-new.git\n\tbranch = new\n[submodule \"Nested tools\"]\n\tpath = vendor/tools/nested\n\turl = https://example.test/tools.git\n",
        );
        let new_commit = native_commit(&repository, new_tree, Some(old_commit));

        let old_metadata = read_submodule_metadata(
            &repository,
            repository
                .read_commit(&old_commit)
                .expect("read old commit")
                .tree,
        )
        .expect("read old submodule metadata");
        let new_metadata = read_submodule_metadata(
            &repository,
            repository
                .read_commit(&new_commit)
                .expect("read new commit")
                .tree,
        )
        .expect("read new submodule metadata");
        assert_eq!(
            old_metadata
                .get("vendor/Chorus")
                .and_then(|metadata| metadata.url.as_deref()),
            Some("https://example.test/chorus-old.git")
        );
        assert_eq!(
            new_metadata
                .get("vendor/Chorus")
                .and_then(|metadata| metadata.url.as_deref()),
            Some("https://example.test/chorus-new.git")
        );
        assert_eq!(
            new_metadata
                .get("vendor/Chorus")
                .and_then(|metadata| metadata.branch.as_deref()),
            Some("new")
        );
        assert_eq!(
            new_metadata
                .get("vendor/tools/nested")
                .map(|metadata| metadata.name.as_str()),
            Some("Nested tools")
        );
        fs::remove_dir_all(path).expect("remove native repository");
    }

    #[test]
    fn submodule_metadata_ignores_invalid_non_utf8_symlink_and_oversized_config() {
        let (path, repository) = native_repository();
        for (mode, config) in [
            (
                0o100644,
                b"[submodule \"broken\"\n\tpath = vendor/Chorus\n".as_slice(),
            ),
            (0o100644, b"[submodule \"broken\"]\n\xff".as_slice()),
            (
                0o120000,
                b"[submodule \"symlink\"]\n\tpath = vendor/Chorus\n".as_slice(),
            ),
        ] {
            let tree = native_tree(&repository, mode, config);
            assert!(
                read_submodule_metadata(&repository, tree)
                    .expect("read malformed submodule metadata")
                    .is_empty()
            );
        }
        let mut oversized =
            b"[submodule \"Large\"]\npath = vendor/Chorus\nurl = https://example.test/large.git\n#"
                .to_vec();
        oversized.resize(super::MAX_GITMODULES_BYTES + 1, b'x');
        let tree = native_tree(&repository, 0o100644, &oversized);
        assert!(
            read_submodule_metadata(&repository, tree)
                .expect("read oversized submodule metadata")
                .is_empty()
        );
        fs::remove_dir_all(path).expect("remove native repository");
    }

    #[test]
    fn submodule_metadata_does_not_follow_committed_include() {
        let (path, repository) = native_repository();
        let external = path.join("external.gitmodules");
        fs::write(
            &external,
            b"[submodule \"External\"]\n\tpath = external\n\turl = https://example.test/external.git\n",
        )
        .expect("write external module");
        let config = format!(
            "[include]\n\tpath = {}\n[submodule \"Local\"]\n\tpath = local\n\turl = https://example.test/local.git\n",
            external.display()
        );
        let tree = native_tree(&repository, 0o100644, config.as_bytes());
        let metadata =
            read_submodule_metadata(&repository, tree).expect("read committed submodule metadata");
        assert_eq!(metadata.len(), 1);
        assert!(metadata.contains_key("local"));
        assert!(!metadata.contains_key("external"));
        fs::remove_dir_all(path).expect("remove native repository");
    }

    #[test]
    fn lfs_pointer_preserves_zero_size_with_extension_fields() {
        let pointer = b"version https://git-lfs.github.com/spec/v1\next-0-test sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\noid sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\nsize 0\n";
        assert_eq!(
            super::lfs_pointer_metadata(pointer),
            Some(super::LfsPointerMetadata {
                oid: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                size: 0,
            })
        );
    }

    #[test]
    fn lfs_pointer_rejects_invalid_object_identity() {
        let pointer =
            b"version https://git-lfs.github.com/spec/v1\noid sha256:not-a-hash\nsize 12345\n";
        assert_eq!(super::lfs_pointer_metadata(pointer), None);
    }

    #[test]
    fn lfs_pointer_rejects_ambiguous_size() {
        let pointer = b"version https://git-lfs.github.com/spec/v1\noid sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\nsize 100\nsize 200\n";
        assert_eq!(super::lfs_pointer_metadata(pointer), None);
    }

    #[test]
    fn lfs_pointer_preserves_maximum_u64_size() {
        let pointer = format!(
            "version https://git-lfs.github.com/spec/v1\noid sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\nsize {0}\n",
            u64::MAX
        );
        assert_eq!(
            super::lfs_pointer_metadata(pointer.as_bytes())
                .expect("parse maximum LFS pointer size")
                .size,
            u64::MAX
        );
    }

    fn signed_commit() -> Vec<u8> {
        let payload = b"tree 0000000000000000000000000000000000000000\nauthor Alice <alice@example.com> 1 +0000\ncommitter Alice <alice@example.com> 1 +0000\n\nSigned commit\n";
        let private_key = PrivateKey::from(Ed25519Keypair::from_seed(&[7; 32]));
        let signature = private_key
            .sign("git", HashAlg::Sha512, payload)
            .expect("sign commit")
            .to_pem(LineEnding::LF)
            .expect("encode signature");
        let header_end = payload
            .windows(2)
            .position(|bytes| bytes == b"\n\n")
            .expect("commit headers");
        let mut body = Vec::with_capacity(payload.len() + signature.len() + 32);
        body.extend_from_slice(&payload[..=header_end]);
        for (index, line) in signature.lines().enumerate() {
            if index == 0 {
                body.extend_from_slice(b"gpgsig ");
            } else {
                body.push(b' ');
            }
            body.extend_from_slice(line.as_bytes());
            body.push(b'\n');
        }
        body.extend_from_slice(&payload[header_end + 1..]);
        body
    }
    #[test]
    fn counts_every_commit_reachable_from_default_branch() {
        let path =
            std::env::temp_dir().join(format!("gitadel-commit-count-test-{}", Uuid::new_v4()));
        assert!(
            Command::new("git")
                .args(["init", "--quiet"])
                .arg(&path)
                .status()
                .expect("run git init")
                .success()
        );
        for (key, value) in [
            ("user.name", "Gitadel Test"),
            ("user.email", "gitadel@example.test"),
        ] {
            assert!(
                Command::new("git")
                    .arg("-C")
                    .arg(&path)
                    .args(["config", key, value])
                    .status()
                    .expect("configure git identity")
                    .success()
            );
        }
        for message in ["first", "second"] {
            assert!(
                Command::new("git")
                    .arg("-C")
                    .arg(&path)
                    .args(["commit", "--quiet", "--allow-empty", "-m", message])
                    .status()
                    .expect("create test commit")
                    .success()
            );
        }

        let repository = GitRepository::open(path.join(".git")).expect("open test repository");
        let tip = repository
            .peel_to_commit_oid(repository.rev_parse("HEAD").expect("resolve HEAD"))
            .expect("peel HEAD to a commit");
        assert_eq!(
            count_reachable_commits(&repository, tip).expect("count reachable commits"),
            2
        );
        fs::remove_dir_all(path).expect("remove test repository");
    }

    #[test]
    fn verifies_git_ssh_signature() {
        assert!(matches!(
            verify_ssh_commit_signature(&signed_commit()),
            SshCommitSignature::Valid(_)
        ));
    }

    #[test]
    fn rejects_signature_after_commit_changes() {
        let mut body = signed_commit();
        body.extend_from_slice(b"tampered");
        assert!(matches!(
            verify_ssh_commit_signature(&body),
            SshCommitSignature::Invalid
        ));
    }
}
