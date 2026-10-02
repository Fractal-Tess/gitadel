use std::{
    cmp::Ordering,
    collections::{BTreeMap, HashMap, HashSet},
    fs::File,
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{Arc, LazyLock, Mutex as StdMutex, PoisonError},
    time::{Duration as StdDuration, Instant},
};

use axum::{
    Json,
    body::Body,
    body::Bytes,
    extract::{Path as AxumPath, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use axum_extra::extract::cookie::CookieJar;
use chrono::{DateTime, Duration, NaiveDate, Utc};
use comrak::{Options, markdown_to_html};
use russh::keys::ssh_key::{HashAlg, PublicKey, SshSig};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder};
use serde::{Deserialize, Deserializer, Serialize};
use sha2::{Digest as _, Sha256};
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
use tokio::{
    sync::{Mutex, OwnedMutexGuard, OwnedSemaphorePermit, Semaphore},
    task::JoinSet,
};

use super::{
    RepositoryState,
    resources::{self, accessible_repositories},
};
use crate::{
    entity::{
        repository, repository_cache_entry, repository_release, ssh_key as ssh_key_entity, user,
    },
    filesystem::{create_private_directory_async, open_private_file},
    identity::ApiError,
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

/// A repository's branches and who committed on which day.
///
/// The cached form is keyed on the branch tips and holds every day from
/// `since` on, so it stays valid until a branch moves. Callers get the
/// [`window`](Self::window) they asked for, cut from it at read time.
#[derive(Clone, Deserialize, Serialize)]
pub(super) struct GitOverview {
    branch_count: usize,
    head: Option<String>,
    /// Commits per day. Derived from `authors`, so it is not stored.
    #[serde(skip)]
    pub(super) activity: BTreeMap<NaiveDate, usize>,
    /// The same commits split by who wrote them, so a profile can count only
    /// its owner's without walking the history again.
    pub(super) authors: Vec<AuthorActivity>,
    /// The first day `authors` covers; commits authored earlier were left out.
    since: NaiveDate,
}

impl GitOverview {
    /// The overview restricted to commits authored from `start_date` to
    /// `end_date`, both included.
    fn window(mut self, start_date: NaiveDate, end_date: NaiveDate) -> Self {
        self.authors
            .retain(|author| author.date >= start_date && author.date <= end_date);
        self.activity = BTreeMap::new();
        for author in &self.authors {
            *self.activity.entry(author.date).or_default() += author.count;
        }
        self
    }

    /// Whether any commit was authored on `date`, when this overview reaches
    /// back far enough to tell.
    fn has_commits_on(&self, date: NaiveDate) -> Option<bool> {
        (date >= self.since).then(|| self.authors.iter().any(|author| author.date == date))
    }
}

#[cfg(test)]
impl GitOverview {
    pub(super) fn for_tests(
        activity: BTreeMap<NaiveDate, usize>,
        authors: Vec<AuthorActivity>,
    ) -> Self {
        Self {
            branch_count: 1,
            head: None,
            activity,
            authors,
            since: NaiveDate::MIN,
        }
    }
}

/// Commits in one repository on one day by one author address, and the SSH key
/// that validly signed them, if any.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(super) struct AuthorActivity {
    pub(super) date: NaiveDate,
    pub(super) email: String,
    pub(super) signer: Option<String>,
    pub(super) count: usize,
}

/// The lowercased author address of a commit, and the fingerprint of the SSH
/// key that validly signed it.
fn commit_authorship(
    git: &GitRepository,
    oid: &ObjectId,
    signature: Option<&sley::Signature>,
) -> sley::Result<(String, Option<String>)> {
    let email = signature
        .map(|signature| String::from_utf8_lossy(signature.email.as_bytes()).to_ascii_lowercase())
        .unwrap_or_default();
    let signer = match verify_ssh_commit_signature(&git.read_object(oid)?.body) {
        SshCommitSignature::Valid(fingerprint) => Some(fingerprint),
        SshCommitSignature::Unsigned | SshCommitSignature::Invalid => None,
    };
    Ok((email, signer))
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
    let activity =
        repository_overview(&state, &repository, start_date, end_date, WalkSlot::Acquire)
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
    let path = state.repository_path(&repository);
    let read = read_git(path, move |git| {
        let commit_oid = git.peel_to_commit_oid(git.rev_parse(&revision)?)?;
        let resolved = git.resolve_path(&commit_oid.to_hex(), &requested_path)?;
        if resolved.object_type != GitObjectType::Blob {
            return Err(GitError::InvalidPath(requested_path));
        }
        // The response is a function of the commit and blob it names and of
        // the release that rendered it, so those make its validator; checking
        // it here answers a repeat view without reading the blob.
        let etag = format!(
            "W/\"{}-{}-{}\"",
            env!("CARGO_PKG_VERSION"),
            commit_oid.to_hex(),
            resolved.oid.to_hex()
        );
        if if_none_match(&headers, &etag) {
            return Ok(BlobRead::NotModified(etag));
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
        Ok(BlobRead::Found(Box::new((response, image_content, etag))))
    })
    .await?;
    let (mut response, image_content, etag) = match read {
        BlobRead::NotModified(etag) => return not_modified(&etag),
        BlobRead::Found(found) => *found,
    };
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
    // A failed preview can succeed later without the commit changing, for
    // example once an LFS object is uploaded, so it is not given a validator.
    let cacheable = response.image_error.is_none();
    let mut response = Json(response).into_response();
    if cacheable {
        insert_validator_headers(response.headers_mut(), &etag)?;
    }
    Ok(response)
}

enum BlobRead {
    NotModified(String),
    Found(Box<(BlobResponse, Option<Vec<u8>>, String)>),
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
    // Resolve the blob before reading it: its id is the validator, so a
    // client that already holds these bytes costs no blob read at all.
    let (blob_oid, size) = read_git(repository_path.clone(), move |git| {
        let resolved = git.resolve_path(&revision, &requested_path)?;
        if resolved.object_type != GitObjectType::Blob {
            return Err(GitError::InvalidPath(requested_path));
        }
        let size = git
            .read_object_header(&resolved.oid)?
            .map_or(u64::MAX, |(_, size)| size);
        Ok((resolved.oid, size))
    })
    .await?;
    let etag = format!("\"{}\"", blob_oid.to_hex());
    if if_none_match(&headers, &etag) {
        return not_modified(&etag);
    }
    let permit = raw_blob_memory(size).await?;
    let content = read_git(repository_path, move |git| git.blobs().read(blob_oid)).await?;
    let content_type = mime_guess::from_path(&mime_path)
        .first_or_octet_stream()
        .to_string();
    let mut content_length = None;
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
        content_length = Some(content.len());
        metered_body(content, permit)
    };
    let mut response = Response::new(body);
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(&content_type).map_err(ApiError::internal)?,
    );
    if let Some(length) = content_length {
        response
            .headers_mut()
            .insert(header::CONTENT_LENGTH, HeaderValue::from(length));
    }
    // `no-cache` still revalidates on every use, as a revision may be a
    // branch that moves; the validator makes that revalidation cheap.
    insert_validator_headers(response.headers_mut(), &etag)?;
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

/// Raw blobs up to this size are served without reserving memory for them.
const UNMETERED_RAW_BLOB_BYTES: u64 = 1024 * 1024;
/// How much blob content `raw` responses may hold in memory at once, in MiB.
const RAW_BLOB_MEMORY_BUDGET_MIB: u32 = 256;

/// Sley hands out a blob as one buffer, with no streaming reader, so `raw`
/// holds a file in memory until the client has received it. This budget, one
/// permit per MiB, bounds what concurrent downloads of large files can pin.
static RAW_BLOB_MEMORY: LazyLock<Arc<Semaphore>> =
    LazyLock::new(|| Arc::new(Semaphore::new(RAW_BLOB_MEMORY_BUDGET_MIB as usize)));

/// The permits a blob of `size` bytes takes from [`RAW_BLOB_MEMORY`], or none
/// for a small one. A blob larger than the whole budget takes all of it, so
/// it is served alone rather than refused.
fn raw_blob_permits(size: u64) -> Option<u32> {
    (size > UNMETERED_RAW_BLOB_BYTES).then(|| {
        u32::try_from(size.div_ceil(1024 * 1024))
            .unwrap_or(u32::MAX)
            .min(RAW_BLOB_MEMORY_BUDGET_MIB)
    })
}

/// Wait for room to hold a blob of `size` bytes in memory.
async fn raw_blob_memory(size: u64) -> Result<Option<OwnedSemaphorePermit>, ApiError> {
    match raw_blob_permits(size) {
        Some(permits) => Arc::clone(&RAW_BLOB_MEMORY)
            .acquire_many_owned(permits)
            .await
            .map(Some)
            .map_err(ApiError::internal),
        None => Ok(None),
    }
}

/// A body over `content` that keeps `permit` until the last byte is handed to
/// the connection or the client goes away.
fn metered_body(content: Vec<u8>, permit: Option<OwnedSemaphorePermit>) -> Body {
    const CHUNK_BYTES: usize = 64 * 1024;
    let Some(permit) = permit else {
        return Body::from(content);
    };
    let content = Bytes::from(content);
    Body::from_stream(futures_util::stream::unfold(
        (content, permit),
        |(mut content, permit)| async move {
            if content.is_empty() {
                return None;
            }
            let chunk = content.split_to(content.len().min(CHUNK_BYTES));
            Some((Ok::<_, std::convert::Infallible>(chunk), (content, permit)))
        },
    ))
}

/// Whether the request's `If-None-Match` names `etag`, by the weak comparison
/// conditional GETs use.
fn if_none_match(headers: &HeaderMap, etag: &str) -> bool {
    let opaque = etag.trim_start_matches("W/");
    headers
        .get_all(header::IF_NONE_MATCH)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .any(|candidate| candidate == "*" || candidate.trim_start_matches("W/") == opaque)
}

/// The validator and the cache policy that makes clients use it: store the
/// response privately, and revalidate it before every reuse.
fn insert_validator_headers(headers: &mut HeaderMap, etag: &str) -> Result<(), ApiError> {
    headers.insert(
        header::ETAG,
        HeaderValue::from_str(etag).map_err(ApiError::internal)?,
    );
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-cache"),
    );
    Ok(())
}

