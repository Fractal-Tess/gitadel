use crate::openapi::ContractRouterExt;

use axum::{
    Json,
    extract::{Path as AxumPath, State},
    http::HeaderMap,
};
use axum_extra::extract::cookie::CookieJar;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{ApiError, IdentityState, SCOPE_READ, SCOPE_WRITE};

pub(super) fn router() -> utoipa_axum::router::OpenApiRouter<IdentityState> {
    utoipa_axum::router::OpenApiRouter::new()
        .api_routes(crate::openapi::routes!(
            crate::identity::mirror_identities::list_identities,
            crate::identity::mirror_identities::create_identity
        ))
        .api_routes(crate::openapi::routes!(
            crate::identity::mirror_identities::update_identity,
            crate::identity::mirror_identities::delete_identity
        ))
}

#[derive(Serialize, utoipa::ToSchema)]
pub(crate) struct IdentityUsageResponse {
    pub repository: String,
    pub last_attempted_at: Option<chrono::DateTime<Utc>>,
    pub last_synced_at: Option<chrono::DateTime<Utc>>,
    pub last_error: Option<String>,
}

#[derive(Serialize, utoipa::ToSchema)]
pub(crate) struct MirrorIdentityResponse {
    pub id: Uuid,
    pub name: String,
    pub kind: String,
    pub username: Option<String>,
    pub provider: Option<String>,
    pub instance_url: Option<String>,
    pub public_key: Option<String>,
    pub fingerprint: Option<String>,
    pub last_used_at: Option<chrono::DateTime<Utc>>,
    pub usage_history: Vec<IdentityUsageResponse>,
    pub created_at: chrono::DateTime<Utc>,
    pub updated_at: chrono::DateTime<Utc>,
}

#[derive(Deserialize, utoipa::ToSchema)]
struct CreateIdentityRequest {
    name: String,
    token: String,
    server_url: String,
}

#[derive(Deserialize, utoipa::ToSchema)]
struct UpdateIdentityRequest {
    name: String,
    token: Option<String>,
    server_url: String,
}

#[derive(Clone)]
pub(crate) struct MirrorIdentitySecret {
    pub kind: String,
    pub username: Option<String>,
    pub secret: String,
    pub provider: Option<String>,
    pub instance_url: Option<String>,
}

pub(crate) async fn load_secret(
    state: &IdentityState,
    namespace: &str,
    id: Uuid,
) -> Result<MirrorIdentitySecret, ApiError> {
    Ok(domain::load_secret(&state.auth_service(), namespace, id)
        .await?
        .into())
}

pub(crate) async fn mark_identity_used(
    state: &IdentityState,
    namespace: &str,
    id: Uuid,
    used_at: chrono::DateTime<Utc>,
) -> Result<(), ApiError> {
    Ok(domain::mark_identity_used(&state.auth_service(), namespace, id, used_at).await?)
}

