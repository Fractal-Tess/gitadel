//! Namespace profile pages: a year of commit activity and pinned repositories.

use std::collections::{BTreeMap, HashMap, HashSet};

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
        ActivityResponse, MAX_REPOSITORY_ACTIVITY_DAYS, RepositoryOverviewItemResponse,
        activity_response, activity_start_date, read_repository_overview,
        repository_overview_items,
    },
    resources::{accessible_repositories, owned_namespace},
};
use crate::{
    entity::{namespace, namespace_pin, repository},
    identity::{ApiError, SCOPE_WRITE},
};

/// Pins a namespace may feature, as on GitHub.
const MAX_PINS: usize = 6;

#[derive(Serialize)]
pub struct NamespaceActivityResponse {
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

#[derive(Deserialize)]
pub struct SetPinsRequest {
    repository_ids: Vec<Uuid>,
}

async fn ensure_namespace(state: &RepositoryState, slug: &str) -> Result<(), ApiError> {
    namespace::Entity::find_by_id(slug)
        .one(state.identity().database())
        .await?
        .map(|_| ())
        .ok_or_else(ApiError::not_found)
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
    ensure_namespace(&state, &slug).await?;
    let accessible = accessible_repositories(&state, &headers, &jar).await?;
    let repositories = accessible
        .repositories
        .into_iter()
        .filter(|repository| repository.namespace == slug && !repository.mirrored)
        .collect::<Vec<_>>();

    let end_date = Utc::now().date_naive();
    let start_date = activity_start_date(end_date, MAX_REPOSITORY_ACTIVITY_DAYS)?;
    let mut pending = JoinSet::new();
    for repository in repositories {
        let state = state.clone();
        let slots = state.analysis_slots.clone();
        pending.spawn(async move {
            let _permit = slots.acquire_owned().await.map_err(ApiError::internal)?;
            let name = format!("{}/{}", repository.namespace, repository.name);
            let result = read_repository_overview(&state, &repository, start_date, end_date).await;
            Ok::<_, ApiError>((name, result.map(|overview| overview.activity)))
        });
    }

    let mut days = BTreeMap::new();
    let mut repository_count = 0;
    while let Some(result) = pending.join_next().await {
        let (name, activity) = result.map_err(ApiError::internal)??;
        match activity {
            Ok(activity) => {
                repository_count += 1;
                merge_activity(&mut days, activity);
            }
            // One unreadable repository should not blank the whole graph.
            Err(_) => tracing::warn!(repository = %name, "could not read repository activity"),
        }
    }
    Ok(Json(NamespaceActivityResponse {
        repository_count,
        activity: activity_response(start_date, end_date, days),
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
