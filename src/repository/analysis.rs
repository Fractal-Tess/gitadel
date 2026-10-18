//! Analysis of a repository's content and history: language statistics,
//! overview cards, and commit activity, with the caches that keep them cheap.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::{Arc, LazyLock, Mutex as StdMutex, PoisonError},
    time::Duration as StdDuration,
};

use axum::{
    Json,
    extract::{Path as AxumPath, Query, State},
    http::HeaderMap,
};
use axum_extra::extract::cookie::CookieJar;
use chrono::{DateTime, Duration, NaiveDate, Utc};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use sley::{
    ObjectId, ReachableCommitOptions, ReferenceTarget, Repository as GitRepository, StreamControl,
};
use tokei::{Config as TokeiConfig, LanguageType};
use tokio::{
    sync::{Mutex, OwnedMutexGuard},
    task::JoinSet,
};
use utoipa::ToSchema;

use super::{
    RepositoryState,
    browser::{
        BrowseQuery, count_reachable_commits, default_page, read_git, readable_repository,
        signature_response,
    },
    resources,
    signature::{SshCommitSignature, verify_ssh_commit_signature},
};
use crate::{
    entity::{repository, repository_cache_entry},
    identity::ApiError,
};

const DEFAULT_REPOSITORY_ACTIVITY_DAYS: u16 = 14;
pub(super) const MAX_REPOSITORY_ACTIVITY_DAYS: u16 = 365;
const DEFAULT_OVERVIEW_PER_PAGE: usize = 20;
const MAX_OVERVIEW_PER_PAGE: usize = 50;

#[derive(Deserialize)]
pub struct OverviewQuery {
    #[serde(default = "default_page")]
    page: usize,
    #[serde(default = "default_overview_per_page")]
    per_page: usize,
    q: Option<String>,
    namespace: Option<String>,
}

#[derive(Deserialize)]
pub struct ActivityQuery {
    #[serde(default = "default_repository_activity_days")]
    days: u16,
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
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

#[derive(Serialize, ToSchema)]
pub struct RepositoryOverviewResponse {
    repositories: Vec<RepositoryOverviewItemResponse>,
    page: usize,
    per_page: usize,
    has_next: bool,
}

#[derive(Serialize, ToSchema)]
pub(super) struct RepositoryOverviewItemResponse {
    #[serde(flatten)]
    repository: resources::RepositoryResponse,
    branch_count: usize,
    commit_count: usize,
    total_lines: usize,
    languages: Vec<OverviewLanguageResponse>,
    activity: ActivityResponse,
}

#[derive(Serialize, ToSchema)]
struct OverviewLanguageResponse {
    language: String,
    lines: usize,
}

#[derive(Serialize, ToSchema)]
pub struct ActivityResponse {
    start_date: NaiveDate,
    end_date: NaiveDate,
    total_commits: usize,
    days: Vec<ActivityDayResponse>,
}

#[derive(Serialize, ToSchema)]
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
    let filter = resources::RepositoryFilter {
        q: query.q,
        namespace: query.namespace,
    };
    let accessible =
        resources::accessible_repositories_matching(&state, &headers, &jar, &filter).await?;
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

/// How long the commits authored on one day stay cached.
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
        .filter(
            repository_cache_entry::Column::Kind
                .is_in([super::CACHE_KIND_OVERVIEW, super::CACHE_KIND_DAY_COMMITS]),
        )
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
#[derive(Serialize, ToSchema)]
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
                    super::CACHE_KIND_DAY_COMMITS,
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
            super::CACHE_KIND_DAY_COMMITS,
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

#[cfg(test)]
mod tests {
    use std::{
        collections::{BTreeMap, BTreeSet},
        sync::Arc,
        time::Duration,
    };

    use axum::{
        extract::{Path as AxumPath, Query, State},
        http::HeaderMap,
    };
    use axum_extra::extract::cookie::CookieJar;
    use chrono::{Duration as ChronoDuration, NaiveDate, Utc};
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
    use sley::{
        ReachableCommitOptions, ReferenceTarget, Repository as GitRepository, StreamControl,
    };
    use uuid::Uuid;

    use super::{
        AuthorActivity, DayCommitResponse, authored_on, commit_authorship,
        read_commits_authored_on, read_repository_overview, single_flight,
    };
    use crate::{
        entity::repository_cache_entry,
        repository::{
            browser::BrowseQuery,
            resources::test_support::{TestGit, TestInstance, instance},
        },
    };

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
    /// overview, `:day:` for one day's commits, each under its own kind.
    async fn history_cache_rows(
        instance: &TestInstance,
        marker: &str,
    ) -> Vec<repository_cache_entry::Model> {
        let kind = if marker == ":day:" {
            crate::repository::CACHE_KIND_DAY_COMMITS
        } else {
            crate::repository::CACHE_KIND_OVERVIEW
        };
        repository_cache_entry::Entity::find()
            .filter(repository_cache_entry::Column::Kind.eq(kind))
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
}
