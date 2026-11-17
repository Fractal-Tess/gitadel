//! Repository lookup, control, favorites and collaborator operations.
use crate::content::{RepositoryContent, read_git};
use chrono::Utc;
use gitadel_core::audit::AuditContext;
use gitadel_db::entity::{
    repository, repository_alias, repository_collaborator, repository_favorite, user,
};
use gitadel_identity::{
    IdentityError,
    permissions::{
        RepositoryPermission as Permission, RepositoryPrincipal, authorize_repository,
        personal_repository_owner, repository_namespace_owner,
    },
    validate_slug,
};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, Set,
    TransactionTrait,
};
use std::path::PathBuf;
use uuid::Uuid;
pub struct LifecycleService {
    database: DatabaseConnection,
    content: RepositoryContent,
}
impl LifecycleService {
    pub fn new(database: DatabaseConnection, root: PathBuf) -> Self {
        Self {
            database,
            content: RepositoryContent::new(root),
        }
    }
    pub async fn find(
        &self,
        namespace: &str,
        name: &str,
    ) -> Result<repository::Model, IdentityError> {
        let repository = self.find_including_deleted(namespace, name).await?;
        if repository.deleted_at.is_some() {
            return Err(IdentityError::not_found());
        }
        Ok(repository)
    }

    pub async fn find_including_deleted(
        &self,
        namespace: &str,
        name: &str,
    ) -> Result<repository::Model, IdentityError> {
        if let Some(repository) = repository::Entity::find()
            .filter(repository::Column::Namespace.eq(namespace))
            .filter(repository::Column::Name.eq(name))
            .one(&self.database)
            .await?
        {
            return Ok(repository);
        }
        let alias = repository_alias::Entity::find_by_id((namespace.to_owned(), name.to_owned()))
            .one(&self.database)
            .await?
            .ok_or_else(IdentityError::not_found)?;
        repository::Entity::find_by_id(alias.repository_id)
            .one(&self.database)
            .await?
            .ok_or_else(IdentityError::not_found)
    }

