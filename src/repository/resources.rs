use std::{collections::HashSet, path::Path, process::Stdio};

use axum::{
    Json,
    extract::{Path as AxumPath, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use axum_extra::extract::cookie::CookieJar;
use chrono::{Duration, Utc};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, QueryOrder, Set, TransactionTrait,
    sea_query::Expr,
};
use serde::{Deserialize, Serialize};
use sley::{ObjectFormat, Repository as SleyRepository};
use tokio::{fs, process::Command};
use uuid::Uuid;

use super::{Permission, RepositoryState, validate_repository_name};
use crate::{
    entity::{
        instance, namespace, organization_member, repository, repository_alias,
        repository_collaborator, repository_favorite, user,
    },
    identity::{ApiError, SCOPE_READ, SCOPE_WRITE, validate_slug},
};

#[derive(Serialize)]
pub struct RepositoryResponse {
    id: Uuid,
    namespace: String,
    name: String,
    description: Option<String>,
    visibility: String,
    object_format: String,
    default_branch: String,
    archived_at: Option<chrono::DateTime<Utc>>,
    created_at: chrono::DateTime<Utc>,
    updated_at: chrono::DateTime<Utc>,
    favorited: bool,
    ssh_clone_url: String,
    can_manage: bool,
}

pub(super) struct AccessibleRepositories {
    pub(super) favorite_ids: HashSet<Uuid>,
    pub(super) manageable_ids: HashSet<Uuid>,
    pub(super) repositories: Vec<repository::Model>,
}

impl RepositoryResponse {
    pub(super) fn new(
        repository: repository::Model,
        state: &RepositoryState,
        favorited: bool,
        can_manage: bool,
    ) -> Self {
        let ssh_clone_url = state.ssh_clone_url(&repository);
        Self {
            id: repository.id,
            namespace: repository.namespace,
            name: repository.name,
            description: repository.description,
            visibility: repository.visibility,
            object_format: repository.object_format,
            default_branch: repository.default_branch,
            archived_at: repository.archived_at,
            created_at: repository.created_at,
            updated_at: repository.updated_at,
            favorited,
            ssh_clone_url,
            can_manage,
        }
    }
}

pub async fn list_repositories(
    State(state): State<RepositoryState>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<Vec<RepositoryResponse>>, ApiError> {
    let accessible = accessible_repositories(&state, &headers, &jar).await?;
    let response = accessible
        .repositories
        .into_iter()
        .map(|repository| {
            let favorited = accessible.favorite_ids.contains(&repository.id);
            let can_manage = accessible.manageable_ids.contains(&repository.id);
            RepositoryResponse::new(repository, &state, favorited, can_manage)
        })
        .collect();
    Ok(Json(response))
}

pub(super) async fn accessible_repositories(
    state: &RepositoryState,
    headers: &HeaderMap,
    jar: &CookieJar,
) -> Result<AccessibleRepositories, ApiError> {
    let user_id = state
        .identity()
        .optional_user(headers, jar, SCOPE_READ)
        .await?
        .map(|account| account.id);
    let favorite_ids = if let Some(user_id) = user_id {
        repository_favorite::Entity::find()
            .filter(repository_favorite::Column::UserId.eq(user_id))
            .all(state.identity().database())
            .await?
            .into_iter()
            .map(|favorite| favorite.repository_id)
            .collect()
    } else {
        HashSet::new()
    };
    let repositories = repository::Entity::find()
        .filter(repository::Column::DeletedAt.is_null())
        .order_by_desc(repository::Column::UpdatedAt)
        .all(state.identity().database())
        .await?;
    let mut accessible = Vec::with_capacity(repositories.len());
    let mut manageable_ids = HashSet::new();
    for repository in repositories {
        if state
            .can_access(&repository, user_id, Permission::Read)
            .await?
        {
            if state
                .can_access(&repository, user_id, Permission::Manage)
                .await?
            {
                manageable_ids.insert(repository.id);
            }
            accessible.push(repository);
        }
    }
    Ok(AccessibleRepositories {
        favorite_ids,
        manageable_ids,
        repositories: accessible,
    })
}

#[derive(Deserialize)]
pub struct CreateRepositoryRequest {
    namespace: String,
    name: String,
    description: Option<String>,
    visibility: Option<String>,
    object_format: Option<String>,
}

pub(super) struct CreateRepositoryOptions {
    pub(super) namespace: String,
    pub(super) name: String,
    pub(super) description: Option<String>,
    pub(super) visibility: Option<String>,
    pub(super) object_format: Option<String>,
}

pub async fn create_repository(
    State(state): State<RepositoryState>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<CreateRepositoryRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let actor = state
        .identity()
        .authenticate(&headers, &jar, SCOPE_WRITE)
        .await?;
    let repository = create_owned_repository(
        &state,
        actor.user.id,
        CreateRepositoryOptions {
            namespace: request.namespace,
            name: request.name,
            description: request.description,
            visibility: request.visibility,
            object_format: request.object_format,
        },
    )
    .await?;

    Ok((
        StatusCode::CREATED,
        Json(RepositoryResponse::new(repository, &state, false, true)),
    ))
}

pub(super) async fn create_owned_repository(
    state: &RepositoryState,
    actor_user_id: Uuid,
    options: CreateRepositoryOptions,
) -> Result<repository::Model, ApiError> {
    let namespace_slug = validate_slug(&options.namespace, "Namespace")?;
    let name = validate_repository_name(&options.name)?;
    let description = options
        .description
        .map(|description| description.trim().to_owned())
        .filter(|description| !description.is_empty());
    if description.as_ref().is_some_and(|value| value.len() > 512) {
        return Err(ApiError::bad_request(
            "Repository descriptions must be at most 512 characters.",
        ));
    }
    let visibility = if let Some(visibility) = options.visibility {
        visibility
    } else {
        instance::Entity::find_by_id(1)
            .one(state.identity().database())
            .await?
            .ok_or_else(|| ApiError::internal("instance settings row is missing"))?
            .default_repository_visibility
    };
    if visibility != "public" && visibility != "private" {
        return Err(ApiError::bad_request(
            "Repository visibility must be public or private.",
        ));
    }
    let (object_format, sley_format) = match options.object_format.as_deref().unwrap_or("sha1") {
        "sha1" => ("sha1", ObjectFormat::Sha1),
        "sha256" => ("sha256", ObjectFormat::Sha256),
        _ => {
            return Err(ApiError::bad_request(
                "Repository object format must be sha1 or sha256.",
            ));
        }
    };

    let owner = namespace::Entity::find_by_id(&namespace_slug)
        .one(state.identity().database())
        .await?
        .ok_or_else(ApiError::not_found)?;
    match owner.kind.as_str() {
        "user" if owner.user_id == Some(actor_user_id) => {}
        "organization" => {
            let organization_id = owner.organization_id.ok_or_else(ApiError::not_found)?;
            let membership =
                organization_member::Entity::find_by_id((organization_id, actor_user_id))
                    .one(state.identity().database())
                    .await?
                    .filter(|membership| membership.role == "owner")
                    .ok_or_else(ApiError::not_found)?;
            drop(membership);
        }
        _ => return Err(ApiError::not_found()),
    }

    ensure_location_available(state, &namespace_slug, &name).await?;

    let storage_key = Uuid::new_v4();
    let now = Utc::now();
    let repository = repository::ActiveModel {
        id: Set(Uuid::new_v4()),
        namespace: Set(namespace_slug.clone()),
        name: Set(name.clone()),
        description: Set(description),
        visibility: Set(visibility),
        object_format: Set(object_format.to_owned()),
        default_branch: Set("main".to_owned()),
        storage_key: Set(storage_key),
        created_by: Set(actor_user_id),
        archived_at: Set(None),
        deleted_at: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
    };
    let transaction = state.identity().database().begin().await?;
    let repository = repository.insert(&transaction).await?;
    let repository_path = state.repository_path(&repository);

    if let Err(error) = initialize_repository(&repository_path, sley_format).await {
        cleanup_repository(&repository_path).await;
        return Err(error);
    }
    if let Err(error) = state
        .identity()
        .audit_on(
            &transaction,
            Some(actor_user_id),
            "repository.create",
            Some(format!("{namespace_slug}/{name}")),
        )
        .await
    {
        cleanup_repository(&repository_path).await;
        return Err(error);
    }
    if let Err(error) = transaction.commit().await {
        cleanup_repository(&repository_path).await;
        return Err(error.into());
    }

    Ok(repository)
}

pub(super) async fn record_push(
    state: &RepositoryState,
    repository_id: Uuid,
    actor_user_id: Uuid,
    target: String,
) -> Result<(), ApiError> {
    let transaction = state.identity().database().begin().await?;
    repository::Entity::update_many()
        .col_expr(repository::Column::UpdatedAt, Expr::value(Utc::now()))
        .filter(repository::Column::Id.eq(repository_id))
        .exec(&transaction)
        .await?;
    state
        .identity()
        .audit_on(
            &transaction,
            Some(actor_user_id),
            "repository.push",
            Some(target),
        )
        .await?;
    transaction.commit().await?;
    Ok(())
}

pub async fn get_repository(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name)): AxumPath<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<RepositoryResponse>, ApiError> {
    let repository = state.find(&namespace, &name).await?;
    let user_id = state
        .identity()
        .optional_user(&headers, &jar, SCOPE_READ)
        .await?
        .map(|account| account.id);
    if repository.visibility != "public" && user_id.is_none() {
        return Err(ApiError::not_found());
    }
    state
        .authorize(&repository, user_id, Permission::Read)
        .await?;
    let favorited = if let Some(user_id) = user_id {
        repository_favorite::Entity::find_by_id((user_id, repository.id))
            .one(state.identity().database())
            .await?
            .is_some()
    } else {
        false
    };
    let can_manage = state
        .can_access(&repository, user_id, Permission::Manage)
        .await?;
    Ok(Json(RepositoryResponse::new(
        repository, &state, favorited, can_manage,
    )))
}

#[derive(Deserialize)]
pub struct UpdateRepositoryControlRequest {
    description: Option<Option<String>>,
    visibility: Option<String>,
    default_branch: Option<String>,
    name: Option<String>,
    namespace: Option<String>,
}

pub async fn update_repository_control(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name)): AxumPath<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<UpdateRepositoryControlRequest>,
) -> Result<Json<RepositoryResponse>, ApiError> {
    let actor = state
        .identity()
        .authenticate(&headers, &jar, SCOPE_WRITE)
        .await?;
    let repository = state.find(&namespace, &name).await?;
    state
        .authorize(&repository, Some(actor.user.id), Permission::Manage)
        .await?;
    if request.description.is_none()
        && request.visibility.is_none()
        && request.default_branch.is_none()
        && request.name.is_none()
        && request.namespace.is_none()
    {
        return Err(ApiError::bad_request(
            "Provide at least one repository control change.",
        ));
    }

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
        return Err(ApiError::bad_request(
            "Transfers must include a repository name.",
        ));
    }
    let moved = next_name.is_some() || next_namespace.is_some();
    let target_name = next_name.unwrap_or_else(|| repository.name.clone());
    let target_namespace = next_namespace.unwrap_or_else(|| repository.namespace.clone());
    if moved {
        ensure_owned_namespace(&state, actor.user.id, &target_namespace).await?;
        if target_namespace != repository.namespace || target_name != repository.name {
            ensure_location_available(&state, &target_namespace, &target_name).await?;
        }
    }

    if let Some(default_branch) = request.default_branch.as_deref() {
        set_default_branch(&state, &repository, default_branch).await?;
    }

    let transaction = state.identity().database().begin().await?;
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
        active.default_branch = Set(default_branch);
    }
    active.updated_at = Set(Utc::now());
    let updated = active.update(&transaction).await?;
    let action = if moved {
        "repository.transfer"
    } else {
        "repository.update"
    };
    state
        .identity()
        .audit_on(
            &transaction,
            Some(actor.user.id),
            action,
            Some(format!("{}/{}", updated.namespace, updated.name)),
        )
        .await?;
    transaction.commit().await?;
    Ok(Json(RepositoryResponse::new(updated, &state, false, true)))
}

