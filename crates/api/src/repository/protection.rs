//! Branch and tag protection rules.
//!
//! Rules are loaded once per push into a [`RefGuard`], which is evaluated
//! synchronously inside the receive-pack worker before any ref is written.
//! Every ref write path (HTTP and SSH receive-pack and in-app commits) asks
//! the same guard, so a rule cannot be bypassed by switching transports.

use axum::{
    Json,
    extract::{Path as AxumPath, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use axum_extra::extract::cookie::CookieJar;
#[cfg(test)]
use chrono::Utc;
pub use gitadel_repositories::protection::ProtectionRuleResponse;
use serde::Deserialize;
use utoipa::ToSchema;
use uuid::Uuid;

use super::{Permission, RepositoryState};
use crate::{
    entity::repository,
    identity::{ApiError, SCOPE_READ, SCOPE_WRITE},
};

pub(crate) use gitadel_repositories::protection::{RefCheck, RefGuard};
pub(crate) async fn load_guard(
    state: &RepositoryState,
    repository: &repository::Model,
    actor: Option<Uuid>,
) -> Result<RefGuard, ApiError> {
    Ok(
        gitadel_repositories::protection::load_guard(
            state.identity().database(),
            repository,
            actor,
        )
        .await?,
    )
}
fn service(state: &RepositoryState) -> gitadel_repositories::protection::ProtectionService<'_> {
    gitadel_repositories::protection::ProtectionService::new(
        state.identity().database(),
        gitadel_core::audit::AuditContext {
            remote_address: crate::identity::remote_address(),
        },
    )
}
#[derive(Deserialize, ToSchema)]
pub struct CreateProtectionRuleRequest {
    kind: String,
    pattern: String,
    block_force_push: Option<bool>,
    block_deletion: Option<bool>,
    block_update: Option<bool>,
    restrict_pushes: Option<bool>,
    admins_bypass: Option<bool>,
    #[serde(default)]
    allowed_users: Vec<String>,
}

#[derive(Deserialize, ToSchema)]
pub struct UpdateProtectionRuleRequest {
    pattern: Option<String>,
    block_force_push: Option<bool>,
    block_deletion: Option<bool>,
    block_update: Option<bool>,
    restrict_pushes: Option<bool>,
    admins_bypass: Option<bool>,
    allowed_users: Option<Vec<String>>,
}

#[utoipa::path(
    get, path = "/repositories/{namespace}/{name}/protection-rules",
    operation_id = "get_repositories_namespace_name_protection_rules",
    summary = "List the repository's protection rules",
    tag = "Branch protection",
    params(
        ("namespace" = String, Path),
        ("name" = String, Path),
    ),
    responses(
        (status = 200, description = "Success", body = Vec<ProtectionRuleResponse>, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn list_rules(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name)): AxumPath<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<Vec<ProtectionRuleResponse>>, ApiError> {
    let (_, repository) = state
        .authenticated_repository(
            &headers,
            &jar,
            &namespace,
            &name,
            Permission::Manage,
            SCOPE_READ,
        )
        .await?;
    Ok(Json(
        service(&state)
            .list_rules(
                &crate::repository::principal(&headers, &jar)?,
                repository.id,
            )
            .await?,
    ))
}
#[utoipa::path(
    post, path = "/repositories/{namespace}/{name}/protection-rules",
    operation_id = "post_repositories_namespace_name_protection_rules",
    summary = "Create a protection rule",
    tag = "Branch protection",

    params(
        ("namespace" = String, Path),
        ("name" = String, Path),
    ),
    responses(
        (status = 201, description = "Success", body = crate::repository::protection::ProtectionRuleResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn create_rule(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name)): AxumPath<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<CreateProtectionRuleRequest>,
) -> Result<impl IntoResponse + crate::openapi::TypedResponse, ApiError> {
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
    Ok((
        StatusCode::CREATED,
        Json(
            service(&state)
                .create_rule(
                    &crate::repository::principal(&headers, &jar)?,
                    repository.id,
                    request.into(),
                )
                .await?,
        ),
    ))
}
#[utoipa::path(
    patch, path = "/repositories/{namespace}/{name}/protection-rules/{id}",
    operation_id = "patch_repositories_namespace_name_protection_rules_id",
    summary = "Update a protection rule",
    tag = "Branch protection",

    params(
        ("namespace" = String, Path),
        ("name" = String, Path),
        ("id" = Uuid, Path),
    ),
    responses(
        (status = 200, description = "Success", body = ProtectionRuleResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn update_rule(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name, id)): AxumPath<(String, String, Uuid)>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<UpdateProtectionRuleRequest>,
) -> Result<Json<ProtectionRuleResponse>, ApiError> {
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
    Ok(Json(
        service(&state)
            .update_rule(
                &crate::repository::principal(&headers, &jar)?,
                repository.id,
                id,
                request.into(),
            )
            .await?,
    ))
}
#[utoipa::path(
    delete, path = "/repositories/{namespace}/{name}/protection-rules/{id}",
    operation_id = "delete_repositories_namespace_name_protection_rules_id",
    summary = "Delete a protection rule",
    tag = "Branch protection",
    params(
        ("namespace" = String, Path),
        ("name" = String, Path),
        ("id" = Uuid, Path),
    ),
    responses(
        (status = 204, description = "Success"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn delete_rule(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name, id)): AxumPath<(String, String, Uuid)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<StatusCode, ApiError> {
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
    service(&state)
        .delete_rule(
            &crate::repository::principal(&headers, &jar)?,
            repository.id,
            id,
        )
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
#[cfg(test)]
pub(crate) fn test_rule(
    kind: &str,
    pattern: &str,
) -> crate::entity::repository_protection_rule::Model {
    let now = Utc::now();
    crate::entity::repository_protection_rule::Model {
        id: Uuid::new_v4(),
        repository_id: Uuid::new_v4(),
        kind: kind.to_owned(),
        pattern: pattern.to_owned(),
        block_force_push: kind == "branch",
        block_deletion: true,
        block_update: kind == "tag",
        restrict_pushes: false,
        admins_bypass: true,
        created_at: now,
        updated_at: now,
    }
}

impl From<CreateProtectionRuleRequest>
    for gitadel_repositories::protection::CreateProtectionRuleRequest
{
    fn from(value: CreateProtectionRuleRequest) -> Self {
        Self {
            kind: value.kind,
            pattern: value.pattern,
            block_force_push: value.block_force_push,
            block_deletion: value.block_deletion,
            block_update: value.block_update,
            restrict_pushes: value.restrict_pushes,
            admins_bypass: value.admins_bypass,
            allowed_users: value.allowed_users,
        }
    }
}

impl From<UpdateProtectionRuleRequest>
    for gitadel_repositories::protection::UpdateProtectionRuleRequest
{
    fn from(value: UpdateProtectionRuleRequest) -> Self {
        Self {
            pattern: value.pattern,
            block_force_push: value.block_force_push,
            block_deletion: value.block_deletion,
            block_update: value.block_update,
            restrict_pushes: value.restrict_pushes,
            admins_bypass: value.admins_bypass,
            allowed_users: value.allowed_users,
        }
    }
}
