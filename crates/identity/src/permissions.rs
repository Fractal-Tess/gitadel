//! Namespace and organization management authorization.
use crate::{AuthenticatedUser, IdentityError};
use gitadel_db::entity::{namespace, organization, organization_member};
use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter};
use uuid::Uuid;

/// Credential source retained across domain calls. Raw credentials are never
/// serialized or included in Debug output; each operation revalidates them.
#[derive(Clone)]
pub struct RepositoryPrincipal {
    source: RepositoryCredential,
}

#[derive(Clone)]
enum RepositoryCredential {
    Anonymous,
    Session(String),
    Token(String),
}

impl RepositoryPrincipal {
    pub fn anonymous() -> Self {
        Self {
            source: RepositoryCredential::Anonymous,
        }
    }
    pub fn session(raw: String) -> Self {
        Self {
            source: RepositoryCredential::Session(raw),
        }
    }
    pub fn token(raw: String) -> Self {
        Self {
            source: RepositoryCredential::Token(raw),
        }
    }

    /// Authenticate against the writer with the operation's exact scope.
    /// Identity preserves PAT read-scope mapping and OAuth scope distinctions.
    pub async fn authenticate(
        &self,
        database: &sea_orm::DatabaseConnection,
        required_scope: i32,
    ) -> Result<AuthenticatedUser, IdentityError> {
        let identity = crate::IdentityService::new(database);
        match &self.source {
            RepositoryCredential::Anonymous => Err(IdentityError::unauthorized()),
            RepositoryCredential::Session(raw) => identity.authenticate_session(raw).await,
            RepositoryCredential::Token(raw) => {
                identity.authenticate_token(raw, required_scope).await
            }
        }
    }

    /// Optional browser sessions retain anonymous-read behavior when expired;
    /// an explicitly supplied token must authenticate with the required scope.
    pub async fn optional_user(
        &self,
        database: &sea_orm::DatabaseConnection,
        required_scope: i32,
    ) -> Result<Option<gitadel_db::entity::user::Model>, IdentityError> {
        match &self.source {
            RepositoryCredential::Anonymous => Ok(None),
            RepositoryCredential::Session(raw) => {
                crate::IdentityService::new(database)
                    .session_user(raw)
                    .await
            }
            RepositoryCredential::Token(_) => Ok(Some(
                self.authenticate(database, required_scope).await?.user,
            )),
        }
    }
}

/// Repository capabilities are independent of token scopes. Callers authenticate
/// the required scope before requesting one of these capabilities.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RepositoryPermission {
    Read,
    Write,
    Manage,
}

/// A namespace that may own a repository created or transferred by this user.
pub async fn repository_namespace_owner<C: ConnectionTrait>(
    connection: &C,
    slug: &str,
    actor: Uuid,
) -> Result<namespace::Model, IdentityError> {
    let owner = namespace::Entity::find_by_id(slug)
        .one(connection)
        .await?
        .ok_or_else(IdentityError::not_found)?;
    let permitted = match owner.kind.as_str() {
        "user" => owner.user_id == Some(actor),
        "organization" => match owner.organization_id {
            Some(id) => organization_member::Entity::find_by_id((id, actor))
                .filter(organization_member::Column::Role.eq("owner"))
                .one(connection)
                .await?
                .is_some(),
            None => false,
        },
        _ => false,
    };
    if permitted {
        Ok(owner)
    } else {
        Err(IdentityError::not_found())
    }
}

/// Collaborator management is available only to a personal namespace owner.
pub async fn personal_repository_owner<C: ConnectionTrait>(
    connection: &C,
    slug: &str,
    actor: Uuid,
) -> Result<(), IdentityError> {
    let owner = namespace::Entity::find_by_id(slug)
        .one(connection)
        .await?
        .ok_or_else(IdentityError::not_found)?;
    if owner.kind == "user" && owner.user_id == Some(actor) {
        Ok(())
    } else {
        Err(IdentityError::not_found())
    }
}

/// Shared visibility predicate for lists, catalog pages and individual checks.
/// The alias and parameter are supplied by trusted query builders, never users.
pub fn repository_policy_sql(
    alias: &str,
    actor_parameter: &str,
    authenticated: bool,
    permission: RepositoryPermission,
) -> String {
    let public = if permission == RepositoryPermission::Read {
        format!("{alias}.visibility = 'public'")
    } else {
        "0 = 1".to_owned()
    };
    if !authenticated {
        return format!("{alias}.deleted_at IS NULL AND ({public})");
    }
    let organization_role = if permission == RepositoryPermission::Manage {
        " AND role = 'owner'"
    } else {
        ""
    };
    let collaborator = match permission {
        RepositoryPermission::Manage => String::new(),
        permission => format!(
            " OR ({alias}.id IN (SELECT repository_id FROM repository_collaborators WHERE user_id = {actor_parameter}{}) AND {alias}.namespace IN (SELECT slug FROM namespaces WHERE kind = 'user'))",
            if permission == RepositoryPermission::Write {
                " AND role = 'write'"
            } else {
                ""
            },
        ),
    };
    format!(
        "{alias}.deleted_at IS NULL AND ({public} \
         OR {alias}.namespace IN (SELECT slug FROM namespaces WHERE kind = 'user' AND user_id = {actor_parameter}) \
         OR {alias}.namespace IN (SELECT slug FROM namespaces WHERE kind = 'organization' AND organization_id IN \
         (SELECT organization_id FROM organization_members WHERE user_id = {actor_parameter}{organization_role})){collaborator})"
    )
}

