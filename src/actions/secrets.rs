//! Actions secrets and variables at repository and namespace scope.
//!
//! Secrets are sealed with AES-256-GCM under a key derived from the SSH host
//! key, which is already a persistent, private, backed-up instance secret.
//! Secret values are write-only: the API only ever returns their names.
//! Repository values override namespace values with the same name.

use std::collections::{BTreeMap, HashMap};

use aes_gcm::{
    Aes256Gcm, KeyInit, Nonce,
    aead::{Aead, Payload},
};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    routing::{get, put},
};
use axum_extra::extract::cookie::CookieJar;
use chrono::Utc;
use hmac::{Hmac, Mac};
use russh::keys::{PrivateKey, ssh_encoding::Encode};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, IntoActiveModel, PaginatorTrait,
    QueryFilter, QueryOrder, Set,
};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use uuid::Uuid;

use crate::{
    entity::{action_secret, action_variable, repository},
    identity::{ApiError, AuthenticatedUser, SCOPE_READ, SCOPE_WRITE},
    repository::Permission,
};

use super::ActionsState;

const KEY_CONTEXT: &[u8] = b"gitadel actions secrets v1";
const NONCE_BYTES: usize = 12;
const MAX_NAME_BYTES: usize = 100;
pub(crate) const MAX_VALUE_BYTES: usize = 64 * 1024;
const MAX_ENTRIES_PER_SCOPE: u64 = 500;
const RESERVED_PREFIXES: [&str; 3] = ["GITHUB_", "GITEA_", "FORGEJO_"];

/// Seals and opens stored secret values.
pub(crate) struct SecretCipher {
    cipher: Aes256Gcm,
}

impl SecretCipher {
    /// Derives the sealing key from the SSH host key's private key material.
    ///
    /// Replacing the host key makes previously stored secrets unreadable; they
    /// must then be set again.
    pub(crate) fn from_host_key(key: &PrivateKey) -> anyhow::Result<Self> {
        let material = key
            .key_data()
            .encode_vec()
            .map_err(|error| anyhow::anyhow!("could not encode the SSH host key: {error}"))?;
        Self::from_material(&material)
    }

    fn from_material(material: &[u8]) -> anyhow::Result<Self> {
        let mut mac = <Hmac<Sha256> as hmac::KeyInit>::new_from_slice(KEY_CONTEXT)
            .map_err(|_| anyhow::anyhow!("invalid secret key context"))?;
        mac.update(material);
        let key = mac.finalize().into_bytes();
        let cipher = Aes256Gcm::new_from_slice(&key)
            .map_err(|_| anyhow::anyhow!("invalid derived secret key length"))?;
        Ok(Self { cipher })
    }

    fn associated_data(scope: &Scope, name: &str) -> Vec<u8> {
        // Namespace slugs change on rename, so bind to stable identifiers only.
        let scope = match scope {
            Scope::Repository(id) => format!("repository:{id}"),
            Scope::Namespace(_) => "namespace".to_owned(),
        };
        format!("gitadel-actions-secret:v1:{scope}:{name}").into_bytes()
    }

    fn seal(&self, scope: &Scope, name: &str, value: &str) -> Result<Vec<u8>, ApiError> {
        let nonce_bytes: [u8; NONCE_BYTES] = rand::random();
        let nonce = Nonce::from(nonce_bytes);
        let aad = Self::associated_data(scope, name);
        let ciphertext = self
            .cipher
            .encrypt(
                &nonce,
                Payload {
                    msg: value.as_bytes(),
                    aad: &aad,
                },
            )
            .map_err(|_| ApiError::internal("could not seal the secret"))?;
        let mut sealed = Vec::with_capacity(NONCE_BYTES + ciphertext.len());
        sealed.extend_from_slice(&nonce_bytes);
        sealed.extend_from_slice(&ciphertext);
        Ok(sealed)
    }

