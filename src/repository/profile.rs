//! Namespace profile pages: a year of commit activity and pinned repositories.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::Arc,
};

use axum::{
    Json,
    extract::{Path as AxumPath, State},
    http::HeaderMap,
};
use axum_extra::extract::cookie::CookieJar;
use chrono::{NaiveDate, Utc};
use sea_orm::{
    ActiveValue::Set, ColumnTrait, EntityTrait, QueryFilter, QueryOrder, TransactionTrait,
};
use serde::{Deserialize, Serialize};
use tokio::task::JoinSet;
use uuid::Uuid;

use super::{
    RepositoryState,
    browser::{
        ActivityResponse, DayCommitResponse, GitOverview, MAX_REPOSITORY_ACTIVITY_DAYS,
        RepositoryOverviewItemResponse, activity_response, activity_start_date,
        read_commits_authored_on, read_repository_overview, repository_overview_items,
    },
    resources::{accessible_repositories, owned_namespace},
};
use crate::{
    entity::{namespace, namespace_pin, repository, user},
    identity::{
        ApiError, SCOPE_WRITE,
        commit_emails::{CommitIdentities, commit_identities},
    },
};

/// Pins a namespace may feature, as on GitHub.
const MAX_PINS: usize = 6;

#[derive(Serialize)]
pub struct NamespaceActivityResponse {
    /// `person` counts the commits the user wrote anywhere; `organization`
    /// counts every commit in its repositories.
    scope: &'static str,
    /// Repositories the viewer may read that contributed to the graph.
    repository_count: usize,
    #[serde(flatten)]
    activity: ActivityResponse,
}

#[derive(Serialize)]
pub struct PinnedRepositoriesResponse {
    repositories: Vec<RepositoryOverviewItemResponse>,
    limit: usize,
}

#[derive(Serialize)]
pub struct RepositoryDayResponse {
    namespace: String,
    name: String,
    commits: Vec<DayCommitResponse>,
}

#[derive(Serialize)]
pub struct NamespaceDayResponse {
    date: NaiveDate,
    total_commits: usize,
    /// Repositories with commits that day, busiest first.
    repositories: Vec<RepositoryDayResponse>,
}

#[derive(Deserialize)]
pub struct SetPinsRequest {
    repository_ids: Vec<Uuid>,
}

async fn ensure_namespace(state: &RepositoryState, slug: &str) -> Result<(), ApiError> {
    load_namespace(state, slug).await.map(|_| ())
}

async fn load_namespace(state: &RepositoryState, slug: &str) -> Result<namespace::Model, ApiError> {
    namespace::Entity::find_by_id(slug)
        .one(state.identity().database())
        .await?
        .ok_or_else(ApiError::not_found)
}

/// Whose work a profile shows. A person's profile counts the commits they
/// wrote, wherever they live; an organization's counts every commit in its own
/// repositories.
enum ProfileScope {
    Person(CommitIdentities),
    Organization(String),
}

async fn profile_scope(
    state: &RepositoryState,
    namespace: &namespace::Model,
) -> Result<ProfileScope, ApiError> {
    let database = state.identity().database();
    if let Some(user_id) = namespace.user_id
        && let Some(account) = user::Entity::find_by_id(user_id).one(database).await?
    {
        return Ok(ProfileScope::Person(
            commit_identities(database, &account).await?,
        ));
    }
    Ok(ProfileScope::Organization(namespace.slug.clone()))
}

impl ProfileScope {
    /// The repositories this profile draws from, among those the viewer can
    /// read. Mirrors are left out: their history is someone else's.
    fn includes(&self, repository: &repository::Model) -> bool {
        !repository.mirrored
            && match self {
                Self::Person(_) => true,
                Self::Organization(slug) => repository.namespace == *slug,
            }
    }