pub async fn archive_repository(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name)): AxumPath<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<StatusCode, ApiError> {
    update_archive_state(&state, &headers, &jar, &namespace, &name, true).await
}

pub async fn unarchive_repository(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name)): AxumPath<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<StatusCode, ApiError> {
    update_archive_state(&state, &headers, &jar, &namespace, &name, false).await
}

pub async fn soft_delete_repository(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name)): AxumPath<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<StatusCode, ApiError> {
    let (actor, repository) =
        managed_repository(&state, &headers, &jar, &namespace, &name, false).await?;
    let transaction = state.identity().database().begin().await?;
    let mut active: repository::ActiveModel = repository.clone().into();
    active.deleted_at = Set(Some(Utc::now()));
    active.updated_at = Set(Utc::now());
    active.update(&transaction).await?;
    state
        .identity()
        .audit_on(
            &transaction,
            Some(actor.user.id),
            "repository.delete",
            Some(format!("{namespace}/{name}")),
        )
        .await?;
    transaction.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn restore_repository(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name)): AxumPath<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<StatusCode, ApiError> {
    let (actor, repository) =
        managed_repository(&state, &headers, &jar, &namespace, &name, true).await?;
    let deleted_at = repository
        .deleted_at
        .ok_or_else(|| ApiError::bad_request("This repository is not deleted."))?;
    if deleted_at < Utc::now() - Duration::days(30) {
        return Err(ApiError::bad_request(
            "This repository’s 30-day recovery period has ended.",
        ));
    }
    let transaction = state.identity().database().begin().await?;
    let mut active: repository::ActiveModel = repository.clone().into();
    active.deleted_at = Set(None);
    active.updated_at = Set(Utc::now());
    active.update(&transaction).await?;
    state
        .identity()
        .audit_on(
            &transaction,
            Some(actor.user.id),
            "repository.restore",
            Some(format!("{}/{}", repository.namespace, repository.name)),
        )
        .await?;
    transaction.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub struct PurgeRepositoryRequest {
    confirmation: String,
}

pub async fn purge_repository(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name)): AxumPath<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<PurgeRepositoryRequest>,
) -> Result<StatusCode, ApiError> {
    if request.confirmation != "purge" {
        return Err(ApiError::bad_request(
            "Confirm permanent deletion with confirmation: purge.",
        ));
    }
    let (actor, repository) =
        managed_repository(&state, &headers, &jar, &namespace, &name, true).await?;
    if repository.deleted_at.is_none() {
        return Err(ApiError::bad_request(
            "Soft-delete the repository before permanently purging it.",
        ));
    }
    let transaction = state.identity().database().begin().await?;
    state
        .identity()
        .audit_on(
            &transaction,
            Some(actor.user.id),
            "repository.purge",
            Some(format!("{}/{}", repository.namespace, repository.name)),
        )
        .await?;
    repository::Entity::delete_by_id(repository.id)
        .exec(&transaction)
        .await?;
    transaction.commit().await?;
    cleanup_repository(&state.repository_path(&repository)).await;
    Ok(StatusCode::NO_CONTENT)
}

