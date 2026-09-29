//! Administrator management of accounts and pending invitations.
//!
//! Disabling is the reversible way to remove someone's access and keeps every
//! reference to the account intact. Deleting is permanent and is refused while
//! the account still anchors data that other people rely on: repositories in
//! its namespace, organizations it alone owns, or issues, comments, releases,
//! and attachments it authored. Repositories it created in other namespaces are
//! reattributed to the deleting administrator.

use std::collections::HashSet;

use axum::{
    Json,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
};
use axum_extra::extract::cookie::CookieJar;
use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, Condition, ConnectionTrait, EntityTrait, PaginatorTrait,
    QueryFilter, QueryOrder, QuerySelect, Set, TransactionTrait, sea_query::Expr,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{ApiError, IdentityState, SCOPE_READ, SCOPE_WRITE, admin::require_admin};
use crate::entity::{
    api_token, invitation, issue_attachment, issue_comment, namespace, oauth_access_token,
    oauth_authorization_code, organization, organization_member, repository, repository_alias,
    repository_issue, repository_release, session, user, user_totp,
};

#[derive(Serialize)]
pub struct AdminUserResponse {
    id: Uuid,
    username: String,
    is_admin: bool,
    disabled_at: Option<chrono::DateTime<Utc>>,
    avatar_updated_at: Option<chrono::DateTime<Utc>>,
    two_factor_enabled: bool,
    created_at: chrono::DateTime<Utc>,
}

impl AdminUserResponse {
    fn new(account: user::Model, two_factor_enabled: bool) -> Self {
        Self {
            id: account.id,
            username: account.username,
            is_admin: account.is_admin,
            disabled_at: account.disabled_at,
            avatar_updated_at: account.avatar_updated_at,
            two_factor_enabled,
            created_at: account.created_at,
        }
    }
}

#[derive(Serialize)]
pub struct AdminUserPage {
    users: Vec<AdminUserResponse>,
    total: u64,
}

#[derive(Deserialize)]
pub struct UserListQuery {
    #[serde(default)]
    q: String,
    limit: Option<u64>,
    offset: Option<u64>,
}

pub async fn list_users(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
    Query(query): Query<UserListQuery>,
) -> Result<Json<AdminUserPage>, ApiError> {
    require_admin(&state, &headers, &jar, SCOPE_READ).await?;
    let needle = query.q.trim().to_ascii_lowercase();
    if needle.len() > 39
        || !needle
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err(ApiError::bad_request("The username search is invalid."));
    }
    Ok(Json(
        user_page(
            state.database(),
            &needle,
            query.limit.unwrap_or(50).clamp(1, 100),
            query.offset.unwrap_or(0),
        )
        .await?,
    ))
}

async fn user_page<C: ConnectionTrait>(
    connection: &C,
    needle: &str,
    limit: u64,
    offset: u64,
) -> Result<AdminUserPage, ApiError> {
    let mut accounts = user::Entity::find();
    if !needle.is_empty() {
        accounts = accounts.filter(user::Column::Username.contains(needle));
    }
    let total = accounts.clone().count(connection).await?;
    let rows = accounts
        .order_by_asc(user::Column::Username)
        .limit(limit)
        .offset(offset)
        .all(connection)
        .await?;
    let ids: Vec<Uuid> = rows.iter().map(|account| account.id).collect();
    let enrolled: HashSet<Uuid> = if ids.is_empty() {
        HashSet::new()
    } else {
        user_totp::Entity::find()
            .select_only()
            .column(user_totp::Column::UserId)
            .filter(user_totp::Column::UserId.is_in(ids))
            .filter(user_totp::Column::ConfirmedAt.is_not_null())
            .into_tuple::<Uuid>()
            .all(connection)
            .await?
            .into_iter()
            .collect()
    };
    Ok(AdminUserPage {
        users: rows
            .into_iter()
            .map(|account| {
                let two_factor = enrolled.contains(&account.id);
                AdminUserResponse::new(account, two_factor)
            })
            .collect(),
        total,
    })
}

async fn find_account<C: ConnectionTrait>(
    connection: &C,
    username: &str,
) -> Result<user::Model, ApiError> {
    user::Entity::find()
        .filter(user::Column::Username.eq(username))
        .one(connection)
        .await?
        .ok_or_else(ApiError::not_found)
}

