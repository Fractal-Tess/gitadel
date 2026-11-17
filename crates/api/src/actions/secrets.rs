//! Actions secrets and variables at repository and namespace scope.
//!
//! Secrets are sealed by [`crate::secrets`] under a key derived from the SSH
//! host key, bound to their scope and name. Secret values are write-only: the
//! API only ever returns their names.
//! Repository values override namespace values with the same name.

use crate::openapi::ContractRouterExt;

use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
};
use axum_extra::extract::cookie::CookieJar;
use chrono::Utc;
use serde::{Deserialize, Serialize};

pub(crate) use crate::secrets::SecretCipher;
use crate::{
    identity::{ApiError, AuthenticatedUser, SCOPE_READ, SCOPE_WRITE},
    repository::Permission,
};

use super::ActionsState;

pub(crate) use gitadel_actions::secrets::Scope;
fn service(state: &ActionsState) -> gitadel_actions::secrets::SecretService<'_> {
    gitadel_actions::secrets::SecretService::new(
        state.repository().identity().database(),
        state.secret_cipher(),
        gitadel_core::audit::AuditContext {
            remote_address: crate::identity::remote_address(),
        },
    )
}
pub(crate) fn router() -> utoipa_axum::router::OpenApiRouter<ActionsState> {
    utoipa_axum::router::OpenApiRouter::new()
        .api_routes(crate::openapi::routes!(
            crate::actions::secrets::list_repository_secrets
        ))
        .api_routes(crate::openapi::routes!(
            crate::actions::secrets::set_repository_secret,
            crate::actions::secrets::delete_repository_secret
        ))
        .api_routes(crate::openapi::routes!(
            crate::actions::secrets::list_repository_variables
        ))
        .api_routes(crate::openapi::routes!(
            crate::actions::secrets::set_repository_variable,
            crate::actions::secrets::delete_repository_variable
        ))
        .api_routes(crate::openapi::routes!(
            crate::actions::secrets::list_namespace_secrets
        ))
        .api_routes(crate::openapi::routes!(
            crate::actions::secrets::set_namespace_secret,
            crate::actions::secrets::delete_namespace_secret
        ))
        .api_routes(crate::openapi::routes!(
            crate::actions::secrets::list_namespace_variables
        ))
        .api_routes(crate::openapi::routes!(
            crate::actions::secrets::set_namespace_variable,
            crate::actions::secrets::delete_namespace_variable
        ))
}

#[derive(Serialize, utoipa::ToSchema)]
struct SecretResponse {
    name: String,
    created_at: chrono::DateTime<Utc>,
    updated_at: chrono::DateTime<Utc>,
}

#[derive(Serialize, utoipa::ToSchema)]
struct SecretsResponse {
    secrets: Vec<SecretResponse>,
}

#[derive(Serialize, utoipa::ToSchema)]
struct VariableResponse {
    name: String,
    value: String,
    created_at: chrono::DateTime<Utc>,
    updated_at: chrono::DateTime<Utc>,
}

#[derive(Serialize, utoipa::ToSchema)]
struct VariablesResponse {
    variables: Vec<VariableResponse>,
}

#[derive(Deserialize, utoipa::ToSchema)]
struct ValueRequest {
    value: String,
}

async fn authorize_repository(
    state: &ActionsState,
    headers: &HeaderMap,
    jar: &CookieJar,
    namespace: &str,
    name: &str,
    scope: i32,
) -> Result<(AuthenticatedUser, Scope, String), ApiError> {
    let (actor, repository) = state
        .repository()
        .authenticated_repository(headers, jar, namespace, name, Permission::Manage, scope)
        .await?;
    let target = format!("{}/{}", repository.namespace, repository.name);
    Ok((actor, Scope::Repository(repository.id), target))
}