    pub async fn update_repository_control(
        &self,
        principal: &RepositoryPrincipal,
        repository_id: Uuid,
        request: UpdateRepositoryControlRequest,
        audit: AuditContext,
    ) -> Result<(repository::Model, bool), IdentityError> {
        let actor = principal
            .authenticate(&self.database, gitadel_identity::SCOPE_WRITE)
            .await?;
        let repository = authorize_repository(
            &self.database,
            repository_id,
            Some(actor.user.id),
            Permission::Manage,
        )
        .await?;
        if request.description.is_none()
            && request.visibility.is_none()
            && request.default_branch.is_none()
            && request.name.is_none()
            && request.namespace.is_none()
        {
            return Err(IdentityError::bad_request(
                "Provide at least one repository control change.",
            ));
        }
        let default_branch_changed = request.default_branch.is_some();

        let description = request.description.map(normalize_description).transpose()?;
        let visibility = request.visibility.map(validate_visibility).transpose()?;
        let next_name = request
            .name
            .map(|value| validate_repository_name(&value))
            .transpose()?;
        let next_namespace = request
            .namespace
            .map(|value| validate_slug(&value, "Namespace"))
            .transpose()?;
        if next_namespace.is_some() && next_name.is_none() {
            return Err(IdentityError::bad_request(
                "Transfers must include a repository name.",
            ));
        }
        let moved = next_name.is_some() || next_namespace.is_some();
        let target_name = next_name.unwrap_or_else(|| repository.name.clone());
        let target_namespace = next_namespace.unwrap_or_else(|| repository.namespace.clone());
        if moved {
            repository_namespace_owner(&self.database, &target_namespace, actor.user.id).await?;
            if target_namespace != repository.namespace || target_name != repository.name {
                self.ensure_location_available(&target_namespace, &target_name)
                    .await?;
            }
        }

        if let Some(default_branch) = request.default_branch.as_deref() {
            self.set_default_branch(&repository, default_branch).await?;
        }

        let transaction = self.database.begin().await?;
        let mut active: repository::ActiveModel = repository.clone().into();
        if let Some(description) = description {
            active.description = Set(description);
        }
        if let Some(visibility) = visibility {
            active.visibility = Set(visibility);
        }
        if moved {
            if target_namespace != repository.namespace || target_name != repository.name {
                repository_alias::ActiveModel {
                    repository_id: Set(repository.id),
                    namespace: Set(repository.namespace.clone()),
                    name: Set(repository.name.clone()),
                    created_at: Set(Utc::now()),
                }
                .insert(&transaction)
                .await?;
            }
            active.namespace = Set(target_namespace.clone());
            active.name = Set(target_name.clone());
            repository_collaborator::Entity::delete_many()
                .filter(repository_collaborator::Column::RepositoryId.eq(repository.id))
                .exec(&transaction)
                .await?;
        }
        if let Some(default_branch) = request.default_branch {
            active.default_branch = Set(Some(default_branch));
        }
        active.updated_at = Set(Utc::now());
        let updated = active.update(&transaction).await?;
        let action = if moved {
            "repository.transfer"
        } else {
            "repository.update"
        };
        gitadel_db::audit::record(
            &transaction,
            Some(actor.user.id),
            action,
            Some(format!("{}/{}", updated.namespace, updated.name)),
            audit,
        )
        .await?;
        transaction.commit().await?;
        Ok((updated, default_branch_changed))
    }
    pub async fn favorite_repository(
        &self,
        principal: &RepositoryPrincipal,
        repository_id: Uuid,
        _audit: AuditContext,
    ) -> Result<(), IdentityError> {
        let actor = principal
            .authenticate(&self.database, gitadel_identity::SCOPE_WRITE)
            .await?;
        let repository = authorize_repository(
            &self.database,
            repository_id,
            Some(actor.user.id),
            Permission::Read,
        )
        .await?;
        if repository_favorite::Entity::find_by_id((actor.user.id, repository.id))
            .one(&self.database)
            .await?
            .is_none()
        {
            repository_favorite::ActiveModel {
                user_id: Set(actor.user.id),
                repository_id: Set(repository.id),
                created_at: Set(Utc::now()),
            }
            .insert(&self.database)
            .await?;
        }
        Ok(())
    }
    pub async fn unfavorite_repository(
        &self,
        principal: &RepositoryPrincipal,
        repository_id: Uuid,
        _audit: AuditContext,
    ) -> Result<(), IdentityError> {
        let actor = principal
            .authenticate(&self.database, gitadel_identity::SCOPE_WRITE)
            .await?;
        let repository = authorize_repository(
            &self.database,
            repository_id,
            Some(actor.user.id),
            Permission::Read,
        )
        .await?;
        repository_favorite::Entity::delete_by_id((actor.user.id, repository.id))
            .exec(&self.database)
            .await?;
        Ok(())
    }
    pub async fn list_collaborators(
        &self,
        principal: &RepositoryPrincipal,
        repository_id: Uuid,
        _audit: AuditContext,
    ) -> Result<Vec<CollaboratorResponse>, IdentityError> {
        let actor = principal
            .authenticate(&self.database, gitadel_identity::SCOPE_READ)
            .await?;
        let repository = authorize_repository(
            &self.database,
            repository_id,
            Some(actor.user.id),
            Permission::Manage,
        )
        .await?;
        personal_repository_owner(&self.database, &repository.namespace, actor.user.id).await?;
        let collaborators = repository_collaborator::Entity::find()
            .filter(repository_collaborator::Column::RepositoryId.eq(repository.id))
            .order_by_asc(repository_collaborator::Column::CreatedAt)
            .all(&self.database)
            .await?;
        let mut response = Vec::with_capacity(collaborators.len());
        for collaborator in collaborators {
            if let Some(account) = user::Entity::find_by_id(collaborator.user_id)
                .one(&self.database)
                .await?
            {
                response.push(CollaboratorResponse {
                    username: account.username,
                    role: collaborator.role,
                    created_at: collaborator.created_at,
                });
            }
        }
        Ok(response)
    }
    pub async fn add_collaborator(
        &self,
        principal: &RepositoryPrincipal,
        repository_id: Uuid,
        request: AddCollaboratorRequest,
        audit: AuditContext,
    ) -> Result<CollaboratorResponse, IdentityError> {
        let actor = principal
            .authenticate(&self.database, gitadel_identity::SCOPE_WRITE)
            .await?;
        let repository = authorize_repository(
            &self.database,
            repository_id,
            Some(actor.user.id),
            Permission::Manage,
        )
        .await?;
        let namespace = &repository.namespace;
        let name = &repository.name;
        personal_repository_owner(&self.database, &repository.namespace, actor.user.id).await?;
        if request.role != "read" && request.role != "write" {
            return Err(IdentityError::bad_request(
                "Collaborator role must be read or write.",
            ));
        }
        let username = validate_slug(&request.username, "Username")?;
        let account = user::Entity::find()
            .filter(user::Column::Username.eq(&username))
            .filter(user::Column::DisabledAt.is_null())
            .one(&self.database)
            .await?
            .ok_or_else(IdentityError::not_found)?;
        if account.id == actor.user.id {
            return Err(IdentityError::bad_request(
                "The repository owner is not a collaborator.",
            ));
        }
        if repository_collaborator::Entity::find_by_id((repository.id, account.id))
            .one(&self.database)
            .await?
            .is_some()
        {
            return Err(IdentityError::conflict(
                "That user is already a repository collaborator.",
            ));
        }

        let transaction = self.database.begin().await?;
        let collaborator = repository_collaborator::ActiveModel {
            repository_id: Set(repository.id),
            user_id: Set(account.id),
            role: Set(request.role),
            created_at: Set(Utc::now()),
        }
        .insert(&transaction)
        .await?;
        gitadel_db::audit::record(
            &transaction,
            Some(actor.user.id),
            "repository.collaborator.add",
            Some(format!("{namespace}/{name}/{username}")),
            audit,
        )
        .await?;
        transaction.commit().await?;

        Ok(CollaboratorResponse {
            username,
            role: collaborator.role,
            created_at: collaborator.created_at,
        })
    }
    pub async fn remove_collaborator(
        &self,
        principal: &RepositoryPrincipal,
        repository_id: Uuid,
        username: String,
        audit: AuditContext,
    ) -> Result<(), IdentityError> {
        let actor = principal
            .authenticate(&self.database, gitadel_identity::SCOPE_WRITE)
            .await?;
        let repository = authorize_repository(
            &self.database,
            repository_id,
            Some(actor.user.id),
            Permission::Manage,
        )
        .await?;
        let namespace = &repository.namespace;
        let name = &repository.name;
        personal_repository_owner(&self.database, &repository.namespace, actor.user.id).await?;
        let account = user::Entity::find()
            .filter(user::Column::Username.eq(&username))
            .one(&self.database)
            .await?
            .ok_or_else(IdentityError::not_found)?;
        let transaction = self.database.begin().await?;
        let deleted = repository_collaborator::Entity::delete_by_id((repository.id, account.id))
            .exec(&transaction)
            .await?;
        if deleted.rows_affected == 0 {
            return Err(IdentityError::not_found());
        }
        gitadel_db::audit::record(
            &transaction,
            Some(actor.user.id),
            "repository.collaborator.remove",
            Some(format!("{namespace}/{name}/{username}")),
            audit,
        )
        .await?;
        transaction.commit().await?;
        Ok(())
    }
    pub async fn ensure_location_available(
        &self,
        namespace: &str,
        name: &str,
    ) -> Result<(), IdentityError> {
        let database = &self.database;
        if repository::Entity::find()
            .filter(repository::Column::Namespace.eq(namespace))
            .filter(repository::Column::Name.eq(name))
            .one(database)
            .await?
            .is_some()
            || repository_alias::Entity::find_by_id((namespace.to_owned(), name.to_owned()))
                .one(database)
                .await?
                .is_some()
        {
            return Err(IdentityError::conflict(
                "That repository location is already in use.",
            ));
        }
        Ok(())
    }

