//! Per-repository deploy keys.
//!
//! A deploy key is an SSH public key bound to exactly one repository, with
//! read-only or read-write access. Fingerprints are unique across deploy keys
//! and user SSH keys, so an SSH login resolves to a single identity.

use axum::{
    Json,
    extract::{Path as AxumPath, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use axum_extra::extract::cookie::CookieJar;
use chrono::{DateTime, Duration, Utc};
use russh::keys::ssh_key::{HashAlg, PublicKey};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, QueryOrder, Set,
    TransactionTrait,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{Permission, RepositoryState};
use crate::{
    entity::{repository, repository_deploy_key, ssh_key},
    identity::{ApiError, SCOPE_READ, SCOPE_WRITE, validate_name},
};

const MAX_DEPLOY_KEYS_PER_REPOSITORY: usize = 50;

/// What an authenticated deploy key may do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DeployKeyAccess {
    pub key_id: Uuid,
    pub repository_id: Uuid,
    pub read_only: bool,
    /// Pushes need a user for audit events and webhook payloads; deploy key
    /// pushes are attributed to the key's creator, or the repository creator
    /// once that account is gone.
    pub attributed_user: Uuid,
}

impl DeployKeyAccess {
    /// Deploy keys reach only their own repository. Every other repository
    /// is reported as missing so a key cannot probe for private names.
    pub(crate) fn authorize(
        &self,
        repository: &repository::Model,
        permission: Permission,
    ) -> Result<(), ApiError> {
        if repository.id != self.repository_id || repository.deleted_at.is_some() {
            return Err(ApiError::not_found());
        }
        match permission {
            Permission::Read => Ok(()),
            Permission::Write if self.read_only => {
                Err(ApiError::forbidden("This deploy key is read-only."))
            }
            Permission::Write if repository.archived_at.is_some() => {
                Err(ApiError::forbidden("Archived repositories are read-only."))
            }
            Permission::Write if repository.mirrored => {
                Err(ApiError::forbidden("Mirrored repositories are read-only."))
            }
            Permission::Write => Ok(()),
            Permission::Manage => Err(ApiError::not_found()),
        }
    }
}

/// Resolves an SSH key fingerprint to a deploy key.
pub(crate) async fn authenticate<C: ConnectionTrait>(
    database: &C,
    fingerprint: &str,
) -> Result<Option<DeployKeyAccess>, ApiError> {
    let Some(key) = repository_deploy_key::Entity::find()
        .filter(repository_deploy_key::Column::Fingerprint.eq(fingerprint))
        .one(database)
        .await?
    else {
        return Ok(None);
    };
    let Some(repository) = repository::Entity::find_by_id(key.repository_id)
        .one(database)
        .await?
        .filter(|repository| repository.deleted_at.is_none())
    else {
        return Ok(None);
    };
    let access = DeployKeyAccess {
        key_id: key.id,
        repository_id: key.repository_id,
        read_only: key.read_only,
        attributed_user: key.created_by.unwrap_or(repository.created_by),
    };
    let now = Utc::now();
    if key
        .last_used_at
        .is_none_or(|last_used_at| now - last_used_at > Duration::minutes(5))
    {
        let mut active: repository_deploy_key::ActiveModel = key.into();
        active.last_used_at = Set(Some(now));
        active.update(database).await?;
    }
    Ok(Some(access))
}

/// Returns true when a user SSH key or deploy key already uses `fingerprint`.
pub(crate) async fn fingerprint_in_use<C: ConnectionTrait>(
    database: &C,
    fingerprint: &str,
) -> Result<bool, ApiError> {
    if ssh_key::Entity::find()
        .filter(ssh_key::Column::Fingerprint.eq(fingerprint))
        .one(database)
        .await?
        .is_some()
    {
        return Ok(true);
    }
    Ok(repository_deploy_key::Entity::find()
        .filter(repository_deploy_key::Column::Fingerprint.eq(fingerprint))
        .one(database)
        .await?
        .is_some())
}

#[derive(Serialize)]
pub struct DeployKeyResponse {
    id: Uuid,
    title: String,
    fingerprint: String,
    public_key: String,
    read_only: bool,
    created_at: DateTime<Utc>,
    last_used_at: Option<DateTime<Utc>>,
}

impl From<repository_deploy_key::Model> for DeployKeyResponse {
    fn from(key: repository_deploy_key::Model) -> Self {
        Self {
            id: key.id,
            title: key.title,
            fingerprint: key.fingerprint,
            public_key: key.public_key,
            read_only: key.read_only,
            created_at: key.created_at,
            last_used_at: key.last_used_at,
        }
    }
}

#[derive(Deserialize)]
pub struct CreateDeployKeyRequest {
    title: String,
    key: String,
    #[serde(default = "default_read_only")]
    read_only: bool,
}

fn default_read_only() -> bool {
    true
}

pub async fn list_deploy_keys(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name)): AxumPath<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<Vec<DeployKeyResponse>>, ApiError> {
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
    let keys = repository_deploy_key::Entity::find()
        .filter(repository_deploy_key::Column::RepositoryId.eq(repository.id))
        .order_by_asc(repository_deploy_key::Column::CreatedAt)
        .all(state.identity().database())
        .await?;
    Ok(Json(keys.into_iter().map(Into::into).collect()))
}