fn not_modified(etag: &str) -> Result<Response, ApiError> {
    let mut response = Response::new(Body::empty());
    *response.status_mut() = StatusCode::NOT_MODIFIED;
    insert_validator_headers(response.headers_mut(), etag)?;
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
    // A miss counts every line of the tree. Requests for the same commit wait
    // for one computation, and that computation takes an analysis slot like
    // the overview's, so a burst of misses cannot occupy every blocking thread.
    let _flight = single_flight(format!("stats:{}:{commit_oid}", repository.id)).await;
    let _permit = state
        .analysis_slots
        .acquire()
        .await
        .map_err(ApiError::internal)?;
    if let Some(cached) = state.cached_stats(repository.id, &commit_oid).await? {
        return Ok(Json(cached));
    }
    let computed = read_git(path, move |git| compute_stats(git, tree_oid)).await?;
    state
        .cache_stats(repository.id, &commit_oid, &computed)
        .await?;
    Ok(Json(computed))
}

/// Computations in progress, by key, for [`single_flight`].
static IN_FLIGHT: LazyLock<StdMutex<HashMap<String, Arc<Mutex<()>>>>> =
    LazyLock::new(StdMutex::default);

/// Held by the one caller allowed to compute the value for a key.
struct FlightGuard {
    key: String,
    lock: Option<Arc<Mutex<()>>>,
    held: Option<OwnedMutexGuard<()>>,
}

