//! Scoped Actions secrets and variables. HTTP authentication belongs to callers.
use chrono::Utc;
use gitadel_core::audit::AuditContext;
use gitadel_db::entity::{action_secret, action_variable, repository};
use gitadel_identity::permissions::RepositoryPrincipal;
use gitadel_identity::{
    AuthenticatedUser, IdentityError,
    permissions::{RepositoryPermission, authorize_namespace, authorize_repository},
};
pub use gitadel_secrets::SecretCipher;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait,
    IntoActiveModel, PaginatorTrait, QueryFilter, QueryOrder, Set,
};
use std::collections::{BTreeMap, HashMap};
use uuid::Uuid;
const MAX_NAME_BYTES: usize = 100;
pub const MAX_VALUE_BYTES: usize = 64 * 1024;
const MAX_ENTRIES_PER_SCOPE: u64 = 500;
const RESERVED_PREFIXES: [&str; 3] = ["GITHUB_", "GITEA_", "FORGEJO_"];

/// Associated data that binds a sealed value to its scope and name.
fn associated_data(scope: &Scope, name: &str) -> Vec<u8> {
    // Namespace slugs change on rename, so bind to stable identifiers only.
    let scope = match scope {
        Scope::Repository(id) => format!("repository:{id}"),
        Scope::Namespace(_) => "namespace".to_owned(),
    };
    format!("gitadel-actions-secret:v1:{scope}:{name}").into_bytes()
}

fn seal(
    cipher: &SecretCipher,
    scope: &Scope,
    name: &str,
    value: &str,
) -> Result<Vec<u8>, IdentityError> {
    cipher
        .seal_bytes(&associated_data(scope, name), value.as_bytes())
        .map_err(IdentityError::internal)
}

fn open(cipher: &SecretCipher, scope: &Scope, name: &str, sealed: &[u8]) -> Option<String> {
    let plaintext = cipher.open_bytes(&associated_data(scope, name), sealed)?;
    String::from_utf8(plaintext).ok()
}

#[derive(Clone, Debug)]
pub enum Scope {
    Repository(Uuid),
    Namespace(String),
}

