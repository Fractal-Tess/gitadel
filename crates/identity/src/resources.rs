//! Account keys, tokens and organization workflows.
use crate::{
    AccountService, IdentityError, Principal, SCOPE_READ, SCOPE_SSH_KEYS, SCOPE_WRITE, hash_secret,
    random_secret, validate_name, validate_slug,
};
use chrono::Utc;
use gitadel_db::entity::{
    api_token, namespace, organization, organization_member, repository_deploy_key,
    ssh_key as ssh_key_entity, user,
};
use russh::keys::ssh_key::{HashAlg, PublicKey};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, DatabaseTransaction, EntityTrait,
    PaginatorTrait, QueryFilter, QueryOrder, QuerySelect, Set, TransactionTrait, sea_query::Expr,
};
use uuid::Uuid;
pub struct SshKeyResponse {
    pub id: Uuid,
    pub name: String,
    pub fingerprint: String,
    pub public_key: String,
    pub created_at: chrono::DateTime<Utc>,
    pub last_used_at: Option<chrono::DateTime<Utc>>,
}

pub struct CreateSshKeyRequest {
    pub name: String,
    pub public_key: String,
}

pub struct TokenResponse {
    pub id: Uuid,
    pub name: String,
    pub scopes: Vec<&'static str>,
    pub expires_at: Option<chrono::DateTime<Utc>>,
    pub created_at: chrono::DateTime<Utc>,
    pub last_used_at: Option<chrono::DateTime<Utc>>,
}

pub struct CreateTokenRequest {
    pub name: String,
    pub scopes: Vec<String>,
    pub expires_in_days: Option<i64>,
}

pub struct CreatedTokenResponse {
    pub token: String,
    pub details: TokenResponse,
}

pub struct NamespaceResponse {
    pub slug: String,
    pub kind: String,
}

pub struct OrganizationResponse {
    pub id: Uuid,
    pub slug: String,
    pub display_name: String,
    pub avatar_updated_at: Option<chrono::DateTime<Utc>>,
    pub role: String,
}

pub struct CreateOrganizationRequest {
    pub slug: String,
    pub display_name: String,
}

pub struct UpdateOrganizationRequest {
    pub slug: String,
    pub display_name: String,
}

pub struct MemberResponse {
    pub username: String,
    pub role: String,
    pub created_at: chrono::DateTime<Utc>,
}

pub struct MemberSuggestionsQuery {
    pub q: String,
}

pub struct MemberSuggestionResponse {
    pub id: Uuid,
    pub username: String,
    pub avatar_updated_at: Option<chrono::DateTime<Utc>>,
}

pub struct AddMemberRequest {
    pub username: String,
    pub role: String,
}

pub async fn list_ssh_keys(
    state: &AccountService<'_>,
    actor: &Principal,
) -> Result<Vec<SshKeyResponse>, IdentityError> {
    let actor = state.require_actor(actor, false, crate::SCOPE_READ).await?;
    let rows = ssh_key_entity::Entity::find()
        .filter(ssh_key_entity::Column::UserId.eq(actor.user.id))
        .order_by_asc(ssh_key_entity::Column::CreatedAt)
        .all(state.database())
        .await?;
    Ok(rows.into_iter().map(ssh_key_response).collect())
}

