use std::{
    collections::{HashMap, HashSet},
    time::Duration,
};

use axum::{
    Json,
    extract::{Path as AxumPath, State},
    http::{HeaderMap, StatusCode},
};
use axum_extra::extract::cookie::CookieJar;
use chrono::Utc;
use reqwest::header::{ACCEPT, AUTHORIZATION, HeaderValue, USER_AGENT};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, IntoActiveModel, QueryFilter, QueryOrder, Set,
    TransactionTrait,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::net::lookup_host;
use url::Url;
use uuid::Uuid;

use super::{RepositoryState, import_metadata, resources, validate_repository_name};
use crate::{
    entity::{repository, repository_import, repository_import_item},
    identity::{
        ApiError, SCOPE_READ, SCOPE_WRITE, load_mirror_identity_secret,
        mark_repository_identity_used,
    },
};

const MAX_IMPORT_REPOSITORIES: usize = 500;
const MAX_PROVIDER_PAGES: usize = 100;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ForgeProvider {
    Github,
    Gitlab,
    Gitea,
    Forgejo,
}

impl ForgeProvider {
    fn parse(value: &str) -> Result<Self, ApiError> {
        match value.trim().to_ascii_lowercase().as_str() {
            "github" => Ok(Self::Github),
            "gitlab" => Ok(Self::Gitlab),
            "gitea" => Ok(Self::Gitea),
            "forgejo" => Ok(Self::Forgejo),
            _ => Err(ApiError::bad_request("Unsupported repository provider.")),
        }
    }

    fn slug(self) -> &'static str {
        match self {
            Self::Github => "github",
            Self::Gitlab => "gitlab",
            Self::Gitea => "gitea",
            Self::Forgejo => "forgejo",
        }
    }

    fn default_url(self) -> Option<&'static str> {
        match self {
            Self::Github => Some("https://api.github.com"),
            Self::Gitlab => Some("https://gitlab.com"),
            Self::Gitea | Self::Forgejo => None,
        }
    }

    fn clone_username(self, account: &str) -> String {
        match self {
            Self::Github => "x-access-token".to_owned(),
            Self::Gitlab => "oauth2".to_owned(),
            Self::Gitea | Self::Forgejo => account.to_owned(),
        }
    }
}

#[derive(Deserialize)]
pub struct DiscoverRequest {
    provider: String,
    namespace: String,
    identity_id: Uuid,
}

#[derive(Clone, Serialize)]
pub struct RemoteRepositoryResponse {
    id: String,
    name: String,
    full_name: String,
    description: Option<String>,
    web_url: String,
    clone_url: String,
    visibility: String,
    archived: bool,
    fork: bool,
    default_branch: Option<String>,
}

#[derive(Serialize)]
pub struct DiscoveryResponse {
    provider: String,
    instance_url: String,
    account: String,
    repositories: Vec<RemoteRepositoryResponse>,
}

#[derive(Deserialize)]
pub struct CreateImportRequest {
    provider: String,
    identity_id: Uuid,
    target_namespace: String,
    repositories: Vec<ImportSelection>,
}
#[derive(Deserialize)]
pub struct CreateDirectImportRequest {
    remote_url: String,
    identity_id: Uuid,
    target_namespace: String,
    target_name: String,
    visibility: String,
}

#[derive(Deserialize)]
pub struct ImportSelection {
    source_id: String,
    target_namespace: Option<String>,
    target_name: String,
}

#[derive(Serialize)]
pub struct ImportItemResponse {
    id: Uuid,
    source_id: String,
    source_full_name: String,
    source_web_url: String,
    target_namespace: String,
    target_name: String,
    target_visibility: String,
    state: String,
    attempts: i32,
    repository_id: Option<Uuid>,
    last_error: Option<String>,
}

#[derive(Serialize)]
pub struct ImportResponse {
    id: Uuid,
    provider: String,
    instance_url: String,
    target_namespace: String,
    state: String,
    created_at: chrono::DateTime<Utc>,
    updated_at: chrono::DateTime<Utc>,
    items: Vec<ImportItemResponse>,
}

struct Discovery {
    provider: ForgeProvider,
    instance_url: Url,
    account: String,
    repositories: Vec<RemoteRepositoryResponse>,
}

pub async fn discover(
    State(state): State<RepositoryState>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<DiscoverRequest>,
) -> Result<Json<DiscoveryResponse>, ApiError> {
    let actor = state
        .identity()
        .authenticate(&headers, &jar, SCOPE_READ)
        .await?;
    let provider = ForgeProvider::parse(&request.provider)?;
    resources::ensure_owned_namespace(&state, actor.user.id, &request.namespace).await?;
    let (token, instance_url) =
        load_import_identity(&state, &request.namespace, request.identity_id, provider).await?;
    let result = discover_remote(&state, provider.slug(), instance_url, &token).await?;
    Ok(Json(DiscoveryResponse {
        provider: result.provider.slug().to_owned(),
        instance_url: result.instance_url.to_string(),
        account: result.account,
        repositories: result.repositories,
    }))
}