    /// Commits per day in one repository that this profile counts.
    fn count(&self, overview: GitOverview) -> BTreeMap<NaiveDate, usize> {
        match self {
            Self::Organization(_) => overview.activity,
            Self::Person(identities) => {
                let mut days = BTreeMap::new();
                for author in overview.authors {
                    if identities.matches(&author.email, author.signer.as_deref()) {
                        *days.entry(author.date).or_default() += author.count;
                    }
                }
                days
            }
        }
    }

    fn counts_commit(&self, commit: &DayCommitResponse) -> bool {
        match self {
            Self::Organization(_) => true,
            Self::Person(identities) => {
                identities.matches(&commit.author_email, commit.signer.as_deref())
            }
        }
    }
}

fn merge_activity(total: &mut BTreeMap<NaiveDate, usize>, repository: BTreeMap<NaiveDate, usize>) {
    for (date, count) in repository {
        *total.entry(date).or_default() += count;
    }
}

/// Commits per day over the last year across the namespace's repositories that
/// the viewer can read. Mirrors are left out: their history is someone else's.
pub async fn activity(
    State(state): State<RepositoryState>,
    AxumPath(slug): AxumPath<String>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<NamespaceActivityResponse>, ApiError> {
    let namespace = load_namespace(&state, &slug).await?;
    let scope = Arc::new(profile_scope(&state, &namespace).await?);
    let accessible = accessible_repositories(&state, &headers, &jar).await?;
    let repositories = accessible
        .repositories
        .into_iter()
        .filter(|repository| scope.includes(repository))
        .collect::<Vec<_>>();

    let end_date = Utc::now().date_naive();
    let start_date = activity_start_date(end_date, MAX_REPOSITORY_ACTIVITY_DAYS)?;
    let mut pending = JoinSet::new();
    for repository in repositories {
        let state = state.clone();
        let slots = state.analysis_slots.clone();
        let scope = scope.clone();
        pending.spawn(async move {
            let _permit = slots.acquire_owned().await.map_err(ApiError::internal)?;
            let name = format!("{}/{}", repository.namespace, repository.name);
            let result = read_repository_overview(&state, &repository, start_date, end_date).await;
            Ok::<_, ApiError>((name, result.map(|overview| scope.count(overview))))
        });
    }

    let mut days = BTreeMap::new();
    let mut repository_count = 0;
    while let Some(result) = pending.join_next().await {
        let (name, activity) = result.map_err(ApiError::internal)??;
        match activity {
            // A person's profile names only the repositories they wrote in.
            Ok(activity) if activity.is_empty() && matches!(*scope, ProfileScope::Person(_)) => {}
            Ok(activity) => {
                repository_count += 1;
                merge_activity(&mut days, activity);
            }
            // One unreadable repository should not blank the whole graph.
            Err(_) => tracing::warn!(repository = %name, "could not read repository activity"),
        }
    }
    Ok(Json(NamespaceActivityResponse {
        scope: match *scope {
            ProfileScope::Person(_) => "person",
            ProfileScope::Organization(_) => "organization",
        },
        repository_count,
        activity: activity_response(start_date, end_date, days),
    }))
}

/// The commits behind one day of the activity graph, grouped by repository.
/// Reads the same repositories as [`activity`], so the two always agree.
pub async fn activity_day(
    State(state): State<RepositoryState>,
    AxumPath((slug, date)): AxumPath<(String, NaiveDate)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<NamespaceDayResponse>, ApiError> {
    let namespace = load_namespace(&state, &slug).await?;
    let scope = Arc::new(profile_scope(&state, &namespace).await?);
    let accessible = accessible_repositories(&state, &headers, &jar).await?;
    let end_date = Utc::now().date_naive();
    let start_date = activity_start_date(end_date, MAX_REPOSITORY_ACTIVITY_DAYS)?;
    let mut pending = JoinSet::new();
    for repository in accessible
        .repositories
        .into_iter()
        .filter(|repository| scope.includes(repository))
    {
        let state = state.clone();
        let slots = state.analysis_slots.clone();
        let scope = scope.clone();
        pending.spawn(async move {
            let _permit = slots.acquire_owned().await.map_err(ApiError::internal)?;
            // The cached activity says which repositories have anything to
            // show that day, so only those are walked again.
            if (start_date..=end_date).contains(&date)
                && let Ok(overview) =
                    read_repository_overview(&state, &repository, start_date, end_date).await
                && !scope.count(overview).contains_key(&date)
            {
                return Ok::<_, ApiError>((repository, Ok(Vec::new())));
            }
            let commits = read_commits_authored_on(&state, &repository, date)
                .await
                .map(|commits| {
                    commits
                        .into_iter()
                        .filter(|commit| scope.counts_commit(commit))
                        .collect::<Vec<_>>()
                });
            Ok((repository, commits))
        });
    }

    let mut repositories = Vec::new();
    while let Some(result) = pending.join_next().await {
        let (repository, commits) = result.map_err(ApiError::internal)??;
        match commits {
            Ok(commits) if !commits.is_empty() => repositories.push(RepositoryDayResponse {
                namespace: repository.namespace,
                name: repository.name,
                commits,
            }),
            Ok(_) => {}
            Err(_) => tracing::warn!(
                repository = %format!("{}/{}", repository.namespace, repository.name),
                "could not read repository activity"
            ),
        }
    }
    repositories.sort_by(|left, right| {
        right
            .commits
            .len()
            .cmp(&left.commits.len())
            .then_with(|| left.name.cmp(&right.name))
    });
    Ok(Json(NamespaceDayResponse {
        date,
        total_commits: repositories.iter().map(|day| day.commits.len()).sum(),
        repositories,
    }))
}

/// The pinned repositories the viewer can still read, in pin order. Pins whose
/// repository was deleted, moved to another namespace, or hidden from the
/// viewer are skipped rather than reported.
async fn pinned_repositories(
    state: &RepositoryState,
    slug: &str,
    accessible: super::resources::AccessibleRepositories,
) -> Result<Vec<RepositoryOverviewItemResponse>, ApiError> {
    let pins = namespace_pin::Entity::find()
        .filter(namespace_pin::Column::Namespace.eq(slug))
        .order_by_asc(namespace_pin::Column::Position)
        .all(state.identity().database())
        .await?;
    let mut readable = accessible
        .repositories
        .into_iter()
        .filter(|repository| repository.namespace == slug)
        .map(|repository| (repository.id, repository))
        .collect::<HashMap<_, _>>();
    let ordered = pins
        .iter()
        .filter_map(|pin| readable.remove(&pin.repository_id))
        .collect::<Vec<repository::Model>>();
    repository_overview_items(
        state,
        ordered,
        &accessible.favorite_ids,
        &accessible.manageable_ids,
        &accessible.writable_ids,
    )
    .await
}

pub async fn list_pins(
    State(state): State<RepositoryState>,
    AxumPath(slug): AxumPath<String>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<PinnedRepositoriesResponse>, ApiError> {
    ensure_namespace(&state, &slug).await?;
    let accessible = accessible_repositories(&state, &headers, &jar).await?;
    let repositories = pinned_repositories(&state, &slug, accessible).await?;
    Ok(Json(PinnedRepositoriesResponse {
        repositories,
        limit: MAX_PINS,
    }))
}

/// Checks a requested pin list before it replaces the current one: within the
/// limit, no repeats, and every repository readable and in this namespace.
fn validate_pins(requested: &[Uuid], allowed: &HashSet<Uuid>) -> Result<(), ApiError> {
    if requested.len() > MAX_PINS {
        return Err(ApiError::bad_request(format!(
            "Pin at most {MAX_PINS} repositories."
        )));
    }
    let mut seen = HashSet::new();
    for id in requested {
        if !seen.insert(*id) {
            return Err(ApiError::bad_request(
                "A repository can only be pinned once.",
            ));
        }
        if !allowed.contains(id) {
            return Err(ApiError::bad_request(
                "Only repositories in this namespace can be pinned.",
            ));
        }
    }
    Ok(())
}

/// Replaces the namespace's pins with `repository_ids`, in the order given.
/// Only the account or organization owner may.
pub async fn set_pins(
    State(state): State<RepositoryState>,
    AxumPath(slug): AxumPath<String>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<SetPinsRequest>,
) -> Result<Json<PinnedRepositoriesResponse>, ApiError> {
    let actor = state
        .identity()
        .authenticate(&headers, &jar, SCOPE_WRITE)
        .await?;
    owned_namespace(&state, &slug, actor.user.id).await?;
    let accessible = accessible_repositories(&state, &headers, &jar).await?;
    let allowed = accessible
        .repositories
        .iter()
        .filter(|repository| repository.namespace == slug)
        .map(|repository| repository.id)
        .collect::<HashSet<_>>();
    validate_pins(&request.repository_ids, &allowed)?;

    let now = Utc::now();
    let transaction = state.identity().database().begin().await?;
    namespace_pin::Entity::delete_many()
        .filter(namespace_pin::Column::Namespace.eq(slug.as_str()))
        .exec(&transaction)
        .await?;
    if !request.repository_ids.is_empty() {
        let rows = request
            .repository_ids
            .iter()
            .enumerate()
            .map(|(position, repository_id)| namespace_pin::ActiveModel {
                namespace: Set(slug.clone()),
                repository_id: Set(*repository_id),
                position: Set(i32::try_from(position).unwrap_or(i32::MAX)),
                created_at: Set(now),
            });
        namespace_pin::Entity::insert_many(rows)
            .exec(&transaction)
            .await?;
    }
    transaction.commit().await?;

    let repositories = pinned_repositories(&state, &slug, accessible).await?;
    Ok(Json(PinnedRepositoriesResponse {
        repositories,
        limit: MAX_PINS,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repository::browser::AuthorActivity;

    fn date(day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 9, day).unwrap()
    }

    #[test]
    fn merged_activity_sums_days_across_repositories() {
        let mut total = BTreeMap::from([(date(1), 2), (date(2), 1)]);
        merge_activity(&mut total, BTreeMap::from([(date(2), 4), (date(3), 5)]));
        assert_eq!(
            total,
            BTreeMap::from([(date(1), 2), (date(2), 5), (date(3), 5)])
        );
    }

    #[test]
    fn a_person_counts_only_commits_they_wrote_or_signed() {
        let author = |day, email: &str, signer: Option<&str>, count| AuthorActivity {
            date: date(day),
            email: email.to_owned(),
            signer: signer.map(str::to_owned),
            count,
        };
        let overview = GitOverview::for_tests(
            BTreeMap::from([(date(1), 7), (date(2), 4)]),
            vec![
                author(1, "me@example.com", None, 2),
                author(1, "someone@example.com", None, 5),
                author(2, "laptop@example.com", Some("SHA256:mine"), 3),
                author(2, "someone@example.com", Some("SHA256:theirs"), 1),
            ],
        );
        let person = ProfileScope::Person(CommitIdentities {
            emails: HashSet::from(["me@example.com".to_owned()]),
            fingerprints: HashSet::from(["SHA256:mine".to_owned()]),
        });
        assert_eq!(
            person.count(overview.clone()),
            BTreeMap::from([(date(1), 2), (date(2), 3)])
        );
        let organization = ProfileScope::Organization("team".to_owned());
        assert_eq!(
            organization.count(overview),
            BTreeMap::from([(date(1), 7), (date(2), 4)])
        );
    }

    #[test]
    fn pins_accept_only_unique_repositories_from_the_namespace() {
        let ids = (0..8).map(|_| Uuid::new_v4()).collect::<Vec<_>>();
        let allowed = ids.iter().copied().collect::<HashSet<_>>();

        assert!(validate_pins(&[], &allowed).is_ok());
        assert!(validate_pins(&ids[..MAX_PINS], &allowed).is_ok());
        assert!(validate_pins(&ids[..=MAX_PINS], &allowed).is_err());
        assert!(validate_pins(&[ids[0], ids[0]], &allowed).is_err());
        assert!(validate_pins(&[Uuid::new_v4()], &allowed).is_err());
    }
}
