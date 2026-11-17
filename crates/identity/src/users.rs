//! Administrator account and invitation workflows.
use crate::{AccountService, IdentityError, Principal};
use chrono::Utc;
use gitadel_db::entity::{
    api_token, invitation, issue_attachment, issue_comment, namespace, oauth_access_token,
    oauth_authorization_code, organization, organization_member, repository, repository_alias,
    repository_issue, repository_release, session, user, user_totp,
};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, Condition, ConnectionTrait, EntityTrait, PaginatorTrait,
    QueryFilter, QueryOrder, QuerySelect, Set, TransactionTrait, sea_query::Expr,
};
use std::collections::HashSet;
use uuid::Uuid;
pub struct AdminUserResponse {
    pub id: Uuid,
    pub username: String,
    pub is_admin: bool,
    pub disabled_at: Option<chrono::DateTime<Utc>>,
    pub avatar_updated_at: Option<chrono::DateTime<Utc>>,
    pub two_factor_enabled: bool,
    pub created_at: chrono::DateTime<Utc>,
}

pub struct AdminUserPage {
    pub users: Vec<AdminUserResponse>,
    pub total: u64,
}

pub struct UserListQuery {
    pub q: String,
    pub limit: Option<u64>,
    pub offset: Option<u64>,
}

pub struct PendingInvitationResponse {
    /// The stored token hash. It identifies the invitation without revealing
    /// the token that redeems it.
    pub id: String,
    pub created_by: Option<String>,
    pub created_at: chrono::DateTime<Utc>,
    pub expires_at: chrono::DateTime<Utc>,
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

pub async fn list_users(
    state: &AccountService<'_>,
    actor: &Principal,
    query: UserListQuery,
) -> Result<AdminUserPage, IdentityError> {
    let _actor = state.require_actor(actor, true, crate::SCOPE_READ).await?;
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
    user_page(
        state.database(),
        &needle,
        query.limit.unwrap_or(50).clamp(1, 100),
        query.offset.unwrap_or(0),
    )
    .await
}

pub async fn disable_user(
    state: &AccountService<'_>,
    actor: &Principal,
    username: String,
) -> Result<AdminUserResponse, IdentityError> {
    let actor = state.require_actor(actor, true, crate::SCOPE_WRITE).await?;
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
    Ok(AdminUserResponse::new(account, two_factor))
}

pub async fn enable_user(
    state: &AccountService<'_>,
    actor: &Principal,
    username: String,
) -> Result<AdminUserResponse, IdentityError> {
    let actor = state.require_actor(actor, true, crate::SCOPE_WRITE).await?;
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
    Ok(AdminUserResponse::new(account, two_factor))
}

pub async fn delete_user(
    state: &AccountService<'_>,
    actor: &Principal,
    username: String,
) -> Result<(), IdentityError> {
    let actor = state.require_actor(actor, true, crate::SCOPE_WRITE).await?;
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
    Ok(())
}

pub async fn reset_two_factor(
    state: &AccountService<'_>,
    actor: &Principal,
    username: String,
) -> Result<(), IdentityError> {
    let actor = state.require_actor(actor, true, crate::SCOPE_WRITE).await?;
    let transaction = state.database().begin().await?;
    let account = find_account(&transaction, &username).await?;
    if !super::two_factor::remove_on(&transaction, account.id).await? {
        return Err(IdentityError::bad_request(
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
    Ok(())
}

pub async fn list_invitations(
    state: &AccountService<'_>,
    actor: &Principal,
) -> Result<Vec<PendingInvitationResponse>, IdentityError> {
    let _actor = state.require_actor(actor, true, crate::SCOPE_READ).await?;
    pending_invitations(state.database()).await
}

pub async fn revoke_invitation(
    state: &AccountService<'_>,
    actor: &Principal,
    id: String,
) -> Result<(), IdentityError> {
    let actor = state.require_actor(actor, true, crate::SCOPE_WRITE).await?;
    let transaction = state.database().begin().await?;
    let deleted = invitation::Entity::delete_many()
        .filter(invitation::Column::TokenHash.eq(&id))
        .filter(invitation::Column::UsedAt.is_null())
        .exec(&transaction)
        .await?;
    if deleted.rows_affected == 0 {
        return Err(IdentityError::not_found());
    }
    state
        .audit_on(&transaction, Some(actor.user.id), "invitation.revoke", None)
        .await?;
    transaction.commit().await?;
    Ok(())
}

async fn user_page<C: ConnectionTrait>(
    connection: &C,
    needle: &str,
    limit: u64,
    offset: u64,
) -> Result<AdminUserPage, IdentityError> {
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
) -> Result<user::Model, IdentityError> {
    user::Entity::find()
        .filter(user::Column::Username.eq(username))
        .one(connection)
        .await?
        .ok_or_else(IdentityError::not_found)
}

async fn ensure_other_admin<C: ConnectionTrait>(
    connection: &C,
    account: &user::Model,
) -> Result<(), IdentityError> {
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
        return Err(IdentityError::conflict(
            "The instance must keep at least one enabled administrator.",
        ));
    }
    Ok(())
}

async fn two_factor_enabled<C: ConnectionTrait>(
    connection: &C,
    user_id: Uuid,
) -> Result<bool, IdentityError> {
    crate::two_factor::is_enabled(connection, user_id).await
}

async fn disable_account<C: ConnectionTrait>(
    connection: &C,
    actor_id: Uuid,
    account: user::Model,
) -> Result<user::Model, IdentityError> {
    if account.id == actor_id {
        return Err(IdentityError::bad_request(
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

async fn delete_account<C: ConnectionTrait>(
    connection: &C,
    actor_id: Uuid,
    account: &user::Model,
) -> Result<(), IdentityError> {
    if account.id == actor_id {
        return Err(IdentityError::bad_request(
            "You cannot delete your own account.",
        ));
    }
    ensure_other_admin(connection, account).await?;

    let personal_repositories = repository::Entity::find()
        .filter(repository::Column::Namespace.eq(&account.username))
        .count(connection)
        .await?;
    if personal_repositories > 0 {
        return Err(IdentityError::conflict(format!(
            "{} still owns {personal_repositories} repositories, including deleted ones awaiting purge. Move or purge them first.",
            account.username
        )));
    }

    let sole_owned = sole_owned_organizations(connection, account.id).await?;
    if !sole_owned.is_empty() {
        return Err(IdentityError::conflict(format!(
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
        return Err(IdentityError::conflict(format!(
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
) -> Result<Vec<String>, IdentityError> {
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

async fn pending_invitations<C: ConnectionTrait>(
    connection: &C,
) -> Result<Vec<PendingInvitationResponse>, IdentityError> {
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

pub(crate) async fn delete_organization_on<C: ConnectionTrait>(
    connection: &C,
    organization: &organization::Model,
) -> Result<(), IdentityError> {
    let repositories = repository::Entity::find()
        .filter(repository::Column::Namespace.eq(&organization.slug))
        .count(connection)
        .await?;
    if repositories > 0 {
        return Err(IdentityError::conflict(format!(
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
    use sea_orm::DatabaseConnection;

    use super::*;
    use gitadel_db::entity::{namespace_integration, passkey};

    use gitadel_db::test_support::database;

    async fn account(database: &DatabaseConnection, username: &str, is_admin: bool) -> user::Model {
        crate::accounts::insert_account_on(database, username, "hash".into(), is_admin, Utc::now())
            .await
            .unwrap()
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
        assert!(matches!(error, crate::IdentityError::Conflict(_)));
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
        assert!(matches!(error, crate::IdentityError::Conflict(_)));
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
        assert!(error.to_string().contains("team"));
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
        assert!(matches!(
            delete_organization_on(&database, &organization)
                .await
                .unwrap_err(),
            crate::IdentityError::Conflict(_)
        ));

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