pub async fn create_import(
    State(state): State<RepositoryState>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<CreateImportRequest>,
) -> Result<(StatusCode, Json<ImportResponse>), ApiError> {
    let actor = state
        .identity()
        .authenticate(&headers, &jar, SCOPE_WRITE)
        .await?;
    resources::ensure_owned_namespace(&state, actor.user.id, &request.target_namespace).await?;
    if request.repositories.is_empty() || request.repositories.len() > MAX_IMPORT_REPOSITORIES {
        return Err(ApiError::bad_request(format!(
            "Select between 1 and {MAX_IMPORT_REPOSITORIES} repositories."
        )));
    }
    let provider = ForgeProvider::parse(&request.provider)?;
    let (token, instance_url) = load_import_identity(
        &state,
        &request.target_namespace,
        request.identity_id,
        provider,
    )
    .await?;
    let discovery = discover_remote(&state, provider.slug(), instance_url, &token).await?;
    let available: HashMap<_, _> = discovery
        .repositories
        .into_iter()
        .map(|repository| (repository.id.clone(), repository))
        .collect();
    let mut selected_ids = HashSet::new();
    let mut target_names = HashSet::new();
    let now = Utc::now();
    let import = repository_import::Model {
        id: Uuid::new_v4(),
        created_by: actor.user.id,
        target_namespace: request.target_namespace,
        provider: discovery.provider.slug().to_owned(),
        instance_url: discovery.instance_url.to_string(),
        identity_id: Some(request.identity_id),
        state: "queued".to_owned(),
        created_at: now,
        updated_at: now,
    };
    let mut items = Vec::with_capacity(request.repositories.len());
    for selection in request.repositories {
        if !selected_ids.insert(selection.source_id.clone()) {
            return Err(ApiError::bad_request(
                "A source repository was selected more than once.",
            ));
        }
        let target_namespace = selection
            .target_namespace
            .as_deref()
            .unwrap_or(&import.target_namespace)
            .to_owned();
        if target_namespace != import.target_namespace {
            return Err(ApiError::bad_request(
                "Every repository in an import must use the identity owner's namespace.",
            ));
        }
        resources::ensure_owned_namespace(&state, actor.user.id, &target_namespace).await?;
        let target_name = validate_repository_name(&selection.target_name)?;
        if !target_names.insert((target_namespace.clone(), target_name.clone())) {
            return Err(ApiError::bad_request(
                "Two selected repositories have the same destination namespace and name.",
            ));
        }
        let source = available
            .get(&selection.source_id)
            .ok_or_else(|| ApiError::bad_request("A selected source repository is unavailable."))?;
        let target_visibility = import_visibility(&source.visibility);
        items.push(repository_import_item::Model {
            id: Uuid::new_v4(),
            import_id: import.id,
            source_id: source.id.clone(),
            source_full_name: source.full_name.clone(),
            source_web_url: source.web_url.clone(),
            source_clone_url: source.clone_url.clone(),
            target_namespace,
            target_name,
            target_visibility: target_visibility.to_owned(),
            state: "queued".to_owned(),
            attempts: 0,
            repository_id: None,
            last_error: None,
            created_at: now,
            updated_at: now,
        });
    }

    let transaction = state.identity().database().begin().await?;
    import
        .clone()
        .into_active_model()
        .insert(&transaction)
        .await?;
    repository_import_item::Entity::insert_many(
        items
            .iter()
            .cloned()
            .map(IntoActiveModel::into_active_model),
    )
    .exec(&transaction)
    .await?;
    state
        .identity()
        .audit_on(
            &transaction,
            Some(actor.user.id),
            "repository.import.start",
            Some(import.id.to_string()),
        )
        .await?;
    transaction.commit().await?;

    let username = discovery.provider.clone_username(&discovery.account);
    let queued = items.clone();
    let worker_state = state.clone();
    let worker_import = import.clone();
    let authentication = super::mirrors::ImportAuthentication {
        username: Some(username),
        secret: Some(token.clone()),
        ssh_private_key: None,
    };
    let task_state = worker_state.clone();
    task_state.spawn_task(async move {
        run_items(
            worker_state,
            worker_import,
            queued,
            authentication,
            Some(token),
        )
        .await;
    });

    Ok((StatusCode::ACCEPTED, Json(import_response(import, items))))
}