/// ORM form of the same policy, for authoritative reads and bulk queries.
pub fn repository_condition(
    actor: Option<Uuid>,
    permission: RepositoryPermission,
) -> sea_orm::sea_query::SimpleExpr {
    use sea_orm::sea_query::Expr;
    let sql = repository_policy_sql("repositories", "?", actor.is_some(), permission);
    // The predicate repeats this value in owner, organization and collaborator
    // subqueries. SeaQuery binds each occurrence rather than interpolating it.
    let count = sql.bytes().filter(|byte| *byte == b'?').count();
    Expr::cust_with_values(sql, std::iter::repeat_n(actor, count))
}

/// Check current membership and repository visibility on the supplied writer.
pub async fn can_access_repository<C: ConnectionTrait>(
    connection: &C,
    repository_id: Uuid,
    actor: Option<Uuid>,
    permission: RepositoryPermission,
) -> Result<bool, IdentityError> {
    use gitadel_db::entity::repository;
    Ok(repository::Entity::find_by_id(repository_id)
        .filter(repository_condition(actor, permission))
        .one(connection)
        .await?
        .is_some())
}

/// Conceal inaccessible repositories before reporting write restrictions.
/// Read access can still support issue creation with a write-scoped credential.
pub async fn authorize_repository<C: ConnectionTrait>(
    connection: &C,
    repository_id: Uuid,
    actor: Option<Uuid>,
    permission: RepositoryPermission,
) -> Result<gitadel_db::entity::repository::Model, IdentityError> {
    use gitadel_db::entity::repository;
    let repository = repository::Entity::find_by_id(repository_id)
        .filter(repository_condition(actor, permission))
        .one(connection)
        .await?
        .ok_or_else(IdentityError::not_found)?;
    if permission == RepositoryPermission::Write {
        if repository.archived_at.is_some() {
            return Err(IdentityError::forbidden(
                "Archived repositories are read-only.",
            ));
        }
        if repository.mirrored {
            return Err(IdentityError::forbidden(
                "Mirrored repositories are read-only.",
            ));
        }
    }
    Ok(repository)
}
pub async fn authorize_namespace<C: ConnectionTrait>(
    connection: &C,
    actor: &AuthenticatedUser,
    slug: &str,
) -> Result<(), IdentityError> {
    let owner = namespace::Entity::find_by_id(slug)
        .one(connection)
        .await?
        .ok_or_else(IdentityError::not_found)?;
    let permitted = match owner.kind.as_str() {
        "user" => owner.user_id == Some(actor.user.id),
        "organization" => match owner.organization_id {
            Some(organization_id) => {
                organization_member::Entity::find_by_id((organization_id, actor.user.id))
                    .filter(organization_member::Column::Role.eq("owner"))
                    .one(connection)
                    .await?
                    .is_some()
            }
            None => false,
        },
        _ => false,
    };
    if permitted {
        Ok(())
    } else {
        Err(IdentityError::not_found())
    }
}

pub async fn accessible_organization<C: ConnectionTrait>(
    connection: &C,
    slug: &str,
    user_id: Uuid,
) -> Result<organization::Model, IdentityError> {
    let organization = organization::Entity::find()
        .filter(organization::Column::Slug.eq(slug))
        .one(connection)
        .await?
        .ok_or_else(IdentityError::not_found)?;
    organization_member::Entity::find_by_id((organization.id, user_id))
        .one(connection)
        .await?
        .ok_or_else(IdentityError::not_found)?;
    Ok(organization)
}

pub async fn owned_organization<C: ConnectionTrait>(
    connection: &C,
    slug: &str,
    user_id: Uuid,
) -> Result<organization::Model, IdentityError> {
    let organization = accessible_organization(connection, slug, user_id).await?;
    let membership = organization_member::Entity::find_by_id((organization.id, user_id))
        .one(connection)
        .await?
        .ok_or_else(IdentityError::not_found)?;
    if membership.role != "owner" {
        return Err(IdentityError::forbidden(
            "Only an organization owner can manage this organization.",
        ));
    }
    Ok(organization)
}

#[cfg(test)]
mod repository_boundaries_tests {
    use super::*;
    use chrono::{Duration, Utc};
    use gitadel_db::{
        entity::{
            api_token, oauth_access_token, oauth_application, repository, repository_collaborator,
        },
        test_support,
    };
    use sea_orm::{ActiveModelTrait, Set};