/// An instance without an enabled administrator could never be managed again,
/// so the last one can be neither disabled nor deleted.
async fn ensure_other_admin<C: ConnectionTrait>(
    connection: &C,
    account: &user::Model,
) -> Result<(), ApiError> {
    if !account.is_admin {
        return Ok(());
    }
    let others = user::Entity::find()
        .filter(user::Column::IsAdmin.eq(true))
        .filter(user::Column::DisabledAt.is_null())
        .filter(user::Column::Id.ne(account.id))
        .count(connection)
        .await?;
    if others == 0 {
        return Err(ApiError::conflict(
            "The instance must keep at least one enabled administrator.",
        ));
    }
    Ok(())
}

async fn two_factor_enabled<C: ConnectionTrait>(
    connection: &C,
    user_id: Uuid,
) -> Result<bool, ApiError> {
    super::two_factor::is_enabled(connection, user_id).await
}

/// Disables an account and revokes everything that could still act for it.
async fn disable_account<C: ConnectionTrait>(
    connection: &C,
    actor_id: Uuid,
    account: user::Model,
) -> Result<user::Model, ApiError> {
    if account.id == actor_id {
        return Err(ApiError::bad_request(
            "You cannot disable your own account.",
        ));
    }
    if account.disabled_at.is_some() {
        return Ok(account);
    }
    ensure_other_admin(connection, &account).await?;
    let now = Utc::now();
    session::Entity::delete_many()
        .filter(session::Column::UserId.eq(account.id))
        .exec(connection)
        .await?;
    api_token::Entity::update_many()
        .col_expr(api_token::Column::RevokedAt, Expr::value(Some(now)))
        .filter(api_token::Column::UserId.eq(account.id))
        .filter(api_token::Column::RevokedAt.is_null())
        .exec(connection)
        .await?;
    oauth_access_token::Entity::update_many()
        .col_expr(
            oauth_access_token::Column::RevokedAt,
            Expr::value(Some(now)),
        )
        .filter(oauth_access_token::Column::UserId.eq(account.id))
        .filter(oauth_access_token::Column::RevokedAt.is_null())
        .exec(connection)
        .await?;
    oauth_authorization_code::Entity::delete_many()
        .filter(oauth_authorization_code::Column::UserId.eq(account.id))
        .exec(connection)
        .await?;
    let mut active: user::ActiveModel = account.into();
    active.disabled_at = Set(Some(now));
    active.updated_at = Set(now);
    Ok(active.update(connection).await?)
}