async fn update_archive_state(
    state: &RepositoryState,
    headers: &HeaderMap,
    jar: &CookieJar,
    namespace: &str,
    name: &str,
    archived: bool,
) -> Result<StatusCode, ApiError> {
    let (actor, repository) =
        managed_repository(state, headers, jar, namespace, name, false).await?;
    let transaction = state.identity().database().begin().await?;
    let mut active: repository::ActiveModel = repository.into();
    active.archived_at = Set(archived.then(Utc::now));
    active.updated_at = Set(Utc::now());
    active.update(&transaction).await?;
    state
        .identity()
        .audit_on(
            &transaction,
            Some(actor.user.id),
            if archived {
                "repository.archive"
            } else {
                "repository.unarchive"
            },
            Some(format!("{namespace}/{name}")),
        )
        .await?;
    transaction.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn managed_repository(
    state: &RepositoryState,
    headers: &HeaderMap,
    jar: &CookieJar,
    namespace: &str,
    name: &str,
    include_deleted: bool,
) -> Result<(crate::identity::AuthenticatedUser, repository::Model), ApiError> {
    let actor = state
        .identity()
        .authenticate(headers, jar, SCOPE_WRITE)
        .await?;
    let repository = if include_deleted {
        state.find_including_deleted(namespace, name).await?
    } else {
        state.find(namespace, name).await?
    };
    if repository.deleted_at.is_some() && !include_deleted {
        return Err(ApiError::not_found());
    }
    let owner = namespace::Entity::find_by_id(&repository.namespace)
        .one(state.identity().database())
        .await?
        .ok_or_else(ApiError::not_found)?;
    let allowed = match owner.kind.as_str() {
        "user" => owner.user_id == Some(actor.user.id),
        "organization" => match owner.organization_id {
            Some(id) => organization_member::Entity::find_by_id((id, actor.user.id))
                .one(state.identity().database())
                .await?
                .is_some_and(|member| member.role == "owner"),
            None => false,
        },
        _ => false,
    };
    if !allowed {
        return Err(ApiError::not_found());
    }
    Ok((actor, repository))
}

async fn ensure_owned_namespace(
    state: &RepositoryState,
    actor_user_id: Uuid,
    slug: &str,
) -> Result<(), ApiError> {
    let owner = namespace::Entity::find_by_id(slug)
        .one(state.identity().database())
        .await?
        .ok_or_else(ApiError::not_found)?;
    match owner.kind.as_str() {
        "user" if owner.user_id == Some(actor_user_id) => Ok(()),
        "organization" => {
            let id = owner.organization_id.ok_or_else(ApiError::not_found)?;
            organization_member::Entity::find_by_id((id, actor_user_id))
                .one(state.identity().database())
                .await?
                .filter(|member| member.role == "owner")
                .map(|_| ())
                .ok_or_else(ApiError::not_found)
        }
        _ => Err(ApiError::not_found()),
    }
}

async fn ensure_location_available(
    state: &RepositoryState,
    namespace: &str,
    name: &str,
) -> Result<(), ApiError> {
    let database = state.identity().database();
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
        return Err(ApiError::conflict(
            "That repository location is already in use.",
        ));
    }
    Ok(())
}