pub async fn create_deploy_key(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name)): AxumPath<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<CreateDeployKeyRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let (actor, repository) = state
        .authenticated_repository(
            &headers,
            &jar,
            &namespace,
            &name,
            Permission::Manage,
            SCOPE_WRITE,
        )
        .await?;
    let title = validate_name(&request.title, "Deploy key title")?;
    let key = PublicKey::from_openssh(request.key.trim())
        .map_err(|_| ApiError::bad_request("The SSH public key is not valid OpenSSH format."))?;
    let fingerprint = key.fingerprint(HashAlg::Sha256).to_string();
    let database = state.identity().database();
    let transaction = database.begin().await?;
    if fingerprint_in_use(&transaction, &fingerprint).await? {
        return Err(ApiError::conflict(
            "That SSH key is already registered as a user key or deploy key.",
        ));
    }
    let existing = repository_deploy_key::Entity::find()
        .filter(repository_deploy_key::Column::RepositoryId.eq(repository.id))
        .all(&transaction)
        .await?;
    if existing.len() >= MAX_DEPLOY_KEYS_PER_REPOSITORY {
        return Err(ApiError::bad_request(
            "A repository can have at most 50 deploy keys.",
        ));
    }
    let row = repository_deploy_key::ActiveModel {
        id: Set(Uuid::new_v4()),
        repository_id: Set(repository.id),
        title: Set(title),
        fingerprint: Set(fingerprint),
        public_key: Set(key.to_openssh().map_err(ApiError::internal)?),
        read_only: Set(request.read_only),
        created_by: Set(Some(actor.user.id)),
        created_at: Set(Utc::now()),
        last_used_at: Set(None),
    }
    .insert(&transaction)
    .await?;
    state
        .identity()
        .audit_on(
            &transaction,
            Some(actor.user.id),
            "repository.deploy_key.create",
            Some(format!("{namespace}/{name}:{}", row.fingerprint)),
        )
        .await?;
    transaction.commit().await?;
    Ok((StatusCode::CREATED, Json(DeployKeyResponse::from(row))))
}