    fn open(&self, scope: &Scope, name: &str, sealed: &[u8]) -> Option<String> {
        let (nonce, ciphertext) = sealed.split_at_checked(NONCE_BYTES)?;
        let nonce = Nonce::try_from(nonce).ok()?;
        let aad = Self::associated_data(scope, name);
        let plaintext = self
            .cipher
            .decrypt(
                &nonce,
                Payload {
                    msg: ciphertext,
                    aad: &aad,
                },
            )
            .ok()?;
        String::from_utf8(plaintext).ok()
    }
}

#[derive(Clone, Debug)]
enum Scope {
    Repository(Uuid),
    Namespace(String),
}

/// Validates and canonicalizes a secret or variable name like GitHub does:
/// ASCII letters, digits, and underscores, not starting with a digit or a
/// reserved `GITHUB_`, `GITEA_`, or `FORGEJO_` prefix. Names are uppercased.
pub(crate) fn canonical_name(name: &str) -> Result<String, String> {
    let name = name.trim().to_ascii_uppercase();
    if name.is_empty() || name.len() > MAX_NAME_BYTES {
        return Err(format!(
            "Names must contain 1 to {MAX_NAME_BYTES} characters."
        ));
    }
    if !name
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Err("Names may contain only letters, digits, and underscores.".to_owned());
    }
    if name.as_bytes()[0].is_ascii_digit() {
        return Err("Names must not start with a digit.".to_owned());
    }
    if let Some(prefix) = RESERVED_PREFIXES
        .iter()
        .find(|prefix| name.starts_with(**prefix))
    {
        return Err(format!(
            "Names must not start with the reserved {prefix} prefix."
        ));
    }
    Ok(name)
}

/// Secrets and variables visible to a job of `repository`.
pub(crate) struct JobValues {
    pub(crate) secrets: HashMap<String, String>,
    pub(crate) vars: HashMap<String, String>,
}

pub(crate) async fn job_values<C: ConnectionTrait>(
    database: &C,
    cipher: &SecretCipher,
    repository: &repository::Model,
) -> Result<JobValues, sea_orm::DbErr> {
    let mut secrets = BTreeMap::new();
    let namespace_secrets = action_secret::Entity::find()
        .filter(action_secret::Column::Namespace.eq(&repository.namespace))
        .all(database)
        .await?;
    let repository_secrets = action_secret::Entity::find()
        .filter(action_secret::Column::RepositoryId.eq(repository.id))
        .all(database)
        .await?;
    // Repository rows are applied last so they override namespace rows.
    for (scope, rows) in [
        (
            Scope::Namespace(repository.namespace.clone()),
            namespace_secrets,
        ),
        (Scope::Repository(repository.id), repository_secrets),
    ] {
        for row in rows {
            match cipher.open(&scope, &row.name, &row.sealed) {
                Some(value) => {
                    secrets.insert(row.name, value);
                }
                None => tracing::warn!(
                    repository_id = %repository.id,
                    secret = row.name,
                    "could not open an Actions secret; was the SSH host key replaced?"
                ),
            }
        }
    }
    let mut vars = BTreeMap::new();
    let namespace_vars = action_variable::Entity::find()
        .filter(action_variable::Column::Namespace.eq(&repository.namespace))
        .all(database)
        .await?;
    let repository_vars = action_variable::Entity::find()
        .filter(action_variable::Column::RepositoryId.eq(repository.id))
        .all(database)
        .await?;
    for row in namespace_vars.into_iter().chain(repository_vars) {
        vars.insert(row.name, row.value);
    }
    Ok(JobValues {
        secrets: secrets.into_iter().collect(),
        vars: vars.into_iter().collect(),
    })
}

