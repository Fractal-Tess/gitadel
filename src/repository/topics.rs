use std::collections::{HashMap, HashSet};

use axum::{
    Json,
    extract::{Path as AxumPath, Query, State},
    http::HeaderMap,
};
use axum_extra::extract::cookie::CookieJar;
use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, QueryOrder, Set, TransactionTrait,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{Permission, RepositoryState, resources::accessible_repositories};
use crate::{
    entity::{repository_topic, topic},
    identity::{ApiError, SCOPE_READ, SCOPE_WRITE},
};

/// Matches the `topics.name` column width and Gitea's own topic limits.
const MAX_TOPIC_LENGTH: usize = 35;
const MAX_TOPICS_PER_REPOSITORY: usize = 25;
const MAX_SUGGESTIONS: usize = 20;

#[derive(Serialize)]
pub struct TopicsResponse {
    topics: Vec<String>,
}

#[derive(Deserialize)]
pub struct ReplaceTopicsRequest {
    topics: Vec<String>,
}

#[derive(Deserialize)]
pub struct SuggestTopicsQuery {
    q: Option<String>,
}

pub async fn list_topics(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name)): AxumPath<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<TopicsResponse>, ApiError> {
    let repository = state.find(&namespace, &name).await?;
    let user_id = state
        .identity()
        .optional_user(&headers, &jar, SCOPE_READ)
        .await?
        .map(|account| account.id);
    state
        .authorize(&repository, user_id, Permission::Read)
        .await?;
    Ok(Json(TopicsResponse {
        topics: repository_topics(&state, repository.id).await?,
    }))
}

pub async fn replace_topics(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name)): AxumPath<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<ReplaceTopicsRequest>,
) -> Result<Json<TopicsResponse>, ApiError> {
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
    let names = normalize_topics(request.topics)?;

    let transaction = state.identity().database().begin().await?;
    let previous = repository_topic::Entity::find()
        .filter(repository_topic::Column::RepositoryId.eq(repository.id))
        .all(&transaction)
        .await?;
    repository_topic::Entity::delete_many()
        .filter(repository_topic::Column::RepositoryId.eq(repository.id))
        .exec(&transaction)
        .await?;

    let now = Utc::now();
    let mut existing: HashMap<String, Uuid> = if names.is_empty() {
        HashMap::new()
    } else {
        topic::Entity::find()
            .filter(topic::Column::Name.is_in(names.clone()))
            .all(&transaction)
            .await?
            .into_iter()
            .map(|record| (record.name, record.id))
            .collect()
    };
    for name in &names {
        let topic_id = match existing.get(name) {
            Some(topic_id) => *topic_id,
            None => {
                let created = topic::ActiveModel {
                    id: Set(Uuid::new_v4()),
                    name: Set(name.clone()),
                    created_at: Set(now),
                }
                .insert(&transaction)
                .await?;
                existing.insert(created.name, created.id);
                created.id
            }
        };
        repository_topic::ActiveModel {
            repository_id: Set(repository.id),
            topic_id: Set(topic_id),
            created_at: Set(now),
        }
        .insert(&transaction)
        .await?;
    }

    // Detached topics would otherwise linger forever and pollute suggestions.
    let retained: HashSet<Uuid> = existing.values().copied().collect();
    let orphan_candidates: Vec<Uuid> = previous
        .iter()
        .map(|link| link.topic_id)
        .filter(|topic_id| !retained.contains(topic_id))
        .collect();
    if !orphan_candidates.is_empty() {
        let still_used: HashSet<Uuid> = repository_topic::Entity::find()
            .filter(repository_topic::Column::TopicId.is_in(orphan_candidates.clone()))
            .all(&transaction)
            .await?
            .into_iter()
            .map(|link| link.topic_id)
            .collect();
        let orphans: Vec<Uuid> = orphan_candidates
            .into_iter()
            .filter(|topic_id| !still_used.contains(topic_id))
            .collect();
        if !orphans.is_empty() {
            topic::Entity::delete_many()
                .filter(topic::Column::Id.is_in(orphans))
                .exec(&transaction)
                .await?;
        }
    }
    transaction.commit().await?;

    Ok(Json(TopicsResponse { topics: names }))
}