pub async fn create_ssh_key(
    state: &AccountService<'_>,
    actor: &Principal,
    request: CreateSshKeyRequest,
) -> Result<SshKeyResponse, IdentityError> {
    let actor = state
        .require_actor(actor, false, crate::SCOPE_SSH_KEYS)
        .await?;
    let name = validate_name(&request.name, "SSH key name")?;
    let key = PublicKey::from_openssh(request.public_key.trim()).map_err(|_| {
        IdentityError::bad_request("The SSH public key is not valid OpenSSH format.")
    })?;
    let fingerprint = key.fingerprint(HashAlg::Sha256).to_string();
    if ssh_key_entity::Entity::find()
        .filter(ssh_key_entity::Column::Fingerprint.eq(&fingerprint))
        .one(state.database())
        .await?
        .is_some()
    {
        return Err(IdentityError::conflict(
            "That SSH key is already registered.",
        ));
    }
    // One fingerprint maps to one SSH identity, so a deploy key cannot also
    // be a user key.
    if repository_deploy_key::Entity::find()
        .filter(repository_deploy_key::Column::Fingerprint.eq(&fingerprint))
        .one(state.database())
        .await?
        .is_some()
    {
        return Err(IdentityError::conflict(
            "That SSH key is already registered as a repository deploy key.",
        ));
    }
    let transaction = state.database().begin().await?;
    let row = ssh_key_entity::ActiveModel {
        id: Set(Uuid::new_v4()),
        user_id: Set(actor.user.id),
        name: Set(name),
        fingerprint: Set(fingerprint),
        public_key: Set(key.to_openssh().map_err(IdentityError::internal)?),
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
    Ok(ssh_key_response(row))
}

pub async fn delete_ssh_key(
    state: &AccountService<'_>,
    actor: &Principal,
    id: Uuid,
) -> Result<(), IdentityError> {
    let actor = state
        .require_actor(actor, false, crate::SCOPE_SSH_KEYS)
        .await?;
    let transaction = state.database().begin().await?;
    let result = ssh_key_entity::Entity::delete_many()
        .filter(ssh_key_entity::Column::Id.eq(id))
        .filter(ssh_key_entity::Column::UserId.eq(actor.user.id))
        .exec(&transaction)
        .await?;
    if result.rows_affected == 0 {
        return Err(IdentityError::not_found());
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
    Ok(())
}

pub async fn list_tokens(
    state: &AccountService<'_>,
    actor: &Principal,
) -> Result<Vec<TokenResponse>, IdentityError> {
    let actor = state.require_actor(actor, false, crate::SCOPE_READ).await?;
    if actor.via_api_token {
        return Err(IdentityError::forbidden(
            "Manage API tokens from a browser session.",
        ));
    }
    let rows = api_token::Entity::find()
        .filter(api_token::Column::UserId.eq(actor.user.id))
        .filter(api_token::Column::RevokedAt.is_null())
        .order_by_asc(api_token::Column::CreatedAt)
        .all(state.database())
        .await?;
    Ok(rows.into_iter().map(token_response).collect())
}

pub async fn create_token(
    state: &AccountService<'_>,
    actor: &Principal,
    request: CreateTokenRequest,
) -> Result<CreatedTokenResponse, IdentityError> {
    let actor = state
        .require_actor(actor, false, crate::SCOPE_WRITE)
        .await?;
    if actor.via_api_token {
        return Err(IdentityError::forbidden(
            "Create API tokens from a browser session.",
        ));
    }
    let name = validate_name(&request.name, "Token name")?;
    let scopes = parse_scopes(&request.scopes)?;
    let expires_at = match request.expires_in_days {
        None => None,
        Some(days) if (1..=3650).contains(&days) => Some(Utc::now() + chrono::Duration::days(days)),
        Some(_) => {
            return Err(IdentityError::bad_request(
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
    Ok(CreatedTokenResponse {
        token: raw_token,
        details: token_response(row),
    })
}

pub async fn revoke_token(
    state: &AccountService<'_>,
    actor: &Principal,
    id: Uuid,
) -> Result<(), IdentityError> {
    let actor = state
        .require_actor(actor, false, crate::SCOPE_WRITE)
        .await?;
    if actor.via_api_token {
        return Err(IdentityError::forbidden(
            "Revoke API tokens from a browser session.",
        ));
    }
    let stored = api_token::Entity::find_by_id(id)
        .filter(api_token::Column::UserId.eq(actor.user.id))
        .filter(api_token::Column::RevokedAt.is_null())
        .one(state.database())
        .await?
        .ok_or_else(IdentityError::not_found)?;
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
    Ok(())
}

pub async fn get_namespace(
    state: &AccountService<'_>,
    slug: String,
) -> Result<NamespaceResponse, IdentityError> {
    let namespace = namespace::Entity::find_by_id(slug)
        .one(state.database())
        .await?
        .ok_or_else(IdentityError::not_found)?;
    Ok(NamespaceResponse {
        slug: namespace.slug,
        kind: namespace.kind,
    })
}

pub async fn list_organizations(
    state: &AccountService<'_>,
    actor: &Principal,
) -> Result<Vec<OrganizationResponse>, IdentityError> {
    let actor = state.require_actor(actor, false, crate::SCOPE_READ).await?;
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
    Ok(response)
}

pub async fn create_organization(
    state: &AccountService<'_>,
    actor: &Principal,
    request: CreateOrganizationRequest,
) -> Result<OrganizationResponse, IdentityError> {
    let actor = state
        .require_actor(actor, false, crate::SCOPE_WRITE)
        .await?;
    let slug = validate_slug(&request.slug, "Organization name")?;
    let display_name = validate_name(&request.display_name, "Display name")?;
    if namespace::Entity::find_by_id(&slug)
        .one(state.database())
        .await?
        .is_some()
    {
        return Err(IdentityError::conflict(
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
    Ok(OrganizationResponse {
        id: organization.id,
        slug: organization.slug,
        display_name: organization.display_name,
        avatar_updated_at: organization.avatar_updated_at,
        role: "owner".to_owned(),
    })
}

pub async fn update_organization(
    state: &AccountService<'_>,
    actor: &Principal,
    slug: String,
    request: UpdateOrganizationRequest,
) -> Result<OrganizationResponse, IdentityError> {
    let actor = state
        .require_actor(actor, false, crate::SCOPE_WRITE)
        .await?;
    let organization = owned_organization(state, &slug, actor.user.id).await?;
    let next_slug = validate_slug(&request.slug, "Organization slug")?;
    let display_name = validate_name(&request.display_name, "Display name")?;
    if next_slug != slug
        && namespace::Entity::find_by_id(&next_slug)
            .one(state.database())
            .await?
            .is_some()
    {
        return Err(IdentityError::bad_request(
            "That namespace is already in use.",
        ));
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
    Ok(OrganizationResponse {
        id: organization.id,
        slug: organization.slug,
        display_name: organization.display_name,
        avatar_updated_at: organization.avatar_updated_at,
        role: "owner".to_owned(),
    })
}

pub async fn delete_organization(
    state: &AccountService<'_>,
    actor: &Principal,
    slug: String,
) -> Result<(), IdentityError> {
    let actor = state
        .require_actor(actor, false, crate::SCOPE_WRITE)
        .await?;
    let organization = if actor.user.is_admin {
        organization::Entity::find()
            .filter(organization::Column::Slug.eq(&slug))
            .one(state.database())
            .await?
            .ok_or_else(IdentityError::not_found)?
    } else {
        owned_organization(state, &slug, actor.user.id).await?
    };
    let transaction = state.database().begin().await?;
    crate::users::delete_organization_on(&transaction, &organization).await?;
    state
        .audit_on(
            &transaction,
            Some(actor.user.id),
            "organization.delete",
            Some(slug),
        )
        .await?;
    transaction.commit().await?;
    Ok(())
}

pub async fn list_members(
    state: &AccountService<'_>,
    actor: &Principal,
    slug: String,
) -> Result<Vec<MemberResponse>, IdentityError> {
    let actor = state.require_actor(actor, false, crate::SCOPE_READ).await?;
    let organization = accessible_organization(state, &slug, actor.user.id).await?;
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
    Ok(response)
}

pub async fn list_member_suggestions(
    state: &AccountService<'_>,
    actor: &Principal,
    slug: String,
    query: MemberSuggestionsQuery,
) -> Result<Vec<MemberSuggestionResponse>, IdentityError> {
    let actor = state.require_actor(actor, false, crate::SCOPE_READ).await?;
    let organization = owned_organization(state, &slug, actor.user.id).await?;
    let needle = query.q.trim().to_ascii_lowercase();
    if needle.len() > 39
        || !needle
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err(IdentityError::bad_request(
            "The username search is invalid.",
        ));
    }
    Ok(member_suggestions(state.database(), organization.id, &needle).await?)
}

pub async fn add_member(
    state: &AccountService<'_>,
    actor: &Principal,
    slug: String,
    request: AddMemberRequest,
) -> Result<MemberResponse, IdentityError> {
    let actor = state
        .require_actor(actor, false, crate::SCOPE_WRITE)
        .await?;
    let organization = owned_organization(state, &slug, actor.user.id).await?;
    if request.role != "owner" && request.role != "member" {
        return Err(IdentityError::bad_request(
            "Organization role must be owner or member.",
        ));
    }
    let username = validate_slug(&request.username, "Username")?;
    let account = user::Entity::find()
        .filter(user::Column::Username.eq(&username))
        .filter(user::Column::DisabledAt.is_null())
        .one(state.database())
        .await?
        .ok_or_else(IdentityError::not_found)?;
    if organization_member::Entity::find_by_id((organization.id, account.id))
        .one(state.database())
        .await?
        .is_some()
    {
        return Err(IdentityError::conflict(
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
    Ok(MemberResponse {
        username,
        role: membership.role,
        created_at: membership.created_at,
    })
}

pub async fn remove_member(
    state: &AccountService<'_>,
    actor: &Principal,
    slug: String,
    username: String,
) -> Result<(), IdentityError> {
    let actor = state
        .require_actor(actor, false, crate::SCOPE_WRITE)
        .await?;
    let organization = owned_organization(state, &slug, actor.user.id).await?;
    let account = user::Entity::find()
        .filter(user::Column::Username.eq(&username))
        .one(state.database())
        .await?
        .ok_or_else(IdentityError::not_found)?;
    let transaction = state.database().begin().await?;
    let membership = organization_member::Entity::find_by_id((organization.id, account.id))
        .one(&transaction)
        .await?
        .ok_or_else(IdentityError::not_found)?;
    if membership.role == "owner" {
        let owner_count = organization_member::Entity::find()
            .filter(organization_member::Column::OrganizationId.eq(organization.id))
            .filter(organization_member::Column::Role.eq("owner"))
            .count(&transaction)
            .await?;
        if owner_count <= 1 {
            return Err(IdentityError::conflict(
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
    Ok(())
}

pub(crate) fn ssh_key_response(row: ssh_key_entity::Model) -> SshKeyResponse {
    SshKeyResponse {
        id: row.id,
        name: row.name,
        fingerprint: row.fingerprint,
        public_key: row.public_key,
        created_at: row.created_at,
        last_used_at: row.last_used_at,
    }
}

pub(crate) fn token_response(row: api_token::Model) -> TokenResponse {
    TokenResponse {
        id: row.id,
        name: row.name,
        scopes: scope_names(row.scopes),
        expires_at: row.expires_at,
        created_at: row.created_at,
        last_used_at: row.last_used_at,
    }
}

pub fn parse_scopes(requested: &[String]) -> Result<i32, IdentityError> {
    if requested.is_empty() {
        return Err(IdentityError::bad_request(
            "Select at least one token scope.",
        ));
    }
    let mut scopes = 0;
    for scope in requested {
        scopes |= match scope.as_str() {
            "read" => SCOPE_READ,
            "write" => SCOPE_WRITE,
            "ssh_keys" => SCOPE_SSH_KEYS,
            _ => {
                return Err(IdentityError::bad_request(format!(
                    "Unknown token scope: {scope}."
                )));
            }
        };
    }
    Ok(scopes)
}

pub fn scope_names(scopes: i32) -> Vec<&'static str> {
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

pub async fn update_organization_profile(
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

        crate::accounts::move_namespace_resources(transaction, current_slug, &next_slug).await?;
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

pub async fn member_suggestions(
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

pub(crate) async fn accessible_organization(
    state: &AccountService<'_>,
    slug: &str,
    user_id: Uuid,
) -> Result<organization::Model, IdentityError> {
    crate::permissions::accessible_organization(state.database(), slug, user_id).await
}

pub(crate) async fn owned_organization(
    state: &AccountService<'_>,
    slug: &str,
    user_id: Uuid,
) -> Result<organization::Model, IdentityError> {
    crate::permissions::owned_organization(state.database(), slug, user_id).await
}