fn normalize_description(value: Option<String>) -> Result<Option<String>, ApiError> {
    let value = value
        .map(|description| description.trim().to_owned())
        .filter(|description| !description.is_empty());
    if value
        .as_ref()
        .is_some_and(|description| description.len() > 512)
    {
        return Err(ApiError::bad_request(
            "Repository descriptions must be at most 512 characters.",
        ));
    }
    Ok(value)
}

fn validate_visibility(value: String) -> Result<String, ApiError> {
    match value.as_str() {
        "public" | "private" => Ok(value),
        _ => Err(ApiError::bad_request(
            "Repository visibility must be public or private.",
        )),
    }
}

async fn set_default_branch(
    state: &RepositoryState,
    repository: &repository::Model,
    branch: &str,
) -> Result<(), ApiError> {
    let path = state.repository_path(repository);
    run_git(&path, &["check-ref-format", "--branch", branch])
        .await
        .map_err(|_| ApiError::bad_request("Default branch is not a valid branch name."))?;
    let reference = format!("refs/heads/{branch}");
    run_git(&path, &["show-ref", "--verify", "--quiet", &reference])
        .await
        .map_err(|_| ApiError::bad_request("Default branch must already exist."))?;
    run_git(&path, &["symbolic-ref", "HEAD", &reference]).await
}

