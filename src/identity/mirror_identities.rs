use std::collections::HashMap;

use axum::{
    Json, Router,
    extract::{Path as AxumPath, State},
    http::HeaderMap,
    routing::{get, put},
};
use axum_extra::extract::cookie::CookieJar;
use chrono::Utc;
use reqwest::{Client, StatusCode, header};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, ModelTrait, QueryFilter, QueryOrder, Set,
    TransactionTrait,
};
use serde::{Deserialize, Serialize};
use url::Url;
use uuid::Uuid;

use super::{ApiError, IdentityState, SCOPE_READ, SCOPE_WRITE, integrations::authorize_namespace};
use crate::{
    entity::{namespace_mirror_identity, repository, repository_import, repository_mirror},
    network::{normalize_public_https_origin, pinned_public_https_client},
};

const MAX_IDENTITY_NAME_LENGTH: usize = 80;
const MAX_SECRET_LENGTH: usize = 8_192;

pub(super) fn router() -> Router<IdentityState> {
    Router::new()
        .route(
            "/namespaces/{namespace}/mirror-identities",
            get(list_identities).post(create_identity),
        )
        .route(
            "/namespaces/{namespace}/mirror-identities/{id}",
            put(update_identity).delete(delete_identity),
        )
}

#[derive(Serialize)]
pub(crate) struct IdentityUsageResponse {
    pub repository: String,
    pub last_attempted_at: Option<chrono::DateTime<Utc>>,
    pub last_synced_at: Option<chrono::DateTime<Utc>>,
    pub last_error: Option<String>,
}

#[derive(Serialize)]
pub(crate) struct MirrorIdentityResponse {
    pub id: Uuid,
    pub name: String,
    pub kind: String,
    pub username: Option<String>,
    pub provider: Option<String>,
    pub instance_url: Option<String>,
    pub public_key: Option<String>,
    pub fingerprint: Option<String>,
    pub last_used_at: Option<chrono::DateTime<Utc>>,
    pub usage_history: Vec<IdentityUsageResponse>,
    pub created_at: chrono::DateTime<Utc>,
    pub updated_at: chrono::DateTime<Utc>,
}

impl From<namespace_mirror_identity::Model> for MirrorIdentityResponse {
    fn from(identity: namespace_mirror_identity::Model) -> Self {
        Self {
            id: identity.id,
            name: identity.name,
            kind: identity.kind,
            username: identity.username,
            provider: identity.provider,
            instance_url: identity.instance_url,
            public_key: identity.public_key,
            fingerprint: identity.fingerprint,
            last_used_at: identity.last_used_at,
            usage_history: Vec::new(),
            created_at: identity.created_at,
            updated_at: identity.updated_at,
        }
    }
}

#[derive(Deserialize)]
struct CreateIdentityRequest {
    name: String,
    token: String,
    server_url: String,
}

#[derive(Deserialize)]
struct UpdateIdentityRequest {
    name: String,
    token: Option<String>,
    server_url: String,
}

#[derive(Clone)]
pub(crate) struct MirrorIdentitySecret {
    pub kind: String,
    pub username: Option<String>,
    pub secret: String,
    pub provider: Option<String>,
    pub instance_url: Option<String>,
}

pub(crate) async fn load_secret(
    state: &IdentityState,
    namespace: &str,
    id: Uuid,
) -> Result<MirrorIdentitySecret, ApiError> {
    let identity = namespace_mirror_identity::Entity::find_by_id(id)
        .filter(namespace_mirror_identity::Column::Namespace.eq(namespace))
        .one(state.database())
        .await?
        .ok_or_else(|| ApiError::bad_request("The selected mirror identity no longer exists."))?;
    Ok(MirrorIdentitySecret {
        kind: identity.kind,
        username: identity.username,
        secret: identity.secret,
        provider: identity.provider,
        instance_url: identity.instance_url,
    })
}