pub(crate) fn router() -> Router<ActionsState> {
    Router::new()
        .route(
            "/repositories/{namespace}/{name}/actions/secrets",
            get(list_repository_secrets),
        )
        .route(
            "/repositories/{namespace}/{name}/actions/secrets/{secret}",
            put(set_repository_secret).delete(delete_repository_secret),
        )
        .route(
            "/repositories/{namespace}/{name}/actions/variables",
            get(list_repository_variables),
        )
        .route(
            "/repositories/{namespace}/{name}/actions/variables/{variable}",
            put(set_repository_variable).delete(delete_repository_variable),
        )
        .route(
            "/namespaces/{namespace}/actions/secrets",
            get(list_namespace_secrets),
        )
        .route(
            "/namespaces/{namespace}/actions/secrets/{secret}",
            put(set_namespace_secret).delete(delete_namespace_secret),
        )
        .route(
            "/namespaces/{namespace}/actions/variables",
            get(list_namespace_variables),
        )
        .route(
            "/namespaces/{namespace}/actions/variables/{variable}",
            put(set_namespace_variable).delete(delete_namespace_variable),
        )
}

#[derive(Serialize)]
struct SecretResponse {
    name: String,
    created_at: chrono::DateTime<Utc>,
    updated_at: chrono::DateTime<Utc>,
}

#[derive(Serialize)]
struct SecretsResponse {
    secrets: Vec<SecretResponse>,
}

#[derive(Serialize)]
struct VariableResponse {
    name: String,
    value: String,
    created_at: chrono::DateTime<Utc>,
    updated_at: chrono::DateTime<Utc>,
}

#[derive(Serialize)]
struct VariablesResponse {
    variables: Vec<VariableResponse>,
}

#[derive(Deserialize)]
struct ValueRequest {
    value: String,
}

async fn authorize_repository(
    state: &ActionsState,
    headers: &HeaderMap,
    jar: &CookieJar,
    namespace: &str,
    name: &str,
    scope: i32,
) -> Result<(AuthenticatedUser, Scope, String), ApiError> {
    let (actor, repository) = state
        .repository()
        .authenticated_repository(headers, jar, namespace, name, Permission::Manage, scope)
        .await?;
    let target = format!("{}/{}", repository.namespace, repository.name);
    Ok((actor, Scope::Repository(repository.id), target))
}

async fn authorize_namespace(
    state: &ActionsState,
    headers: &HeaderMap,
    jar: &CookieJar,
    namespace: &str,
    scope: i32,
) -> Result<(AuthenticatedUser, Scope, String), ApiError> {
    let identity = state.repository().identity();
    let actor = identity.authenticate(headers, jar, scope).await?;
    crate::identity::authorize_namespace(identity, &actor, namespace).await?;
    Ok((
        actor,
        Scope::Namespace(namespace.to_owned()),
        namespace.to_owned(),
    ))
}

macro_rules! scoped_filter {
    ($entity:ident, $select:expr, $scope:expr) => {
        match $scope {
            Scope::Repository(id) => $select.filter($entity::Column::RepositoryId.eq(*id)),
            Scope::Namespace(slug) => $select.filter($entity::Column::Namespace.eq(slug.as_str())),
        }
    };
}

fn validated_name(name: &str) -> Result<String, ApiError> {
    canonical_name(name).map_err(ApiError::bad_request)
}

fn validated_value(value: &str) -> Result<(), ApiError> {
    if value.len() > MAX_VALUE_BYTES {
        return Err(ApiError::bad_request(format!(
            "Values must not exceed {MAX_VALUE_BYTES} bytes."
        )));
    }
    if value.contains('\0') {
        return Err(ApiError::bad_request("Values must not contain NUL bytes."));
    }
    Ok(())
}

async fn list_secrets(state: &ActionsState, scope: &Scope) -> Result<SecretsResponse, ApiError> {
    let select = scoped_filter!(action_secret, action_secret::Entity::find(), scope);
    let secrets = select
        .order_by_asc(action_secret::Column::Name)
        .all(state.repository().identity().database())
        .await?
        .into_iter()
        .map(|row| SecretResponse {
            name: row.name,
            created_at: row.created_at,
            updated_at: row.updated_at,
        })
        .collect();
    Ok(SecretsResponse { secrets })
}