pub async fn favorite_repository(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name)): AxumPath<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<StatusCode, ApiError> {
    let (actor, repository) = state
        .authenticated_repository(
            &headers,
            &jar,
            &namespace,
            &name,
            Permission::Read,
            SCOPE_WRITE,
        )
        .await?;
    if repository_favorite::Entity::find_by_id((actor.user.id, repository.id))
        .one(state.identity().database())
        .await?
        .is_none()
    {
        repository_favorite::ActiveModel {
            user_id: Set(actor.user.id),
            repository_id: Set(repository.id),
            created_at: Set(Utc::now()),
        }
        .insert(state.identity().database())
        .await?;
    }
    Ok(StatusCode::NO_CONTENT)
}

pub async fn unfavorite_repository(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name)): AxumPath<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<StatusCode, ApiError> {
    let (actor, repository) = state
        .authenticated_repository(
            &headers,
            &jar,
            &namespace,
            &name,
            Permission::Read,
            SCOPE_WRITE,
        )
        .await?;
    repository_favorite::Entity::delete_by_id((actor.user.id, repository.id))
        .exec(state.identity().database())
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize)]
pub struct CollaboratorResponse {
    username: String,
    role: String,
    created_at: chrono::DateTime<Utc>,
}

pub async fn list_collaborators(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name)): AxumPath<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<Vec<CollaboratorResponse>>, ApiError> {
    let (actor, repository) = state
        .authenticated_repository(
            &headers,
            &jar,
            &namespace,
            &name,
            Permission::Manage,
            SCOPE_READ,
        )
        .await?;
    state.personal_owner(&repository, &actor.user).await?;
    let collaborators = repository_collaborator::Entity::find()
        .filter(repository_collaborator::Column::RepositoryId.eq(repository.id))
        .order_by_asc(repository_collaborator::Column::CreatedAt)
        .all(state.identity().database())
        .await?;
    let mut response = Vec::with_capacity(collaborators.len());
    for collaborator in collaborators {
        if let Some(account) = user::Entity::find_by_id(collaborator.user_id)
            .one(state.identity().database())
            .await?
        {
            response.push(CollaboratorResponse {
                username: account.username,
                role: collaborator.role,
                created_at: collaborator.created_at,
            });
        }
    }
    Ok(Json(response))
}