pub async fn create_direct_import(
    State(state): State<RepositoryState>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<CreateDirectImportRequest>,
) -> Result<(StatusCode, Json<ImportResponse>), ApiError> {
    let actor = state
        .identity()
        .authenticate(&headers, &jar, SCOPE_WRITE)
        .await?;
    resources::ensure_owned_namespace(&state, actor.user.id, &request.target_namespace).await?;
    let target_name = validate_repository_name(&request.target_name)?;
    let identity = load_mirror_identity_secret(
        state.identity(),
        &request.target_namespace,
        request.identity_id,
    )
    .await?;
    let source_instance_url = match identity.kind.as_str() {
        "token" => identity.instance_url.as_deref(),
        "basic" => Some(request.remote_url.as_str()),
        "ssh" => None,
        _ => return Err(ApiError::bad_request("The selected identity is invalid.")),
    };
    let remote_url = super::mirrors::validate_direct_import_identity(
        source_instance_url,
        &request.remote_url,
        &identity.kind,
    )?;
    let authentication = match identity.kind.as_str() {
        "ssh" => super::mirrors::ImportAuthentication {
            username: None,
            secret: None,
            ssh_private_key: Some(identity.secret),
        },
        "basic" => super::mirrors::ImportAuthentication {
            username: identity.username,
            secret: Some(identity.secret),
            ssh_private_key: None,
        },
        "token" => super::mirrors::ImportAuthentication {
            username: Some(
                match identity.provider.as_deref() {
                    Some("github") => "x-access-token",
                    Some("gitlab") => "oauth2",
                    Some("gitea" | "forgejo") => "git",
                    _ => {
                        return Err(ApiError::bad_request(
                            "The selected token identity has no valid provider.",
                        ));
                    }
                }
                .to_owned(),
            ),
            secret: Some(identity.secret),
            ssh_private_key: None,
        },
        _ => return Err(ApiError::bad_request("The selected identity is invalid.")),
    };
    let now = Utc::now();
    let import = repository_import::Model {
        id: Uuid::new_v4(),
        created_by: actor.user.id,
        target_namespace: request.target_namespace.clone(),
        provider: "direct".to_owned(),
        instance_url: remote_url.clone(),
        identity_id: Some(request.identity_id),
        state: "queued".to_owned(),
        created_at: now,
        updated_at: now,
    };
    let item = repository_import_item::Model {
        id: Uuid::new_v4(),
        import_id: import.id,
        source_id: remote_url.clone(),
        source_full_name: remote_url.clone(),
        source_web_url: remote_url.clone(),
        source_clone_url: remote_url,
        target_namespace: request.target_namespace,
        target_name,
        target_visibility: request.visibility,
        state: "queued".to_owned(),
        attempts: 0,
        repository_id: None,
        last_error: None,
        created_at: now,
        updated_at: now,
    };
    let transaction = state.identity().database().begin().await?;
    import
        .clone()
        .into_active_model()
        .insert(&transaction)
        .await?;
    item.clone()
        .into_active_model()
        .insert(&transaction)
        .await?;
    state
        .identity()
        .audit_on(
            &transaction,
            Some(actor.user.id),
            "repository.import.start",
            Some(import.id.to_string()),
        )
        .await?;
    transaction.commit().await?;
    let worker_state = state.clone();
    let worker_import = import.clone();
    let worker_item = item.clone();
    let task_state = worker_state.clone();
    task_state.spawn_task(async move {
        run_items(
            worker_state,
            worker_import,
            vec![worker_item],
            authentication,
            None,
        )
        .await;
    });
    Ok((
        StatusCode::ACCEPTED,
        Json(import_response(import, vec![item])),
    ))
}