    pub async fn set_default_branch(
        &self,
        repository: &repository::Model,
        branch: &str,
    ) -> Result<(), IdentityError> {
        if !valid_branch_name(branch) {
            return Err(IdentityError::bad_request(
                "Default branch is not a valid branch name.",
            ));
        }
        let path = self.content.repository_path(repository);
        let reference = format!("refs/heads/{branch}");
        let reference_for_read = reference.clone();
        read_git(path, move |git| {
            if !git.reference_exists(&reference_for_read)? {
                return Err(sley::GitError::NotFound(sley::NotFoundKind::Reference {
                    name: reference_for_read,
                }));
            }
            git.set_head_symref(reference, sley::HeadUpdateOptions::new())
                .map_err(|error| sley::GitError::Transaction(error.to_string()))
        })
        .await
        .map_err(|_| IdentityError::bad_request("Default branch must already exist."))
    }

    pub async fn synchronize_default_head(
        &self,
        repository: &repository::Model,
    ) -> Result<(), IdentityError> {
        let Some(branch) = repository.default_branch.as_deref() else {
            return Ok(());
        };
        self.set_head_to_branch(repository, branch).await
    }

    pub async fn set_head_to_branch(
        &self,
        repository: &repository::Model,
        branch: &str,
    ) -> Result<(), IdentityError> {
        if !valid_branch_name(branch) {
            return Err(IdentityError::bad_request(
                "Default branch is not a valid branch name.",
            ));
        }
        let path = self.content.repository_path(repository);
        let reference = format!("refs/heads/{branch}");
        let reference_for_read = reference.clone();
        read_git(path, move |git| {
            if !git.reference_exists(&reference_for_read)? {
                return Ok(());
            }
            git.set_head_symref(reference, sley::HeadUpdateOptions::new())
                .map_err(|error| sley::GitError::Transaction(error.to_string()))
        })
        .await
    }
}
fn normalize_description(value: Option<String>) -> Result<Option<String>, IdentityError> {
    let value = value
        .map(|description| description.trim().to_owned())
        .filter(|description| !description.is_empty());
    if value
        .as_ref()
        .is_some_and(|description| description.len() > 512)
    {
        return Err(IdentityError::bad_request(
            "Repository descriptions must be at most 512 characters.",
        ));
    }
    Ok(value)
}