#[derive(Deserialize)]
pub struct AddCollaboratorRequest {
    username: String,
    role: String,
}

pub async fn add_collaborator(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name)): AxumPath<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<AddCollaboratorRequest>,
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
    state.personal_owner(&repository, &actor.user).await?;
    if request.role != "read" && request.role != "write" {
        return Err(ApiError::bad_request(
            "Collaborator role must be read or write.",
        ));
    }
    let username = validate_slug(&request.username, "Username")?;
    let account = user::Entity::find()
        .filter(user::Column::Username.eq(&username))
        .filter(user::Column::DisabledAt.is_null())
        .one(state.identity().database())
        .await?
        .ok_or_else(ApiError::not_found)?;
    if account.id == actor.user.id {
        return Err(ApiError::bad_request(
            "The repository owner is not a collaborator.",
        ));
    }
    if repository_collaborator::Entity::find_by_id((repository.id, account.id))
        .one(state.identity().database())
        .await?
        .is_some()
    {
        return Err(ApiError::conflict(
            "That user is already a repository collaborator.",
        ));
    }

    let transaction = state.identity().database().begin().await?;
    let collaborator = repository_collaborator::ActiveModel {
        repository_id: Set(repository.id),
        user_id: Set(account.id),
        role: Set(request.role),
        created_at: Set(Utc::now()),
    }
    .insert(&transaction)
    .await?;
    state
        .identity()
        .audit_on(
            &transaction,
            Some(actor.user.id),
            "repository.collaborator.add",
            Some(format!("{namespace}/{name}/{username}")),
        )
        .await?;
    transaction.commit().await?;

    Ok((
        StatusCode::CREATED,
        Json(CollaboratorResponse {
            username,
            role: collaborator.role,
            created_at: collaborator.created_at,
        }),
    ))
}