async fn authorize_namespace(
    state: &ActionsState,
    headers: &HeaderMap,
    jar: &CookieJar,
    namespace: &str,
    scope: i32,
) -> Result<(AuthenticatedUser, Scope, String), ApiError> {
    let identity = state.repository().identity();
    let actor = identity.authenticate(headers, jar, scope).await?;
    crate::identity::authorize_namespace(identity, &actor, namespace).await?;
    Ok((
        actor,
        Scope::Namespace(namespace.to_owned()),
        namespace.to_owned(),
    ))
}

#[utoipa::path(
    get, path = "/repositories/{namespace}/{name}/actions/secrets",
    operation_id = "get_repositories_namespace_name_actions_secrets",
    summary = "List repository secrets",
    tag = "Actions",
    params(
        ("namespace" = String, Path),
        ("name" = String, Path),
    ),
    security((), ("token" = []), ("session" = [])),
    responses(
        (status = 200, description = "Success", body = SecretsResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
async fn list_repository_secrets(
    State(state): State<ActionsState>,
    Path((namespace, name)): Path<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<SecretsResponse>, ApiError> {
    let (_, scope, _) =
        authorize_repository(&state, &headers, &jar, &namespace, &name, SCOPE_READ).await?;
    Ok(Json(
        service(&state)
            .list_secrets(&crate::repository::principal(&headers, &jar)?, &scope)
            .await?
            .into(),
    ))
}

#[utoipa::path(
    put, path = "/repositories/{namespace}/{name}/actions/secrets/{secret}",
    operation_id = "put_repositories_namespace_name_actions_secrets_secret",
    summary = "Set repository secret",
    tag = "Actions",
    params(
        ("namespace" = String, Path),
        ("name" = String, Path),
        ("secret" = String, Path),
    ),
    responses(
        (status = 200, description = "Success", body = SecretResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
async fn set_repository_secret(
    State(state): State<ActionsState>,
    Path((namespace, name, secret)): Path<(String, String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<ValueRequest>,
) -> Result<Json<SecretResponse>, ApiError> {
    let (_, scope, _) =
        authorize_repository(&state, &headers, &jar, &namespace, &name, SCOPE_WRITE).await?;
    Ok(Json(
        service(&state)
            .set_secret(
                &crate::repository::principal(&headers, &jar)?,
                &scope,
                &secret,
                &request.value,
            )
            .await?
            .into(),
    ))
}

#[utoipa::path(
    delete, path = "/repositories/{namespace}/{name}/actions/secrets/{secret}",
    operation_id = "delete_repositories_namespace_name_actions_secrets_secret",
    summary = "Delete repository secret",
    tag = "Actions",
    params(
        ("namespace" = String, Path),
        ("name" = String, Path),
        ("secret" = String, Path),
    ),
    responses(
        (status = 204, description = "Success"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
async fn delete_repository_secret(
    State(state): State<ActionsState>,
    Path((namespace, name, secret)): Path<(String, String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<StatusCode, ApiError> {
    let (_, scope, _) =
        authorize_repository(&state, &headers, &jar, &namespace, &name, SCOPE_WRITE).await?;
    service(&state)
        .delete_secret(
            &crate::repository::principal(&headers, &jar)?,
            &scope,
            &secret,
        )
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    get, path = "/repositories/{namespace}/{name}/actions/variables",
    operation_id = "get_repositories_namespace_name_actions_variables",
    summary = "List repository variables",
    tag = "Actions",
    params(
        ("namespace" = String, Path),
        ("name" = String, Path),
    ),
    security((), ("token" = []), ("session" = [])),
    responses(
        (status = 200, description = "Success", body = VariablesResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
async fn list_repository_variables(
    State(state): State<ActionsState>,
    Path((namespace, name)): Path<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<VariablesResponse>, ApiError> {
    let (_, scope, _) =
        authorize_repository(&state, &headers, &jar, &namespace, &name, SCOPE_READ).await?;
    Ok(Json(
        service(&state)
            .list_variables(&crate::repository::principal(&headers, &jar)?, &scope)
            .await?
            .into(),
    ))
}

#[utoipa::path(
    put, path = "/repositories/{namespace}/{name}/actions/variables/{variable}",
    operation_id = "put_repositories_namespace_name_actions_variables_variable",
    summary = "Set repository variable",
    tag = "Actions",
    params(
        ("namespace" = String, Path),
        ("name" = String, Path),
        ("variable" = String, Path),
    ),
    responses(
        (status = 200, description = "Success", body = VariableResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
async fn set_repository_variable(
    State(state): State<ActionsState>,
    Path((namespace, name, variable)): Path<(String, String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<ValueRequest>,
) -> Result<Json<VariableResponse>, ApiError> {
    let (_, scope, _) =
        authorize_repository(&state, &headers, &jar, &namespace, &name, SCOPE_WRITE).await?;
    Ok(Json(
        service(&state)
            .set_variable(
                &crate::repository::principal(&headers, &jar)?,
                &scope,
                &variable,
                request.value,
            )
            .await?
            .into(),
    ))
}

#[utoipa::path(
    delete, path = "/repositories/{namespace}/{name}/actions/variables/{variable}",
    operation_id = "delete_repositories_namespace_name_actions_variables_variable",
    summary = "Delete repository variable",
    tag = "Actions",
    params(
        ("namespace" = String, Path),
        ("name" = String, Path),
        ("variable" = String, Path),
    ),
    responses(
        (status = 204, description = "Success"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
async fn delete_repository_variable(
    State(state): State<ActionsState>,
    Path((namespace, name, variable)): Path<(String, String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<StatusCode, ApiError> {
    let (_, scope, _) =
        authorize_repository(&state, &headers, &jar, &namespace, &name, SCOPE_WRITE).await?;
    service(&state)
        .delete_variable(
            &crate::repository::principal(&headers, &jar)?,
            &scope,
            &variable,
        )
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    get, path = "/namespaces/{namespace}/actions/secrets",
    operation_id = "get_namespaces_namespace_actions_secrets",
    summary = "List namespace secrets",
    tag = "Actions",
    params(
        ("namespace" = String, Path),
    ),
    responses(
        (status = 200, description = "Success", body = SecretsResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
async fn list_namespace_secrets(
    State(state): State<ActionsState>,
    Path(namespace): Path<String>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<SecretsResponse>, ApiError> {
    let (_, scope, _) = authorize_namespace(&state, &headers, &jar, &namespace, SCOPE_READ).await?;
    Ok(Json(
        service(&state)
            .list_secrets(&crate::repository::principal(&headers, &jar)?, &scope)
            .await?
            .into(),
    ))
}

#[utoipa::path(
    put, path = "/namespaces/{namespace}/actions/secrets/{secret}",
    operation_id = "put_namespaces_namespace_actions_secrets_secret",
    summary = "Set namespace secret",
    tag = "Actions",
    params(
        ("namespace" = String, Path),
        ("secret" = String, Path),
    ),
    responses(
        (status = 200, description = "Success", body = SecretResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
async fn set_namespace_secret(
    State(state): State<ActionsState>,
    Path((namespace, secret)): Path<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<ValueRequest>,
) -> Result<Json<SecretResponse>, ApiError> {
    let (_, scope, _) =
        authorize_namespace(&state, &headers, &jar, &namespace, SCOPE_WRITE).await?;
    Ok(Json(
        service(&state)
            .set_secret(
                &crate::repository::principal(&headers, &jar)?,
                &scope,
                &secret,
                &request.value,
            )
            .await?
            .into(),
    ))
}

#[utoipa::path(
    delete, path = "/namespaces/{namespace}/actions/secrets/{secret}",
    operation_id = "delete_namespaces_namespace_actions_secrets_secret",
    summary = "Delete namespace secret",
    tag = "Actions",
    params(
        ("namespace" = String, Path),
        ("secret" = String, Path),
    ),
    responses(
        (status = 204, description = "Success"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
async fn delete_namespace_secret(
    State(state): State<ActionsState>,
    Path((namespace, secret)): Path<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<StatusCode, ApiError> {
    let (_, scope, _) =
        authorize_namespace(&state, &headers, &jar, &namespace, SCOPE_WRITE).await?;
    service(&state)
        .delete_secret(
            &crate::repository::principal(&headers, &jar)?,
            &scope,
            &secret,
        )
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    get, path = "/namespaces/{namespace}/actions/variables",
    operation_id = "get_namespaces_namespace_actions_variables",
    summary = "List namespace variables",
    tag = "Actions",
    params(
        ("namespace" = String, Path),
    ),
    responses(
        (status = 200, description = "Success", body = VariablesResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
async fn list_namespace_variables(
    State(state): State<ActionsState>,
    Path(namespace): Path<String>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<VariablesResponse>, ApiError> {
    let (_, scope, _) = authorize_namespace(&state, &headers, &jar, &namespace, SCOPE_READ).await?;
    Ok(Json(
        service(&state)
            .list_variables(&crate::repository::principal(&headers, &jar)?, &scope)
            .await?
            .into(),
    ))
}

#[utoipa::path(
    put, path = "/namespaces/{namespace}/actions/variables/{variable}",
    operation_id = "put_namespaces_namespace_actions_variables_variable",
    summary = "Set namespace variable",
    tag = "Actions",
    params(
        ("namespace" = String, Path),
        ("variable" = String, Path),
    ),
    responses(
        (status = 200, description = "Success", body = VariableResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
async fn set_namespace_variable(
    State(state): State<ActionsState>,
    Path((namespace, variable)): Path<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<ValueRequest>,
) -> Result<Json<VariableResponse>, ApiError> {
    let (_, scope, _) =
        authorize_namespace(&state, &headers, &jar, &namespace, SCOPE_WRITE).await?;
    Ok(Json(
        service(&state)
            .set_variable(
                &crate::repository::principal(&headers, &jar)?,
                &scope,
                &variable,
                request.value,
            )
            .await?
            .into(),
    ))
}

#[utoipa::path(
    delete, path = "/namespaces/{namespace}/actions/variables/{variable}",
    operation_id = "delete_namespaces_namespace_actions_variables_variable",
    summary = "Delete namespace variable",
    tag = "Actions",
    params(
        ("namespace" = String, Path),
        ("variable" = String, Path),
    ),
    responses(
        (status = 204, description = "Success"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
async fn delete_namespace_variable(
    State(state): State<ActionsState>,
    Path((namespace, variable)): Path<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<StatusCode, ApiError> {
    let (_, scope, _) =
        authorize_namespace(&state, &headers, &jar, &namespace, SCOPE_WRITE).await?;
    service(&state)
        .delete_variable(
            &crate::repository::principal(&headers, &jar)?,
            &scope,
            &variable,
        )
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

impl From<gitadel_actions::secrets::SecretResponse> for SecretResponse {
    fn from(value: gitadel_actions::secrets::SecretResponse) -> Self {
        Self {
            name: value.name,
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

impl From<gitadel_actions::secrets::VariableResponse> for VariableResponse {
    fn from(value: gitadel_actions::secrets::VariableResponse) -> Self {
        Self {
            name: value.name,
            value: value.value,
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

impl From<gitadel_actions::secrets::SecretsResponse> for SecretsResponse {
    fn from(value: gitadel_actions::secrets::SecretsResponse) -> Self {
        Self {
            secrets: value.secrets.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<gitadel_actions::secrets::VariablesResponse> for VariablesResponse {
    fn from(value: gitadel_actions::secrets::VariablesResponse) -> Self {
        Self {
            variables: value.variables.into_iter().map(Into::into).collect(),
        }
    }
}