/// Wait until no other caller is computing the value for `key`.
///
/// The holder computes and caches; a caller that waited re-checks the cache
/// once it gets the guard and finds the holder's result there.
async fn single_flight(key: String) -> FlightGuard {
    let lock = Arc::clone(
        IN_FLIGHT
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(key.clone())
            .or_default(),
    );
    // Built before waiting, so a caller that gives up still tidies the map.
    let mut guard = FlightGuard {
        key,
        lock: Some(Arc::clone(&lock)),
        held: None,
    };
    guard.held = Some(lock.lock_owned().await);
    guard
}

impl Drop for FlightGuard {
    fn drop(&mut self) {
        let mut in_flight = IN_FLIGHT.lock().unwrap_or_else(PoisonError::into_inner);
        self.held = None;
        self.lock = None;
        // Only the map still refers to the lock: nobody holds or awaits it.
        if in_flight
            .get(&self.key)
            .is_some_and(|lock| Arc::strong_count(lock) == 1)
        {
            in_flight.remove(&self.key);
        }
    }
}

pub(super) async fn read_repository_overview(
    state: &RepositoryState,
    repository: &repository::Model,
    start_date: NaiveDate,
    end_date: NaiveDate,
) -> Result<GitOverview, ApiError> {
    // Every caller of this entry point already runs under an analysis slot.
    repository_overview(state, repository, start_date, end_date, WalkSlot::Held).await
}

/// Whether the caller of [`repository_overview`] already holds one of the
/// analysis slots a history walk runs under.
#[derive(Clone, Copy)]
enum WalkSlot {
    Held,
    Acquire,
}

/// How long the commits authored on one day stay cached. The cache table only
/// admits the kinds it was created with, so they are stored beside the
/// overview, under the same kind, and told apart by their key.
const DAY_COMMITS_CACHE_LIFETIME: StdDuration = StdDuration::from_secs(7 * 24 * 60 * 60);

/// The branches of a repository at one moment: what its history-derived
/// caches are keyed on, and where a walk of that history starts.
struct BranchTips {
    /// Digest over every branch name and the object it points at.
    digest: String,
    branch_count: usize,
    tips: Vec<(String, ObjectId)>,
}

impl BranchTips {
    fn read(git: &GitRepository) -> sley::Result<Self> {
        let references = git.references().list_refs_with_prefix("refs/heads/")?;
        let branch_count = references.len();
        let mut tips = references
            .into_iter()
            .filter_map(|reference| match reference.target {
                ReferenceTarget::Direct(target) => Some((reference.name, target)),
                ReferenceTarget::Symbolic(_) => None,
            })
            .collect::<Vec<_>>();
        tips.sort_unstable_by(|left, right| left.0.cmp(&right.0));
        let mut digest = Sha256::new();
        digest.update(branch_count.to_le_bytes());
        for (name, target) in &tips {
            digest.update(name.as_bytes());
            digest.update(b"\0");
            digest.update(target.to_hex().as_bytes());
            digest.update(b"\n");
        }
        Ok(Self {
            digest: hex::encode(digest.finalize()),
            branch_count,
            tips,
        })
    }

    /// Entries cached for other branch states start with a different digest.
    fn overview_key(&self, repository: &repository::Model) -> String {
        format!(
            "{}:v3:{}",
            self.digest,
            repository.default_branch.as_deref().unwrap_or("")
        )
    }

    fn day_key(&self, date: NaiveDate) -> String {
        format!("{}:day:{date}", self.digest)
    }
}

/// Drop the history caches of every branch state but the current one, which
/// nothing will ask for again.
async fn prune_stale_history_caches(
    state: &RepositoryState,
    repository_id: uuid::Uuid,
    tips: &BranchTips,
) -> Result<(), ApiError> {
    repository_cache_entry::Entity::delete_many()
        .filter(repository_cache_entry::Column::RepositoryId.eq(repository_id))
        .filter(repository_cache_entry::Column::Kind.eq(super::CACHE_KIND_OVERVIEW))
        .filter(repository_cache_entry::Column::CacheKey.not_like(format!("{}:%", tips.digest)))
        .exec(state.identity().database())
        .await?;
    Ok(())
}