pub async fn get_import(
    State(state): State<RepositoryState>,
    AxumPath(id): AxumPath<Uuid>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<ImportResponse>, ApiError> {
    let actor = state
        .identity()
        .authenticate(&headers, &jar, SCOPE_READ)
        .await?;
    let import = repository_import::Entity::find_by_id(id)
        .filter(repository_import::Column::CreatedBy.eq(actor.user.id))
        .one(state.identity().database())
        .await?
        .ok_or_else(ApiError::not_found)?;
    let items = repository_import_item::Entity::find()
        .filter(repository_import_item::Column::ImportId.eq(id))
        .order_by_asc(repository_import_item::Column::CreatedAt)
        .all(state.identity().database())
        .await?;
    Ok(Json(import_response(import, items)))
}

pub async fn retry_import(
    State(state): State<RepositoryState>,
    AxumPath(id): AxumPath<Uuid>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<(StatusCode, Json<ImportResponse>), ApiError> {
    let actor = state
        .identity()
        .authenticate(&headers, &jar, SCOPE_WRITE)
        .await?;
    let mut import = repository_import::Entity::find_by_id(id)
        .filter(repository_import::Column::CreatedBy.eq(actor.user.id))
        .one(state.identity().database())
        .await?
        .ok_or_else(ApiError::not_found)?;
    let provider = ForgeProvider::parse(&import.provider)?;
    let identity_id = import
        .identity_id
        .ok_or_else(|| ApiError::bad_request("This import has no reusable identity."))?;
    let (token, _) =
        load_import_identity(&state, &import.target_namespace, identity_id, provider).await?;
    let discovery = discover_remote(
        &state,
        &import.provider,
        Some(import.instance_url.clone()),
        &token,
    )
    .await?;
    let available: HashMap<_, _> = discovery
        .repositories
        .into_iter()
        .map(|repository| (repository.id.clone(), repository))
        .collect();
    let failed = repository_import_item::Entity::find()
        .filter(repository_import_item::Column::ImportId.eq(id))
        .filter(repository_import_item::Column::State.is_in(["failed", "credentials_required"]))
        .all(state.identity().database())
        .await?;
    if failed.is_empty() {
        return Err(ApiError::conflict(
            "This import has no failed repositories to retry.",
        ));
    }
    let now = Utc::now();
    let transaction = state.identity().database().begin().await?;
    let mut queued = Vec::new();
    for mut item in failed {
        let source = available.get(&item.source_id).ok_or_else(|| {
            ApiError::bad_request(format!("{} is no longer available.", item.source_full_name))
        })?;
        item.source_clone_url = source.clone_url.clone();
        item.state = "queued".to_owned();
        item.last_error = None;
        item.updated_at = now;
        item.clone()
            .into_active_model()
            .update(&transaction)
            .await?;
        queued.push(item);
    }
    import.state = "queued".to_owned();
    import.updated_at = now;
    import
        .clone()
        .into_active_model()
        .update(&transaction)
        .await?;
    transaction.commit().await?;

    let username = discovery.provider.clone_username(&discovery.account);
    let worker_state = state.clone();
    let worker_import = import.clone();
    let worker_items = queued.clone();
    let authentication = super::mirrors::ImportAuthentication {
        username: Some(username),
        secret: Some(token.clone()),
        ssh_private_key: None,
    };
    let task_state = worker_state.clone();
    task_state.spawn_task(async move {
        run_items(
            worker_state,
            worker_import,
            worker_items,
            authentication,
            Some(token),
        )
        .await;
    });
    let all_items = repository_import_item::Entity::find()
        .filter(repository_import_item::Column::ImportId.eq(id))
        .order_by_asc(repository_import_item::Column::CreatedAt)
        .all(state.identity().database())
        .await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(import_response(import, all_items)),
    ))
}