#[utoipa::path(
    get, path = "/namespaces/{namespace}/mirror-identities",
    operation_id = "get_namespaces_namespace_mirror_identities",
    summary = "List identities",
    tag = "Mirrors",
    params(
        ("namespace" = String, Path),
    ),
    responses(
        (status = 200, description = "Success", body = Vec<MirrorIdentityResponse>, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
async fn list_identities(
    State(state): State<IdentityState>,
    AxumPath(namespace): AxumPath<String>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<Vec<MirrorIdentityResponse>>, ApiError> {
    let actor = state
        .authenticate_principal(&headers, &jar, SCOPE_READ)
        .await?;
    Ok(Json(
        domain::list_identities(&state.auth_service(), &actor, namespace)
            .await?
            .into_iter()
            .map(Into::into)
            .collect(),
    ))
}

#[utoipa::path(
    post, path = "/namespaces/{namespace}/mirror-identities",
    operation_id = "post_namespaces_namespace_mirror_identities",
    summary = "Create identity",
    tag = "Mirrors",
    params(
        ("namespace" = String, Path),
    ),
    responses(
        (status = 200, description = "Success", body = MirrorIdentityResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
async fn create_identity(
    State(state): State<IdentityState>,
    AxumPath(namespace): AxumPath<String>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<CreateIdentityRequest>,
) -> Result<Json<MirrorIdentityResponse>, ApiError> {
    let actor = state
        .authenticate_principal(&headers, &jar, SCOPE_WRITE)
        .await?;
    Ok(Json(
        domain::create_identity(
            &state.auth_service(),
            &actor,
            namespace,
            domain::CreateIdentityRequest {
                name: request.name,
                token: request.token,
                server_url: request.server_url,
            },
        )
        .await?
        .into(),
    ))
}

#[utoipa::path(
    put, path = "/namespaces/{namespace}/mirror-identities/{id}",
    operation_id = "put_namespaces_namespace_mirror_identities_id",
    summary = "Update identity",
    tag = "Mirrors",
    params(
        ("namespace" = String, Path),
        ("id" = Uuid, Path),
    ),
    responses(
        (status = 200, description = "Success", body = MirrorIdentityResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
async fn update_identity(
    State(state): State<IdentityState>,
    AxumPath((namespace, id)): AxumPath<(String, Uuid)>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<UpdateIdentityRequest>,
) -> Result<Json<MirrorIdentityResponse>, ApiError> {
    let actor = state
        .authenticate_principal(&headers, &jar, SCOPE_WRITE)
        .await?;
    Ok(Json(
        domain::update_identity(
            &state.auth_service(),
            &actor,
            namespace,
            id,
            domain::UpdateIdentityRequest {
                name: request.name,
                token: request.token,
                server_url: request.server_url,
            },
        )
        .await?
        .into(),
    ))
}

#[utoipa::path(
    delete, path = "/namespaces/{namespace}/mirror-identities/{id}",
    operation_id = "delete_namespaces_namespace_mirror_identities_id",
    summary = "Delete identity",
    tag = "Mirrors",
    params(
        ("namespace" = String, Path),
        ("id" = Uuid, Path),
    ),
    responses(
        (status = 204, description = "Success"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
async fn delete_identity(
    State(state): State<IdentityState>,
    AxumPath((namespace, id)): AxumPath<(String, Uuid)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<axum::http::StatusCode, ApiError> {
    let actor = state
        .authenticate_principal(&headers, &jar, SCOPE_WRITE)
        .await?;
    domain::delete_identity(&state.auth_service(), &actor, namespace, id).await?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

use gitadel_identity::mirror_identities as domain;
impl From<domain::IdentityUsageResponse> for IdentityUsageResponse {
    fn from(v: domain::IdentityUsageResponse) -> Self {
        Self {
            repository: v.repository,
            last_attempted_at: v.last_attempted_at,
            last_synced_at: v.last_synced_at,
            last_error: v.last_error,
        }
    }
}

impl From<domain::MirrorIdentityResponse> for MirrorIdentityResponse {
    fn from(v: domain::MirrorIdentityResponse) -> Self {
        Self {
            id: v.id,
            name: v.name,
            kind: v.kind,
            username: v.username,
            provider: v.provider,
            instance_url: v.instance_url,
            public_key: v.public_key,
            fingerprint: v.fingerprint,
            last_used_at: v.last_used_at,
            usage_history: v.usage_history.into_iter().map(Into::into).collect(),
            created_at: v.created_at,
            updated_at: v.updated_at,
        }
    }
}

impl From<domain::MirrorIdentitySecret> for MirrorIdentitySecret {
    fn from(v: domain::MirrorIdentitySecret) -> Self {
        Self {
            kind: v.kind,
            username: v.username,
            secret: v.secret,
            provider: v.provider,
            instance_url: v.instance_url,
        }
    }
}

#[cfg(test)]
mod tests {
    use gitadel_identity::mirror_identities::{validate_identity_name, validate_token};

    #[test]
    fn identity_requires_a_name() {
        assert!(validate_identity_name(" ").is_err());
    }

    #[test]
    fn identity_requires_an_access_token() {
        assert!(validate_token(" ").is_err());
    }
}