pub async fn delete_deploy_key(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name, id)): AxumPath<(String, String, Uuid)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<StatusCode, ApiError> {
    let (actor, repository) = state
        .authenticated_repository(
            &headers,
            &jar,
            &namespace,
            &name,
            Permission::Manage,
            SCOPE_WRITE,
        )
        .await?;
    let transaction = state.identity().database().begin().await?;
    let key = repository_deploy_key::Entity::find_by_id(id)
        .filter(repository_deploy_key::Column::RepositoryId.eq(repository.id))
        .one(&transaction)
        .await?
        .ok_or_else(ApiError::not_found)?;
    repository_deploy_key::Entity::delete_by_id(key.id)
        .exec(&transaction)
        .await?;
    state
        .identity()
        .audit_on(
            &transaction,
            Some(actor.user.id),
            "repository.deploy_key.delete",
            Some(format!("{namespace}/{name}:{}", key.fingerprint)),
        )
        .await?;
    transaction.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use sea_orm::{ConnectOptions, Database};
    use sea_orm_migration::MigratorTrait;

    use super::*;

    async fn insert_repository(
        database: &sea_orm::DatabaseConnection,
        owner: &crate::entity::user::Model,
        name: &str,
    ) -> repository::Model {
        let now = Utc::now();
        repository::ActiveModel {
            id: Set(Uuid::new_v4()),
            namespace: Set(owner.username.clone()),
            name: Set(name.to_owned()),
            description: Set(None),
            website_url: Set(None),
            visibility: Set("private".to_owned()),
            object_format: Set("sha1".to_owned()),
            mirrored: Set(false),
            default_branch: Set(None),
            issue_counter: Set(0),
            storage_key: Set(Uuid::new_v4()),
            created_by: Set(owner.id),
            archived_at: Set(None),
            deleted_at: Set(None),
            icon_updated_at: Set(None),
            icon_source: Set(None),
            created_at: Set(now),
            updated_at: Set(now),
        }
        .insert(database)
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn deploy_keys_reach_only_their_repository() {
        let mut options = ConnectOptions::new("sqlite::memory:");
        options.max_connections(1);
        let database = Database::connect(options).await.unwrap();
        crate::migration::Migrator::up(&database, None)
            .await
            .unwrap();
        let owner = crate::identity::bootstrap_admin(
            &database,
            "deploy-owner",
            "deploy-test-password".to_owned(),
        )
        .await
        .unwrap();
        let bound = insert_repository(&database, &owner, "bound").await;
        let other = insert_repository(&database, &owner, "other").await;
        repository_deploy_key::ActiveModel {
            id: Set(Uuid::new_v4()),
            repository_id: Set(bound.id),
            title: Set("ci".to_owned()),
            fingerprint: Set("SHA256:deploy".to_owned()),
            public_key: Set("ssh-ed25519 AAAA".to_owned()),
            read_only: Set(true),
            created_by: Set(Some(owner.id)),
            created_at: Set(Utc::now()),
            last_used_at: Set(None),
        }
        .insert(&database)
        .await
        .unwrap();

        assert!(
            authenticate(&database, "SHA256:unknown")
                .await
                .unwrap()
                .is_none()
        );
        let access = authenticate(&database, "SHA256:deploy")
            .await
            .unwrap()
            .expect("deploy key authenticates");
        assert_eq!(access.repository_id, bound.id);
        assert_eq!(access.attributed_user, owner.id);
        assert!(
            fingerprint_in_use(&database, "SHA256:deploy")
                .await
                .unwrap()
        );

        access.authorize(&bound, Permission::Read).unwrap();
        let write = access.authorize(&bound, Permission::Write).unwrap_err();
        assert_eq!(
            write.into_response().status(),
            axum::http::StatusCode::FORBIDDEN
        );
        for permission in [Permission::Read, Permission::Write, Permission::Manage] {
            let denied = access.authorize(&other, permission).unwrap_err();
            assert_eq!(
                denied.into_response().status(),
                axum::http::StatusCode::NOT_FOUND
            );
        }

        let writable = DeployKeyAccess {
            read_only: false,
            ..access
        };
        writable.authorize(&bound, Permission::Write).unwrap();
        assert!(writable.authorize(&other, Permission::Write).is_err());
        assert!(writable.authorize(&bound, Permission::Manage).is_err());
    }
}