async fn set_secret(
    state: &ActionsState,
    actor: &AuthenticatedUser,
    scope: &Scope,
    target: &str,
    name: &str,
    value: &str,
) -> Result<SecretResponse, ApiError> {
    let name = validated_name(name)?;
    validated_value(value)?;
    let database = state.repository().identity().database();
    let sealed = state.secret_cipher().seal(scope, &name, value)?;
    let now = Utc::now();
    let existing = scoped_filter!(action_secret, action_secret::Entity::find(), scope)
        .filter(action_secret::Column::Name.eq(&name))
        .one(database)
        .await?;
    let row = match existing {
        Some(row) => {
            let mut active = row.into_active_model();
            active.sealed = Set(sealed);
            active.updated_by = Set(Some(actor.user.id));
            active.updated_at = Set(now);
            active.update(database).await?
        }
        None => {
            let count = scoped_filter!(action_secret, action_secret::Entity::find(), scope)
                .count(database)
                .await?;
            if count >= MAX_ENTRIES_PER_SCOPE {
                return Err(ApiError::conflict(format!(
                    "At most {MAX_ENTRIES_PER_SCOPE} secrets may be stored here."
                )));
            }
            let (repository_id, namespace) = scope_columns(scope);
            action_secret::ActiveModel {
                id: Default::default(),
                repository_id: Set(repository_id),
                namespace: Set(namespace),
                name: Set(name.clone()),
                sealed: Set(sealed),
                updated_by: Set(Some(actor.user.id)),
                created_at: Set(now),
                updated_at: Set(now),
            }
            .insert(database)
            .await?
        }
    };
    state
        .repository()
        .identity()
        .audit(
            Some(actor.user.id),
            "actions.secret.set",
            Some(format!("{target}:{name}")),
        )
        .await?;
    Ok(SecretResponse {
        name: row.name,
        created_at: row.created_at,
        updated_at: row.updated_at,
    })
}