    #[tokio::test]
    async fn bulk_and_single_checks_share_owner_collaborator_and_concealment_rules() {
        let database = test_support::database().await;
        let owner = test_support::owner(&database, "owner").await;
        let reader = test_support::owner(&database, "reader").await;
        let project = test_support::repository(&database, &owner, "project").await;
        repository_collaborator::ActiveModel {
            repository_id: Set(project.id),
            user_id: Set(reader.id),
            role: Set("read".to_owned()),
            created_at: Set(Utc::now()),
        }
        .insert(&database)
        .await
        .unwrap();
        for (actor, expected) in [
            (None, [false, false, false]),
            (Some(owner.id), [true, true, true]),
            (Some(reader.id), [true, false, false]),
        ] {
            for (permission, expected) in [
                RepositoryPermission::Read,
                RepositoryPermission::Write,
                RepositoryPermission::Manage,
            ]
            .into_iter()
            .zip(expected)
            {
                let single = can_access_repository(&database, project.id, actor, permission)
                    .await
                    .unwrap();
                let bulk = repository::Entity::find()
                    .filter(repository_condition(actor, permission))
                    .all(&database)
                    .await
                    .unwrap();
                assert_eq!((single, !bulk.is_empty()), (expected, expected));
            }
        }
        let mut archived: repository::ActiveModel = project.clone().into();
        archived.archived_at = Set(Some(Utc::now()));
        archived.update(&database).await.unwrap();
        assert!(matches!(
            authorize_repository(&database, project.id, None, RepositoryPermission::Write).await,
            Err(IdentityError::NotFound)
        ));
        assert!(matches!(
            authorize_repository(
                &database,
                project.id,
                Some(owner.id),
                RepositoryPermission::Write
            )
            .await,
            Err(IdentityError::Forbidden(_))
        ));
        assert!(
            authorize_repository(
                &database,
                project.id,
                Some(reader.id),
                RepositoryPermission::Read
            )
            .await
            .is_ok()
        );
    }

    #[tokio::test]
    async fn source_reauthentication_preserves_pat_oauth_scope_mapping_and_revocation() {
        let database = test_support::database().await;
        let owner = test_support::owner(&database, "owner").await;
        let now = Utc::now();
        let pat = api_token::ActiveModel {
            id: Set(Uuid::new_v4()),
            user_id: Set(owner.id),
            name: Set("read".to_owned()),
            token_hash: Set(crate::hash_secret("pat")),
            scopes: Set(crate::SCOPE_READ),
            expires_at: Set(None),
            created_at: Set(now),
            last_used_at: Set(None),
            revoked_at: Set(None),
        }
        .insert(&database)
        .await
        .unwrap();
        let application = oauth_application::ActiveModel {
            id: Set(Uuid::new_v4()),
            user_id: Set(owner.id),
            name: Set("client".to_owned()),
            client_id: Set("client".to_owned()),
            client_secret_hash: Set("unused".to_owned()),
            redirect_uri: Set("https://example.test/callback".to_owned()),
            created_at: Set(now),
        }
        .insert(&database)
        .await
        .unwrap();
        oauth_access_token::ActiveModel {
            token_hash: Set(crate::hash_secret("oauth")),
            application_id: Set(application.id),
            user_id: Set(owner.id),
            scopes: Set(crate::SCOPE_REPOSITORY_READ),
            scope: Set("read:repository".to_owned()),
            created_at: Set(now),
            expires_at: Set(now + Duration::hours(1)),
            last_used_at: Set(None),
            revoked_at: Set(None),
        }
        .insert(&database)
        .await
        .unwrap();
        let personal = RepositoryPrincipal::token("pat".to_owned());
        let oauth = RepositoryPrincipal::token("oauth".to_owned());
        assert!(
            personal
                .authenticate(&database, crate::SCOPE_REPOSITORY_READ)
                .await
                .unwrap()
                .via_api_token
        );
        assert!(
            oauth
                .authenticate(&database, crate::SCOPE_REPOSITORY_READ)
                .await
                .is_ok()
        );
        assert!(matches!(
            oauth.authenticate(&database, crate::SCOPE_READ).await,
            Err(IdentityError::Forbidden(_))
        ));
        assert!(matches!(
            personal.authenticate(&database, crate::SCOPE_WRITE).await,
            Err(IdentityError::Forbidden(_))
        ));
        let mut revoked: api_token::ActiveModel = pat.into();
        revoked.revoked_at = Set(Some(now));
        revoked.update(&database).await.unwrap();
        assert!(matches!(
            personal.authenticate(&database, crate::SCOPE_READ).await,
            Err(IdentityError::Unauthorized)
        ));
        let raw = crate::create_session_on(&database, owner.id, Duration::hours(1))
            .await
            .unwrap();
        assert!(
            !RepositoryPrincipal::session(raw)
                .authenticate(&database, crate::SCOPE_WRITE)
                .await
                .unwrap()
                .via_api_token
        );
    }
}
