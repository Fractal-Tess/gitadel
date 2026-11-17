//! Namespace mirror credentials and provider identification.
use crate::{IdentityError, Principal, runtime::AuthService};
use chrono::Utc;
use gitadel_core::network::{normalize_public_https_origin, pinned_public_https_client};
use gitadel_db::entity::{
    namespace_mirror_identity, repository, repository_import, repository_mirror,
};
use gitadel_secrets::SecretKind;
use reqwest::{Client, StatusCode, header};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, ModelTrait, QueryFilter, QueryOrder, Set,
    TransactionTrait,
};
use std::collections::HashMap;
use url::Url;
use uuid::Uuid;
const MAX_IDENTITY_NAME_LENGTH: usize = 80;
const MAX_SECRET_LENGTH: usize = 8_192;
pub struct IdentityUsageResponse {
    pub repository: String,
    pub last_attempted_at: Option<chrono::DateTime<Utc>>,
    pub last_synced_at: Option<chrono::DateTime<Utc>>,
    pub last_error: Option<String>,
}
pub struct MirrorIdentityResponse {
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
pub struct CreateIdentityRequest {
    pub name: String,
    pub token: String,
    pub server_url: String,
}
pub struct UpdateIdentityRequest {
    pub name: String,
    pub token: Option<String>,
    pub server_url: String,
}
pub struct MirrorIdentitySecret {
    pub kind: String,
    pub username: Option<String>,
    pub secret: String,
    pub provider: Option<String>,
    pub instance_url: Option<String>,
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
pub async fn load_secret(
    state: &AuthService<'_>,
    namespace: &str,
    id: Uuid,
) -> Result<MirrorIdentitySecret, IdentityError> {
    let identity = namespace_mirror_identity::Entity::find_by_id(id)
        .filter(namespace_mirror_identity::Column::Namespace.eq(namespace))
        .one(state.database())
        .await?
        .ok_or_else(|| {
            IdentityError::bad_request("The selected mirror identity no longer exists.")
        })?;
    let secret = open_secret(state, &identity)?;
    Ok(MirrorIdentitySecret {
        kind: identity.kind,
        username: identity.username,
        secret,
        provider: identity.provider,
        instance_url: identity.instance_url,
    })
}
pub fn seal_secret(
    state: &AuthService<'_>,
    id: Uuid,
    secret: &str,
) -> Result<String, IdentityError> {
    state
        .secret_cipher()
        .seal_text(SecretKind::MirrorIdentitySecret, &id, secret)
        .map_err(IdentityError::internal)
}
pub fn open_secret(
    state: &AuthService<'_>,
    identity: &namespace_mirror_identity::Model,
) -> Result<String, IdentityError> {
    Ok(state.secret_cipher().open_text(
        SecretKind::MirrorIdentitySecret,
        &identity.id,
        &identity.secret,
    )?)
}
pub async fn mark_identity_used(
    state: &AuthService<'_>,
    namespace: &str,
    id: Uuid,
    used_at: chrono::DateTime<Utc>,
) -> Result<(), IdentityError> {
    let identity = namespace_mirror_identity::Entity::find_by_id(id)
        .filter(namespace_mirror_identity::Column::Namespace.eq(namespace))
        .one(state.database())
        .await?
        .ok_or_else(|| IdentityError::bad_request("The selected identity no longer exists."))?;
    let mut active: namespace_mirror_identity::ActiveModel = identity.into();
    active.last_used_at = Set(Some(used_at));
    active.updated_at = Set(used_at);
    active.update(state.database()).await?;
    Ok(())
}
pub fn validate_identity_name(name: &str) -> Result<String, IdentityError> {
    let name = name.trim();
    if name.is_empty() || name.len() > MAX_IDENTITY_NAME_LENGTH {
        return Err(IdentityError::bad_request(
            "Identity names must contain 1 to 80 characters.",
        ));
    }
    Ok(name.to_owned())
}
pub fn validate_token(token: &str) -> Result<String, IdentityError> {
    let token = token.trim();
    if token.is_empty() || token.len() > MAX_SECRET_LENGTH {
        return Err(IdentityError::bad_request(
            "Access tokens must contain 1 to 8192 characters.",
        ));
    }
    Ok(token.to_owned())
}
pub async fn detect_provider(
    server_url: &str,
    token: &str,
) -> Result<(String, String), IdentityError> {
    let origin = normalize_public_https_origin(server_url).map_err(IdentityError::bad_request)?;
    let server_url = origin.to_string().trim_end_matches('/').to_owned();
    if origin
        .host_str()
        .is_some_and(|host| host.eq_ignore_ascii_case("github.com"))
    {
        let api = Url::parse("https://api.github.com").map_err(|error| {
            IdentityError::internal(format!("GitHub API URL is invalid: {error}"))
        })?;
        let client = pinned_public_https_client(
            &api,
            std::time::Duration::from_secs(20),
            reqwest::redirect::Policy::none(),
        )
        .await
        .map_err(IdentityError::bad_request)?;
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
        return Err(IdentityError::bad_request(
            "GitHub rejected this access token.",
        ));
    }

    let client = pinned_public_https_client(
        &origin,
        std::time::Duration::from_secs(20),
        reqwest::redirect::Policy::none(),
    )
    .await
    .map_err(IdentityError::bad_request)?;
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
        return Err(IdentityError::bad_request(
            "Forgejo rejected this access token.",
        ));
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
        return Err(IdentityError::bad_request(
            "Gitea rejected this access token.",
        ));
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
    Err(IdentityError::bad_request(
        "Could not identify a supported GitHub, GitLab, Gitea, or Forgejo server with this token.",
    ))
}
pub fn endpoint(origin: &Url, path: &str) -> Result<Url, IdentityError> {
    origin.join(path).map_err(|error| {
        IdentityError::internal(format!("Could not build provider endpoint: {error}"))
    })
}
pub async fn plain_get(client: &Client, url: Url) -> bool {
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
#[derive(Clone, Copy)]
enum TokenHeader {
    Bearer,
    Gitlab,
}
pub async fn list_identities(
    state: &AuthService<'_>,
    actor: &Principal,
    namespace: String,
) -> Result<Vec<MirrorIdentityResponse>, IdentityError> {
    let actor = state.require_actor(actor, false, crate::SCOPE_READ).await?;
    crate::permissions::authorize_namespace(state.database(), &actor, &namespace).await?;
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
    Ok(identities
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
        .collect())
}
pub async fn create_identity(
    state: &AuthService<'_>,
    actor: &Principal,
    namespace: String,
    request: CreateIdentityRequest,
) -> Result<MirrorIdentityResponse, IdentityError> {
    let actor = state
        .require_actor(actor, false, crate::SCOPE_WRITE)
        .await?;
    crate::permissions::authorize_namespace(state.database(), &actor, &namespace).await?;
    let name = validate_identity_name(&request.name)?;
    let token = validate_token(&request.token)?;
    let (provider, server_url) = detect_provider(&request.server_url, &token).await?;
    let now = Utc::now();
    let id = Uuid::new_v4();
    let sealed = seal_secret(state, id, &token)?;
    let transaction = state.database().begin().await?;
    let identity = namespace_mirror_identity::ActiveModel {
        id: Set(id),
        namespace: Set(namespace.clone()),
        name: Set(name),
        kind: Set("token".to_owned()),
        username: Set(None),
        secret: Set(sealed),
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
    Ok(identity.into())
}
pub async fn update_identity(
    state: &AuthService<'_>,
    actor: &Principal,
    namespace: String,
    id: Uuid,
    request: UpdateIdentityRequest,
) -> Result<MirrorIdentityResponse, IdentityError> {
    let actor = state
        .require_actor(actor, false, crate::SCOPE_WRITE)
        .await?;
    crate::permissions::authorize_namespace(state.database(), &actor, &namespace).await?;
    let current = namespace_mirror_identity::Entity::find_by_id(id)
        .filter(namespace_mirror_identity::Column::Namespace.eq(&namespace))
        .filter(namespace_mirror_identity::Column::Kind.eq("token"))
        .one(state.database())
        .await?
        .ok_or_else(IdentityError::not_found)?;
    let name = validate_identity_name(&request.name)?;
    let token = match request.token.as_deref() {
        Some(token) => validate_token(token)?,
        None => open_secret(state, &current)?,
    };
    let (provider, server_url) = detect_provider(&request.server_url, &token).await?;
    let sealed = seal_secret(state, id, &token)?;
    let transaction = state.database().begin().await?;
    let mut identity: namespace_mirror_identity::ActiveModel = current.into();
    identity.name = Set(name);
    identity.secret = Set(sealed);
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
    Ok(identity.into())
}
pub async fn delete_identity(
    state: &AuthService<'_>,
    actor: &Principal,
    namespace: String,
    id: Uuid,
) -> Result<(), IdentityError> {
    let actor = state
        .require_actor(actor, false, crate::SCOPE_WRITE)
        .await?;
    crate::permissions::authorize_namespace(state.database(), &actor, &namespace).await?;
    let transaction = state.database().begin().await?;
    let identity = namespace_mirror_identity::Entity::find_by_id(id)
        .filter(namespace_mirror_identity::Column::Namespace.eq(&namespace))
        .one(&transaction)
        .await?
        .ok_or_else(IdentityError::not_found)?;
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
        return Err(IdentityError::conflict(
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
    Ok(())
}