pub async fn remove_collaborator(
    State(state): State<RepositoryState>,
    AxumPath((namespace, name, username)): AxumPath<(String, String, String)>,
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
    state.personal_owner(&repository, &actor.user).await?;
    let account = user::Entity::find()
        .filter(user::Column::Username.eq(&username))
        .one(state.identity().database())
        .await?
        .ok_or_else(ApiError::not_found)?;
    let transaction = state.identity().database().begin().await?;
    let deleted = repository_collaborator::Entity::delete_by_id((repository.id, account.id))
        .exec(&transaction)
        .await?;
    if deleted.rows_affected == 0 {
        return Err(ApiError::not_found());
    }
    state
        .identity()
        .audit_on(
            &transaction,
            Some(actor.user.id),
            "repository.collaborator.remove",
            Some(format!("{namespace}/{name}/{username}")),
        )
        .await?;
    transaction.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn initialize_repository(path: &Path, object_format: ObjectFormat) -> Result<(), ApiError> {
    let owned_path = path.to_owned();
    tokio::task::spawn_blocking(move || {
        SleyRepository::init_with_format(&owned_path, object_format, true)
            .map(|_| ())
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(ApiError::internal)?
    .map_err(ApiError::internal)?;

    run_git(path, &["symbolic-ref", "HEAD", "refs/heads/main"]).await?;
    Ok(())
}

async fn run_git(path: &Path, arguments: &[&str]) -> Result<(), ApiError> {
    let output = Command::new("git")
        .arg("--git-dir")
        .arg(path)
        .args(arguments)
        .stdin(Stdio::null())
        .output()
        .await
        .map_err(ApiError::internal)?;
    if output.status.success() {
        Ok(())
    } else {
        Err(ApiError::internal(String::from_utf8_lossy(&output.stderr)))
    }
}

async fn cleanup_repository(path: &Path) {
    if let Err(error) = fs::remove_dir_all(path).await
        && error.kind() != std::io::ErrorKind::NotFound
    {
        tracing::error!(%error, path = %path.display(), "could not clean up repository directory");
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::{
        config::{DatabaseSettings, Settings, StorageSettings},
        database::connect_and_migrate,
        identity::{IdentityState, bootstrap_admin},
    };

    #[tokio::test]
    async fn record_push_should_move_repository_to_front_of_updated_order() {
        let test_root = std::env::temp_dir().join(format!("gitadel-push-order-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(&test_root).await.unwrap();
        let database = connect_and_migrate(&DatabaseSettings {
            url: format!(
                "sqlite://{}?mode=rwc",
                test_root.join("gitadel.db").display()
            ),
        })
        .await
        .unwrap();
        let account = bootstrap_admin(&database, "archivist", "test-password".to_owned())
            .await
            .unwrap();
        let settings = Settings::default();
        let public_url = settings.server.public_url;
        let identity = IdentityState::new(database, settings.auth, public_url.clone()).unwrap();
        let state = RepositoryState::new(
            identity,
            StorageSettings {
                repository_root: test_root.join("repositories"),
                lfs_root: test_root.join("lfs"),
            },
            public_url,
            2222,
        )
        .await
        .unwrap();
        let first = create_owned_repository(
            &state,
            account.id,
            CreateRepositoryOptions {
                namespace: account.username.clone(),
                name: "first".to_owned(),
                description: None,
                visibility: None,
                object_format: None,
            },
        )
        .await
        .unwrap();
        tokio::time::sleep(Duration::from_millis(2)).await;
        create_owned_repository(
            &state,
            account.id,
            CreateRepositoryOptions {
                namespace: account.username,
                name: "second".to_owned(),
                description: None,
                visibility: None,
                object_format: None,
            },
        )
        .await
        .unwrap();
        tokio::time::sleep(Duration::from_millis(2)).await;

        record_push(&state, first.id, account.id, "archivist/first".to_owned())
            .await
            .unwrap();
        let repositories = repository::Entity::find()
            .order_by_desc(repository::Column::UpdatedAt)
            .all(state.identity().database())
            .await
            .unwrap();
        let actual = repositories[0].id;
        drop(state);
        tokio::fs::remove_dir_all(test_root).await.unwrap();

        assert_eq!(actual, first.id);
    }
}