fn validate_visibility(value: String) -> Result<String, IdentityError> {
    match value.as_str() {
        "public" | "private" => Ok(value),
        _ => Err(IdentityError::bad_request(
            "Repository visibility must be public or private.",
        )),
    }
}

pub struct UpdateRepositoryControlRequest {
    pub description: Option<Option<String>>,
    pub visibility: Option<String>,
    pub default_branch: Option<String>,
    pub name: Option<String>,
    pub namespace: Option<String>,
}
pub struct CollaboratorResponse {
    pub username: String,
    pub role: String,
    pub created_at: chrono::DateTime<Utc>,
}
pub struct AddCollaboratorRequest {
    pub username: String,
    pub role: String,
}
pub fn validate_repository_name(value: &str) -> Result<String, IdentityError> {
    let name = value.trim();

    let valid_length = (1..=100).contains(&name.len());
    let valid_chars = name
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'));
    let lower_name = name.to_ascii_lowercase();
    let reserved =
        matches!(name, "." | "..") || name.contains("..") || lower_name.ends_with(".git");
    if RESERVED_REPOSITORY_NAMES.contains(&lower_name.as_str()) {
        return Err(IdentityError::bad_request(format!(
            "The repository name '{name}' is reserved for namespace management.",
        )));
    }
    if !valid_length || !valid_chars || reserved {
        return Err(IdentityError::bad_request(
            "Repository names may contain letters, numbers, periods, underscores, and hyphens.",
        ));
    }
    Ok(name.to_owned())
}

use crate::branches::valid_branch_name;

const RESERVED_REPOSITORY_NAMES: &[&str] = &[
    "integrations",
    "members",
    "mirror-credentials",
    "runners",
    "settings",
];

#[cfg(test)]
mod direct_boundaries_tests {
    use super::*;
    use gitadel_db::{entity::audit_event, test_support};

    #[tokio::test]
    async fn rename_keeps_storage_identity_resolves_alias_and_audits_atomically() {
        let database = test_support::database().await;
        let owner = test_support::owner(&database, "owner").await;
        let reader = test_support::owner(&database, "reader").await;
        let project = test_support::repository(&database, &owner, "before").await;
        let raw =
            gitadel_identity::create_session_on(&database, owner.id, chrono::Duration::hours(1))
                .await
                .unwrap();
        let principal = RepositoryPrincipal::session(raw);
        let service = LifecycleService::new(database.clone(), PathBuf::from("unused"));
        service
            .add_collaborator(
                &principal,
                project.id,
                AddCollaboratorRequest {
                    username: reader.username,
                    role: "write".to_owned(),
                },
                AuditContext::default(),
            )
            .await
            .unwrap();
        let (renamed, changed_head) = service
            .update_repository_control(
                &principal,
                project.id,
                UpdateRepositoryControlRequest {
                    name: Some("after".to_owned()),
                    namespace: None,
                    description: None,
                    visibility: None,
                    default_branch: None,
                },
                AuditContext {
                    remote_address: Some("192.0.2.1".parse().unwrap()),
                },
            )
            .await
            .unwrap();
        assert!(!changed_head);
        assert_eq!(renamed.storage_key, project.storage_key);
        assert_eq!(
            service.find("owner", "before").await.unwrap().id,
            project.id
        );
        assert_eq!(service.find("owner", "after").await.unwrap().id, project.id);
        assert!(
            repository_collaborator::Entity::find_by_id((project.id, reader.id))
                .one(&database)
                .await
                .unwrap()
                .is_none()
        );
        let audit = audit_event::Entity::find()
            .filter(audit_event::Column::Action.eq("repository.transfer"))
            .one(&database)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(audit.remote_address.as_deref(), Some("192.0.2.1"));
    }
}
