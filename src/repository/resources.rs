use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

use super::{Permission, RepositoryState, browser::read_git, mirrors, validate_repository_name};
use crate::{
    blob_store::{ObjectPrefix, targets},
    entity::{
        lfs_object, namespace, organization_member, repository, repository_alias,
        repository_collaborator, repository_favorite, repository_topic, topic, user,
    },
    identity::{ApiError, SCOPE_READ, SCOPE_WRITE, validate_slug},
};
use axum::{
    Json,
    extract::{Path as AxumPath, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use axum_extra::extract::cookie::CookieJar;
use chrono::{Duration, Utc};
use sea_orm::sea_query::OnConflict;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DbBackend, EntityTrait, IntoActiveModel,
    QueryFilter, QueryOrder, Set, Statement, TransactionTrait,
};
use serde::{Deserialize, Serialize};
use sley::{FullName, ObjectFormat, Repository as SleyRepository};
use tokio::fs;
use uuid::Uuid;

#[derive(Serialize)]
pub struct RepositoryResponse {
    id: Uuid,
    namespace: String,
    name: String,
    description: Option<String>,
    website_url: Option<String>,
    visibility: String,
    topics: Vec<String>,
    object_format: String,
    mirrored: bool,
    default_branch: Option<String>,
    archived_at: Option<chrono::DateTime<Utc>>,
    icon_updated_at: Option<chrono::DateTime<Utc>>,
    icon_source: Option<String>,
    created_at: chrono::DateTime<Utc>,
    updated_at: chrono::DateTime<Utc>,
    favorited: bool,
    ssh_clone_url: String,
    can_manage: bool,
    can_write: bool,
}

pub(super) struct AccessibleRepositories {
    pub(super) favorite_ids: HashSet<Uuid>,
    pub(super) manageable_ids: HashSet<Uuid>,
    pub(super) writable_ids: HashSet<Uuid>,
    pub(super) repositories: Vec<repository::Model>,
}

impl RepositoryResponse {
    pub(super) fn new(
        repository: repository::Model,
        state: &RepositoryState,
        favorited: bool,
        can_manage: bool,
        can_write: bool,
    ) -> Self {
        let ssh_clone_url = state.ssh_clone_url(&repository);
        Self {
            id: repository.id,
            namespace: repository.namespace,
            name: repository.name,
            description: repository.description,
            website_url: repository.website_url,
            visibility: repository.visibility,
            topics: Vec::new(),
            object_format: repository.object_format,
            mirrored: repository.mirrored,
            default_branch: repository.default_branch,
            archived_at: repository.archived_at,
            icon_updated_at: repository.icon_updated_at,
            icon_source: repository.icon_source,
            created_at: repository.created_at,
            updated_at: repository.updated_at,
            favorited,
            can_write,
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
    let repository_ids = accessible
        .repositories
        .iter()
        .map(|repository| repository.id)
        .collect::<Vec<_>>();
    let mut topics =
        repository_topics_by_repository(state.identity().database(), &repository_ids).await?;
    let response = accessible
        .repositories
        .into_iter()
        .map(|repository| {
            let favorited = accessible.favorite_ids.contains(&repository.id);
            let can_manage = accessible.manageable_ids.contains(&repository.id);
            let can_write = accessible.writable_ids.contains(&repository.id);
            let mut response =
                RepositoryResponse::new(repository, &state, favorited, can_manage, can_write);
            response.topics = topics.remove(&response.id).unwrap_or_default();
            response
        })
        .collect();
    Ok(Json(response))
}

async fn repository_topics_by_repository(
    database: &sea_orm::DatabaseConnection,
    repository_ids: &[Uuid],
) -> Result<HashMap<Uuid, Vec<String>>, ApiError> {
    if repository_ids.is_empty() {
        return Ok(HashMap::new());
    }

    let links = repository_topic::Entity::find()
        .filter(repository_topic::Column::RepositoryId.is_in(repository_ids.iter().copied()))
        .all(database);
    let topics = topic::Entity::find().all(database);
    let (links, topics) = tokio::try_join!(links, topics)?;
    let topic_names = topics
        .into_iter()
        .map(|topic| (topic.id, topic.name))
        .collect::<HashMap<_, _>>();
    let mut by_repository = HashMap::<Uuid, Vec<String>>::new();
    for link in links {
        if let Some(name) = topic_names.get(&link.topic_id) {
            by_repository
                .entry(link.repository_id)
                .or_default()
                .push(name.clone());
        }
    }
    for topics in by_repository.values_mut() {
        topics.sort_unstable();
    }
    Ok(by_repository)
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
    let has_write_scope = if headers.contains_key(axum::http::header::AUTHORIZATION) {
        state
            .identity()
            .authenticate(headers, jar, SCOPE_WRITE)
            .await
            .is_ok()
    } else {
        true
    };
    let database = state.identity().database();

    let Some(user_id) = user_id else {
        let repositories = repository::Entity::find()
            .filter(repository::Column::DeletedAt.is_null())
            .filter(repository::Column::Visibility.eq("public"))
            .order_by_desc(repository::Column::UpdatedAt)
            .all(database)
            .await?;
        return Ok(AccessibleRepositories {
            favorite_ids: HashSet::new(),
            manageable_ids: HashSet::new(),
            writable_ids: HashSet::new(),
            repositories,
        });
    };

    let favorites = repository_favorite::Entity::find()
        .filter(repository_favorite::Column::UserId.eq(user_id))
        .all(database);
    let namespaces = namespace::Entity::find().all(database);
    let memberships = organization_member::Entity::find()
        .filter(organization_member::Column::UserId.eq(user_id))
        .all(database);
    let collaborators = repository_collaborator::Entity::find()
        .filter(repository_collaborator::Column::UserId.eq(user_id))
        .all(database);
    let repositories = repository::Entity::find()
        .filter(repository::Column::DeletedAt.is_null())
        .order_by_desc(repository::Column::UpdatedAt)
        .all(database);
    let (favorites, namespaces, memberships, collaborators, repositories) = tokio::try_join!(
        favorites,
        namespaces,
        memberships,
        collaborators,
        repositories
    )?;

    let favorite_ids = favorites
        .into_iter()
        .map(|favorite| favorite.repository_id)
        .collect();
    let organization_roles: HashMap<Uuid, String> = memberships
        .into_iter()
        .map(|membership| (membership.organization_id, membership.role))
        .collect();
    let mut collaborator_ids = HashSet::new();
    let mut writable_collaborator_ids = HashSet::new();
    for collaborator in collaborators {
        collaborator_ids.insert(collaborator.repository_id);
        if collaborator.role == "write" {
            writable_collaborator_ids.insert(collaborator.repository_id);
        }
    }
    let mut readable_namespaces = HashSet::new();
    let mut writable_namespaces = HashSet::new();
    let mut manageable_namespaces = HashSet::new();
    for namespace in namespaces {
        let personally_owned = namespace.user_id == Some(user_id);
        let organization_role = namespace
            .organization_id
            .and_then(|id| organization_roles.get(&id));
        if personally_owned || organization_role.is_some() {
            readable_namespaces.insert(namespace.slug.clone());
        }
        if personally_owned || organization_role.is_some() {
            writable_namespaces.insert(namespace.slug.clone());
        }
        if personally_owned || organization_role.is_some_and(|role| role == "owner") {
            manageable_namespaces.insert(namespace.slug);
        }
    }

    let mut accessible = Vec::with_capacity(repositories.len());
    let mut manageable_ids = HashSet::new();
    let mut writable_ids = HashSet::new();
    for repository in repositories {
        let can_manage = manageable_namespaces.contains(&repository.namespace);
        let can_read = repository.visibility == "public"
            || readable_namespaces.contains(&repository.namespace)
            || collaborator_ids.contains(&repository.id);
        if !can_read {
            continue;
        }
        if can_manage {
            manageable_ids.insert(repository.id);
        }
        if has_write_scope
            && !repository.mirrored
            && repository.archived_at.is_none()
            && (writable_namespaces.contains(&repository.namespace)
                || writable_collaborator_ids.contains(&repository.id))
        {
            writable_ids.insert(repository.id);
        }
        accessible.push(repository);
    }

    Ok(AccessibleRepositories {
        favorite_ids,
        manageable_ids,
        writable_ids,
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
    mirror: Option<mirrors::CreateMirrorRequest>,
}

pub(super) struct CreateRepositoryOptions {
    pub(super) namespace: String,
    pub(super) name: String,
    pub(super) description: Option<String>,
    pub(super) visibility: Option<String>,
    pub(super) object_format: Option<String>,
    pub(super) mirror: Option<mirrors::CreateMirrorRequest>,
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
            mirror: request.mirror,
        },
    )
    .await?;

    let can_write = !repository.mirrored && repository.archived_at.is_none();
    Ok((
        StatusCode::CREATED,
        Json(RepositoryResponse::new(
            repository, &state, false, true, can_write,
        )),
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
        user::Entity::find_by_id(actor_user_id)
            .one(state.identity().database())
            .await?
            .ok_or_else(|| ApiError::internal("repository owner account is missing"))?
            .default_repository_visibility
    };
    if visibility != "public" && visibility != "private" {
        return Err(ApiError::bad_request(
            "Repository visibility must be public or private.",
        ));
    }
    let mirrored = options.mirror.is_some();
    let (object_format, sley_format) = if mirrored {
        ("sha1", ObjectFormat::Sha1)
    } else {
        match options.object_format.as_deref().unwrap_or("sha1") {
            "sha1" => ("sha1", ObjectFormat::Sha1),
            "sha256" => ("sha256", ObjectFormat::Sha256),
            _ => {
                return Err(ApiError::bad_request(
                    "Repository object format must be sha1 or sha256.",
                ));
            }
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
    let mirror = match options.mirror {
        Some(request) => {
            Some(mirrors::PreparedMirror::prepare(state, &namespace_slug, request).await?)
        }
        None => None,
    };

    ensure_location_available(state, &namespace_slug, &name).await?;

    let storage_key = Uuid::new_v4();
    let now = Utc::now();
    let repository = repository::Model {
        id: Uuid::new_v4(),
        namespace: namespace_slug.clone(),
        name: name.clone(),
        description,
        website_url: None,
        visibility,
        object_format: object_format.to_owned(),
        mirrored,
        default_branch: None,
        issue_counter: 0,
        storage_key,
        created_by: actor_user_id,
        archived_at: None,
        deleted_at: None,
        icon_updated_at: None,
        icon_source: None,
        created_at: now,
        updated_at: now,
    };
    let repository_path = state.repository_path(&repository);
    let mirror_initialization = if let Some(mirror) = mirror.as_ref() {
        match mirrors::initialize(state, &repository_path, mirror).await {
            Ok(initialized) => Some(initialized),
            Err(error) => {
                cleanup_repository(&repository_path).await;
                return Err(error);
            }
        }
    } else {
        if let Err(error) = initialize_repository(&repository_path, sley_format).await {
            cleanup_repository(&repository_path).await;
            return Err(error);
        }
        None
    };

    let transaction = match state.identity().database().begin().await {
        Ok(transaction) => transaction,
        Err(error) => {
            cleanup_repository(&repository_path).await;
            return Err(error.into());
        }
    };
    let repository = match repository.into_active_model().insert(&transaction).await {
        Ok(repository) => repository,
        Err(error) => {
            cleanup_repository(&repository_path).await;
            return Err(error.into());
        }
    };
    let repository = if let Some(mirror) = mirror {
        let Some(initialized) = mirror_initialization else {
            cleanup_repository(&repository_path).await;
            return Err(ApiError::internal(
                "mirror initialization result is missing",
            ));
        };
        let mut active = repository.into_active_model();
        active.object_format = Set(initialized.object_format);
        active.default_branch = Set(initialized.default_branch);
        let repository = match active.update(&transaction).await {
            Ok(repository) => repository,
            Err(error) => {
                cleanup_repository(&repository_path).await;
                return Err(error.into());
            }
        };
        if let Err(error) =
            mirrors::insert(&transaction, repository.id, &repository.namespace, mirror).await
        {
            cleanup_repository(&repository_path).await;
            return Err(error);
        }
        repository
    } else {
        repository
    };
    if let Err(error) = state
        .identity()
        .audit_on(
            &transaction,
            Some(actor_user_id),
            if mirrored {
                "repository.mirror.create"
            } else {
                "repository.create"
            },
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
    if mirrored {
        match mirrors::import_initial_metadata(state, &repository).await {
            Ok(repository) => return Ok(repository),
            Err(error) => {
                tracing::warn!(
                    %error,
                    repository_id = %repository.id,
                    "initial GitHub mirror metadata import failed"
                );
            }
        }
    }

    Ok(repository)
}

pub(super) struct ImportRepositoryOptions {
    pub(super) namespace: String,
    pub(super) name: String,
    pub(super) description: Option<String>,
    pub(super) source_url: String,
    pub(super) source_instance_url: Option<String>,
    pub(super) visibility: String,
    pub(super) remote_url: String,
    pub(super) authentication: mirrors::ImportAuthentication,
}

pub(super) async fn create_imported_repository(
    state: &RepositoryState,
    actor_user_id: Uuid,
    options: ImportRepositoryOptions,
) -> Result<repository::Model, ApiError> {
    let namespace = validate_slug(&options.namespace, "Namespace")?;
    let name = validate_repository_name(&options.name)?;
    let description = options
        .description
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    if description.as_ref().is_some_and(|value| value.len() > 512) {
        return Err(ApiError::bad_request(
            "Repository descriptions must be at most 512 characters.",
        ));
    }
    if options.visibility != "public" && options.visibility != "private" {
        return Err(ApiError::bad_request(
            "Repository visibility must be public or private.",
        ));
    }

    let owner = namespace::Entity::find_by_id(&namespace)
        .one(state.identity().database())
        .await?
        .ok_or_else(ApiError::not_found)?;
    match owner.kind.as_str() {
        "user" if owner.user_id == Some(actor_user_id) => {}
        "organization" => {
            let organization_id = owner.organization_id.ok_or_else(ApiError::not_found)?;
            organization_member::Entity::find_by_id((organization_id, actor_user_id))
                .one(state.identity().database())
                .await?
                .filter(|membership| membership.role == "owner")
                .ok_or_else(ApiError::not_found)?;
        }
        _ => return Err(ApiError::not_found()),
    }
    ensure_location_available(state, &namespace, &name).await?;

    let now = Utc::now();
    let mut repository = repository::Model {
        id: Uuid::new_v4(),
        namespace: namespace.clone(),
        name: name.clone(),
        description,
        website_url: Some(options.source_url),
        visibility: options.visibility,
        object_format: "sha1".to_owned(),
        mirrored: false,
        default_branch: None,
        issue_counter: 0,
        storage_key: Uuid::new_v4(),
        created_by: actor_user_id,
        archived_at: None,
        deleted_at: None,
        icon_updated_at: None,
        icon_source: None,
        created_at: now,
        updated_at: now,
    };
    let path = state.repository_path(&repository);
    let lfs_path = state.lfs_repository_path(&repository);
    let storage_key = repository.storage_key;
    let initialized = match mirrors::initialize_import(
        state,
        &path,
        repository.storage_key,
        options.source_instance_url.as_deref(),
        &options.remote_url,
        &options.authentication,
    )
    .await
    {
        Ok(initialized) => initialized,
        Err(error) => {
            cleanup_lfs_blobs(state, storage_key).await;
            cleanup_repository_storage(&path, &lfs_path).await;
            return Err(error);
        }
    };
    repository.object_format = initialized.object_format;
    repository.default_branch = initialized.default_branch;

    // Keep target metadata consistent with the live store until the import commits.
    // Acquire before the transaction so cutover cannot wait on our database lock.
    let operation_guard = state.lfs_operation_guard().await;
    let committed: Result<repository::Model, ApiError> = async {
        let transaction = state.identity().database().begin().await?;
        let repository = repository.into_active_model().insert(&transaction).await?;
        catalog_repository_lfs(state, &transaction, &repository).await?;
        state
            .identity()
            .audit_on(
                &transaction,
                Some(actor_user_id),
                "repository.import.create",
                Some(format!("{namespace}/{name}")),
            )
            .await?;
        transaction.commit().await?;
        Ok(repository)
    }
    .await;
    drop(operation_guard);
    let repository = match committed {
        Ok(repository) => repository,
        Err(error) => {
            cleanup_lfs_blobs(state, storage_key).await;
            cleanup_repository_storage(&path, &lfs_path).await;
            return Err(error);
        }
    };
    state.queue_repository_analysis(repository.id).await;
    Ok(repository)
}

pub(super) async fn record_push(
    state: &RepositoryState,
    repository_id: Uuid,
    actor_user_id: Uuid,
    target: String,
) -> Result<(), ApiError> {
    state.invalidate_repository_size(repository_id).await;
    let repository = repository::Entity::find_by_id(repository_id)
        .one(state.identity().database())
        .await?
        .ok_or_else(ApiError::not_found)?;
    let candidate = if repository.default_branch.is_none() {
        let path = state.repository_path(&repository);
        read_git(path, |git| select_default_branch(git, None)).await?
    } else {
        None
    };

    let transaction = state.identity().database().begin().await?;
    if let Some(branch) = candidate {
        let updated = repository::Entity::update_many()
            .col_expr(
                repository::Column::DefaultBranch,
                sea_orm::sea_query::Expr::value(branch.clone()),
            )
            .col_expr(
                repository::Column::UpdatedAt,
                sea_orm::sea_query::Expr::value(Utc::now()),
            )
            .filter(repository::Column::Id.eq(repository_id))
            .filter(repository::Column::DefaultBranch.is_null())
            .exec(&transaction)
            .await?;
        if updated.rows_affected > 0 {
            // Keep the transaction open while changing HEAD: a competing
            // first push cannot observe the new metadata until this succeeds.
            set_head_to_branch(state, &repository, &branch).await?;
        }
    } else {
        repository::Entity::update_many()
            .col_expr(
                repository::Column::UpdatedAt,
                sea_orm::sea_query::Expr::value(Utc::now()),
            )
            .filter(repository::Column::Id.eq(repository_id))
            .exec(&transaction)
            .await?;
    }
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

    // A losing first-push race, or a previous filesystem failure, must not
    // overwrite the winner's metadata. Re-read and repair HEAD from it.
    let current = repository::Entity::find_by_id(repository_id)
        .one(state.identity().database())
        .await?
        .ok_or_else(ApiError::not_found)?;
    synchronize_default_head(state, &current).await?;
    state.queue_repository_analysis(repository_id).await;
    Ok(())
}

/// Select and persist a repository's first branch, or repair its bare HEAD.
/// The conditional update makes concurrent first pushes choose one winner.
pub(super) async fn ensure_default_branch(
    state: &RepositoryState,
    repository: &repository::Model,
) -> Result<repository::Model, ApiError> {
    if repository.default_branch.is_none() {
        let path = state.repository_path(repository);
        let candidate = read_git(path, |git| select_default_branch(git, None)).await?;
        if let Some(branch) = candidate {
            let transaction = state.identity().database().begin().await?;
            let updated = repository::Entity::update_many()
                .col_expr(
                    repository::Column::DefaultBranch,
                    sea_orm::sea_query::Expr::value(branch.clone()),
                )
                .filter(repository::Column::Id.eq(repository.id))
                .filter(repository::Column::DefaultBranch.is_null())
                .exec(&transaction)
                .await?;
            if updated.rows_affected > 0 {
                set_head_to_branch(state, repository, &branch).await?;
            }
            transaction.commit().await?;
        }
    }
    let current = repository::Entity::find_by_id(repository.id)
        .one(state.identity().database())
        .await?
        .ok_or_else(ApiError::not_found)?;
    synchronize_default_head(state, &current).await?;
    Ok(current)
}

/// One-time repair for repositories created before an unresolved default
/// branch was representable. Only rows marked by the migration are examined.
pub(super) async fn backfill_legacy_empty_defaults(
    state: &RepositoryState,
) -> Result<(), ApiError> {
    let database = state.identity().database();
    let backend = database.get_database_backend();
    let rows = database
        .query_all_raw(Statement::from_string(
            backend,
            "SELECT repository_id FROM repository_default_branch_backfills",
        ))
        .await?;
    for row in rows {
        let repository_id: Uuid = row.try_get("", "repository_id")?;
        let Some(repository) = repository::Entity::find_by_id(repository_id)
            .one(database)
            .await?
        else {
            continue;
        };
        let path = state.repository_path(&repository);
        let has_branches = read_git(path, |git| {
            Ok(!git
                .references()
                .list_refs_with_prefix("refs/heads/")?
                .is_empty())
        })
        .await?;
        let transaction = database.begin().await?;
        let marker = if backend == DbBackend::Postgres {
            "$1"
        } else {
            "?"
        };
        if !has_branches {
            let mut active = repository.into_active_model();
            active.default_branch = Set(None);
            active.updated_at = Set(Utc::now());
            active.update(&transaction).await?;
        }
        transaction
            .execute_raw(Statement::from_sql_and_values(
                backend,
                &format!(
                    "DELETE FROM repository_default_branch_backfills WHERE repository_id = {marker}"
                ),
                vec![repository_id.into()],
            ))
            .await?;
        transaction.commit().await?;
    }
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
    let has_write_scope = if headers.contains_key(axum::http::header::AUTHORIZATION) {
        state
            .identity()
            .authenticate(&headers, &jar, SCOPE_WRITE)
            .await
            .is_ok()
    } else {
        true
    };
    let can_write = has_write_scope
        && !repository.mirrored
        && repository.archived_at.is_none()
        && state
            .can_access(&repository, user_id, Permission::Write)
            .await?;
    Ok(Json(RepositoryResponse::new(
        repository, &state, favorited, can_manage, can_write,
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
        active.default_branch = Set(Some(default_branch));
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
    if default_branch_changed {
        state.queue_repository_analysis(updated.id).await;
    }
    let can_write = !updated.mirrored && updated.archived_at.is_none();
    Ok(Json(RepositoryResponse::new(
        updated, &state, false, true, can_write,
    )))
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
    let _registry_operation = state.registry_storage().lock_operation().await;
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
    cleanup_lfs_blobs(&state, repository.storage_key).await;
    let registry_prefix = ObjectPrefix::new(format!("registry/{}", repository.storage_key))
        .map_err(ApiError::internal)?;
    if let Err(error) = targets::delete_prefix_from_all(
        state.identity().database(),
        state.local_lfs_root().to_path_buf(),
        &registry_prefix,
    )
    .await
    {
        tracing::error!(%error, "could not clean repository registry objects from every target");
    }
    cleanup_repository_storage(
        &state.repository_path(&repository),
        &state.lfs_repository_path(&repository),
    )
    .await;
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

pub(super) async fn ensure_owned_namespace(
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

fn valid_branch_name(branch: &str) -> bool {
    !branch.is_empty()
        && branch != "HEAD"
        && !branch.starts_with('-')
        && FullName::new(format!("refs/heads/{branch}")).is_ok()
}

async fn set_default_branch(
    state: &RepositoryState,
    repository: &repository::Model,
    branch: &str,
) -> Result<(), ApiError> {
    if !valid_branch_name(branch) {
        return Err(ApiError::bad_request(
            "Default branch is not a valid branch name.",
        ));
    }
    let path = state.repository_path(repository);
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
    .map_err(|_| ApiError::bad_request("Default branch must already exist."))
}
pub(super) async fn synchronize_default_head(
    state: &RepositoryState,
    repository: &repository::Model,
) -> Result<(), ApiError> {
    let Some(branch) = repository.default_branch.as_deref() else {
        return Ok(());
    };
    set_head_to_branch(state, repository, branch).await
}

async fn set_head_to_branch(
    state: &RepositoryState,
    repository: &repository::Model,
    branch: &str,
) -> Result<(), ApiError> {
    if !valid_branch_name(branch) {
        return Err(ApiError::bad_request(
            "Default branch is not a valid branch name.",
        ));
    }
    let path = state.repository_path(repository);
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
    .map_err(ApiError::from)
}

fn select_default_branch(
    repository: &SleyRepository,
    configured: Option<&str>,
) -> sley::Result<Option<String>> {
    let mut branches = Vec::new();
    for reference in repository
        .references()
        .list_refs_with_prefix("refs/heads/")?
    {
        let Some(name) = reference.name.strip_prefix("refs/heads/") else {
            continue;
        };
        if !valid_branch_name(name) {
            continue;
        }
        let Ok(oid) = repository.rev_parse(&reference.name) else {
            continue;
        };
        let Ok(oid) = repository.peel_to_commit_oid(oid) else {
            continue;
        };
        let Ok(commit) = repository.read_commit(&oid) else {
            continue;
        };
        let commit_time = commit
            .committer_signature()
            .map_or(i64::MIN, |signature| signature.time.seconds);
        branches.push((name.to_owned(), commit_time));
    }

    if let Some(configured) = configured
        .filter(|configured| valid_branch_name(configured))
        .filter(|configured| branches.iter().any(|(name, _)| name == configured))
    {
        return Ok(Some(configured.to_owned()));
    }
    for preferred in ["main", "master", "prod", "staging"] {
        if branches.iter().any(|(name, _)| name == preferred) {
            return Ok(Some(preferred.to_owned()));
        }
    }
    Ok(branches
        .into_iter()
        .max_by(|left, right| left.1.cmp(&right.1).then_with(|| right.0.cmp(&left.0)))
        .map(|(name, _)| name))
}

pub(super) fn select_advertised_or_default_branch(
    repository: &SleyRepository,
    advertised: Option<&str>,
) -> sley::Result<Option<String>> {
    select_default_branch(repository, advertised)
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
        let repository = SleyRepository::init_with_format(&owned_path, object_format, true)
            .map_err(ApiError::internal)?;
        repository
            .set_head_symref("refs/heads/main", sley::HeadUpdateOptions::new())
            .map_err(|error| ApiError::internal(error.to_string()))
    })
    .await
    .map_err(ApiError::internal)?
}

async fn cleanup_repository(path: &Path) {
    if let Err(error) = fs::remove_dir_all(path).await
        && error.kind() != std::io::ErrorKind::NotFound
    {
        tracing::error!(%error, path = %path.display(), "could not clean up repository directory");
    }
}

async fn catalog_repository_lfs(
    state: &RepositoryState,
    transaction: &sea_orm::DatabaseTransaction,
    repository: &repository::Model,
) -> Result<(), ApiError> {
    let prefix =
        ObjectPrefix::new(repository.storage_key.to_string()).map_err(ApiError::internal)?;
    for object in state
        .lfs_store()
        .list(&prefix)
        .await
        .map_err(ApiError::internal)?
    {
        let oid = object
            .key
            .as_str()
            .rsplit('/')
            .next()
            .ok_or_else(|| ApiError::internal("LFS object key has no digest"))?;
        lfs_object::Entity::insert(lfs_object::ActiveModel {
            repository_id: Set(repository.id),
            oid: Set(oid.to_owned()),
            size: Set(i64::try_from(object.size).map_err(ApiError::internal)?),
            storage_target_id: Set(state.lfs_target_id()),
            created_at: Set(Utc::now()),
        })
        .on_conflict(
            OnConflict::columns([lfs_object::Column::RepositoryId, lfs_object::Column::Oid])
                .update_columns([
                    lfs_object::Column::Size,
                    lfs_object::Column::StorageTargetId,
                ])
                .to_owned(),
        )
        .exec(transaction)
        .await?;
    }
    Ok(())
}

async fn cleanup_lfs_blobs(state: &RepositoryState, storage_key: Uuid) {
    let prefix = match ObjectPrefix::new(storage_key.to_string()) {
        Ok(prefix) => prefix,
        Err(error) => {
            tracing::error!(%error, "could not construct repository LFS cleanup prefix");
            return;
        }
    };
    let _operation_guard = state.lfs_operation_guard().await;
    if let Err(error) = targets::delete_prefix_from_all(
        state.identity().database(),
        state.local_lfs_root().to_path_buf(),
        &prefix,
    )
    .await
    {
        tracing::error!(%error, "could not clean repository LFS objects from every target");
    }
}

async fn cleanup_repository_storage(repository_path: &Path, lfs_path: &Path) {
    cleanup_repository(repository_path).await;
    if let Err(error) = fs::remove_dir_all(lfs_path).await
        && error.kind() != std::io::ErrorKind::NotFound
    {
        tracing::error!(%error, path = %lfs_path.display(), "could not clean up Git LFS directory");
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path, process::Command};

    use sley::Repository as SleyRepository;
    use uuid::Uuid;

    use super::select_default_branch;

    fn git(path: &Path, arguments: &[&str]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(path)
            .args(arguments)
            .env("GIT_AUTHOR_NAME", "Gitadel Test")
            .env("GIT_AUTHOR_EMAIL", "gitadel@example.test")
            .env("GIT_COMMITTER_NAME", "Gitadel Test")
            .env("GIT_COMMITTER_EMAIL", "gitadel@example.test")
            .output()
            .expect("run git command");
        assert!(
            output.status.success(),
            "git {} failed: {}",
            arguments.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn commit(path: &Path, message: &str, date: &str) {
        let output = Command::new("git")
            .arg("-C")
            .arg(path)
            .args(["commit", "--quiet", "--allow-empty", "-m", message])
            .env("GIT_AUTHOR_NAME", "Gitadel Test")
            .env("GIT_AUTHOR_EMAIL", "gitadel@example.test")
            .env("GIT_COMMITTER_NAME", "Gitadel Test")
            .env("GIT_COMMITTER_EMAIL", "gitadel@example.test")
            .env("GIT_AUTHOR_DATE", date)
            .env("GIT_COMMITTER_DATE", date)
            .output()
            .expect("create test commit");
        assert!(
            output.status.success(),
            "git commit failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn selected_branch(path: &Path, configured: Option<&str>) -> Option<String> {
        let repository = SleyRepository::open(path.join(".git")).expect("open test repository");
        select_default_branch(&repository, configured).expect("select default branch")
    }

    #[test]
    fn first_default_prefers_configured_then_ranked_then_latest_tip() {
        let path =
            std::env::temp_dir().join(format!("gitadel-default-branch-test-{}", Uuid::new_v4()));
        fs::create_dir(&path).expect("create test repository directory");
        git(&path, &["init", "--quiet", "--initial-branch=main"]);
        commit(&path, "main", "2026-01-01T00:00:00Z");
        git(&path, &["branch", "master"]);
        git(&path, &["switch", "--quiet", "--create", "feature"]);
        commit(&path, "feature", "2026-01-03T00:00:00Z");
        git(&path, &["switch", "--quiet", "--create", "staging"]);
        commit(&path, "staging", "2026-01-02T00:00:00Z");

        assert_eq!(
            selected_branch(&path, Some("feature")).as_deref(),
            Some("feature")
        );
        assert_eq!(selected_branch(&path, None).as_deref(), Some("main"));
        git(&path, &["branch", "--delete", "--force", "main"]);
        assert_eq!(selected_branch(&path, None).as_deref(), Some("master"));
        git(&path, &["branch", "--delete", "--force", "master"]);
        git(&path, &["switch", "--quiet", "--detach"]);
        git(&path, &["branch", "--delete", "--force", "staging"]);
        assert_eq!(selected_branch(&path, None).as_deref(), Some("feature"));

        fs::remove_dir_all(path).expect("remove test repository");
    }

    #[test]
    fn first_default_uses_lexical_tie_break_and_leaves_tags_only_unresolved() {
        let path =
            std::env::temp_dir().join(format!("gitadel-default-branch-tie-{}", Uuid::new_v4()));
        fs::create_dir(&path).expect("create test repository directory");
        git(&path, &["init", "--quiet", "--initial-branch=beta"]);
        commit(&path, "beta", "2026-01-01T00:00:00Z");
        git(&path, &["switch", "--quiet", "--create", "alpha"]);
        commit(&path, "alpha", "2026-01-01T00:00:00Z");
        assert_eq!(selected_branch(&path, None).as_deref(), Some("alpha"));

        git(&path, &["tag", "only-tag"]);
        git(&path, &["switch", "--quiet", "--detach"]);
        git(&path, &["branch", "--delete", "--force", "alpha"]);
        git(&path, &["branch", "--delete", "--force", "beta"]);
        assert_eq!(selected_branch(&path, None), None);
        fs::remove_dir_all(path).expect("remove test repository");
    }
}
