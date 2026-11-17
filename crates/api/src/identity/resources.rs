use axum::{
    Json,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use axum_extra::extract::cookie::CookieJar;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use super::{ApiError, IdentityState, Pagination, SCOPE_READ, SCOPE_SSH_KEYS, SCOPE_WRITE};
#[cfg(test)]
use crate::entity::organization;

#[derive(Serialize, ToSchema)]
pub struct SshKeyResponse {
    id: Uuid,
    name: String,
    fingerprint: String,
    public_key: String,
    created_at: chrono::DateTime<Utc>,
    last_used_at: Option<chrono::DateTime<Utc>>,
}

#[utoipa::path(
    get, path = "/me/ssh-keys",
    operation_id = "get_me_ssh_keys",
    summary = "List SSH keys",
    tag = "Account",
    responses(
        (status = 200, description = "Success", body = Vec<SshKeyResponse>, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn list_ssh_keys(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<Vec<SshKeyResponse>>, ApiError> {
    let actor = state
        .authenticate_principal(&headers, &jar, SCOPE_READ)
        .await?;
    Ok(Json(
        domain::list_ssh_keys(&state.account_service(), &actor)
            .await?
            .into_iter()
            .map(Into::into)
            .collect(),
    ))
}

#[derive(Deserialize, ToSchema)]
pub struct CreateSshKeyRequest {
    name: String,
    public_key: String,
}

#[utoipa::path(
    post, path = "/me/ssh-keys",
    operation_id = "post_me_ssh_keys",
    summary = "Add an SSH key",
    tag = "Account",

    responses(
        (status = 201, description = "Success", body = crate::identity::resources::SshKeyResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn create_ssh_key(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<CreateSshKeyRequest>,
) -> Result<impl IntoResponse + crate::openapi::TypedResponse, ApiError> {
    let actor = state
        .authenticate_principal(&headers, &jar, SCOPE_SSH_KEYS)
        .await?;
    Ok((
        StatusCode::CREATED,
        Json::<SshKeyResponse>(
            domain::create_ssh_key(
                &state.account_service(),
                &actor,
                CreateSshKeyRequestDomain {
                    name: request.name,
                    public_key: request.public_key,
                },
            )
            .await?
            .into(),
        ),
    ))
}

#[utoipa::path(
    delete, path = "/me/ssh-keys/{id}",
    operation_id = "delete_me_ssh_keys_id",
    summary = "Delete an SSH key",
    tag = "Account",
    params(
        ("id" = Uuid, Path),
    ),
    responses(
        (status = 204, description = "Success"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn delete_ssh_key(
    State(state): State<IdentityState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<StatusCode, ApiError> {
    let actor = state
        .authenticate_principal(&headers, &jar, SCOPE_SSH_KEYS)
        .await?;
    domain::delete_ssh_key(&state.account_service(), &actor, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize, ToSchema)]
#[schema(as = ApiTokenResponse)]
pub struct TokenResponse {
    id: Uuid,
    name: String,
    scopes: Vec<&'static str>,
    expires_at: Option<chrono::DateTime<Utc>>,
    created_at: chrono::DateTime<Utc>,
    last_used_at: Option<chrono::DateTime<Utc>>,
}

#[utoipa::path(
    get, path = "/me/tokens",
    operation_id = "get_me_tokens",
    summary = "List API tokens",
    tag = "Account",
    responses(
        (status = 200, description = "Success", body = Vec<TokenResponse>, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn list_tokens(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<Vec<TokenResponse>>, ApiError> {
    let actor = state
        .authenticate_principal(&headers, &jar, SCOPE_READ)
        .await?;
    Ok(Json(
        domain::list_tokens(&state.account_service(), &actor)
            .await?
            .into_iter()
            .map(Into::into)
            .collect(),
    ))
}

#[derive(Deserialize, ToSchema)]
pub struct CreateTokenRequest {
    name: String,
    scopes: Vec<String>,
    expires_in_days: Option<i64>,
}

#[derive(Serialize, ToSchema)]
pub struct CreatedTokenResponse {
    token: String,
    details: TokenResponse,
}

#[utoipa::path(
    post, path = "/me/tokens",
    operation_id = "post_me_tokens",
    summary = "Create an API token",
    tag = "Account",

    responses(
        (status = 201, description = "Success", body = crate::identity::resources::CreatedTokenResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn create_token(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<CreateTokenRequest>,
) -> Result<impl IntoResponse + crate::openapi::TypedResponse, ApiError> {
    let actor = state
        .authenticate_principal(&headers, &jar, SCOPE_WRITE)
        .await?;
    let result = domain::create_token(
        &state.account_service(),
        &actor,
        CreateTokenRequestDomain {
            name: request.name,
            scopes: request.scopes,
            expires_in_days: request.expires_in_days,
        },
    )
    .await?;
    Ok((
        StatusCode::CREATED,
        Json::<CreatedTokenResponse>(result.into()),
    ))
}

#[utoipa::path(
    delete, path = "/me/tokens/{id}",
    operation_id = "delete_me_tokens_id",
    summary = "Revoke an API token",
    tag = "Account",
    params(
        ("id" = Uuid, Path),
    ),
    responses(
        (status = 204, description = "Success"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn revoke_token(
    State(state): State<IdentityState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<StatusCode, ApiError> {
    let actor = state
        .authenticate_principal(&headers, &jar, SCOPE_WRITE)
        .await?;
    domain::revoke_token(&state.account_service(), &actor, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize, ToSchema)]
pub struct NamespaceResponse {
    slug: String,
    kind: String,
}

#[utoipa::path(
    get, path = "/namespaces/{slug}",
    operation_id = "get_namespaces_slug",
    summary = "Get whether a namespace is a user or an organization",
    tag = "Namespaces",
    params(
        ("slug" = String, Path),
    ),
    security(),
    responses(
        (status = 200, description = "Success", body = NamespaceResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn get_namespace(
    State(state): State<IdentityState>,
    Path(slug): Path<String>,
) -> Result<Json<NamespaceResponse>, ApiError> {
    Ok(Json(
        domain::get_namespace(&state.account_service(), slug)
            .await?
            .into(),
    ))
}

#[derive(Serialize, ToSchema)]
pub struct OrganizationResponse {
    id: Uuid,
    slug: String,
    display_name: String,
    avatar_updated_at: Option<chrono::DateTime<Utc>>,
    role: String,
}

#[utoipa::path(
    get, path = "/organizations",
    operation_id = "get_organizations",
    summary = "List the organizations you belong to",
    tag = "Organizations",
    responses(
        (status = 200, description = "Success", body = Vec<OrganizationResponse>, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn list_organizations(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<Vec<OrganizationResponse>>, ApiError> {
    let actor = state
        .authenticate_principal(&headers, &jar, SCOPE_READ)
        .await?;
    Ok(Json(
        domain::list_organizations(&state.account_service(), &actor)
            .await?
            .into_iter()
            .map(Into::into)
            .collect(),
    ))
}

#[derive(Deserialize, ToSchema)]
pub struct CreateOrganizationRequest {
    slug: String,
    display_name: String,
}

#[utoipa::path(
    post, path = "/organizations",
    operation_id = "post_organizations",
    summary = "Create an organization",
    tag = "Organizations",

    responses(
        (status = 201, description = "Success", body = crate::identity::resources::OrganizationResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn create_organization(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<CreateOrganizationRequest>,
) -> Result<impl IntoResponse + crate::openapi::TypedResponse, ApiError> {
    let actor = state
        .authenticate_principal(&headers, &jar, SCOPE_WRITE)
        .await?;
    Ok((
        StatusCode::CREATED,
        Json::<OrganizationResponse>(
            domain::create_organization(
                &state.account_service(),
                &actor,
                CreateOrganizationRequestDomain {
                    slug: request.slug,
                    display_name: request.display_name,
                },
            )
            .await?
            .into(),
        ),
    ))
}

#[derive(Deserialize, ToSchema)]
pub struct UpdateOrganizationRequest {
    slug: String,
    display_name: String,
}

#[utoipa::path(
    put, path = "/organizations/{slug}",
    operation_id = "put_organizations_slug",
    summary = "Rename or update an organization",
    tag = "Organizations",

    params(
        ("slug" = String, Path),
    ),
    responses(
        (status = 200, description = "Success", body = OrganizationResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn update_organization(
    State(state): State<IdentityState>,
    Path(slug): Path<String>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<UpdateOrganizationRequest>,
) -> Result<Json<OrganizationResponse>, ApiError> {
    let actor = state
        .authenticate_principal(&headers, &jar, SCOPE_WRITE)
        .await?;
    Ok(Json(
        domain::update_organization(
            &state.account_service(),
            &actor,
            slug,
            UpdateOrganizationRequestDomain {
                slug: request.slug,
                display_name: request.display_name,
            },
        )
        .await?
        .into(),
    ))
}

/// Organization owners and instance administrators may delete an organization
/// once it owns no repositories.
#[utoipa::path(
    delete, path = "/organizations/{slug}",
    operation_id = "delete_organizations_slug",
    summary = "Delete an empty organization",
    tag = "Organizations",
    params(
        ("slug" = String, Path),
    ),
    responses(
        (status = 204, description = "Success"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn delete_organization(
    State(state): State<IdentityState>,
    Path(slug): Path<String>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<StatusCode, ApiError> {
    let actor = state
        .authenticate_principal(&headers, &jar, SCOPE_WRITE)
        .await?;
    domain::delete_organization(&state.account_service(), &actor, slug).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize, ToSchema)]
pub struct MemberResponse {
    username: String,
    role: String,
    created_at: chrono::DateTime<Utc>,
}

#[utoipa::path(
    get, path = "/organizations/{slug}/members",
    operation_id = "get_organizations_slug_members",
    summary = "List an organization's members",
    tag = "Organizations",
    params(
        ("slug" = String, Path),
    ),
    responses(
        (status = 200, description = "Success", body = Vec<MemberResponse>, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn list_members(
    State(state): State<IdentityState>,
    Path(slug): Path<String>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<Vec<MemberResponse>>, ApiError> {
    let actor = state
        .authenticate_principal(&headers, &jar, SCOPE_READ)
        .await?;
    Ok(Json(
        domain::list_members(&state.account_service(), &actor, slug)
            .await?
            .into_iter()
            .map(Into::into)
            .collect(),
    ))
}

#[derive(Deserialize, utoipa::ToSchema, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct MemberSuggestionsQuery {
    #[serde(default)]
    q: String,
}

#[derive(Serialize, ToSchema)]
pub struct MemberSuggestionResponse {
    id: Uuid,
    username: String,
    avatar_updated_at: Option<chrono::DateTime<Utc>>,
}

#[utoipa::path(
    get, path = "/organizations/{slug}/member-suggestions",
    operation_id = "get_organizations_slug_member_suggestions",
    summary = "Suggest users to add as members",
    tag = "Organizations",
    params(
        ("slug" = String, Path),
        MemberSuggestionsQuery,
    ),
    responses(
        (status = 200, description = "Success", body = Vec<MemberSuggestionResponse>, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn list_member_suggestions(
    State(state): State<IdentityState>,
    Path(slug): Path<String>,
    headers: HeaderMap,
    jar: CookieJar,
    Query(query): Query<MemberSuggestionsQuery>,
) -> Result<Json<Vec<MemberSuggestionResponse>>, ApiError> {
    let actor = state
        .authenticate_principal(&headers, &jar, SCOPE_READ)
        .await?;
    Ok(Json(
        domain::list_member_suggestions(
            &state.account_service(),
            &actor,
            slug,
            MemberSuggestionsQueryDomain { q: query.q },
        )
        .await?
        .into_iter()
        .map(Into::into)
        .collect(),
    ))
}

#[derive(Deserialize, ToSchema)]
pub struct AddMemberRequest {
    username: String,
    role: String,
}

#[utoipa::path(
    post, path = "/organizations/{slug}/members",
    operation_id = "post_organizations_slug_members",
    summary = "Add a member to an organization",
    tag = "Organizations",

    params(
        ("slug" = String, Path),
    ),
    responses(
        (status = 201, description = "Success", body = crate::identity::resources::MemberResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn add_member(
    State(state): State<IdentityState>,
    Path(slug): Path<String>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<AddMemberRequest>,
) -> Result<impl IntoResponse + crate::openapi::TypedResponse, ApiError> {
    let actor = state
        .authenticate_principal(&headers, &jar, SCOPE_WRITE)
        .await?;
    Ok((
        StatusCode::CREATED,
        Json::<MemberResponse>(
            domain::add_member(
                &state.account_service(),
                &actor,
                slug,
                AddMemberRequestDomain {
                    username: request.username,
                    role: request.role,
                },
            )
            .await?
            .into(),
        ),
    ))
}

#[utoipa::path(
    delete, path = "/organizations/{slug}/members/{username}",
    operation_id = "delete_organizations_slug_members_username",
    summary = "Remove a member from an organization",
    tag = "Organizations",
    params(
        ("slug" = String, Path),
        ("username" = String, Path),
    ),
    responses(
        (status = 204, description = "Success"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn remove_member(
    State(state): State<IdentityState>,
    Path((slug, username)): Path<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<StatusCode, ApiError> {
    let actor = state
        .authenticate_principal(&headers, &jar, SCOPE_WRITE)
        .await?;
    domain::remove_member(&state.account_service(), &actor, slug, username).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize, ToSchema)]
pub struct AuditEventResponse {
    id: i64,
    actor_user_id: Option<Uuid>,
    actor_username: Option<String>,
    action: String,
    target: Option<String>,
    created_at: chrono::DateTime<Utc>,
}

/// Paging for the audit history, newest first, beside the shared `limit`.
///
/// `before` is the stable way to page: pass the `id` of the last event of the
/// previous page. `offset` skips rows instead and shifts as events arrive.
#[derive(Deserialize, utoipa::ToSchema, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct AuditCursor {
    offset: Option<u64>,
    before: Option<i64>,
}

#[utoipa::path(
    get, path = "/audit",
    operation_id = "get_audit",
    summary = "List the instance audit history",
    tag = "Account",
    params(
        Pagination,
    ),
    responses(
        (status = 200, description = "Success", body = Vec<AuditEventResponse>, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn list_audit(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Query(pagination): Query<Pagination>,
    Query(cursor): Query<AuditCursor>,
) -> Result<Json<Vec<AuditEventResponse>>, ApiError> {
    let actor = state
        .authenticate_principal(&headers, &jar, SCOPE_READ)
        .await?;
    let rows = gitadel_instance::service::list_audit(
        &state.instance_service(),
        &actor,
        pagination.limit,
        cursor.offset,
        cursor.before,
    )
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(|(row, actor)| AuditEventResponse {
                id: row.id,
                actor_user_id: row.actor_user_id,
                actor_username: actor.map(|actor| actor.username),
                action: row.action,
                target: row.target,
                created_at: row.created_at,
            })
            .collect(),
    ))
}

#[cfg(test)]
use gitadel_instance::service::audit_page;

use gitadel_identity::resources as domain;
impl From<domain::SshKeyResponse> for SshKeyResponse {
    fn from(value: domain::SshKeyResponse) -> Self {
        Self {
            id: value.id,
            name: value.name,
            fingerprint: value.fingerprint,
            public_key: value.public_key,
            created_at: value.created_at,
            last_used_at: value.last_used_at,
        }
    }
}
use domain::CreateSshKeyRequest as CreateSshKeyRequestDomain;

impl From<domain::TokenResponse> for TokenResponse {
    fn from(value: domain::TokenResponse) -> Self {
        Self {
            id: value.id,
            name: value.name,
            scopes: value.scopes,
            expires_at: value.expires_at,
            created_at: value.created_at,
            last_used_at: value.last_used_at,
        }
    }
}
use domain::CreateTokenRequest as CreateTokenRequestDomain;

impl From<domain::CreatedTokenResponse> for CreatedTokenResponse {
    fn from(value: domain::CreatedTokenResponse) -> Self {
        Self {
            token: value.token,
            details: value.details.into(),
        }
    }
}
impl From<domain::NamespaceResponse> for NamespaceResponse {
    fn from(value: domain::NamespaceResponse) -> Self {
        Self {
            slug: value.slug,
            kind: value.kind,
        }
    }
}
impl From<domain::OrganizationResponse> for OrganizationResponse {
    fn from(value: domain::OrganizationResponse) -> Self {
        Self {
            id: value.id,
            slug: value.slug,
            display_name: value.display_name,
            avatar_updated_at: value.avatar_updated_at,
            role: value.role,
        }
    }
}
use domain::CreateOrganizationRequest as CreateOrganizationRequestDomain;

use domain::UpdateOrganizationRequest as UpdateOrganizationRequestDomain;

impl From<domain::MemberResponse> for MemberResponse {
    fn from(value: domain::MemberResponse) -> Self {
        Self {
            username: value.username,
            role: value.role,
            created_at: value.created_at,
        }
    }
}
use domain::MemberSuggestionsQuery as MemberSuggestionsQueryDomain;

impl From<domain::MemberSuggestionResponse> for MemberSuggestionResponse {
    fn from(value: domain::MemberSuggestionResponse) -> Self {
        Self {
            id: value.id,
            username: value.username,
            avatar_updated_at: value.avatar_updated_at,
        }
    }
}
use domain::AddMemberRequest as AddMemberRequestDomain;

#[cfg(test)]
use domain::{member_suggestions, update_organization_profile};

#[cfg(test)]
use crate::entity::audit_event;
#[cfg(test)]
use crate::entity::namespace;
#[cfg(test)]
use crate::entity::organization_member;
#[cfg(test)]
use crate::entity::user;
#[cfg(test)]
use sea_orm::ActiveModelTrait;
#[cfg(test)]
use sea_orm::EntityTrait;
#[cfg(test)]
use sea_orm::Set;
#[cfg(test)]
use sea_orm::TransactionTrait;

#[cfg(test)]
mod tests {
    use sea_orm::{ConnectOptions, Database, NotSet};
    use sea_orm_migration::MigratorTrait;

    use super::*;
    use crate::{
        entity::{action_runner, namespace_integration, namespace_mirror_identity, repository},
        migration::Migrator,
    };

    #[tokio::test]
    async fn organization_slug_update_moves_namespace_resources() {
        let mut options = ConnectOptions::new("sqlite::memory:");
        options.max_connections(1).sqlx_logging(false);
        let database = Database::connect(options).await.unwrap();
        Migrator::up(&database, None).await.unwrap();

        let now = Utc::now();
        let user_id = Uuid::new_v4();
        user::ActiveModel {
            id: Set(user_id),
            username: Set("owner".to_owned()),
            password_hash: Set("hash".to_owned()),
            is_admin: Set(false),
            default_repository_visibility: Set("private".to_owned()),
            theme_preference: Set("system".to_owned()),
            motion_preference: Set("system".to_owned()),
            disabled_at: Set(None),
            avatar_updated_at: Set(None),
            created_at: Set(now),
            updated_at: Set(now),
        }
        .insert(&database)
        .await
        .unwrap();
        let organization = organization::ActiveModel {
            id: Set(Uuid::new_v4()),
            slug: Set("old-team".to_owned()),
            display_name: Set("Old team".to_owned()),
            avatar_updated_at: Set(None),
            created_at: Set(now),
            updated_at: Set(now),
        }
        .insert(&database)
        .await
        .unwrap();
        namespace::ActiveModel {
            slug: Set("old-team".to_owned()),
            kind: Set("organization".to_owned()),
            user_id: Set(None),
            organization_id: Set(Some(organization.id)),
            created_at: Set(now),
        }
        .insert(&database)
        .await
        .unwrap();
        repository::ActiveModel {
            id: Set(Uuid::new_v4()),
            namespace: Set("old-team".to_owned()),
            name: Set("project".to_owned()),
            description: Set(None),
            website_url: Set(None),
            visibility: Set("private".to_owned()),
            object_format: Set("sha1".to_owned()),
            mirrored: Set(false),
            default_branch: Set(Some("main".to_owned())),
            issue_counter: Set(0),
            storage_key: Set(Uuid::new_v4()),
            created_by: Set(user_id),
            archived_at: Set(None),
            deleted_at: Set(None),
            icon_updated_at: Set(None),
            icon_source: Set(None),
            created_at: Set(now),
            updated_at: Set(now),
        }
        .insert(&database)
        .await
        .unwrap();
        namespace_integration::ActiveModel {
            id: Set(Uuid::new_v4()),
            namespace: Set("old-team".to_owned()),
            provider: Set("github".to_owned()),
            name: Set("GitHub".to_owned()),
            enabled: Set(true),
            url: Set("https://api.github.com".to_owned()),
            api_key: Set("secret".to_owned()),
            dokploy_internal_url: Set(None),
            created_at: Set(now),
            updated_at: Set(now),
        }
        .insert(&database)
        .await
        .unwrap();
        namespace_mirror_identity::ActiveModel {
            id: Set(Uuid::new_v4()),
            namespace: Set("old-team".to_owned()),
            name: Set("Mirror key".to_owned()),
            kind: Set("token".to_owned()),
            username: Set(None),
            secret: Set("secret".to_owned()),
            provider: Set(Some("github".to_owned())),
            instance_url: Set(Some("https://api.github.com".to_owned())),
            public_key: Set(None),
            fingerprint: Set(None),
            last_used_at: Set(None),
            created_at: Set(now),
            updated_at: Set(now),
        }
        .insert(&database)
        .await
        .unwrap();
        action_runner::ActiveModel {
            id: NotSet,
            uuid: Set(Uuid::new_v4().to_string()),
            namespace: Set(Some("old-team".to_owned())),
            name: Set("runner".to_owned()),
            token_hash: Set("runner-token".to_owned()),
            approved_labels: Set("[]".to_owned()),
            version: Set("test".to_owned()),
            ephemeral: Set(false),
            disabled_at: Set(None),
            deleted_at: Set(None),
            last_seen_at: Set(None),
            created_by: Set(Some(user_id)),
            created_at: Set(now),
        }
        .insert(&database)
        .await
        .unwrap();

        let transaction = database.begin().await.unwrap();
        let updated = update_organization_profile(
            &transaction,
            organization,
            "old-team",
            "new-team".to_owned(),
            "New team".to_owned(),
        )
        .await
        .unwrap();
        transaction.commit().await.unwrap();

        let repository_namespace = repository::Entity::find()
            .one(&database)
            .await
            .unwrap()
            .unwrap()
            .namespace;
        let integration_namespace = namespace_integration::Entity::find()
            .one(&database)
            .await
            .unwrap()
            .unwrap()
            .namespace;
        let identity_namespace = namespace_mirror_identity::Entity::find()
            .one(&database)
            .await
            .unwrap()
            .unwrap()
            .namespace;
        let runner_namespace = action_runner::Entity::find()
            .one(&database)
            .await
            .unwrap()
            .unwrap()
            .namespace;
        let old_namespace = namespace::Entity::find_by_id("old-team")
            .one(&database)
            .await
            .unwrap();
        let new_namespace = namespace::Entity::find_by_id("new-team")
            .one(&database)
            .await
            .unwrap();

        assert_eq!(
            (
                updated.slug.as_str(),
                updated.display_name.as_str(),
                repository_namespace.as_str(),
                integration_namespace.as_str(),
                identity_namespace.as_str(),
                runner_namespace.as_deref(),
                old_namespace.is_none(),
                new_namespace.is_some(),
            ),
            (
                "new-team",
                "New team",
                "new-team",
                "new-team",
                "new-team",
                Some("new-team"),
                true,
                true,
            )
        );
    }

    #[tokio::test]
    async fn member_suggestions_exclude_existing_and_disabled_users() {
        let mut options = ConnectOptions::new("sqlite::memory:");
        options.max_connections(1).sqlx_logging(false);
        let database = Database::connect(options).await.unwrap();
        Migrator::up(&database, None).await.unwrap();

        let now = Utc::now();
        let existing_id = Uuid::new_v4();
        let organization_id = Uuid::new_v4();
        for (id, username, disabled_at) in [
            (existing_id, "alice", None),
            (Uuid::new_v4(), "alicia", None),
            (Uuid::new_v4(), "alina", Some(now)),
            (Uuid::new_v4(), "bob", None),
        ] {
            user::ActiveModel {
                id: Set(id),
                username: Set(username.to_owned()),
                password_hash: Set("hash".to_owned()),
                is_admin: Set(false),
                default_repository_visibility: Set("private".to_owned()),
                theme_preference: Set("system".to_owned()),
                motion_preference: Set("system".to_owned()),
                disabled_at: Set(disabled_at),
                avatar_updated_at: Set(None),
                created_at: Set(now),
                updated_at: Set(now),
            }
            .insert(&database)
            .await
            .unwrap();
        }
        organization::ActiveModel {
            id: Set(organization_id),
            slug: Set("team".to_owned()),
            display_name: Set("Team".to_owned()),
            avatar_updated_at: Set(None),
            created_at: Set(now),
            updated_at: Set(now),
        }
        .insert(&database)
        .await
        .unwrap();
        organization_member::ActiveModel {
            organization_id: Set(organization_id),
            user_id: Set(existing_id),
            role: Set("member".to_owned()),
            created_at: Set(now),
        }
        .insert(&database)
        .await
        .unwrap();

        let suggestions = member_suggestions(&database, organization_id, "ali")
            .await
            .unwrap();

        assert_eq!(
            suggestions
                .into_iter()
                .map(|suggestion| suggestion.username)
                .collect::<Vec<_>>(),
            vec!["alicia"]
        );
    }

    #[tokio::test]
    async fn audit_history_pages_past_the_first_page() {
        let database = crate::actions::test_support::database().await;
        let now = Utc::now();
        for index in 0..7 {
            audit_event::ActiveModel {
                id: NotSet,
                actor_user_id: Set(None),
                action: Set(format!("event.{index}")),
                target: Set(None),
                remote_address: Set(None),
                created_at: Set(now),
            }
            .insert(&database)
            .await
            .unwrap();
        }
        let ids = |rows: Vec<(audit_event::Model, Option<user::Model>)>| {
            rows.into_iter().map(|(row, _)| row.id).collect::<Vec<_>>()
        };

        let first = ids(audit_page(&database, 3, 0, None).await.unwrap());
        let by_offset = ids(audit_page(&database, 3, 3, None).await.unwrap());
        let by_cursor = ids(audit_page(&database, 3, 0, first.last().copied())
            .await
            .unwrap());
        let last = ids(audit_page(&database, 3, 6, None).await.unwrap());
        let past_the_end = ids(audit_page(&database, 3, 7, None).await.unwrap());

        assert_eq!(first, [7, 6, 5]);
        assert_eq!(by_offset, [4, 3, 2]);
        assert_eq!(by_cursor, by_offset);
        assert_eq!(last, [1]);
        assert!(past_the_end.is_empty());
    }
}