/// Commits per author and day from `since` on, over every commit reachable
/// from a branch.
fn walk_overview(
    git: &GitRepository,
    tips: &BranchTips,
    default_reference: Option<&str>,
    since: NaiveDate,
) -> sley::Result<GitOverview> {
    let mut roots = Vec::with_capacity(tips.tips.len());
    let mut default_index = None;
    for (name, target) in &tips.tips {
        let commit_oid = git.peel_to_commit_oid(*target)?;
        if default_reference == Some(name.as_str()) {
            default_index = Some(roots.len());
        }
        roots.push(commit_oid);
    }
    let head = default_index.map(|index: usize| roots[index].to_hex());
    let mut authors = BTreeMap::<(NaiveDate, String, Option<String>), usize>::new();
    if !roots.is_empty() {
        git.rev_graph().stream_reachable_commits(
            roots,
            ReachableCommitOptions::new(),
            |metadata| {
                let commit = git.read_commit(&metadata.oid)?;
                let signature = commit.author_signature();
                if let Some(date) = signature.as_ref().and_then(authored_on)
                    && date >= since
                {
                    let (email, signer) =
                        commit_authorship(git, &metadata.oid, signature.as_ref())?;
                    *authors.entry((date, email, signer)).or_default() += 1;
                }
                Ok(StreamControl::Continue)
            },
        )?;
    }
    Ok(GitOverview {
        branch_count: tips.branch_count,
        head,
        activity: BTreeMap::new(),
        authors: authors
            .into_iter()
            .map(|((date, email, signer), count)| AuthorActivity {
                date,
                email,
                signer,
                count,
            })
            .collect(),
        since,
    })
}

async fn repository_overview(
    state: &RepositoryState,
    repository: &repository::Model,
    start_date: NaiveDate,
    end_date: NaiveDate,
    slot: WalkSlot,
) -> Result<GitOverview, ApiError> {
    let path = state.repository_path(repository);
    let tips = read_git(path.clone(), BranchTips::read).await?;
    // Keyed on the branch tips and not on the dates asked for: the walk over
    // all history happens once per push, and every window, on any later day,
    // is cut from its result.
    let cache_key = tips.overview_key(repository);
    // An entry that starts after the window asked for cannot answer it.
    let cached = async || {
        Ok::<_, ApiError>(
            state
                .cached_overview(repository.id, &cache_key)
                .await?
                .filter(|overview| start_date >= overview.since),
        )
    };
    if let Some(overview) = cached().await? {
        return Ok(overview.window(start_date, end_date));
    }

    let _permit = match slot {
        WalkSlot::Held => None,
        WalkSlot::Acquire => Some(
            state
                .analysis_slots
                .acquire()
                .await
                .map_err(ApiError::internal)?,
        ),
    };
    let _flight = single_flight(format!("overview:{}:{cache_key}", repository.id)).await;
    if let Some(overview) = cached().await? {
        return Ok(overview.window(start_date, end_date));
    }

    // Activity windows end today and reach back at most a year, so that much
    // history answers every window asked for while these tips stand. Older
    // commits are left out to keep the entry small.
    let since = start_date
        .min(Utc::now().date_naive() - Duration::days(i64::from(MAX_REPOSITORY_ACTIVITY_DAYS)));
    let default_reference = repository
        .default_branch
        .as_ref()
        .map(|branch| format!("refs/heads/{branch}"));
    let (tips, overview) = read_git(path, move |git| {
        let overview = walk_overview(git, &tips, default_reference.as_deref(), since)?;
        Ok((tips, overview))
    })
    .await?;
    state
        .cache_overview(repository.id, &cache_key, &overview)
        .await?;
    prune_stale_history_caches(state, repository.id, &tips).await?;
    Ok(overview.window(start_date, end_date))
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
    #[serde(skip)]
    pub(super) author_email: String,
    #[serde(skip)]
    pub(super) signer: Option<String>,
}

/// A [`DayCommitResponse`] as cached: with the authorship a profile filters
/// on, which the API response leaves out.
#[derive(Deserialize, Serialize)]
struct CachedDayCommit {
    oid: String,
    title: String,
    author_name: String,
    timestamp: i64,
    author_email: String,
    signer: Option<String>,
}

impl From<&DayCommitResponse> for CachedDayCommit {
    fn from(commit: &DayCommitResponse) -> Self {
        Self {
            oid: commit.oid.clone(),
            title: commit.title.clone(),
            author_name: commit.author_name.clone(),
            timestamp: commit.timestamp,
            author_email: commit.author_email.clone(),
            signer: commit.signer.clone(),
        }
    }
}

impl From<CachedDayCommit> for DayCommitResponse {
    fn from(commit: CachedDayCommit) -> Self {
        Self {
            short_oid: commit.oid[..12.min(commit.oid.len())].to_owned(),
            oid: commit.oid,
            title: commit.title,
            author_name: commit.author_name,
            timestamp: commit.timestamp,
            author_email: commit.author_email,
            signer: commit.signer,
        }
    }
}