pub async fn cancel_import(
    State(state): State<RepositoryState>,
    AxumPath(id): AxumPath<Uuid>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<StatusCode, ApiError> {
    let actor = state
        .identity()
        .authenticate(&headers, &jar, SCOPE_WRITE)
        .await?;
    let import = repository_import::Entity::find_by_id(id)
        .filter(repository_import::Column::CreatedBy.eq(actor.user.id))
        .one(state.identity().database())
        .await?
        .ok_or_else(ApiError::not_found)?;
    let now = Utc::now();
    repository_import_item::Entity::update_many()
        .col_expr(
            repository_import_item::Column::State,
            sea_orm::sea_query::Expr::value("cancelled"),
        )
        .col_expr(
            repository_import_item::Column::UpdatedAt,
            sea_orm::sea_query::Expr::value(now),
        )
        .filter(repository_import_item::Column::ImportId.eq(id))
        .filter(repository_import_item::Column::State.eq("queued"))
        .exec(state.identity().database())
        .await?;
    let mut active = import.into_active_model();
    active.state = Set("cancelled".to_owned());
    active.updated_at = Set(now);
    active.update(state.identity().database()).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn recover_interrupted(state: &RepositoryState) -> Result<(), ApiError> {
    let now = Utc::now();
    repository_import_item::Entity::update_many()
        .col_expr(
            repository_import_item::Column::State,
            sea_orm::sea_query::Expr::value("credentials_required"),
        )
        .col_expr(
            repository_import_item::Column::LastError,
            sea_orm::sea_query::Expr::value(
                "Gitadel restarted. Reconnect the source credential to resume.",
            ),
        )
        .col_expr(
            repository_import_item::Column::UpdatedAt,
            sea_orm::sea_query::Expr::value(now),
        )
        .filter(repository_import_item::Column::State.is_in(["queued", "cloning", "metadata"]))
        .exec(state.identity().database())
        .await?;
    repository_import::Entity::update_many()
        .col_expr(
            repository_import::Column::State,
            sea_orm::sea_query::Expr::value("credentials_required"),
        )
        .col_expr(
            repository_import::Column::UpdatedAt,
            sea_orm::sea_query::Expr::value(now),
        )
        .filter(repository_import::Column::State.is_in(["queued", "running"]))
        .exec(state.identity().database())
        .await?;
    Ok(())
}

async fn run_items(
    state: RepositoryState,
    mut import: repository_import::Model,
    items: Vec<repository_import_item::Model>,
    authentication: super::mirrors::ImportAuthentication,
    metadata_token: Option<String>,
) {
    import.state = "running".to_owned();
    import.updated_at = Utc::now();
    let _ = import
        .clone()
        .into_active_model()
        .update(state.identity().database())
        .await;
    for queued_item in items {
        let Ok(Some(mut item)) = repository_import_item::Entity::find_by_id(queued_item.id)
            .one(state.identity().database())
            .await
        else {
            continue;
        };
        if item.state != "queued" {
            continue;
        }

        item.attempts += 1;
        item.state = if item.repository_id.is_some() {
            "metadata"
        } else {
            "cloning"
        }
        .to_owned();
        item.last_error = None;
        item.updated_at = Utc::now();
        let Ok(mut item) = item
            .into_active_model()
            .update(state.identity().database())
            .await
        else {
            continue;
        };

        let repository_result = match item.repository_id {
            Some(repository_id) => repository::Entity::find_by_id(repository_id)
                .one(state.identity().database())
                .await
                .map_err(ApiError::from)
                .and_then(|repository| {
                    repository.ok_or_else(|| {
                        ApiError::bad_request(
                            "The repository created by this import no longer exists.",
                        )
                    })
                }),
            None => {
                resources::create_imported_repository(
                    &state,
                    import.created_by,
                    resources::ImportRepositoryOptions {
                        namespace: item.target_namespace.clone(),
                        name: item.target_name.clone(),
                        description: Some(format!("Imported from {}", item.source_full_name)),
                        source_url: item.source_web_url.clone(),
                        source_instance_url: (import.provider != "direct")
                            .then(|| import.instance_url.clone()),
                        visibility: item.target_visibility.clone(),
                        remote_url: item.source_clone_url.clone(),
                        authentication: authentication.clone(),
                    },
                )
                .await
            }
        };

        let result = match repository_result {
            Ok(repository) => {
                item.repository_id = Some(repository.id);
                if let Some(identity_id) = import.identity_id {
                    let _ = mark_repository_identity_used(
                        state.identity(),
                        &import.target_namespace,
                        identity_id,
                        Utc::now(),
                    )
                    .await;
                }
                item.state = "metadata".to_owned();
                item.updated_at = Utc::now();
                match item
                    .clone()
                    .into_active_model()
                    .update(state.identity().database())
                    .await
                {
                    Ok(saved) => {
                        item = saved;
                        if import.provider == "direct" {
                            Ok(None)
                        } else {
                            match metadata_token.as_deref() {
                                Some(token) => import_metadata::import_repository_metadata(
                                    &state,
                                    &repository,
                                    &import.provider,
                                    &import.instance_url,
                                    &item.source_id,
                                    &item.source_full_name,
                                    token,
                                    import.created_by,
                                )
                                .await
                                .map(|report| {
                                    tracing::info!(
                                        repository_id = %repository.id,
                                        labels = report.labels,
                                        releases = report.releases,
                                        assets = report.assets,
                                        skipped_assets = report.skipped_assets,
                                        "imported repository metadata"
                                    );
                                    (report.skipped_assets > 0).then(|| {
                                        format!(
                                            "{} release assets exceeded the 2 GiB limit and were skipped.",
                                            report.skipped_assets
                                        )
                                    })
                                }),
                                None => Err(ApiError::internal(
                                    "import metadata token is unavailable",
                                )),
                            }
                        }
                    }
                    Err(error) => Err(ApiError::from(error)),
                }
            }
            Err(error) => Err(error),
        };

        let mut active = item.into_active_model();
        active.updated_at = Set(Utc::now());
        match result {
            Ok(warning) => {
                active.state = Set("completed".to_owned());
                active.last_error = Set(warning);
            }
            Err(error) => {
                active.state = Set("failed".to_owned());
                active.last_error = Set(Some(error.to_string()));
            }
        }
        let _ = active.update(state.identity().database()).await;
    }
    let all = match repository_import_item::Entity::find()
        .filter(repository_import_item::Column::ImportId.eq(import.id))
        .all(state.identity().database())
        .await
    {
        Ok(items) => items,
        Err(error) => {
            tracing::error!(import_id = %import.id, %error, "could not finalize repository import");
            import.state = "completed_with_errors".to_owned();
            import.updated_at = Utc::now();
            let _ = import
                .into_active_model()
                .update(state.identity().database())
                .await;
            return;
        }
    };
    import.state = if all.iter().all(|item| item.state == "completed") {
        "completed"
    } else if all.iter().any(|item| item.state == "failed") {
        "completed_with_errors"
    } else if all.iter().any(|item| item.state == "credentials_required") {
        "credentials_required"
    } else if all.iter().any(|item| item.state == "cancelled") {
        "cancelled"
    } else {
        "completed_with_errors"
    }
    .to_owned();
    import.updated_at = Utc::now();
    let _ = import
        .into_active_model()
        .update(state.identity().database())
        .await;
}

async fn discover_remote(
    state: &RepositoryState,
    provider_value: &str,
    instance_value: Option<String>,
    token: &str,
) -> Result<Discovery, ApiError> {
    let provider = ForgeProvider::parse(provider_value)?;
    if token.trim().is_empty() || token.len() > 8_192 {
        return Err(ApiError::bad_request("Enter a valid source access token."));
    }
    let instance_url = normalize_instance_url(provider, instance_value).await?;
    match provider {
        ForgeProvider::Github => discover_github(state, instance_url, token).await,
        ForgeProvider::Gitlab => discover_gitlab(state, instance_url, token).await,
        ForgeProvider::Gitea | ForgeProvider::Forgejo => {
            discover_gitea_family(state, provider, instance_url, token).await
        }
    }
}

async fn normalize_instance_url(
    provider: ForgeProvider,
    value: Option<String>,
) -> Result<Url, ApiError> {
    let raw = value
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or_else(|| provider.default_url())
        .ok_or_else(|| ApiError::bad_request("Enter the source instance URL."))?;
    let mut url =
        Url::parse(raw).map_err(|_| ApiError::bad_request("Source instance URL is invalid."))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(ApiError::bad_request(
            "Source instance URL must be an HTTP or HTTPS URL without credentials.",
        ));
    }
    url.set_query(None);
    url.set_fragment(None);
    let normalized_path = url.path().trim_end_matches('/').to_owned();
    url.set_path(&normalized_path);
    let host = url
        .host_str()
        .ok_or_else(|| ApiError::bad_request("Source instance URL must include a host."))?;
    let port = url.port_or_known_default().unwrap_or(443);
    let found = lookup_host((host, port))
        .await
        .map_err(|error| {
            ApiError::bad_request(format!("Could not resolve source instance: {error}"))
        })?
        .next()
        .is_some();
    if !found {
        return Err(ApiError::bad_request(
            "Source instance did not resolve to an address.",
        ));
    }
    Ok(url)
}

async fn discover_github(
    state: &RepositoryState,
    instance_url: Url,
    token: &str,
) -> Result<Discovery, ApiError> {
    if instance_url.host_str() != Some("api.github.com") {
        return Err(ApiError::bad_request(
            "GitHub imports currently use api.github.com.",
        ));
    }
    let user = get_json(
        state,
        instance_url.join("/user").map_err(ApiError::internal)?,
        token,
        ForgeProvider::Github,
    )
    .await?;
    let account = text(&user, "login")?;
    let mut repositories = Vec::new();
    for page in 1..=MAX_PROVIDER_PAGES {
        let mut url = instance_url
            .join("/user/repos")
            .map_err(ApiError::internal)?;
        url.query_pairs_mut()
            .append_pair("type", "all")
            .append_pair("sort", "full_name")
            .append_pair("per_page", "100")
            .append_pair("page", &page.to_string());
        let values = array(get_json(state, url, token, ForgeProvider::Github).await?)?;
        let count = values.len();
        for value in values {
            repositories.push(RemoteRepositoryResponse {
                id: identifier(&value, "id")?,
                name: text(&value, "name")?,
                full_name: text(&value, "full_name")?,
                description: optional_text(&value, "description"),
                web_url: text(&value, "html_url")?,
                clone_url: text(&value, "clone_url")?,
                visibility: if boolean(&value, "private") {
                    "private"
                } else {
                    "public"
                }
                .to_owned(),
                archived: boolean(&value, "archived"),
                fork: boolean(&value, "fork"),
                default_branch: optional_text(&value, "default_branch"),
            });
        }
        if count < 100 {
            break;
        }
    }
    Ok(Discovery {
        provider: ForgeProvider::Github,
        instance_url,
        account,
        repositories,
    })
}

async fn discover_gitlab(
    state: &RepositoryState,
    instance_url: Url,
    token: &str,
) -> Result<Discovery, ApiError> {
    let user_url = instance_url
        .join("/api/v4/user")
        .map_err(ApiError::internal)?;
    let user = get_json(state, user_url, token, ForgeProvider::Gitlab).await?;
    let account = text(&user, "username")?;
    let mut repositories = Vec::new();
    for page in 1..=MAX_PROVIDER_PAGES {
        let mut url = instance_url
            .join("/api/v4/projects")
            .map_err(ApiError::internal)?;
        url.query_pairs_mut()
            .append_pair("membership", "true")
            .append_pair("simple", "true")
            .append_pair("order_by", "path")
            .append_pair("sort", "asc")
            .append_pair("per_page", "100")
            .append_pair("page", &page.to_string());
        let values = array(get_json(state, url, token, ForgeProvider::Gitlab).await?)?;
        let count = values.len();
        for value in values {
            let visibility =
                optional_text(&value, "visibility").unwrap_or_else(|| "private".to_owned());
            repositories.push(RemoteRepositoryResponse {
                id: identifier(&value, "id")?,
                name: text(&value, "path")?,
                full_name: text(&value, "path_with_namespace")?,
                description: optional_text(&value, "description"),
                web_url: text(&value, "web_url")?,
                clone_url: text(&value, "http_url_to_repo")?,
                visibility,
                archived: boolean(&value, "archived"),
                fork: value
                    .get("forked_from_project")
                    .is_some_and(|value| !value.is_null()),
                default_branch: optional_text(&value, "default_branch"),
            });
        }
        if count < 100 {
            break;
        }
    }
    Ok(Discovery {
        provider: ForgeProvider::Gitlab,
        instance_url,
        account,
        repositories,
    })
}

async fn discover_gitea_family(
    state: &RepositoryState,
    provider: ForgeProvider,
    instance_url: Url,
    token: &str,
) -> Result<Discovery, ApiError> {
    let user_url = instance_url
        .join("/api/v1/user")
        .map_err(ApiError::internal)?;
    let user = get_json(state, user_url, token, provider).await?;
    let account = text(&user, "login").or_else(|_| text(&user, "username"))?;
    let mut repositories = Vec::new();
    for page in 1..=MAX_PROVIDER_PAGES {
        let mut url = instance_url
            .join("/api/v1/user/repos")
            .map_err(ApiError::internal)?;
        url.query_pairs_mut()
            .append_pair("limit", "50")
            .append_pair("page", &page.to_string());
        let values = array(get_json(state, url, token, provider).await?)?;
        let count = values.len();
        for value in values {
            repositories.push(parse_gitea_repository(&value)?);
        }
        if count < 50 {
            break;
        }
    }
    let mut organizations = Vec::new();
    for page in 1..=MAX_PROVIDER_PAGES {
        let mut url = instance_url
            .join("/api/v1/user/orgs")
            .map_err(ApiError::internal)?;
        url.query_pairs_mut()
            .append_pair("limit", "50")
            .append_pair("page", &page.to_string());
        let values = array(get_json(state, url, token, provider).await?)?;
        let count = values.len();
        for value in values {
            organizations.push(
                optional_text(&value, "username")
                    .or_else(|| optional_text(&value, "name"))
                    .ok_or_else(|| {
                        ApiError::bad_request("Source organization is missing its username.")
                    })?,
            );
        }
        if count < 50 {
            break;
        }
    }
    for organization in organizations {
        for page in 1..=MAX_PROVIDER_PAGES {
            let mut url = instance_url.clone();
            url.path_segments_mut()
                .map_err(|_| ApiError::bad_request("Source instance URL cannot be a base URL."))?
                .clear()
                .extend(["api", "v1", "orgs", &organization, "repos"]);
            url.query_pairs_mut()
                .append_pair("limit", "50")
                .append_pair("page", &page.to_string());
            let values = array(get_json(state, url, token, provider).await?)?;
            let count = values.len();
            for value in values {
                repositories.push(parse_gitea_repository(&value)?);
            }
            if count < 50 {
                break;
            }
        }
    }
    let mut unique = HashMap::new();
    for repository in repositories {
        unique.entry(repository.id.clone()).or_insert(repository);
    }
    let mut repositories: Vec<_> = unique.into_values().collect();
    repositories.sort_by(|left, right| left.full_name.cmp(&right.full_name));
    Ok(Discovery {
        provider,
        instance_url,
        account,
        repositories,
    })
}

fn parse_gitea_repository(value: &Value) -> Result<RemoteRepositoryResponse, ApiError> {
    let private = boolean(value, "private");
    Ok(RemoteRepositoryResponse {
        id: identifier(value, "id")?,
        name: text(value, "name")?,
        full_name: text(value, "full_name")?,
        description: optional_text(value, "description"),
        web_url: text(value, "html_url")?,
        clone_url: text(value, "clone_url")?,
        visibility: optional_text(value, "visibility")
            .unwrap_or_else(|| if private { "private" } else { "public" }.to_owned()),
        archived: boolean(value, "archived"),
        fork: boolean(value, "fork"),
        default_branch: optional_text(value, "default_branch"),
    })
}

async fn get_json(
    _state: &RepositoryState,
    url: Url,
    token: &str,
    provider: ForgeProvider,
) -> Result<Value, ApiError> {
    let client = pinned_http_client(&url).await?;
    let mut request = client
        .get(url)
        .header(ACCEPT, "application/json")
        .header(USER_AGENT, "Gitadel repository importer");
    request = match provider {
        ForgeProvider::Github => request
            .header(AUTHORIZATION, bearer(token)?)
            .header("x-github-api-version", "2026-03-10"),
        ForgeProvider::Gitlab => request.header("private-token", token),
        ForgeProvider::Gitea | ForgeProvider::Forgejo => {
            request.header(AUTHORIZATION, bearer(token)?)
        }
    };
    let response = request
        .send()
        .await
        .map_err(|error| ApiError::bad_request(format!("Source request failed: {error}")))?;
    let status = response.status();
    if !status.is_success() {
        return Err(ApiError::bad_request(format!(
            "The source provider rejected the request with status {status}."
        )));
    }
    response
        .json()
        .await
        .map_err(|error| ApiError::bad_request(format!("Source returned invalid JSON: {error}")))
}

async fn pinned_http_client(url: &Url) -> Result<reqwest::Client, ApiError> {
    let host = url
        .host_str()
        .ok_or_else(|| ApiError::bad_request("Source request URL has no host."))?;
    let port = url.port_or_known_default().unwrap_or(443);
    let addresses: Vec<_> = lookup_host((host, port))
        .await
        .map_err(|error| {
            ApiError::bad_request(format!("Could not resolve source instance: {error}"))
        })?
        .collect();
    if addresses.is_empty() {
        return Err(ApiError::bad_request(
            "Source instance did not resolve to an address.",
        ));
    }
    reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .resolve_to_addrs(host, &addresses)
        .build()
        .map_err(ApiError::internal)
}

fn import_visibility(source_visibility: &str) -> &'static str {
    if source_visibility == "public" {
        "public"
    } else {
        "private"
    }
}