pub async fn disable_user(
    State(state): State<IdentityState>,
    Path(username): Path<String>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<AdminUserResponse>, ApiError> {
    let actor = require_admin(&state, &headers, &jar, SCOPE_WRITE).await?;
    let transaction = state.database().begin().await?;
    let account = find_account(&transaction, &username).await?;
    let account = disable_account(&transaction, actor.user.id, account).await?;
    let two_factor = two_factor_enabled(&transaction, account.id).await?;
    state
        .audit_on(
            &transaction,
            Some(actor.user.id),
            "admin.user.disable",
            Some(username),
        )
        .await?;
    transaction.commit().await?;
    Ok(Json(AdminUserResponse::new(account, two_factor)))
}

pub async fn enable_user(
    State(state): State<IdentityState>,
    Path(username): Path<String>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<AdminUserResponse>, ApiError> {
    let actor = require_admin(&state, &headers, &jar, SCOPE_WRITE).await?;
    let transaction = state.database().begin().await?;
    let account = find_account(&transaction, &username).await?;
    let account = if account.disabled_at.is_some() {
        let mut active: user::ActiveModel = account.into();
        active.disabled_at = Set(None);
        active.updated_at = Set(Utc::now());
        active.update(&transaction).await?
    } else {
        account
    };
    let two_factor = two_factor_enabled(&transaction, account.id).await?;
    state
        .audit_on(
            &transaction,
            Some(actor.user.id),
            "admin.user.enable",
            Some(username),
        )
        .await?;
    transaction.commit().await?;
    Ok(Json(AdminUserResponse::new(account, two_factor)))
}

/// Removes an account permanently after checking that nothing shared would be
/// orphaned. See the module documentation for the exact rules.
async fn delete_account<C: ConnectionTrait>(
    connection: &C,
    actor_id: Uuid,
    account: &user::Model,
) -> Result<(), ApiError> {
    if account.id == actor_id {
        return Err(ApiError::bad_request("You cannot delete your own account."));
    }
    ensure_other_admin(connection, account).await?;

    let personal_repositories = repository::Entity::find()
        .filter(repository::Column::Namespace.eq(&account.username))
        .count(connection)
        .await?;
    if personal_repositories > 0 {
        return Err(ApiError::conflict(format!(
            "{} still owns {personal_repositories} repositories, including deleted ones awaiting purge. Move or purge them first.",
            account.username
        )));
    }

    let sole_owned = sole_owned_organizations(connection, account.id).await?;
    if !sole_owned.is_empty() {
        return Err(ApiError::conflict(format!(
            "{} is the only owner of {}. Add another owner or delete the organization first.",
            account.username,
            sole_owned.join(", ")
        )));
    }

    let authored = repository_issue::Entity::find()
        .filter(repository_issue::Column::AuthorUserId.eq(account.id))
        .count(connection)
        .await?
        + issue_comment::Entity::find()
            .filter(issue_comment::Column::AuthorUserId.eq(account.id))
            .count(connection)
            .await?
        + repository_release::Entity::find()
            .filter(repository_release::Column::AuthorUserId.eq(account.id))
            .count(connection)
            .await?
        + issue_attachment::Entity::find()
            .filter(issue_attachment::Column::UploaderUserId.eq(account.id))
            .count(connection)
            .await?;
    if authored > 0 {
        return Err(ApiError::conflict(format!(
            "{} authored issues, comments, releases, or attachments. Disable the account instead to keep that history.",
            account.username
        )));
    }

    repository_issue::Entity::update_many()
        .col_expr(
            repository_issue::Column::AssigneeUserId,
            Expr::value(Option::<Uuid>::None),
        )
        .filter(repository_issue::Column::AssigneeUserId.eq(account.id))
        .exec(connection)
        .await?;
    repository::Entity::update_many()
        .col_expr(repository::Column::CreatedBy, Expr::value(actor_id))
        .filter(repository::Column::CreatedBy.eq(account.id))
        .exec(connection)
        .await?;
    invitation::Entity::delete_many()
        .filter(invitation::Column::CreatedBy.eq(account.id))
        .exec(connection)
        .await?;
    repository_alias::Entity::delete_many()
        .filter(repository_alias::Column::Namespace.eq(&account.username))
        .exec(connection)
        .await?;
    // Sessions, keys, tokens, memberships, the personal namespace and its
    // integrations and runners all cascade from the account row.
    user::Entity::delete_by_id(account.id)
        .exec(connection)
        .await?;
    Ok(())
}

async fn sole_owned_organizations<C: ConnectionTrait>(
    connection: &C,
    user_id: Uuid,
) -> Result<Vec<String>, ApiError> {
    let owned = organization_member::Entity::find()
        .filter(organization_member::Column::UserId.eq(user_id))
        .filter(organization_member::Column::Role.eq("owner"))
        .all(connection)
        .await?;
    let mut slugs = Vec::new();
    for membership in owned {
        let owners = organization_member::Entity::find()
            .filter(organization_member::Column::OrganizationId.eq(membership.organization_id))
            .filter(organization_member::Column::Role.eq("owner"))
            .count(connection)
            .await?;
        if owners > 1 {
            continue;
        }
        if let Some(organization) = organization::Entity::find_by_id(membership.organization_id)
            .one(connection)
            .await?
        {
            slugs.push(organization.slug);
        }
    }
    slugs.sort();
    Ok(slugs)
}

pub async fn delete_user(
    State(state): State<IdentityState>,
    Path(username): Path<String>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<StatusCode, ApiError> {
    let actor = require_admin(&state, &headers, &jar, SCOPE_WRITE).await?;
    let transaction = state.database().begin().await?;
    let account = find_account(&transaction, &username).await?;
    delete_account(&transaction, actor.user.id, &account).await?;
    state
        .audit_on(
            &transaction,
            Some(actor.user.id),
            "admin.user.delete",
            Some(username),
        )
        .await?;
    transaction.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Lets an administrator recover an account whose authenticator and recovery
/// codes are both lost. The user can enroll again after signing in.
pub async fn reset_two_factor(
    State(state): State<IdentityState>,
    Path(username): Path<String>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<StatusCode, ApiError> {
    let actor = require_admin(&state, &headers, &jar, SCOPE_WRITE).await?;
    let transaction = state.database().begin().await?;
    let account = find_account(&transaction, &username).await?;
    if !super::two_factor::remove_on(&transaction, account.id).await? {
        return Err(ApiError::bad_request(
            "That user does not use two-factor authentication.",
        ));
    }
    state
        .audit_on(
            &transaction,
            Some(actor.user.id),
            "admin.user.two_factor.reset",
            Some(username),
        )
        .await?;
    transaction.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize)]
pub struct PendingInvitationResponse {
    /// The stored token hash. It identifies the invitation without revealing
    /// the token that redeems it.
    id: String,
    created_by: Option<String>,
    created_at: chrono::DateTime<Utc>,
    expires_at: chrono::DateTime<Utc>,
}

pub async fn list_invitations(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<Vec<PendingInvitationResponse>>, ApiError> {
    require_admin(&state, &headers, &jar, SCOPE_READ).await?;
    Ok(Json(pending_invitations(state.database()).await?))
}

async fn pending_invitations<C: ConnectionTrait>(
    connection: &C,
) -> Result<Vec<PendingInvitationResponse>, ApiError> {
    let rows = invitation::Entity::find()
        .filter(invitation::Column::UsedAt.is_null())
        .filter(invitation::Column::ExpiresAt.gt(Utc::now()))
        .order_by_desc(invitation::Column::CreatedAt)
        .all(connection)
        .await?;
    let creator_ids: Vec<Uuid> = rows
        .iter()
        .map(|row| row.created_by)
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let creators = if creator_ids.is_empty() {
        Vec::new()
    } else {
        user::Entity::find()
            .filter(user::Column::Id.is_in(creator_ids))
            .all(connection)
            .await?
    };
    Ok(rows
        .into_iter()
        .map(|row| PendingInvitationResponse {
            created_by: creators
                .iter()
                .find(|account| account.id == row.created_by)
                .map(|account| account.username.clone()),
            id: row.token_hash,
            created_at: row.created_at,
            expires_at: row.expires_at,
        })
        .collect())
}

pub async fn revoke_invitation(
    State(state): State<IdentityState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<StatusCode, ApiError> {
    let actor = require_admin(&state, &headers, &jar, SCOPE_WRITE).await?;
    let transaction = state.database().begin().await?;
    let deleted = invitation::Entity::delete_many()
        .filter(invitation::Column::TokenHash.eq(&id))
        .filter(invitation::Column::UsedAt.is_null())
        .exec(&transaction)
        .await?;
    if deleted.rows_affected == 0 {
        return Err(ApiError::not_found());
    }
    state
        .audit_on(&transaction, Some(actor.user.id), "invitation.revoke", None)
        .await?;
    transaction.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Deletes an organization that no longer owns repositories. Its namespace,
/// memberships, avatar, integrations, mirror credentials, and runners cascade.
pub(super) async fn delete_organization_on<C: ConnectionTrait>(
    connection: &C,
    organization: &organization::Model,
) -> Result<(), ApiError> {
    let repositories = repository::Entity::find()
        .filter(repository::Column::Namespace.eq(&organization.slug))
        .count(connection)
        .await?;
    if repositories > 0 {
        return Err(ApiError::conflict(format!(
            "{} still owns {repositories} repositories, including deleted ones awaiting purge. Move or purge them first.",
            organization.slug
        )));
    }
    repository_alias::Entity::delete_many()
        .filter(repository_alias::Column::Namespace.eq(&organization.slug))
        .exec(connection)
        .await?;
    namespace::Entity::delete_many()
        .filter(
            Condition::all()
                .add(namespace::Column::Slug.eq(&organization.slug))
                .add(namespace::Column::OrganizationId.eq(organization.id)),
        )
        .exec(connection)
        .await?;
    organization::Entity::delete_by_id(organization.id)
        .exec(connection)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use sea_orm::{ConnectOptions, Database, DatabaseConnection};
    use sea_orm_migration::MigratorTrait;

    use super::*;
    use crate::{
        entity::{namespace_integration, passkey},
        migration::Migrator,
    };

    async fn database() -> DatabaseConnection {
        let mut options = ConnectOptions::new("sqlite::memory:");
        options.max_connections(1).sqlx_logging(false);
        let database = Database::connect(options).await.unwrap();
        database
            .execute_unprepared("PRAGMA foreign_keys = ON")
            .await
            .unwrap();
        Migrator::up(&database, None).await.unwrap();
        database
    }

    async fn account(database: &DatabaseConnection, username: &str, is_admin: bool) -> user::Model {
        let now = Utc::now();
        let account = user::ActiveModel {
            id: Set(Uuid::new_v4()),
            username: Set(username.to_owned()),
            password_hash: Set("hash".to_owned()),
            is_admin: Set(is_admin),
            default_repository_visibility: Set("private".to_owned()),
            theme_preference: Set("system".to_owned()),
            motion_preference: Set("system".to_owned()),
            disabled_at: Set(None),
            avatar_updated_at: Set(None),
            created_at: Set(now),
            updated_at: Set(now),
        }
        .insert(database)
        .await
        .unwrap();
        namespace::ActiveModel {
            slug: Set(username.to_owned()),
            kind: Set("user".to_owned()),
            user_id: Set(Some(account.id)),
            organization_id: Set(None),
            created_at: Set(now),
        }
        .insert(database)
        .await
        .unwrap();
        account
    }

    async fn organization_owned_by(
        database: &DatabaseConnection,
        slug: &str,
        owners: &[Uuid],
    ) -> organization::Model {
        let now = Utc::now();
        let organization = organization::ActiveModel {
            id: Set(Uuid::new_v4()),
            slug: Set(slug.to_owned()),
            display_name: Set(slug.to_owned()),
            avatar_updated_at: Set(None),
            created_at: Set(now),
            updated_at: Set(now),
        }
        .insert(database)
        .await
        .unwrap();
        namespace::ActiveModel {
            slug: Set(slug.to_owned()),
            kind: Set("organization".to_owned()),
            user_id: Set(None),
            organization_id: Set(Some(organization.id)),
            created_at: Set(now),
        }
        .insert(database)
        .await
        .unwrap();
        for owner in owners {
            organization_member::ActiveModel {
                organization_id: Set(organization.id),
                user_id: Set(*owner),
                role: Set("owner".to_owned()),
                created_at: Set(now),
            }
            .insert(database)
            .await
            .unwrap();
        }
        organization
    }

    async fn repository_in(database: &DatabaseConnection, namespace: &str, created_by: Uuid) {
        let now = Utc::now();
        repository::ActiveModel {
            id: Set(Uuid::new_v4()),
            namespace: Set(namespace.to_owned()),
            name: Set("project".to_owned()),
            description: Set(None),
            website_url: Set(None),
            visibility: Set("private".to_owned()),
            object_format: Set("sha1".to_owned()),
            mirrored: Set(false),
            default_branch: Set(None),
            issue_counter: Set(0),
            storage_key: Set(Uuid::new_v4()),
            created_by: Set(created_by),
            archived_at: Set(None),
            deleted_at: Set(None),
            icon_updated_at: Set(None),
            icon_source: Set(None),
            created_at: Set(now),
            updated_at: Set(now),
        }
        .insert(database)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn disabling_revokes_sessions_and_tokens() {
        let database = database().await;
        let admin = account(&database, "admin", true).await;
        let target = account(&database, "bob", false).await;
        let now = Utc::now();
        session::ActiveModel {
            token_hash: Set("session".to_owned()),
            user_id: Set(target.id),
            expires_at: Set(now + chrono::Duration::hours(1)),
            created_at: Set(now),
            last_seen_at: Set(now),
        }
        .insert(&database)
        .await
        .unwrap();
        api_token::ActiveModel {
            id: Set(Uuid::new_v4()),
            user_id: Set(target.id),
            name: Set("ci".to_owned()),
            token_hash: Set("token".to_owned()),
            scopes: Set(super::super::SCOPE_READ),
            expires_at: Set(None),
            created_at: Set(now),
            last_used_at: Set(None),
            revoked_at: Set(None),
        }
        .insert(&database)
        .await
        .unwrap();

        let disabled = disable_account(&database, admin.id, target).await.unwrap();
        let sessions = session::Entity::find().count(&database).await.unwrap();
        let active_tokens = api_token::Entity::find()
            .filter(api_token::Column::RevokedAt.is_null())
            .count(&database)
            .await
            .unwrap();
        assert_eq!(
            (disabled.disabled_at.is_some(), sessions, active_tokens),
            (true, 0, 0)
        );
    }

    #[tokio::test]
    async fn last_enabled_admin_cannot_be_disabled() {
        let database = database().await;
        let admin = account(&database, "admin", true).await;
        let other = account(&database, "other", true).await;
        disable_account(&database, admin.id, other.clone())
            .await
            .unwrap();
        let operator = account(&database, "operator", false).await;

        let error = disable_account(&database, operator.id, admin)
            .await
            .unwrap_err();
        assert_eq!(error.status, StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn deletion_refuses_owner_of_repositories() {
        let database = database().await;
        let admin = account(&database, "admin", true).await;
        let target = account(&database, "bob", false).await;
        repository_in(&database, "bob", target.id).await;

        let error = delete_account(&database, admin.id, &target)
            .await
            .unwrap_err();
        assert_eq!(error.status, StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn deletion_refuses_sole_organization_owner() {
        let database = database().await;
        let admin = account(&database, "admin", true).await;
        let target = account(&database, "bob", false).await;
        organization_owned_by(&database, "team", &[target.id]).await;

        let error = delete_account(&database, admin.id, &target)
            .await
            .unwrap_err();
        assert!(error.message.contains("team"));
    }

    #[tokio::test]
    async fn deletion_cascades_credentials_and_reattributes_repositories() {
        let database = database().await;
        let admin = account(&database, "admin", true).await;
        let target = account(&database, "bob", false).await;
        organization_owned_by(&database, "team", &[admin.id, target.id]).await;
        repository_in(&database, "team", target.id).await;
        passkey::ActiveModel {
            id: Set(Uuid::new_v4()),
            user_id: Set(target.id),
            name: Set("laptop".to_owned()),
            credential_id: Set("credential".to_owned()),
            credential: Set("{}".to_owned()),
            created_at: Set(Utc::now()),
            last_used_at: Set(None),
        }
        .insert(&database)
        .await
        .unwrap();

        delete_account(&database, admin.id, &target).await.unwrap();
        let creator = repository::Entity::find()
            .one(&database)
            .await
            .unwrap()
            .unwrap()
            .created_by;
        let namespace_gone = namespace::Entity::find_by_id("bob")
            .one(&database)
            .await
            .unwrap()
            .is_none();
        let passkeys = passkey::Entity::find().count(&database).await.unwrap();
        assert_eq!((creator, namespace_gone, passkeys), (admin.id, true, 0));
    }

    #[tokio::test]
    async fn pending_invitations_exclude_used_and_expired() {
        let database = database().await;
        let admin = account(&database, "admin", true).await;
        let now = Utc::now();
        for (hash, expires_at, used_at) in [
            ("pending", now + chrono::Duration::hours(1), None),
            ("expired", now - chrono::Duration::hours(1), None),
            ("used", now + chrono::Duration::hours(1), Some(now)),
        ] {
            invitation::ActiveModel {
                token_hash: Set(hash.to_owned()),
                created_by: Set(admin.id),
                expires_at: Set(expires_at),
                used_at: Set(used_at),
                created_at: Set(now),
            }
            .insert(&database)
            .await
            .unwrap();
        }

        let pending = pending_invitations(&database).await.unwrap();
        assert_eq!(
            pending
                .iter()
                .map(|row| (row.id.as_str(), row.created_by.as_deref()))
                .collect::<Vec<_>>(),
            vec![("pending", Some("admin"))]
        );
    }

    #[tokio::test]
    async fn user_page_filters_and_counts() {
        let database = database().await;
        for username in ["alice", "alicia", "bob"] {
            account(&database, username, false).await;
        }

        let page = user_page(&database, "ali", 1, 0).await.unwrap();
        assert_eq!(
            (
                page.total,
                page.users
                    .iter()
                    .map(|row| row.username.as_str())
                    .collect::<Vec<_>>()
            ),
            (2, vec!["alice"])
        );
    }

    #[tokio::test]
    async fn organization_deletion_requires_no_repositories_and_cascades() {
        let database = database().await;
        let admin = account(&database, "admin", true).await;
        let organization = organization_owned_by(&database, "team", &[admin.id]).await;
        repository_in(&database, "team", admin.id).await;
        assert_eq!(
            delete_organization_on(&database, &organization)
                .await
                .unwrap_err()
                .status,
            StatusCode::CONFLICT
        );

        repository::Entity::delete_many()
            .exec(&database)
            .await
            .unwrap();
        let now = Utc::now();
        namespace_integration::ActiveModel {
            id: Set(Uuid::new_v4()),
            namespace: Set("team".to_owned()),
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
        delete_organization_on(&database, &organization)
            .await
            .unwrap();
        let remaining = (
            organization::Entity::find().count(&database).await.unwrap(),
            organization_member::Entity::find()
                .count(&database)
                .await
                .unwrap(),
            namespace_integration::Entity::find()
                .count(&database)
                .await
                .unwrap(),
        );
        assert_eq!(remaining, (0, 0, 0));
    }
}