/// Commits reachable from any branch that were authored on `date`, newest
/// first: the ones the activity graph counted for that day.
pub(super) async fn read_commits_authored_on(
    state: &RepositoryState,
    repository: &repository::Model,
    date: NaiveDate,
) -> Result<Vec<DayCommitResponse>, ApiError> {
    let path = state.repository_path(repository);
    let tips = read_git(path.clone(), BranchTips::read).await?;
    let cache_key = tips.day_key(date);
    let cached = async || {
        Ok::<_, ApiError>(
            state
                .cached_value::<Vec<CachedDayCommit>>(
                    repository.id,
                    super::CACHE_KIND_OVERVIEW,
                    &cache_key,
                )
                .await?
                .map(|commits| {
                    commits
                        .into_iter()
                        .map(DayCommitResponse::from)
                        .collect::<Vec<_>>()
                }),
        )
    };
    if let Some(commits) = cached().await? {
        return Ok(commits);
    }
    // The overview cached for these tips already knows which days saw
    // commits, so an empty day is answered without walking the history.
    if let Some(overview) = state
        .cached_overview(repository.id, &tips.overview_key(repository))
        .await?
        && overview.has_commits_on(date) == Some(false)
    {
        return Ok(Vec::new());
    }
    let _flight = single_flight(format!("day:{}:{cache_key}", repository.id)).await;
    if let Some(commits) = cached().await? {
        return Ok(commits);
    }

    let (tips, mut commits) = read_git(path, move |git| {
        let mut roots = Vec::new();
        for (_, target) in &tips.tips {
            roots.push(git.peel_to_commit_oid(*target)?);
        }
        let mut commits = Vec::new();
        if roots.is_empty() {
            return Ok((tips, commits));
        }
        git.rev_graph().stream_reachable_commits(
            roots,
            ReachableCommitOptions::new(),
            |metadata| {
                let commit = git.read_commit(&metadata.oid)?;
                let signature = commit.author_signature();
                if signature.as_ref().and_then(authored_on) == Some(date) {
                    let (author_email, signer) =
                        commit_authorship(git, &metadata.oid, signature.as_ref())?;
                    let author = signature_response(signature, &commit.author);
                    let message = String::from_utf8_lossy(&commit.message);
                    let oid = metadata.oid.to_hex();
                    commits.push(DayCommitResponse {
                        short_oid: oid[..12.min(oid.len())].to_owned(),
                        oid,
                        title: message.trim().lines().next().unwrap_or_default().to_owned(),
                        author_name: author.name,
                        timestamp: author.timestamp,
                        author_email,
                        signer,
                    });
                }
                Ok(StreamControl::Continue)
            },
        )?;
        Ok((tips, commits))
    })
    .await?;
    commits.sort_by_key(|commit| std::cmp::Reverse(commit.timestamp));
    state
        .cache_value(
            repository.id,
            super::CACHE_KIND_OVERVIEW,
            &cache_key,
            &commits
                .iter()
                .map(CachedDayCommit::from)
                .collect::<Vec<_>>(),
            DAY_COMMITS_CACHE_LIFETIME,
        )
        .await?;
    prune_stale_history_caches(state, repository.id, &tips).await?;
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

/// [`RepositoryState::readable_repository`] for handlers that do not need to
/// know who is asking.
pub(super) async fn readable_repository(
    state: &RepositoryState,
    headers: &HeaderMap,
    jar: &CookieJar,
    namespace: &str,
    name: &str,
) -> Result<repository::Model, ApiError> {
    let (repository, _) = state
        .readable_repository(headers, jar, namespace, name)
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
        collections::{BTreeMap, BTreeSet},
        fs,
        io::Write as _,
        path::PathBuf,
        process::Command,
        sync::Arc,
        time::{Duration, Instant},
    };

    use axum::{
        extract::{Path as AxumPath, Query, State},
        http::{HeaderMap, HeaderValue, StatusCode, header},
        response::Response,
    };
    use axum_extra::extract::cookie::CookieJar;
    use chrono::{Duration as ChronoDuration, NaiveDate, Utc};
    use russh::keys::ssh_key::{HashAlg, LineEnding, PrivateKey, private::Ed25519Keypair};
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
    use sley::{
        BString, CommitObject, GitObjectType, ObjectId, ReachableCommitOptions, ReferenceTarget,
        Repository as GitRepository, StreamControl, TreeEditor,
    };
    use tokio::sync::Semaphore;
    use uuid::Uuid;

    use super::{
        ArchiveFileWriter, AuthorActivity, BrowseQuery, DayCommitResponse,
        MAX_SOURCE_ARCHIVE_BYTES, SshCommitSignature, authored_on, commit_authorship,
        count_reachable_commits, if_none_match, metered_body, raw_blob_permits,
        read_commits_authored_on, read_repository_overview, read_submodule_metadata, single_flight,
        verify_ssh_commit_signature,
    };
    use crate::{
        entity::repository_cache_entry,
        repository::resources::test_support::{TestGit, TestInstance, instance},
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

    const COMMIT_DATE: &str = "2026-03-01T12:00:00Z";

    fn browse(path: &str) -> Query<BrowseQuery> {
        Query(BrowseQuery {
            rev: None,
            path: path.to_owned(),
        })
    }

    fn project() -> AxumPath<(String, String)> {
        AxumPath(("alice".to_owned(), "project".to_owned()))
    }

    fn conditional(etag: &HeaderValue) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(header::IF_NONE_MATCH, etag.clone());
        headers
    }

    async fn body_bytes(response: Response) -> Vec<u8> {
        axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec()
    }

    #[tokio::test]
    async fn raw_answers_a_matching_validator_with_not_modified() {
        let instance = instance().await;
        let (_, git) = instance.git_repository("project").await;
        git.commit_file("notes.txt", b"first\n", COMMIT_DATE);
        let blob_oid = git.run(COMMIT_DATE, &["rev-parse", "HEAD:notes.txt"]);
        let request = |headers: HeaderMap| {
            super::raw(
                State(instance.state.clone()),
                project(),
                headers,
                CookieJar::new(),
                browse("notes.txt"),
            )
        };

        let response = request(HeaderMap::new()).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let etag = response.headers()[header::ETAG].clone();
        assert_eq!(etag, format!("\"{blob_oid}\"").as_str());
        assert_eq!(
            response.headers()[header::CACHE_CONTROL],
            "private, no-cache"
        );
        assert_eq!(response.headers()[header::CONTENT_LENGTH], "6");
        assert_eq!(body_bytes(response).await, b"first\n");

        let response = request(conditional(&etag)).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
        assert_eq!(response.headers()[header::ETAG], etag);
        assert!(body_bytes(response).await.is_empty());

        // The same path at a new commit no longer matches the old validator.
        git.commit_file("notes.txt", b"second\n", COMMIT_DATE);
        let response = request(conditional(&etag)).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_ne!(response.headers()[header::ETAG], etag);
        assert_eq!(body_bytes(response).await, b"second\n");
    }

    #[tokio::test]
    async fn blob_answers_a_matching_validator_with_not_modified() {
        let instance = instance().await;
        let (_, git) = instance.git_repository("project").await;
        let commit_oid = git.commit_file("README.md", b"# Title\n", COMMIT_DATE);
        let blob_oid = git.run(COMMIT_DATE, &["rev-parse", "HEAD:README.md"]);
        let request = |headers: HeaderMap| {
            super::blob(
                State(instance.state.clone()),
                project(),
                headers,
                CookieJar::new(),
                browse("README.md"),
            )
        };

        let response = request(HeaderMap::new()).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let etag = response.headers()[header::ETAG].clone();
        assert_eq!(
            etag,
            format!(
                "W/\"{}-{commit_oid}-{blob_oid}\"",
                env!("CARGO_PKG_VERSION")
            )
            .as_str()
        );
        let body: serde_json::Value = serde_json::from_slice(&body_bytes(response).await).unwrap();
        assert_eq!(body["content"], "# Title\n");
        assert_eq!(body["oid"], blob_oid.as_str());

        let response = request(conditional(&etag)).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
        assert!(body_bytes(response).await.is_empty());

        // A new commit changes the response even where the file is the same.
        git.commit_file("other.txt", b"x\n", COMMIT_DATE);
        let response = request(conditional(&etag)).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn stats_miss_waits_for_an_analysis_slot_and_a_hit_does_not() {
        let instance = instance().await;
        let (_, git) = instance.git_repository("project").await;
        git.commit_file("main.rs", b"fn main() {}\n", COMMIT_DATE);
        let request = || {
            super::stats(
                State(instance.state.clone()),
                project(),
                HeaderMap::new(),
                CookieJar::new(),
                browse(""),
            )
        };

        // With every analysis slot taken, a miss queues instead of computing.
        let slots = Arc::clone(&instance.state.analysis_slots);
        let busy = Arc::clone(&slots)
            .acquire_many_owned(u32::try_from(slots.available_permits()).unwrap())
            .await
            .unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(300), request())
                .await
                .is_err()
        );
        drop(busy);

        let computed = request().await.unwrap().0;
        assert_eq!(computed.len(), 1);
        assert_eq!(computed[0].language, "Rust");
        assert_eq!(computed[0].code, 1);

        // The result is cached now, so no slot is needed to serve it.
        let _busy = Arc::clone(&slots)
            .acquire_many_owned(u32::try_from(slots.available_permits()).unwrap())
            .await
            .unwrap();
        let cached = tokio::time::timeout(Duration::from_secs(10), request())
            .await
            .expect("a cache hit does not wait for a slot")
            .unwrap()
            .0;
        assert_eq!(cached[0].code, 1);
    }

    /// The activity of one window computed straight from the history, as it
    /// was before the walk was cached per branch state: the reference the
    /// cached and sliced result must equal.
    fn direct_overview(
        git: &GitRepository,
        start_date: NaiveDate,
        end_date: NaiveDate,
    ) -> (BTreeMap<NaiveDate, usize>, Vec<AuthorActivity>) {
        let mut roots = Vec::new();
        for reference in git
            .references()
            .list_refs_with_prefix("refs/heads/")
            .unwrap()
        {
            if let ReferenceTarget::Direct(target) = reference.target {
                roots.push(git.peel_to_commit_oid(target).unwrap());
            }
        }
        let mut activity = BTreeMap::new();
        let mut authors = BTreeMap::<(NaiveDate, String, Option<String>), usize>::new();
        git.rev_graph()
            .stream_reachable_commits(roots, ReachableCommitOptions::new(), |metadata| {
                let commit = git.read_commit(&metadata.oid)?;
                let signature = commit.author_signature();
                if let Some(date) = signature.as_ref().and_then(authored_on)
                    && date >= start_date
                    && date <= end_date
                {
                    *activity.entry(date).or_default() += 1;
                    let (email, signer) =
                        commit_authorship(git, &metadata.oid, signature.as_ref())?;
                    *authors.entry((date, email, signer)).or_default() += 1;
                }
                Ok(StreamControl::Continue)
            })
            .unwrap();
        let authors = authors
            .into_iter()
            .map(|((date, email, signer), count)| AuthorActivity {
                date,
                email,
                signer,
                count,
            })
            .collect();
        (activity, authors)
    }

    /// Commit on the checked-out branch, authored at noon UTC `days_ago`.
    fn commit_days_ago(git: &TestGit, days_ago: i64, author: &str) -> String {
        let date = format!(
            "{}T12:00:00+00:00",
            Utc::now().date_naive() - ChronoDuration::days(days_ago)
        );
        let author = format!("--author={author} <{author}@example.test>");
        git.run(
            &date,
            &["commit", "--quiet", "--allow-empty", "-m", "work", &author],
        );
        git.run(&date, &["rev-parse", "HEAD"])
    }

    /// Cached history entries whose key carries `marker`: `:v3:` for the
    /// overview, `:day:` for one day's commits.
    async fn history_cache_rows(
        instance: &TestInstance,
        marker: &str,
    ) -> Vec<repository_cache_entry::Model> {
        repository_cache_entry::Entity::find()
            .filter(repository_cache_entry::Column::Kind.eq(crate::repository::CACHE_KIND_OVERVIEW))
            .filter(repository_cache_entry::Column::CacheKey.contains(marker))
            .all(&instance.database)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn activity_windows_are_cut_from_one_walk_per_branch_state() {
        let instance = instance().await;
        let (repository, git) = instance.git_repository("project").await;
        for (days_ago, author) in [(400, "alice"), (40, "alice"), (3, "bob"), (3, "alice")] {
            commit_days_ago(&git, days_ago, author);
        }
        git.run(COMMIT_DATE, &["switch", "--quiet", "--create", "feature"]);
        commit_days_ago(&git, 3, "alice");
        commit_days_ago(&git, 1, "bob");
        git.run(COMMIT_DATE, &["switch", "--quiet", "main"]);
        commit_days_ago(&git, 0, "bob");
        let native = GitRepository::open_exact_bare(&git.git_dir).unwrap();
        let today = Utc::now().date_naive();
        let window = |days: i64| (today - ChronoDuration::days(days - 1), today);

        // Every window, including ones no request has asked for before, is
        // answered from the entry the first request cached.
        for days in [14, 1, 2, 4, 41, 365] {
            let (start_date, end_date) = window(days);
            let overview =
                read_repository_overview(&instance.state, &repository, start_date, end_date)
                    .await
                    .unwrap();
            let (activity, authors) = direct_overview(&native, start_date, end_date);
            assert_eq!(overview.activity, activity, "{days}-day activity");
            assert_eq!(overview.authors, authors, "{days}-day authors");
            assert_eq!(overview.branch_count, 2);
            assert_eq!(
                overview.head.as_deref(),
                Some(git.run(COMMIT_DATE, &["rev-parse", "main"]).as_str())
            );
        }
        let (start_date, end_date) = window(14);
        let (activity, _) = direct_overview(&native, start_date, end_date);
        assert_eq!(activity.values().sum::<usize>(), 5);
        let rows = history_cache_rows(&instance, ":v3:").await;
        assert_eq!(rows.len(), 1);
        let first_walk = rows[0].created_at;

        // A window that ends before today is cut from the same entry.
        let (start_date, end_date) = (
            today - ChronoDuration::days(50),
            today - ChronoDuration::days(2),
        );
        let overview = read_repository_overview(&instance.state, &repository, start_date, end_date)
            .await
            .unwrap();
        let (activity, authors) = direct_overview(&native, start_date, end_date);
        assert_eq!((overview.activity, overview.authors), (activity, authors));
        let rows = history_cache_rows(&instance, ":v3:").await;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].created_at, first_walk, "no second walk");

        // A window reaching past what the entry holds is still answered right.
        let (start_date, end_date) = window(500);
        let overview = read_repository_overview(&instance.state, &repository, start_date, end_date)
            .await
            .unwrap();
        let (activity, authors) = direct_overview(&native, start_date, end_date);
        assert_eq!(activity.values().sum::<usize>(), 7);
        assert_eq!((overview.activity, overview.authors), (activity, authors));

        // Moving a branch replaces the entry instead of adding one.
        commit_days_ago(&git, 2, "alice");
        let native = GitRepository::open_exact_bare(&git.git_dir).unwrap();
        let (start_date, end_date) = window(14);
        let overview = read_repository_overview(&instance.state, &repository, start_date, end_date)
            .await
            .unwrap();
        let (activity, authors) = direct_overview(&native, start_date, end_date);
        assert_eq!(activity.values().sum::<usize>(), 6);
        assert_eq!((overview.activity, overview.authors), (activity, authors));
        let rows = history_cache_rows(&instance, ":v3:").await;
        assert_eq!(rows.len(), 1);
        assert_ne!(rows[0].created_at, first_walk);
    }

    #[tokio::test]
    async fn day_commits_are_cached_per_branch_state() {
        let instance = instance().await;
        let (repository, git) = instance.git_repository("project").await;
        let older = commit_days_ago(&git, 3, "alice");
        let first = commit_days_ago(&git, 1, "alice");
        git.run(COMMIT_DATE, &["switch", "--quiet", "--create", "feature"]);
        let second = commit_days_ago(&git, 1, "bob");
        let today = Utc::now().date_naive();
        let day = |days_ago: i64| today - ChronoDuration::days(days_ago);
        let oids = |commits: &[DayCommitResponse]| {
            let mut oids = commits
                .iter()
                .map(|commit| commit.oid.clone())
                .collect::<Vec<_>>();
            oids.sort();
            oids
        };
        let mut expected = vec![first, second];
        expected.sort();

        let commits = read_commits_authored_on(&instance.state, &repository, day(1))
            .await
            .unwrap();
        assert_eq!(oids(&commits), expected);
        let emails = commits
            .iter()
            .map(|commit| commit.author_email.as_str())
            .collect::<BTreeSet<_>>();
        assert_eq!(
            emails,
            BTreeSet::from(["alice@example.test", "bob@example.test"])
        );
        let rows = history_cache_rows(&instance, ":day:").await;
        assert_eq!(rows.len(), 1);
        let first_walk = rows[0].created_at;

        // The repeat is read back from the cache, authorship included.
        let again = read_commits_authored_on(&instance.state, &repository, day(1))
            .await
            .unwrap();
        assert_eq!(oids(&again), expected);
        assert_eq!(again[0].short_oid, again[0].oid[..12]);
        assert_eq!(again[0].author_email, commits[0].author_email);
        let rows = history_cache_rows(&instance, ":day:").await;
        assert_eq!(rows[0].created_at, first_walk, "no second walk");

        // With the overview cached, a day without commits needs no walk.
        read_repository_overview(&instance.state, &repository, day(13), today)
            .await
            .unwrap();
        let empty = read_commits_authored_on(&instance.state, &repository, day(2))
            .await
            .unwrap();
        assert!(empty.is_empty());
        assert_eq!(history_cache_rows(&instance, ":day:").await.len(), 1);
        let commits = read_commits_authored_on(&instance.state, &repository, day(3))
            .await
            .unwrap();
        assert_eq!(oids(&commits), [older]);

        // A push drops the days cached for the previous branch state.
        let third = commit_days_ago(&git, 1, "alice");
        expected.push(third);
        expected.sort();
        let commits = read_commits_authored_on(&instance.state, &repository, day(1))
            .await
            .unwrap();
        assert_eq!(oids(&commits), expected);
        assert_eq!(history_cache_rows(&instance, ":day:").await.len(), 1);
    }

    #[tokio::test]
    async fn single_flight_admits_one_holder_per_key() {
        let key = format!("test:{}", Uuid::new_v4());
        let first = single_flight(key.clone()).await;
        // A different key is independent of the held one.
        drop(single_flight(format!("{key}:other")).await);

        let mut second = Box::pin(single_flight(key.clone()));
        assert!(
            tokio::time::timeout(Duration::from_millis(50), &mut second)
                .await
                .is_err()
        );
        drop(first);
        let second = tokio::time::timeout(Duration::from_secs(10), second)
            .await
            .expect("the waiter proceeds once the holder is done");
        drop(second);

        // A waiter that gives up, and the last holder, leave nothing behind.
        let holder = single_flight(key.clone()).await;
        drop(Box::pin(single_flight(key.clone())));
        drop(holder);
        assert!(!super::IN_FLIGHT.lock().unwrap().contains_key(&key));
    }

    #[test]
    fn if_none_match_compares_weakly_across_a_list() {
        let headers = |value: &'static str| {
            let mut headers = HeaderMap::new();
            headers.insert(header::IF_NONE_MATCH, HeaderValue::from_static(value));
            headers
        };
        assert!(!if_none_match(&HeaderMap::new(), "\"abc\""));
        assert!(if_none_match(&headers("\"abc\""), "\"abc\""));
        assert!(if_none_match(&headers("W/\"abc\""), "\"abc\""));
        assert!(if_none_match(&headers("\"abc\""), "W/\"abc\""));
        assert!(if_none_match(&headers("\"x\", \"abc\""), "\"abc\""));
        assert!(if_none_match(&headers("*"), "\"abc\""));
        assert!(!if_none_match(&headers("\"abcd\""), "\"abc\""));
        assert!(!if_none_match(&headers("abc"), "\"abc\""));
    }

    #[test]
    fn raw_blob_memory_is_metered_per_mebibyte() {
        const MIB: u64 = 1024 * 1024;
        assert_eq!(raw_blob_permits(0), None);
        assert_eq!(raw_blob_permits(MIB), None);
        assert_eq!(raw_blob_permits(MIB + 1), Some(2));
        assert_eq!(raw_blob_permits(10 * MIB), Some(10));
        // A blob beyond the whole budget is served alone, not refused.
        assert_eq!(
            raw_blob_permits(u64::MAX),
            Some(super::RAW_BLOB_MEMORY_BUDGET_MIB)
        );
    }

    #[tokio::test]
    async fn metered_body_keeps_its_permit_until_the_body_is_gone() {
        let slots = Arc::new(Semaphore::new(4));
        let permit = Arc::clone(&slots).acquire_many_owned(3).await.unwrap();
        let content = (0..200_000).map(|index| index as u8).collect::<Vec<_>>();

        let body = metered_body(content.clone(), Some(permit));
        assert_eq!(slots.available_permits(), 1);
        let received = axum::body::to_bytes(body, usize::MAX).await.unwrap();
        assert_eq!(received, content);
        assert_eq!(slots.available_permits(), 4);

        // A client that goes away mid-download releases the memory too.
        let permit = Arc::clone(&slots).acquire_many_owned(3).await.unwrap();
        drop(metered_body(content, Some(permit)));
        assert_eq!(slots.available_permits(), 4);
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