async fn load_import_identity(
    state: &RepositoryState,
    namespace: &str,
    identity_id: Uuid,
    provider: ForgeProvider,
) -> Result<(String, Option<String>), ApiError> {
    let identity = load_mirror_identity_secret(state.identity(), namespace, identity_id).await?;
    if identity.kind != "token" || identity.provider.as_deref() != Some(provider.slug()) {
        return Err(ApiError::bad_request(format!(
            "Select a {} token identity owned by this namespace.",
            provider.slug()
        )));
    }
    Ok((identity.secret, identity.instance_url))
}

fn bearer(token: &str) -> Result<HeaderValue, ApiError> {
    HeaderValue::from_str(&format!("Bearer {token}"))
        .map_err(|_| ApiError::bad_request("The source token is not a valid HTTP header value."))
}

fn array(value: Value) -> Result<Vec<Value>, ApiError> {
    value.as_array().cloned().ok_or_else(|| {
        ApiError::bad_request("Source provider returned an invalid repository list.")
    })
}

fn text(value: &Value, key: &str) -> Result<String, ApiError> {
    optional_text(value, key)
        .ok_or_else(|| ApiError::bad_request(format!("Source repository is missing {key}.")))
}

fn optional_text(value: &Value, key: &str) -> Option<String> {
    value.get(key)?.as_str().map(str::to_owned)
}