/// Suggests topic names the caller can already see: every public repository plus
/// the ones they have access to, so private topic names never leak.
pub async fn suggest_topics(
    State(state): State<RepositoryState>,
    Query(query): Query<SuggestTopicsQuery>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<TopicsResponse>, ApiError> {
    let needle = query
        .q
        .map(|value| value.trim().to_lowercase())
        .filter(|value| !value.is_empty());
    let visible: HashSet<Uuid> = accessible_repositories(&state, &headers, &jar)
        .await?
        .repositories
        .into_iter()
        .map(|repository| repository.id)
        .collect();
    if visible.is_empty() {
        return Ok(Json(TopicsResponse { topics: Vec::new() }));
    }

    let mut usage: HashMap<Uuid, usize> = HashMap::new();
    for link in repository_topic::Entity::find()
        .all(state.identity().database())
        .await?
    {
        if visible.contains(&link.repository_id) {
            *usage.entry(link.topic_id).or_default() += 1;
        }
    }
    if usage.is_empty() {
        return Ok(Json(TopicsResponse { topics: Vec::new() }));
    }

    let mut matches: Vec<(usize, String)> = topic::Entity::find()
        .filter(topic::Column::Id.is_in(usage.keys().copied().collect::<Vec<_>>()))
        .all(state.identity().database())
        .await?
        .into_iter()
        .filter(|record| {
            needle
                .as_deref()
                .is_none_or(|needle| record.name.contains(needle))
        })
        .map(|record| (usage.get(&record.id).copied().unwrap_or(0), record.name))
        .collect();
    matches.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
    Ok(Json(TopicsResponse {
        topics: matches
            .into_iter()
            .take(MAX_SUGGESTIONS)
            .map(|(_, name)| name)
            .collect(),
    }))
}

pub(super) async fn repository_topics(
    state: &RepositoryState,
    repository_id: Uuid,
) -> Result<Vec<String>, ApiError> {
    let topic_ids: Vec<Uuid> = repository_topic::Entity::find()
        .filter(repository_topic::Column::RepositoryId.eq(repository_id))
        .all(state.identity().database())
        .await?
        .into_iter()
        .map(|link| link.topic_id)
        .collect();
    if topic_ids.is_empty() {
        return Ok(Vec::new());
    }
    Ok(topic::Entity::find()
        .filter(topic::Column::Id.is_in(topic_ids))
        .order_by_asc(topic::Column::Name)
        .all(state.identity().database())
        .await?
        .into_iter()
        .map(|record| record.name)
        .collect())
}

fn normalize_topics(values: Vec<String>) -> Result<Vec<String>, ApiError> {
    let mut seen = HashSet::new();
    let mut topics = Vec::new();
    for value in values {
        let name = normalize_topic(&value)?;
        if seen.insert(name.clone()) {
            topics.push(name);
        }
    }
    if topics.len() > MAX_TOPICS_PER_REPOSITORY {
        return Err(ApiError::bad_request(
            "Repositories may have at most 25 topics.",
        ));
    }
    topics.sort();
    Ok(topics)
}

fn normalize_topic(value: &str) -> Result<String, ApiError> {
    let name = value.trim().to_lowercase();
    let valid = (1..=MAX_TOPIC_LENGTH).contains(&name.len())
        && name.starts_with(|first: char| first.is_ascii_alphanumeric())
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'));
    if !valid {
        return Err(ApiError::bad_request(
            "Topics must start with a letter or number and may contain up to 35 letters, numbers, hyphens, underscores, and periods.",
        ));
    }
    Ok(name)
}
