use axum::{
    Json,
    extract::{Path as AxumPath, Query, State},
    http::HeaderMap,
};
use axum_extra::extract::cookie::CookieJar;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use super::{Permission, RepositoryState};
use crate::identity::{ApiError, SCOPE_READ, SCOPE_WRITE};

#[derive(Serialize, ToSchema)]
pub struct TopicsResponse {
    topics: Vec<String>,
}

#[derive(Deserialize, ToSchema)]
pub struct ReplaceTopicsRequest {
    topics: Vec<String>,
}

#[derive(Deserialize, utoipa::ToSchema, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct SuggestTopicsQuery {
    q: Option<String>,
}

#[utoipa::path(
    get, path = "/repositories/{namespace}/{name}/topics",
    operation_id = "get_repositories_namespace_name_topics",
    summary = "List the repository's topics",
    tag = "Repositories",
    params(
        ("namespace" = String, Path),
        ("name" = String, Path),
    ),
    security((), ("token" = []), ("session" = [])),
    responses(
        (status = 200, description = "Success", body = TopicsResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
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

#[utoipa::path(
    put, path = "/repositories/{namespace}/{name}/topics",
    operation_id = "put_repositories_namespace_name_topics",
    summary = "Replace the repository's topics",
    tag = "Repositories",

    params(
        ("namespace" = String, Path),
        ("name" = String, Path),
    ),
    responses(
        (status = 200, description = "Success", body = TopicsResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
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
    let topics = gitadel_repositories::topics::replace_topics(
        state.identity().database(),
        &super::principal(&headers, &jar)?,
        repository.id,
        request.topics,
    )
    .await?;
    Ok(Json(TopicsResponse { topics }))
}

/// Suggests topic names the caller can already see: every public repository plus
/// the ones they have access to, so private topic names never leak.
#[utoipa::path(
    get, path = "/topics",
    operation_id = "get_topics",
    summary = "Suggest topics",
    tag = "Instance",
    params(
        SuggestTopicsQuery,
    ),
    security((), ("token" = []), ("session" = [])),
    responses(
        (status = 200, description = "Success", body = TopicsResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn suggest_topics(
    State(state): State<RepositoryState>,
    Query(query): Query<SuggestTopicsQuery>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<TopicsResponse>, ApiError> {
    let _actor = state
        .identity()
        .optional_user(&headers, &jar, SCOPE_READ)
        .await?
        .map(|user| user.id);
    let topics = gitadel_repositories::topics::suggest_topics(
        state.identity().database(),
        &super::principal(&headers, &jar)?,
        query.q,
    )
    .await?;
    Ok(Json(TopicsResponse { topics }))
}

pub(super) async fn repository_topics(
    state: &RepositoryState,
    repository_id: Uuid,
) -> Result<Vec<String>, ApiError> {
    Ok(
        gitadel_repositories::topics::repository_topics(state.identity().database(), repository_id)
            .await?,
    )
}