fn identifier(value: &Value, key: &str) -> Result<String, ApiError> {
    value
        .get(key)
        .and_then(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .or_else(|| value.as_i64().map(|value| value.to_string()))
        })
        .ok_or_else(|| ApiError::bad_request(format!("Source repository is missing {key}.")))
}

fn boolean(value: &Value, key: &str) -> bool {
    value.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn import_response(
    import: repository_import::Model,
    items: Vec<repository_import_item::Model>,
) -> ImportResponse {
    ImportResponse {
        id: import.id,
        provider: import.provider,
        instance_url: import.instance_url,
        target_namespace: import.target_namespace,
        state: import.state,
        created_at: import.created_at,
        updated_at: import.updated_at,
        items: items
            .into_iter()
            .map(|item| ImportItemResponse {
                id: item.id,
                source_id: item.source_id,
                source_full_name: item.source_full_name,
                source_web_url: item.source_web_url,
                target_namespace: item.target_namespace,
                target_name: item.target_name,
                target_visibility: item.target_visibility,
                state: item.state,
                attempts: item.attempts,
                repository_id: item.repository_id,
                last_error: item.last_error,
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::{ForgeProvider, import_visibility, normalize_instance_url, parse_gitea_repository};
    use serde_json::json;

    #[test]
    fn provider_parser_accepts_supported_forges_only() {
        assert_eq!(
            ForgeProvider::parse("GitHub").unwrap(),
            ForgeProvider::Github
        );
        assert_eq!(
            ForgeProvider::parse("forgejo").unwrap(),
            ForgeProvider::Forgejo
        );
        assert!(ForgeProvider::parse("bitbucket").is_err());
    }

    #[test]
    fn gitea_repository_parser_preserves_import_fields() {
        let repository = parse_gitea_repository(&json!({
            "id": 42,
            "name": "project",
            "full_name": "team/project",
            "description": "Example",
            "html_url": "https://forge.example/team/project",
            "clone_url": "https://forge.example/team/project.git",
            "visibility": "limited",
            "archived": true,
            "fork": true,
            "default_branch": "trunk"
        }))
        .unwrap();

        assert_eq!(repository.id, "42");
        assert_eq!(repository.full_name, "team/project");
        assert_eq!(repository.visibility, "limited");
        assert!(repository.archived);
        assert!(repository.fork);
    }

    #[test]
    fn imported_repository_visibility_matches_the_source() {
        assert_eq!(import_visibility("public"), "public");
        assert_eq!(import_visibility("private"), "private");
        assert_eq!(import_visibility("internal"), "private");
    }

    #[tokio::test]
    async fn source_instance_accepts_cleartext_http() {
        assert!(
            normalize_instance_url(ForgeProvider::Gitea, Some("http://example.com".to_owned()),)
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn source_instance_accepts_private_network_addresses() {
        assert!(
            normalize_instance_url(ForgeProvider::Forgejo, Some("https://127.0.0.1".to_owned()),)
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn source_instance_rejects_unsupported_schemes() {
        assert!(
            normalize_instance_url(ForgeProvider::Gitea, Some("ftp://example.com".to_owned()),)
                .await
                .is_err()
        );
    }
}