pub(crate) async fn mark_identity_used(
    state: &IdentityState,
    namespace: &str,
    id: Uuid,
    used_at: chrono::DateTime<Utc>,
) -> Result<(), ApiError> {
    let identity = namespace_mirror_identity::Entity::find_by_id(id)
        .filter(namespace_mirror_identity::Column::Namespace.eq(namespace))
        .one(state.database())
        .await?
        .ok_or_else(|| ApiError::bad_request("The selected identity no longer exists."))?;
    let mut active: namespace_mirror_identity::ActiveModel = identity.into();
    active.last_used_at = Set(Some(used_at));
    active.updated_at = Set(used_at);
    active.update(state.database()).await?;
    Ok(())
}

async fn list_identities(
    State(state): State<IdentityState>,
    AxumPath(namespace): AxumPath<String>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<Vec<MirrorIdentityResponse>>, ApiError> {
    let actor = state.authenticate(&headers, &jar, SCOPE_READ).await?;
    authorize_namespace(&state, &actor, &namespace).await?;
    let identities = namespace_mirror_identity::Entity::find()
        .filter(namespace_mirror_identity::Column::Namespace.eq(&namespace))
        .filter(namespace_mirror_identity::Column::Kind.eq("token"))
        .order_by_asc(namespace_mirror_identity::Column::CreatedAt)
        .all(state.database())
        .await?;
    let repositories = repository::Entity::find()
        .filter(repository::Column::Namespace.eq(&namespace))
        .filter(repository::Column::DeletedAt.is_null())
        .all(state.database())
        .await?;
    let repository_names = repositories
        .into_iter()
        .map(|repository| {
            (
                repository.id,
                format!("{}/{}", repository.namespace, repository.name),
            )
        })
        .collect::<HashMap<_, _>>();
    let mirrors = if repository_names.is_empty() {
        Vec::new()
    } else {
        repository_mirror::Entity::find()
            .filter(repository_mirror::Column::RepositoryId.is_in(repository_names.keys().copied()))
            .order_by_desc(repository_mirror::Column::LastAttemptedAt)
            .all(state.database())
            .await?
    };
    Ok(Json(
        identities
            .into_iter()
            .map(|identity| {
                let id = identity.id;
                let mut response = MirrorIdentityResponse::from(identity);
                response.usage_history = mirrors
                    .iter()
                    .filter(|mirror| mirror.identity_id == Some(id))
                    .filter_map(|mirror| {
                        Some(IdentityUsageResponse {
                            repository: repository_names.get(&mirror.repository_id)?.clone(),
                            last_attempted_at: mirror.last_attempted_at,
                            last_synced_at: mirror.last_synced_at,
                            last_error: mirror.last_error.clone(),
                        })
                    })
                    .take(5)
                    .collect();
                response
            })
            .collect(),
    ))
}

async fn create_identity(
    State(state): State<IdentityState>,
    AxumPath(namespace): AxumPath<String>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<CreateIdentityRequest>,
) -> Result<Json<MirrorIdentityResponse>, ApiError> {
    let actor = state.authenticate(&headers, &jar, SCOPE_WRITE).await?;
    authorize_namespace(&state, &actor, &namespace).await?;
    let name = validate_identity_name(&request.name)?;
    let token = validate_token(&request.token)?;
    let (provider, server_url) = detect_provider(&request.server_url, &token).await?;
    let now = Utc::now();
    let transaction = state.database().begin().await?;
    let identity = namespace_mirror_identity::ActiveModel {
        id: Set(Uuid::new_v4()),
        namespace: Set(namespace.clone()),
        name: Set(name),
        kind: Set("token".to_owned()),
        username: Set(None),
        secret: Set(token),
        provider: Set(Some(provider)),
        instance_url: Set(Some(server_url)),
        public_key: Set(None),
        fingerprint: Set(None),
        last_used_at: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(&transaction)
    .await?;
    state
        .audit_on(
            &transaction,
            Some(actor.user.id),
            "repository.identity.create",
            Some(format!("{namespace}:{}", identity.id)),
        )
        .await?;
    transaction.commit().await?;
    Ok(Json(identity.into()))
}

async fn update_identity(
    State(state): State<IdentityState>,
    AxumPath((namespace, id)): AxumPath<(String, Uuid)>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<UpdateIdentityRequest>,
) -> Result<Json<MirrorIdentityResponse>, ApiError> {
    let actor = state.authenticate(&headers, &jar, SCOPE_WRITE).await?;
    authorize_namespace(&state, &actor, &namespace).await?;
    let current = namespace_mirror_identity::Entity::find_by_id(id)
        .filter(namespace_mirror_identity::Column::Namespace.eq(&namespace))
        .filter(namespace_mirror_identity::Column::Kind.eq("token"))
        .one(state.database())
        .await?
        .ok_or_else(ApiError::not_found)?;
    let name = validate_identity_name(&request.name)?;
    let token = request
        .token
        .as_deref()
        .map(validate_token)
        .transpose()?
        .unwrap_or_else(|| current.secret.clone());
    let (provider, server_url) = detect_provider(&request.server_url, &token).await?;
    let transaction = state.database().begin().await?;
    let mut identity: namespace_mirror_identity::ActiveModel = current.into();
    identity.name = Set(name);
    identity.secret = Set(token);
    identity.provider = Set(Some(provider));
    identity.instance_url = Set(Some(server_url));
    identity.updated_at = Set(Utc::now());
    let identity = identity.update(&transaction).await?;
    state
        .audit_on(
            &transaction,
            Some(actor.user.id),
            "repository.identity.update",
            Some(format!("{namespace}:{id}")),
        )
        .await?;
    transaction.commit().await?;
    Ok(Json(identity.into()))
}

async fn delete_identity(
    State(state): State<IdentityState>,
    AxumPath((namespace, id)): AxumPath<(String, Uuid)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<axum::http::StatusCode, ApiError> {
    let actor = state.authenticate(&headers, &jar, SCOPE_WRITE).await?;
    authorize_namespace(&state, &actor, &namespace).await?;
    let transaction = state.database().begin().await?;
    let identity = namespace_mirror_identity::Entity::find_by_id(id)
        .filter(namespace_mirror_identity::Column::Namespace.eq(&namespace))
        .one(&transaction)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let in_use = repository_mirror::Entity::find()
        .filter(repository_mirror::Column::IdentityId.eq(id))
        .one(&transaction)
        .await?
        .is_some()
        || repository_import::Entity::find()
            .filter(repository_import::Column::IdentityId.eq(id))
            .one(&transaction)
            .await?
            .is_some();
    if in_use {
        return Err(ApiError::conflict(
            "This identity is in use. Unlink it from every mirror and import before deleting it.",
        ));
    }
    identity.delete(&transaction).await?;
    state
        .audit_on(
            &transaction,
            Some(actor.user.id),
            "repository.identity.delete",
            Some(format!("{namespace}:{id}")),
        )
        .await?;
    transaction.commit().await?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

fn validate_identity_name(name: &str) -> Result<String, ApiError> {
    let name = name.trim();
    if name.is_empty() || name.len() > MAX_IDENTITY_NAME_LENGTH {
        return Err(ApiError::bad_request(
            "Identity names must contain 1 to 80 characters.",
        ));
    }
    Ok(name.to_owned())
}

fn validate_token(token: &str) -> Result<String, ApiError> {
    let token = token.trim();
    if token.is_empty() || token.len() > MAX_SECRET_LENGTH {
        return Err(ApiError::bad_request(
            "Access tokens must contain 1 to 8192 characters.",
        ));
    }
    Ok(token.to_owned())
}

async fn detect_provider(server_url: &str, token: &str) -> Result<(String, String), ApiError> {
    let origin = normalize_public_https_origin(server_url).map_err(ApiError::bad_request)?;
    let server_url = origin.to_string().trim_end_matches('/').to_owned();
    if origin
        .host_str()
        .is_some_and(|host| host.eq_ignore_ascii_case("github.com"))
    {
        let api = Url::parse("https://api.github.com")
            .map_err(|error| ApiError::internal(format!("GitHub API URL is invalid: {error}")))?;
        let client = pinned_public_https_client(
            &api,
            std::time::Duration::from_secs(20),
            reqwest::redirect::Policy::none(),
        )
        .await
        .map_err(ApiError::bad_request)?;
        if authenticated_get(
            &client,
            endpoint(&api, "/user")?,
            token,
            TokenHeader::Bearer,
        )
        .await
        {
            return Ok(("github".to_owned(), server_url));
        }
        return Err(ApiError::bad_request("GitHub rejected this access token."));
    }

    let client = pinned_public_https_client(
        &origin,
        std::time::Duration::from_secs(20),
        reqwest::redirect::Policy::none(),
    )
    .await
    .map_err(ApiError::bad_request)?;
    if plain_get(&client, endpoint(&origin, "/api/forgejo/v1/version")?).await {
        if authenticated_get(
            &client,
            endpoint(&origin, "/api/v1/user")?,
            token,
            TokenHeader::Bearer,
        )
        .await
        {
            return Ok(("forgejo".to_owned(), server_url));
        }
        return Err(ApiError::bad_request("Forgejo rejected this access token."));
    }
    if plain_get(&client, endpoint(&origin, "/api/v1/version")?).await {
        if authenticated_get(
            &client,
            endpoint(&origin, "/api/v1/user")?,
            token,
            TokenHeader::Bearer,
        )
        .await
        {
            return Ok(("gitea".to_owned(), server_url));
        }
        return Err(ApiError::bad_request("Gitea rejected this access token."));
    }
    if authenticated_get(
        &client,
        endpoint(&origin, "/api/v4/user")?,
        token,
        TokenHeader::Gitlab,
    )
    .await
    {
        return Ok(("gitlab".to_owned(), server_url));
    }
    if authenticated_get(
        &client,
        endpoint(&origin, "/api/v3/user")?,
        token,
        TokenHeader::Bearer,
    )
    .await
    {
        return Ok(("github".to_owned(), server_url));
    }
    Err(ApiError::bad_request(
        "Could not identify a supported GitHub, GitLab, Gitea, or Forgejo server with this token.",
    ))
}

fn endpoint(origin: &Url, path: &str) -> Result<Url, ApiError> {
    origin
        .join(path)
        .map_err(|error| ApiError::internal(format!("Could not build provider endpoint: {error}")))
}

#[derive(Clone, Copy)]
enum TokenHeader {
    Bearer,
    Gitlab,
}

async fn plain_get(client: &Client, url: Url) -> bool {
    client
        .get(url)
        .header(header::USER_AGENT, "Gitadel")
        .send()
        .await
        .is_ok_and(|response| response.status() == StatusCode::OK)
}

async fn authenticated_get(
    client: &Client,
    url: Url,
    token: &str,
    header_kind: TokenHeader,
) -> bool {
    let request = client.get(url).header(header::USER_AGENT, "Gitadel");
    let request = match header_kind {
        TokenHeader::Bearer => request.bearer_auth(token),
        TokenHeader::Gitlab => request.header("PRIVATE-TOKEN", token),
    };
    request
        .send()
        .await
        .is_ok_and(|response| response.status() == StatusCode::OK)
}

#[cfg(test)]
mod tests {
    use super::{validate_identity_name, validate_token};

    #[test]
    fn identity_requires_a_name() {
        assert!(validate_identity_name(" ").is_err());
    }

    #[test]
    fn identity_requires_an_access_token() {
        assert!(validate_token(" ").is_err());
    }
}