async fn delete_secret(
    state: &ActionsState,
    actor: &AuthenticatedUser,
    scope: &Scope,
    target: &str,
    name: &str,
) -> Result<StatusCode, ApiError> {
    let name = validated_name(name)?;
    let database = state.repository().identity().database();
    let deleted = scoped_filter!(action_secret, action_secret::Entity::delete_many(), scope)
        .filter(action_secret::Column::Name.eq(&name))
        .exec(database)
        .await?;
    if deleted.rows_affected == 0 {
        return Err(ApiError::not_found());
    }
    state
        .repository()
        .identity()
        .audit(
            Some(actor.user.id),
            "actions.secret.delete",
            Some(format!("{target}:{name}")),
        )
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_variables(
    state: &ActionsState,
    scope: &Scope,
) -> Result<VariablesResponse, ApiError> {
    let select = scoped_filter!(action_variable, action_variable::Entity::find(), scope);
    let variables = select
        .order_by_asc(action_variable::Column::Name)
        .all(state.repository().identity().database())
        .await?
        .into_iter()
        .map(variable_response)
        .collect();
    Ok(VariablesResponse { variables })
}

fn variable_response(row: action_variable::Model) -> VariableResponse {
    VariableResponse {
        name: row.name,
        value: row.value,
        created_at: row.created_at,
        updated_at: row.updated_at,
    }
}

async fn set_variable(
    state: &ActionsState,
    actor: &AuthenticatedUser,
    scope: &Scope,
    target: &str,
    name: &str,
    value: String,
) -> Result<VariableResponse, ApiError> {
    let name = validated_name(name)?;
    validated_value(&value)?;
    let database = state.repository().identity().database();
    let now = Utc::now();
    let existing = scoped_filter!(action_variable, action_variable::Entity::find(), scope)
        .filter(action_variable::Column::Name.eq(&name))
        .one(database)
        .await?;
    let row = match existing {
        Some(row) => {
            let mut active = row.into_active_model();
            active.value = Set(value);
            active.updated_by = Set(Some(actor.user.id));
            active.updated_at = Set(now);
            active.update(database).await?
        }
        None => {
            let count = scoped_filter!(action_variable, action_variable::Entity::find(), scope)
                .count(database)
                .await?;
            if count >= MAX_ENTRIES_PER_SCOPE {
                return Err(ApiError::conflict(format!(
                    "At most {MAX_ENTRIES_PER_SCOPE} variables may be stored here."
                )));
            }
            let (repository_id, namespace) = scope_columns(scope);
            action_variable::ActiveModel {
                id: Default::default(),
                repository_id: Set(repository_id),
                namespace: Set(namespace),
                name: Set(name.clone()),
                value: Set(value),
                updated_by: Set(Some(actor.user.id)),
                created_at: Set(now),
                updated_at: Set(now),
            }
            .insert(database)
            .await?
        }
    };
    state
        .repository()
        .identity()
        .audit(
            Some(actor.user.id),
            "actions.variable.set",
            Some(format!("{target}:{name}")),
        )
        .await?;
    Ok(variable_response(row))
}

async fn delete_variable(
    state: &ActionsState,
    actor: &AuthenticatedUser,
    scope: &Scope,
    target: &str,
    name: &str,
) -> Result<StatusCode, ApiError> {
    let name = validated_name(name)?;
    let database = state.repository().identity().database();
    let deleted = scoped_filter!(
        action_variable,
        action_variable::Entity::delete_many(),
        scope
    )
    .filter(action_variable::Column::Name.eq(&name))
    .exec(database)
    .await?;
    if deleted.rows_affected == 0 {
        return Err(ApiError::not_found());
    }
    state
        .repository()
        .identity()
        .audit(
            Some(actor.user.id),
            "actions.variable.delete",
            Some(format!("{target}:{name}")),
        )
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

fn scope_columns(scope: &Scope) -> (Option<Uuid>, Option<String>) {
    match scope {
        Scope::Repository(id) => (Some(*id), None),
        Scope::Namespace(slug) => (None, Some(slug.clone())),
    }
}

async fn list_repository_secrets(
    State(state): State<ActionsState>,
    Path((namespace, name)): Path<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<SecretsResponse>, ApiError> {
    let (_, scope, _) =
        authorize_repository(&state, &headers, &jar, &namespace, &name, SCOPE_READ).await?;
    Ok(Json(list_secrets(&state, &scope).await?))
}

async fn set_repository_secret(
    State(state): State<ActionsState>,
    Path((namespace, name, secret)): Path<(String, String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<ValueRequest>,
) -> Result<Json<SecretResponse>, ApiError> {
    let (actor, scope, target) =
        authorize_repository(&state, &headers, &jar, &namespace, &name, SCOPE_WRITE).await?;
    Ok(Json(
        set_secret(&state, &actor, &scope, &target, &secret, &request.value).await?,
    ))
}

async fn delete_repository_secret(
    State(state): State<ActionsState>,
    Path((namespace, name, secret)): Path<(String, String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<StatusCode, ApiError> {
    let (actor, scope, target) =
        authorize_repository(&state, &headers, &jar, &namespace, &name, SCOPE_WRITE).await?;
    delete_secret(&state, &actor, &scope, &target, &secret).await
}

async fn list_repository_variables(
    State(state): State<ActionsState>,
    Path((namespace, name)): Path<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<VariablesResponse>, ApiError> {
    let (_, scope, _) =
        authorize_repository(&state, &headers, &jar, &namespace, &name, SCOPE_READ).await?;
    Ok(Json(list_variables(&state, &scope).await?))
}

async fn set_repository_variable(
    State(state): State<ActionsState>,
    Path((namespace, name, variable)): Path<(String, String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<ValueRequest>,
) -> Result<Json<VariableResponse>, ApiError> {
    let (actor, scope, target) =
        authorize_repository(&state, &headers, &jar, &namespace, &name, SCOPE_WRITE).await?;
    Ok(Json(
        set_variable(&state, &actor, &scope, &target, &variable, request.value).await?,
    ))
}

async fn delete_repository_variable(
    State(state): State<ActionsState>,
    Path((namespace, name, variable)): Path<(String, String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<StatusCode, ApiError> {
    let (actor, scope, target) =
        authorize_repository(&state, &headers, &jar, &namespace, &name, SCOPE_WRITE).await?;
    delete_variable(&state, &actor, &scope, &target, &variable).await
}

async fn list_namespace_secrets(
    State(state): State<ActionsState>,
    Path(namespace): Path<String>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<SecretsResponse>, ApiError> {
    let (_, scope, _) = authorize_namespace(&state, &headers, &jar, &namespace, SCOPE_READ).await?;
    Ok(Json(list_secrets(&state, &scope).await?))
}

async fn set_namespace_secret(
    State(state): State<ActionsState>,
    Path((namespace, secret)): Path<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<ValueRequest>,
) -> Result<Json<SecretResponse>, ApiError> {
    let (actor, scope, target) =
        authorize_namespace(&state, &headers, &jar, &namespace, SCOPE_WRITE).await?;
    Ok(Json(
        set_secret(&state, &actor, &scope, &target, &secret, &request.value).await?,
    ))
}

async fn delete_namespace_secret(
    State(state): State<ActionsState>,
    Path((namespace, secret)): Path<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<StatusCode, ApiError> {
    let (actor, scope, target) =
        authorize_namespace(&state, &headers, &jar, &namespace, SCOPE_WRITE).await?;
    delete_secret(&state, &actor, &scope, &target, &secret).await
}

async fn list_namespace_variables(
    State(state): State<ActionsState>,
    Path(namespace): Path<String>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<VariablesResponse>, ApiError> {
    let (_, scope, _) = authorize_namespace(&state, &headers, &jar, &namespace, SCOPE_READ).await?;
    Ok(Json(list_variables(&state, &scope).await?))
}

async fn set_namespace_variable(
    State(state): State<ActionsState>,
    Path((namespace, variable)): Path<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<ValueRequest>,
) -> Result<Json<VariableResponse>, ApiError> {
    let (actor, scope, target) =
        authorize_namespace(&state, &headers, &jar, &namespace, SCOPE_WRITE).await?;
    Ok(Json(
        set_variable(&state, &actor, &scope, &target, &variable, request.value).await?,
    ))
}

async fn delete_namespace_variable(
    State(state): State<ActionsState>,
    Path((namespace, variable)): Path<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<StatusCode, ApiError> {
    let (actor, scope, target) =
        authorize_namespace(&state, &headers, &jar, &namespace, SCOPE_WRITE).await?;
    delete_variable(&state, &actor, &scope, &target, &variable).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_follow_github_rules() {
        assert_eq!(canonical_name("deploy_key").unwrap(), "DEPLOY_KEY");
        assert_eq!(canonical_name("_X9").unwrap(), "_X9");
        for invalid in [
            "",
            "9LIVES",
            "HAS SPACE",
            "DASH-ED",
            "GITHUB_TOKEN",
            "gitea_token",
            "FORGEJO_X",
            "ÄPFEL",
        ] {
            assert!(canonical_name(invalid).is_err(), "{invalid}");
        }
        assert!(canonical_name(&"A".repeat(MAX_NAME_BYTES + 1)).is_err());
    }

    #[test]
    fn sealed_secrets_round_trip_and_bind_to_scope_and_name() {
        let cipher = SecretCipher::from_material(b"host key material").unwrap();
        let repository = Scope::Repository(Uuid::new_v4());
        let sealed = cipher.seal(&repository, "TOKEN", "hunter2").unwrap();
        assert!(!sealed.windows(7).any(|window| window == b"hunter2"));
        assert_eq!(
            cipher.open(&repository, "TOKEN", &sealed).as_deref(),
            Some("hunter2")
        );
        assert!(cipher.open(&repository, "OTHER", &sealed).is_none());
        assert!(
            cipher
                .open(&Scope::Repository(Uuid::new_v4()), "TOKEN", &sealed)
                .is_none()
        );
        let other_key = SecretCipher::from_material(b"another host key").unwrap();
        assert!(other_key.open(&repository, "TOKEN", &sealed).is_none());
        assert!(cipher.open(&repository, "TOKEN", &sealed[..8]).is_none());
    }

    #[test]
    fn nonces_are_unique_per_seal() {
        let cipher = SecretCipher::from_material(b"host key material").unwrap();
        let scope = Scope::Namespace("team".to_owned());
        let first = cipher.seal(&scope, "A", "same").unwrap();
        let second = cipher.seal(&scope, "A", "same").unwrap();
        assert_ne!(first, second);
        assert_eq!(cipher.open(&scope, "A", &first).as_deref(), Some("same"));
    }

    async fn insert_secret(
        database: &sea_orm::DatabaseConnection,
        cipher: &SecretCipher,
        scope: Scope,
        name: &str,
        value: &str,
    ) {
        let (repository_id, namespace) = scope_columns(&scope);
        let now = Utc::now();
        action_secret::ActiveModel {
            id: Default::default(),
            repository_id: Set(repository_id),
            namespace: Set(namespace),
            name: Set(name.to_owned()),
            sealed: Set(cipher.seal(&scope, name, value).unwrap()),
            updated_by: Set(None),
            created_at: Set(now),
            updated_at: Set(now),
        }
        .insert(database)
        .await
        .unwrap();
    }

    async fn insert_variable(
        database: &sea_orm::DatabaseConnection,
        scope: Scope,
        name: &str,
        value: &str,
    ) {
        let (repository_id, namespace) = scope_columns(&scope);
        let now = Utc::now();
        action_variable::ActiveModel {
            id: Default::default(),
            repository_id: Set(repository_id),
            namespace: Set(namespace),
            name: Set(name.to_owned()),
            value: Set(value.to_owned()),
            updated_by: Set(None),
            created_at: Set(now),
            updated_at: Set(now),
        }
        .insert(database)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn repository_values_override_namespace_values() {
        use crate::actions::test_support;
        let database = test_support::database().await;
        let alice = test_support::owner(&database, "alice").await;
        test_support::owner(&database, "bob").await;
        let project = test_support::repository(&database, &alice, "project").await;
        let cipher = SecretCipher::from_material(b"host key").unwrap();
        let namespace = || Scope::Namespace("alice".to_owned());
        insert_secret(&database, &cipher, namespace(), "SHARED", "namespace").await;
        insert_secret(&database, &cipher, namespace(), "ONLY_NAMESPACE", "n").await;
        insert_secret(
            &database,
            &cipher,
            Scope::Repository(project.id),
            "SHARED",
            "repo",
        )
        .await;
        insert_secret(
            &database,
            &cipher,
            Scope::Namespace("bob".to_owned()),
            "BOB",
            "x",
        )
        .await;
        insert_variable(&database, namespace(), "REGION", "eu").await;
        insert_variable(&database, Scope::Repository(project.id), "REGION", "us").await;
        insert_variable(&database, namespace(), "TIER", "gold").await;

        let values = job_values(&database, &cipher, &project).await.unwrap();

        assert_eq!(
            values.secrets.get("SHARED").map(String::as_str),
            Some("repo")
        );
        assert_eq!(
            values.secrets.get("ONLY_NAMESPACE").map(String::as_str),
            Some("n")
        );
        assert!(!values.secrets.contains_key("BOB"));
        assert_eq!(values.vars.get("REGION").map(String::as_str), Some("us"));
        assert_eq!(values.vars.get("TIER").map(String::as_str), Some("gold"));

        // Secrets sealed under another host key are skipped rather than leaked.
        let rotated = SecretCipher::from_material(b"new host key").unwrap();
        let values = job_values(&database, &rotated, &project).await.unwrap();
        assert!(values.secrets.is_empty());
    }
}