/// Validates and canonicalizes a secret or variable name like GitHub does:
/// ASCII letters, digits, and underscores, not starting with a digit or a
/// reserved `GITHUB_`, `GITEA_`, or `FORGEJO_` prefix. Names are uppercased.
pub fn canonical_name(name: &str) -> Result<String, String> {
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
pub struct JobValues {
    pub secrets: HashMap<String, String>,
    pub vars: HashMap<String, String>,
}

pub async fn job_values<C: ConnectionTrait>(
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
            match open(cipher, &scope, &row.name, &row.sealed) {
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

pub struct SecretResponse {
    pub name: String,
    pub created_at: chrono::DateTime<Utc>,
    pub updated_at: chrono::DateTime<Utc>,
}

pub struct SecretsResponse {
    pub secrets: Vec<SecretResponse>,
}

pub struct VariableResponse {
    pub name: String,
    pub value: String,
    pub created_at: chrono::DateTime<Utc>,
    pub updated_at: chrono::DateTime<Utc>,
}

pub struct VariablesResponse {
    pub variables: Vec<VariableResponse>,
}

macro_rules! scoped_filter {
    ($entity:ident, $select:expr, $scope:expr) => {
        match $scope {
            Scope::Repository(id) => $select.filter($entity::Column::RepositoryId.eq(*id)),
            Scope::Namespace(slug) => $select.filter($entity::Column::Namespace.eq(slug.as_str())),
        }
    };
}

fn validated_name(name: &str) -> Result<String, IdentityError> {
    canonical_name(name).map_err(IdentityError::bad_request)
}

fn validated_value(value: &str) -> Result<(), IdentityError> {
    if value.len() > MAX_VALUE_BYTES {
        return Err(IdentityError::bad_request(format!(
            "Values must not exceed {MAX_VALUE_BYTES} bytes."
        )));
    }
    if value.contains('\0') {
        return Err(IdentityError::bad_request(
            "Values must not contain NUL bytes.",
        ));
    }
    Ok(())
}

fn scope_columns(scope: &Scope) -> (Option<Uuid>, Option<String>) {
    match scope {
        Scope::Repository(id) => (Some(*id), None),
        Scope::Namespace(slug) => (None, Some(slug.clone())),
    }
}

fn variable_response(row: action_variable::Model) -> VariableResponse {
    VariableResponse {
        name: row.name,
        value: row.value,
        created_at: row.created_at,
        updated_at: row.updated_at,
    }
}

/// Borrowed operation handle. Construction has no side effects or workers.
pub struct SecretService<'a> {
    database: &'a DatabaseConnection,
    cipher: &'a SecretCipher,
    audit: AuditContext,
}
impl<'a> SecretService<'a> {
    pub fn new(
        database: &'a DatabaseConnection,
        cipher: &'a SecretCipher,
        audit: AuditContext,
    ) -> Self {
        Self {
            database,
            cipher,
            audit,
        }
    }
    async fn authorize(
        &self,
        actor: &AuthenticatedUser,
        scope: &Scope,
    ) -> Result<String, IdentityError> {
        match scope {
            Scope::Repository(id) => {
                let repository = authorize_repository(
                    self.database,
                    *id,
                    Some(actor.user.id),
                    RepositoryPermission::Manage,
                )
                .await?;
                Ok(format!("{}/{}", repository.namespace, repository.name))
            }
            Scope::Namespace(slug) => {
                authorize_namespace(self.database, actor, slug).await?;
                Ok(slug.clone())
            }
        }
    }
    pub async fn list_secrets(
        &self,
        principal: &RepositoryPrincipal,
        scope: &Scope,
    ) -> Result<SecretsResponse, IdentityError> {
        let actor = &principal
            .authenticate(self.database, gitadel_identity::SCOPE_READ)
            .await?;
        self.authorize(actor, scope).await?;
        let select = scoped_filter!(action_secret, action_secret::Entity::find(), scope);
        let secrets = select
            .order_by_asc(action_secret::Column::Name)
            .all(self.database)
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

    pub async fn set_secret(
        &self,
        principal: &RepositoryPrincipal,
        scope: &Scope,
        name: &str,
        value: &str,
    ) -> Result<SecretResponse, IdentityError> {
        let actor = &principal
            .authenticate(self.database, gitadel_identity::SCOPE_WRITE)
            .await?;
        let target = self.authorize(actor, scope).await?;
        let name = validated_name(name)?;
        validated_value(value)?;
        let database = self.database;
        let sealed = seal(self.cipher, scope, &name, value)?;
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
                    return Err(IdentityError::conflict(format!(
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
        gitadel_db::audit::record(
            self.database,
            Some(actor.user.id),
            "actions.secret.set",
            Some(format!("{target}:{name}")),
            self.audit,
        )
        .await?;
        Ok(SecretResponse {
            name: row.name,
            created_at: row.created_at,
            updated_at: row.updated_at,
        })
    }

    pub async fn delete_secret(
        &self,
        principal: &RepositoryPrincipal,
        scope: &Scope,
        name: &str,
    ) -> Result<(), IdentityError> {
        let actor = &principal
            .authenticate(self.database, gitadel_identity::SCOPE_WRITE)
            .await?;
        let target = self.authorize(actor, scope).await?;
        let name = validated_name(name)?;
        let database = self.database;
        let deleted = scoped_filter!(action_secret, action_secret::Entity::delete_many(), scope)
            .filter(action_secret::Column::Name.eq(&name))
            .exec(database)
            .await?;
        if deleted.rows_affected == 0 {
            return Err(IdentityError::not_found());
        }
        gitadel_db::audit::record(
            self.database,
            Some(actor.user.id),
            "actions.secret.delete",
            Some(format!("{target}:{name}")),
            self.audit,
        )
        .await?;
        Ok(())
    }

    pub async fn list_variables(
        &self,
        principal: &RepositoryPrincipal,
        scope: &Scope,
    ) -> Result<VariablesResponse, IdentityError> {
        let actor = &principal
            .authenticate(self.database, gitadel_identity::SCOPE_READ)
            .await?;
        self.authorize(actor, scope).await?;
        let select = scoped_filter!(action_variable, action_variable::Entity::find(), scope);
        let variables = select
            .order_by_asc(action_variable::Column::Name)
            .all(self.database)
            .await?
            .into_iter()
            .map(variable_response)
            .collect();
        Ok(VariablesResponse { variables })
    }

    pub async fn set_variable(
        &self,
        principal: &RepositoryPrincipal,
        scope: &Scope,
        name: &str,
        value: String,
    ) -> Result<VariableResponse, IdentityError> {
        let actor = &principal
            .authenticate(self.database, gitadel_identity::SCOPE_WRITE)
            .await?;
        let target = self.authorize(actor, scope).await?;
        let name = validated_name(name)?;
        validated_value(&value)?;
        let database = self.database;
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
                    return Err(IdentityError::conflict(format!(
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
        gitadel_db::audit::record(
            self.database,
            Some(actor.user.id),
            "actions.variable.set",
            Some(format!("{target}:{name}")),
            self.audit,
        )
        .await?;
        Ok(variable_response(row))
    }

    pub async fn delete_variable(
        &self,
        principal: &RepositoryPrincipal,
        scope: &Scope,
        name: &str,
    ) -> Result<(), IdentityError> {
        let actor = &principal
            .authenticate(self.database, gitadel_identity::SCOPE_WRITE)
            .await?;
        let target = self.authorize(actor, scope).await?;
        let name = validated_name(name)?;
        let database = self.database;
        let deleted = scoped_filter!(
            action_variable,
            action_variable::Entity::delete_many(),
            scope
        )
        .filter(action_variable::Column::Name.eq(&name))
        .exec(database)
        .await?;
        if deleted.rows_affected == 0 {
            return Err(IdentityError::not_found());
        }
        gitadel_db::audit::record(
            self.database,
            Some(actor.user.id),
            "actions.variable.delete",
            Some(format!("{target}:{name}")),
            self.audit,
        )
        .await?;
        Ok(())
    }
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
        let sealed = seal(&cipher, &repository, "TOKEN", "hunter2").unwrap();
        assert!(!sealed.windows(7).any(|window| window == b"hunter2"));
        assert_eq!(
            open(&cipher, &repository, "TOKEN", &sealed).as_deref(),
            Some("hunter2")
        );
        assert!(open(&cipher, &repository, "OTHER", &sealed).is_none());
        assert!(
            open(
                &cipher,
                &Scope::Repository(Uuid::new_v4()),
                "TOKEN",
                &sealed
            )
            .is_none()
        );
        let other_key = SecretCipher::from_material(b"another host key").unwrap();
        assert!(open(&other_key, &repository, "TOKEN", &sealed).is_none());
        assert!(open(&cipher, &repository, "TOKEN", &sealed[..8]).is_none());
    }

    #[test]
    fn nonces_are_unique_per_seal() {
        let cipher = SecretCipher::from_material(b"host key material").unwrap();
        let scope = Scope::Namespace("team".to_owned());
        let first = seal(&cipher, &scope, "A", "same").unwrap();
        let second = seal(&cipher, &scope, "A", "same").unwrap();
        assert_ne!(first, second);
        assert_eq!(open(&cipher, &scope, "A", &first).as_deref(), Some("same"));
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
            sealed: Set(seal(cipher, &scope, name, value).unwrap()),
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
        use crate::test_support;
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

    #[tokio::test]
    async fn direct_boundaries_recheck_namespace_ownership_and_credential_revocation() {
        use gitadel_db::test_support;
        let database = test_support::database().await;
        let owner = test_support::owner(&database, "owner").await;
        let outsider = test_support::owner(&database, "outsider").await;
        let cipher = SecretCipher::from_material(b"host key").unwrap();
        let raw =
            gitadel_identity::create_session_on(&database, owner.id, chrono::Duration::hours(1))
                .await
                .unwrap();
        let owner_principal = RepositoryPrincipal::session(raw.clone());
        let outsider_raw =
            gitadel_identity::create_session_on(&database, outsider.id, chrono::Duration::hours(1))
                .await
                .unwrap();
        let outsider_principal = RepositoryPrincipal::session(outsider_raw);
        let service = SecretService::new(&database, &cipher, AuditContext::default());
        let scope = Scope::Namespace("owner".to_owned());
        service
            .set_secret(&owner_principal, &scope, "KEY", "value")
            .await
            .unwrap();
        assert!(matches!(
            service.list_secrets(&outsider_principal, &scope).await,
            Err(IdentityError::NotFound)
        ));
        gitadel_identity::delete_session(&database, &raw)
            .await
            .unwrap();
        assert!(matches!(
            service.delete_secret(&owner_principal, &scope, "KEY").await,
            Err(IdentityError::Unauthorized)
        ));
    }
}
