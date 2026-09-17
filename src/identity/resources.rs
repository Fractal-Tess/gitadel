use axum::{
    Json,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use axum_extra::extract::cookie::CookieJar;
use chrono::Utc;
use russh::keys::ssh_key::{HashAlg, PublicKey};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, DatabaseTransaction, EntityTrait,
    PaginatorTrait, QueryFilter, QueryOrder, QuerySelect, Set, TransactionTrait, sea_query::Expr,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{
    ApiError, IdentityState, Pagination, SCOPE_READ, SCOPE_SSH_KEYS, SCOPE_WRITE, hash_secret,
    random_secret, validate_name, validate_slug,
};
use crate::entity::{
    action_runner, action_runner_registration_token, api_token, audit_event, namespace,
    namespace_integration, namespace_mirror_identity, organization, organization_member,
    repository, repository_alias, repository_import, repository_import_item,
    ssh_key as ssh_key_entity, user,
};

#[derive(Serialize)]
pub struct SshKeyResponse {
    id: Uuid,
    name: String,
    fingerprint: String,
    public_key: String,
    created_at: chrono::DateTime<Utc>,
    last_used_at: Option<chrono::DateTime<Utc>>,
}

pub async fn list_ssh_keys(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<Vec<SshKeyResponse>>, ApiError> {
    let actor = state.authenticate(&headers, &jar, SCOPE_READ).await?;
    let rows = ssh_key_entity::Entity::find()
        .filter(ssh_key_entity::Column::UserId.eq(actor.user.id))
        .order_by_asc(ssh_key_entity::Column::CreatedAt)
        .all(state.database())
        .await?;
    Ok(Json(rows.into_iter().map(ssh_key_response).collect()))
}

#[derive(Deserialize)]
pub struct CreateSshKeyRequest {
    name: String,
    public_key: String,
}

pub async fn create_ssh_key(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<CreateSshKeyRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let actor = state.authenticate(&headers, &jar, SCOPE_SSH_KEYS).await?;
    let name = validate_name(&request.name, "SSH key name")?;
    let key = PublicKey::from_openssh(request.public_key.trim())
        .map_err(|_| ApiError::bad_request("The SSH public key is not valid OpenSSH format."))?;
    let fingerprint = key.fingerprint(HashAlg::Sha256).to_string();
    if ssh_key_entity::Entity::find()
        .filter(ssh_key_entity::Column::Fingerprint.eq(&fingerprint))
        .one(state.database())
        .await?
        .is_some()
    {
        return Err(ApiError::conflict("That SSH key is already registered."));
    }
    let transaction = state.database().begin().await?;
    let row = ssh_key_entity::ActiveModel {
        id: Set(Uuid::new_v4()),
        user_id: Set(actor.user.id),
        name: Set(name),
        fingerprint: Set(fingerprint),
        public_key: Set(key.to_openssh().map_err(ApiError::internal)?),
        created_at: Set(Utc::now()),
        last_used_at: Set(None),
    }
    .insert(&transaction)
    .await?;
    state
        .audit_on(
            &transaction,
            Some(actor.user.id),
            "ssh_key.create",
            Some(row.id.to_string()),
        )
        .await?;
    transaction.commit().await?;
    Ok((StatusCode::CREATED, Json(ssh_key_response(row))))
}

pub async fn delete_ssh_key(
    State(state): State<IdentityState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<StatusCode, ApiError> {
    let actor = state.authenticate(&headers, &jar, SCOPE_SSH_KEYS).await?;
    let transaction = state.database().begin().await?;
    let result = ssh_key_entity::Entity::delete_many()
        .filter(ssh_key_entity::Column::Id.eq(id))
        .filter(ssh_key_entity::Column::UserId.eq(actor.user.id))
        .exec(&transaction)
        .await?;
    if result.rows_affected == 0 {
        return Err(ApiError::not_found());
    }
    state
        .audit_on(
            &transaction,
            Some(actor.user.id),
            "ssh_key.delete",
            Some(id.to_string()),
        )
        .await?;
    transaction.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

fn ssh_key_response(row: ssh_key_entity::Model) -> SshKeyResponse {
    SshKeyResponse {
        id: row.id,
        name: row.name,
        fingerprint: row.fingerprint,
        public_key: row.public_key,
        created_at: row.created_at,
        last_used_at: row.last_used_at,
    }
}

#[derive(Serialize)]
pub struct TokenResponse {
    id: Uuid,
    name: String,
    scopes: Vec<&'static str>,
    expires_at: Option<chrono::DateTime<Utc>>,
    created_at: chrono::DateTime<Utc>,
    last_used_at: Option<chrono::DateTime<Utc>>,
}

pub async fn list_tokens(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<Vec<TokenResponse>>, ApiError> {
    let actor = state.authenticate(&headers, &jar, SCOPE_READ).await?;
    if actor.via_api_token {
        return Err(ApiError::forbidden(
            "Manage API tokens from a browser session.",
        ));
    }
    let rows = api_token::Entity::find()
        .filter(api_token::Column::UserId.eq(actor.user.id))
        .filter(api_token::Column::RevokedAt.is_null())
        .order_by_asc(api_token::Column::CreatedAt)
        .all(state.database())
        .await?;
    Ok(Json(rows.into_iter().map(token_response).collect()))
}

#[derive(Deserialize)]
pub struct CreateTokenRequest {
    name: String,
    scopes: Vec<String>,
    expires_in_days: Option<i64>,
}

#[derive(Serialize)]
pub struct CreatedTokenResponse {
    token: String,
    details: TokenResponse,
}

pub async fn create_token(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<CreateTokenRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let actor = state.authenticate(&headers, &jar, SCOPE_WRITE).await?;
    if actor.via_api_token {
        return Err(ApiError::forbidden(
            "Create API tokens from a browser session.",
        ));
    }
    let name = validate_name(&request.name, "Token name")?;
    let scopes = parse_scopes(&request.scopes)?;
    let expires_at = match request.expires_in_days {
        None => None,
        Some(days) if (1..=3650).contains(&days) => Some(Utc::now() + chrono::Duration::days(days)),
        Some(_) => {
            return Err(ApiError::bad_request(
                "Token lifetime must be between 1 day and 10 years.",
            ));
        }
    };
    let raw_token = format!("gtd_{}", random_secret(32));
    let transaction = state.database().begin().await?;
    let row = api_token::ActiveModel {
        id: Set(Uuid::new_v4()),
        user_id: Set(actor.user.id),
        name: Set(name),
        token_hash: Set(hash_secret(&raw_token)),
        scopes: Set(scopes),
        expires_at: Set(expires_at),
        created_at: Set(Utc::now()),
        last_used_at: Set(None),
        revoked_at: Set(None),
    }
    .insert(&transaction)
    .await?;
    state
        .audit_on(
            &transaction,
            Some(actor.user.id),
            "api_token.create",
            Some(row.id.to_string()),
        )
        .await?;
    transaction.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(CreatedTokenResponse {
            token: raw_token,
            details: token_response(row),
        }),
    ))
}

pub async fn revoke_token(
    State(state): State<IdentityState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<StatusCode, ApiError> {
    let actor = state.authenticate(&headers, &jar, SCOPE_WRITE).await?;
    if actor.via_api_token {
        return Err(ApiError::forbidden(
            "Revoke API tokens from a browser session.",
        ));
    }
    let stored = api_token::Entity::find_by_id(id)
        .filter(api_token::Column::UserId.eq(actor.user.id))
        .filter(api_token::Column::RevokedAt.is_null())
        .one(state.database())
        .await?
        .ok_or_else(ApiError::not_found)?;
    let transaction = state.database().begin().await?;
    let mut active: api_token::ActiveModel = stored.into();
    active.revoked_at = Set(Some(Utc::now()));
    active.update(&transaction).await?;
    state
        .audit_on(
            &transaction,
            Some(actor.user.id),
            "api_token.revoke",
            Some(id.to_string()),
        )
        .await?;
    transaction.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

fn parse_scopes(requested: &[String]) -> Result<i32, ApiError> {
    if requested.is_empty() {
        return Err(ApiError::bad_request("Select at least one token scope."));
    }
    let mut scopes = 0;
    for scope in requested {
        scopes |= match scope.as_str() {
            "read" => SCOPE_READ,
            "write" => SCOPE_WRITE,
            "ssh_keys" => SCOPE_SSH_KEYS,
            _ => {
                return Err(ApiError::bad_request(format!(
                    "Unknown token scope: {scope}."
                )));
            }
        };
    }
    Ok(scopes)
}

fn scope_names(scopes: i32) -> Vec<&'static str> {
    let mut names = Vec::with_capacity(3);
    if scopes & SCOPE_READ != 0 {
        names.push("read");
    }
    if scopes & SCOPE_WRITE != 0 {
        names.push("write");
    }
    if scopes & SCOPE_SSH_KEYS != 0 {
        names.push("ssh_keys");
    }
    names
}

fn token_response(row: api_token::Model) -> TokenResponse {
    TokenResponse {
        id: row.id,
        name: row.name,
        scopes: scope_names(row.scopes),
        expires_at: row.expires_at,
        created_at: row.created_at,
        last_used_at: row.last_used_at,
    }
}

#[derive(Serialize)]
pub struct NamespaceResponse {
    slug: String,
    kind: String,
}

pub async fn get_namespace(
    State(state): State<IdentityState>,
    Path(slug): Path<String>,
) -> Result<Json<NamespaceResponse>, ApiError> {
    let namespace = namespace::Entity::find_by_id(slug)
        .one(state.database())
        .await?
        .ok_or_else(ApiError::not_found)?;
    Ok(Json(NamespaceResponse {
        slug: namespace.slug,
        kind: namespace.kind,
    }))
}

#[derive(Serialize)]
pub struct OrganizationResponse {
    id: Uuid,
    slug: String,
    display_name: String,
    avatar_updated_at: Option<chrono::DateTime<Utc>>,
    role: String,
}

pub async fn list_organizations(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<Vec<OrganizationResponse>>, ApiError> {
    let actor = state.authenticate(&headers, &jar, SCOPE_READ).await?;
    let memberships = organization_member::Entity::find()
        .filter(organization_member::Column::UserId.eq(actor.user.id))
        .all(state.database())
        .await?;
    let mut response = Vec::with_capacity(memberships.len());
    for membership in memberships {
        if let Some(organization) = organization::Entity::find_by_id(membership.organization_id)
            .one(state.database())
            .await?
        {
            response.push(OrganizationResponse {
                id: organization.id,
                slug: organization.slug,
                display_name: organization.display_name,
                avatar_updated_at: organization.avatar_updated_at,
                role: membership.role,
            });
        }
    }
    response.sort_by(|left, right| left.slug.cmp(&right.slug));
    Ok(Json(response))
}

#[derive(Deserialize)]
pub struct CreateOrganizationRequest {
    slug: String,
    display_name: String,
}

pub async fn create_organization(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<CreateOrganizationRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let actor = state.authenticate(&headers, &jar, SCOPE_WRITE).await?;
    let slug = validate_slug(&request.slug, "Organization name")?;
    let display_name = validate_name(&request.display_name, "Display name")?;
    if namespace::Entity::find_by_id(&slug)
        .one(state.database())
        .await?
        .is_some()
    {
        return Err(ApiError::conflict(
            "That organization name is already in use.",
        ));
    }
    let transaction = state.database().begin().await?;
    let now = Utc::now();
    let organization = organization::ActiveModel {
        id: Set(Uuid::new_v4()),
        slug: Set(slug.clone()),
        display_name: Set(display_name),
        avatar_updated_at: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(&transaction)
    .await?;
    namespace::ActiveModel {
        slug: Set(slug.clone()),
        kind: Set("organization".to_owned()),
        user_id: Set(None),
        organization_id: Set(Some(organization.id)),
        created_at: Set(now),
    }
    .insert(&transaction)
    .await?;
    organization_member::ActiveModel {
        organization_id: Set(organization.id),
        user_id: Set(actor.user.id),
        role: Set("owner".to_owned()),
        created_at: Set(now),
    }
    .insert(&transaction)
    .await?;
    state
        .audit_on(
            &transaction,
            Some(actor.user.id),
            "organization.create",
            Some(slug),
        )
        .await?;
    transaction.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(OrganizationResponse {
            id: organization.id,
            slug: organization.slug,
            display_name: organization.display_name,
            avatar_updated_at: organization.avatar_updated_at,
            role: "owner".to_owned(),
        }),
    ))
}

#[derive(Deserialize)]
pub struct UpdateOrganizationRequest {
    slug: String,
    display_name: String,
}

async fn update_organization_profile(
    transaction: &DatabaseTransaction,
    organization: organization::Model,
    current_slug: &str,
    next_slug: String,
    display_name: String,
) -> Result<organization::Model, sea_orm::DbErr> {
    if next_slug != current_slug {
        let current_namespace = namespace::Entity::find_by_id(current_slug)
            .one(transaction)
            .await?
            .ok_or_else(|| {
                sea_orm::DbErr::RecordNotFound(format!("namespace {current_slug} does not exist"))
            })?;
        namespace::Entity::update_many()
            .col_expr(
                namespace::Column::OrganizationId,
                Expr::value(Option::<Uuid>::None),
            )
            .filter(namespace::Column::Slug.eq(current_slug))
            .exec(transaction)
            .await?;
        namespace::ActiveModel {
            slug: Set(next_slug.clone()),
            kind: Set(current_namespace.kind),
            user_id: Set(current_namespace.user_id),
            organization_id: Set(current_namespace.organization_id),
            created_at: Set(current_namespace.created_at),
        }
        .insert(transaction)
        .await?;

        repository::Entity::update_many()
            .col_expr(repository::Column::Namespace, Expr::value(&next_slug))
            .filter(repository::Column::Namespace.eq(current_slug))
            .exec(transaction)
            .await?;
        namespace_integration::Entity::update_many()
            .col_expr(
                namespace_integration::Column::Namespace,
                Expr::value(&next_slug),
            )
            .filter(namespace_integration::Column::Namespace.eq(current_slug))
            .exec(transaction)
            .await?;
        namespace_mirror_identity::Entity::update_many()
            .col_expr(
                namespace_mirror_identity::Column::Namespace,
                Expr::value(&next_slug),
            )
            .filter(namespace_mirror_identity::Column::Namespace.eq(current_slug))
            .exec(transaction)
            .await?;
        action_runner::Entity::update_many()
            .col_expr(action_runner::Column::Namespace, Expr::value(&next_slug))
            .filter(action_runner::Column::Namespace.eq(current_slug))
            .exec(transaction)
            .await?;
        action_runner_registration_token::Entity::update_many()
            .col_expr(
                action_runner_registration_token::Column::Namespace,
                Expr::value(&next_slug),
            )
            .filter(action_runner_registration_token::Column::Namespace.eq(current_slug))
            .exec(transaction)
            .await?;
        repository_import::Entity::update_many()
            .col_expr(
                repository_import::Column::TargetNamespace,
                Expr::value(&next_slug),
            )
            .filter(repository_import::Column::TargetNamespace.eq(current_slug))
            .exec(transaction)
            .await?;
        repository_import_item::Entity::update_many()
            .col_expr(
                repository_import_item::Column::TargetNamespace,
                Expr::value(&next_slug),
            )
            .filter(repository_import_item::Column::TargetNamespace.eq(current_slug))
            .exec(transaction)
            .await?;
        repository_alias::Entity::update_many()
            .col_expr(repository_alias::Column::Namespace, Expr::value(&next_slug))
            .filter(repository_alias::Column::Namespace.eq(current_slug))
            .exec(transaction)
            .await?;
        namespace::Entity::delete_by_id(current_slug)
            .exec(transaction)
            .await?;
    }

    let mut active: organization::ActiveModel = organization.into();
    active.slug = Set(next_slug);
    active.display_name = Set(display_name);
    active.updated_at = Set(Utc::now());
    active.update(transaction).await
}

pub async fn update_organization(
    State(state): State<IdentityState>,
    Path(slug): Path<String>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<UpdateOrganizationRequest>,
) -> Result<Json<OrganizationResponse>, ApiError> {
    let actor = state.authenticate(&headers, &jar, SCOPE_WRITE).await?;
    let organization = owned_organization(&state, &slug, actor.user.id).await?;
    let next_slug = validate_slug(&request.slug, "Organization slug")?;
    let display_name = validate_name(&request.display_name, "Display name")?;
    if next_slug != slug
        && namespace::Entity::find_by_id(&next_slug)
            .one(state.database())
            .await?
            .is_some()
    {
        return Err(ApiError::bad_request("That namespace is already in use."));
    }
    let transaction = state.database().begin().await?;
    let organization =
        update_organization_profile(&transaction, organization, &slug, next_slug, display_name)
            .await?;
    state
        .audit_on(
            &transaction,
            Some(actor.user.id),
            "organization.update",
            Some(format!("{slug} -> {}", organization.slug)),
        )
        .await?;
    transaction.commit().await?;
    Ok(Json(OrganizationResponse {
        id: organization.id,
        slug: organization.slug,
        display_name: organization.display_name,
        avatar_updated_at: organization.avatar_updated_at,
        role: "owner".to_owned(),
    }))
}

#[derive(Serialize)]
pub struct MemberResponse {
    username: String,
    role: String,
    created_at: chrono::DateTime<Utc>,
}

pub async fn list_members(
    State(state): State<IdentityState>,
    Path(slug): Path<String>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<Vec<MemberResponse>>, ApiError> {
    let actor = state.authenticate(&headers, &jar, SCOPE_READ).await?;
    let organization = accessible_organization(&state, &slug, actor.user.id).await?;
    let memberships = organization_member::Entity::find()
        .filter(organization_member::Column::OrganizationId.eq(organization.id))
        .order_by_asc(organization_member::Column::CreatedAt)
        .all(state.database())
        .await?;
    let mut response = Vec::with_capacity(memberships.len());
    for membership in memberships {
        if let Some(account) = user::Entity::find_by_id(membership.user_id)
            .one(state.database())
            .await?
        {
            response.push(MemberResponse {
                username: account.username,
                role: membership.role,
                created_at: membership.created_at,
            });
        }
    }
    Ok(Json(response))
}

#[derive(Deserialize)]
pub struct MemberSuggestionsQuery {
    #[serde(default)]
    q: String,
}

#[derive(Serialize)]
pub struct MemberSuggestionResponse {
    id: Uuid,
    username: String,
    avatar_updated_at: Option<chrono::DateTime<Utc>>,
}

pub async fn list_member_suggestions(
    State(state): State<IdentityState>,
    Path(slug): Path<String>,
    headers: HeaderMap,
    jar: CookieJar,
    Query(query): Query<MemberSuggestionsQuery>,
) -> Result<Json<Vec<MemberSuggestionResponse>>, ApiError> {
    let actor = state.authenticate(&headers, &jar, SCOPE_READ).await?;
    let organization = owned_organization(&state, &slug, actor.user.id).await?;
    let needle = query.q.trim().to_ascii_lowercase();
    if needle.len() > 39
        || !needle
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err(ApiError::bad_request("The username search is invalid."));
    }
    Ok(Json(
        member_suggestions(state.database(), organization.id, &needle).await?,
    ))
}

async fn member_suggestions(
    database: &DatabaseConnection,
    organization_id: Uuid,
    needle: &str,
) -> Result<Vec<MemberSuggestionResponse>, sea_orm::DbErr> {
    let member_ids = organization_member::Entity::find()
        .select_only()
        .column(organization_member::Column::UserId)
        .filter(organization_member::Column::OrganizationId.eq(organization_id))
        .into_tuple::<Uuid>()
        .all(database)
        .await?;
    let mut users = user::Entity::find()
        .filter(user::Column::DisabledAt.is_null())
        .order_by_asc(user::Column::Username)
        .limit(8);
    if !needle.is_empty() {
        users = users.filter(user::Column::Username.contains(needle));
    }
    if !member_ids.is_empty() {
        users = users.filter(user::Column::Id.is_not_in(member_ids));
    }
    Ok(users
        .all(database)
        .await?
        .into_iter()
        .map(|account| MemberSuggestionResponse {
            id: account.id,
            username: account.username,
            avatar_updated_at: account.avatar_updated_at,
        })
        .collect())
}

#[derive(Deserialize)]
pub struct AddMemberRequest {
    username: String,
    role: String,
}

pub async fn add_member(
    State(state): State<IdentityState>,
    Path(slug): Path<String>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<AddMemberRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let actor = state.authenticate(&headers, &jar, SCOPE_WRITE).await?;
    let organization = owned_organization(&state, &slug, actor.user.id).await?;
    if request.role != "owner" && request.role != "member" {
        return Err(ApiError::bad_request(
            "Organization role must be owner or member.",
        ));
    }
    let username = validate_slug(&request.username, "Username")?;
    let account = user::Entity::find()
        .filter(user::Column::Username.eq(&username))
        .filter(user::Column::DisabledAt.is_null())
        .one(state.database())
        .await?
        .ok_or_else(ApiError::not_found)?;
    if organization_member::Entity::find_by_id((organization.id, account.id))
        .one(state.database())
        .await?
        .is_some()
    {
        return Err(ApiError::conflict(
            "That user is already an organization member.",
        ));
    }
    let transaction = state.database().begin().await?;
    let membership = organization_member::ActiveModel {
        organization_id: Set(organization.id),
        user_id: Set(account.id),
        role: Set(request.role),
        created_at: Set(Utc::now()),
    }
    .insert(&transaction)
    .await?;
    state
        .audit_on(
            &transaction,
            Some(actor.user.id),
            "organization.member.add",
            Some(format!("{}/{}", organization.slug, username)),
        )
        .await?;
    transaction.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(MemberResponse {
            username,
            role: membership.role,
            created_at: membership.created_at,
        }),
    ))
}

pub async fn remove_member(
    State(state): State<IdentityState>,
    Path((slug, username)): Path<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<StatusCode, ApiError> {
    let actor = state.authenticate(&headers, &jar, SCOPE_WRITE).await?;
    let organization = owned_organization(&state, &slug, actor.user.id).await?;
    let account = user::Entity::find()
        .filter(user::Column::Username.eq(&username))
        .one(state.database())
        .await?
        .ok_or_else(ApiError::not_found)?;
    let transaction = state.database().begin().await?;
    let membership = organization_member::Entity::find_by_id((organization.id, account.id))
        .one(&transaction)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if membership.role == "owner" {
        let owner_count = organization_member::Entity::find()
            .filter(organization_member::Column::OrganizationId.eq(organization.id))
            .filter(organization_member::Column::Role.eq("owner"))
            .count(&transaction)
            .await?;
        if owner_count <= 1 {
            return Err(ApiError::conflict(
                "An organization must keep at least one owner.",
            ));
        }
    }
    organization_member::Entity::delete_by_id((organization.id, account.id))
        .exec(&transaction)
        .await?;
    state
        .audit_on(
            &transaction,
            Some(actor.user.id),
            "organization.member.remove",
            Some(format!("{}/{}", organization.slug, username)),
        )
        .await?;
    transaction.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn accessible_organization(
    state: &IdentityState,
    slug: &str,
    user_id: Uuid,
) -> Result<organization::Model, ApiError> {
    let organization = organization::Entity::find()
        .filter(organization::Column::Slug.eq(slug))
        .one(state.database())
        .await?
        .ok_or_else(ApiError::not_found)?;
    organization_member::Entity::find_by_id((organization.id, user_id))
        .one(state.database())
        .await?
        .ok_or_else(ApiError::not_found)?;
    Ok(organization)
}

pub(super) async fn owned_organization(
    state: &IdentityState,
    slug: &str,
    user_id: Uuid,
) -> Result<organization::Model, ApiError> {
    let organization = accessible_organization(state, slug, user_id).await?;
    let membership = organization_member::Entity::find_by_id((organization.id, user_id))
        .one(state.database())
        .await?
        .ok_or_else(ApiError::not_found)?;
    if membership.role != "owner" {
        return Err(ApiError::forbidden(
            "Only an organization owner can manage this organization.",
        ));
    }
    Ok(organization)
}

#[derive(Serialize)]
pub struct AuditEventResponse {
    id: i64,
    actor_user_id: Option<Uuid>,
    actor_username: Option<String>,
    action: String,
    target: Option<String>,
    created_at: chrono::DateTime<Utc>,
}

pub async fn list_audit(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Query(pagination): Query<Pagination>,
) -> Result<Json<Vec<AuditEventResponse>>, ApiError> {
    let actor = state.authenticate(&headers, &jar, SCOPE_READ).await?;
    if !actor.user.is_admin {
        return Err(ApiError::forbidden(
            "Only an administrator can read the audit history.",
        ));
    }
    let limit = pagination.limit.unwrap_or(100).clamp(1, 500);
    let rows = audit_event::Entity::find()
        .find_also_related(user::Entity)
        .order_by_desc(audit_event::Column::Id)
        .limit(limit)
        .all(state.database())
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
mod tests {
    use sea_orm::{ConnectOptions, Database, NotSet};
    use sea_orm_migration::MigratorTrait;

    use super::*;
    use crate::migration::Migrator;

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
}
