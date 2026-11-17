use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

use axum::{
    http::{HeaderMap, HeaderValue, header},
    response::{IntoResponse, Response},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use rand::RngExt as _;
use serde::Serialize;
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::{
    identity::{SCOPE_REPOSITORY_READ, SCOPE_WRITE},
    repository::{Permission, RepositoryState},
};

use super::error::RegistryError;

const TOKEN_LIFETIME: Duration = Duration::from_secs(300);
const MAX_TOKENS: usize = 8192;
const MAX_SCOPES: usize = 16;
const MAX_SCOPE_LENGTH: usize = 576;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Actions(u8);

impl Actions {
    pub(crate) const PULL: Self = Self(1);
    pub(crate) const PUSH: Self = Self(2);
    pub(crate) const DELETE: Self = Self(4);

    pub(crate) fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    fn from_scope(value: &str) -> Option<Self> {
        let mut actions = Self(0);
        for action in value.split(',') {
            match action {
                "pull" => actions.0 |= Self::PULL.0,
                "push" => actions.0 |= Self::PUSH.0,
                "delete" => actions.0 |= Self::DELETE.0,
                _ => return None,
            }
        }
        (actions.0 != 0).then_some(actions)
    }

    fn as_scope(self) -> &'static str {
        match (
            self.contains(Self::PULL),
            self.contains(Self::PUSH),
            self.contains(Self::DELETE),
        ) {
            (true, true, true) => "pull,push,delete",
            (true, true, false) => "pull,push",
            (true, false, true) => "pull,delete",
            (true, false, false) => "pull",
            (false, true, true) => "push,delete",
            (false, true, false) => "push",
            (false, false, true) => "delete",
            (false, false, false) => "",
        }
    }

    fn bits(self) -> impl Iterator<Item = Self> {
        [Self::PULL, Self::PUSH, Self::DELETE]
            .into_iter()
            .filter(move |candidate| self.contains(*candidate))
    }
}

enum Credential {
    Anonymous,
    Gitadel(Arc<str>),
}

struct GrantedScope {
    repository: Arc<str>,
    actions: Actions,
}

struct GrantedToken {
    credential: Credential,
    scopes: Vec<GrantedScope>,
    expires_at: Instant,
}

#[derive(Clone)]
pub(crate) struct RegistryAuth {
    tokens: Arc<RwLock<HashMap<String, Arc<GrantedToken>>>>,
}

pub(crate) struct TokenQuery {
    pub service: Option<String>,
    pub scopes: Vec<String>,
    pub account: Option<String>,
}

#[derive(Serialize)]
struct TokenResponse {
    token: String,
    access_token: String,
    expires_in: u64,
    issued_at: String,
}

impl RegistryAuth {
    pub(crate) fn new() -> Self {
        Self {
            tokens: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub(crate) async fn issue(
        &self,
        state: &RepositoryState,
        headers: &HeaderMap,
        query: &TokenQuery,
    ) -> Result<Response, RegistryError> {
        if query
            .service
            .as_deref()
            .is_some_and(|service| service != "gitadel")
        {
            return Err(RegistryError::bad_request("Unknown registry service."));
        }
        let requested = requested_scopes(query)?;
        let credential = self
            .authenticate_basic(state, headers, query.account.as_deref())
            .await?;

        let mut grants = Vec::new();
        for requested_scope in requested {
            match requested_scope {
                RequestedScope::Root => grants.push(GrantedScope {
                    repository: Arc::from(""),
                    actions: Actions::PULL,
                }),
                RequestedScope::Catalog => {
                    if self
                        .scope_permitted(state, &credential, None, Actions::PULL)
                        .await?
                    {
                        grants.push(GrantedScope {
                            repository: Arc::from("_catalog"),
                            actions: Actions::PULL,
                        });
                    }
                }
                RequestedScope::Image(repository, actions) => {
                    let image = super::parse_image_name(&repository)
                        .map_err(|_| RegistryError::bad_request("Invalid repository scope."))?;
                    let Ok(repo) = state.find(image.namespace, image.repository).await else {
                        continue;
                    };

                    let mut granted_actions = Actions(0);
                    for action in actions.bits() {
                        if self
                            .scope_permitted(state, &credential, Some(&repo), action)
                            .await?
                        {
                            granted_actions.0 |= action.0;
                        }
                    }
                    if granted_actions.0 != 0 {
                        merge_scope(&mut grants, repository, granted_actions);
                    }
                }
            }
        }

        if grants.is_empty() {
            if matches!(&credential, Credential::Anonymous) {
                return Err(basic_challenge());
            }
            return Err(RegistryError::forbidden(
                "The requested registry scope is not available.",
            ));
        }

        let mut raw = [0_u8; 32];
        rand::rng().fill(&mut raw);
        let token = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw);
        let now = Instant::now();
        let mut tokens = self.tokens.write().await;
        tokens.retain(|_, value| value.expires_at > now);
        let anonymous_full = matches!(&credential, Credential::Anonymous)
            && tokens
                .values()
                .filter(|token| matches!(&token.credential, Credential::Anonymous))
                .count()
                >= 1024;
        if tokens.len() >= MAX_TOKENS || anonymous_full {
            return Err(RegistryError::Status(
                axum::http::StatusCode::TOO_MANY_REQUESTS,
                "TOOMANYREQUESTS",
                "Registry token capacity reached; retry after existing grants expire.".to_owned(),
            ));
        }
        tokens.insert(
            token.clone(),
            Arc::new(GrantedToken {
                credential,
                scopes: grants,
                expires_at: now + TOKEN_LIFETIME,
            }),
        );
        drop(tokens);

        let issued_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let mut response = (
            axum::http::StatusCode::OK,
            axum::Json(TokenResponse {
                token: token.clone(),
                access_token: token,
                expires_in: TOKEN_LIFETIME.as_secs(),
                issued_at,
            }),
        )
            .into_response();
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        Ok(response)
    }

    pub(crate) async fn verify(
        &self,
        state: &RepositoryState,
        token: &str,
        repository: &str,
        action: Actions,
    ) -> Result<Option<Uuid>, RegistryError> {
        let granted = self.lookup(token).await?;
        let has_scope = granted
            .scopes
            .iter()
            .find(|scope| scope.repository.as_ref() == repository)
            .is_some_and(|scope| scope.actions.contains(action));
        if !has_scope {
            return Err(RegistryError::unauthorized(
                "The token does not grant this scope.",
            ));
        }
        if repository.is_empty() {
            return Err(RegistryError::forbidden(
                "The root scope cannot access registry data.",
            ));
        }
        if repository == "_catalog" {
            return self
                .verify_credential(state, &granted.credential, action)
                .await;
        }

        let image = super::parse_image_name(repository)
            .map_err(|_| RegistryError::unauthorized("Invalid token scope."))?;
        let repo = state
            .find(image.namespace, image.repository)
            .await
            .map_err(|_| RegistryError::unauthorized("Repository is unavailable."))?;
        let actor = self
            .verify_credential(state, &granted.credential, action)
            .await?;
        state
            .authorize(&repo, actor, permission_for(action))
            .await
            .map_err(|_| {
                RegistryError::unauthorized("Repository access is no longer available.")
            })?;
        if action.contains(Actions::DELETE) {
            state
                .authorize(&repo, actor, Permission::Write)
                .await
                .map_err(|_| {
                    RegistryError::unauthorized("Repository access is no longer available.")
                })?;
        }
        Ok(actor)
    }

    pub(crate) async fn verify_root(
        &self,
        state: &RepositoryState,
        token: &str,
    ) -> Result<Option<Uuid>, RegistryError> {
        let granted = self.lookup(token).await?;
        match &granted.credential {
            Credential::Anonymous => Ok(None),
            Credential::Gitadel(source) => Ok(Some(
                state
                    .identity()
                    .authenticate_token(source, 0)
                    .await
                    .map_err(|_| {
                        RegistryError::unauthorized("The source credential is no longer valid.")
                    })?
                    .user
                    .id,
            )),
        }
    }

    pub(crate) fn challenge(&self, repository: &str, actions: Actions, realm: &str) -> String {
        let realm = format!("{}/v2/token", realm.trim_end_matches('/'));
        let realm = quote_header_value(&realm);
        let scope = match repository {
            "" => String::new(),
            "_catalog" => "registry:catalog:*".to_owned(),
            repository => format!("repository:{repository}:{}", actions.as_scope()),
        };
        if scope.is_empty() {
            format!("Bearer realm=\"{realm}\",service=\"gitadel\"")
        } else {
            format!(
                "Bearer realm=\"{realm}\",service=\"gitadel\",scope=\"{}\"",
                quote_header_value(&scope)
            )
        }
    }

    async fn lookup(&self, token: &str) -> Result<Arc<GrantedToken>, RegistryError> {
        let granted = {
            let tokens = self.tokens.read().await;
            tokens.get(token).cloned()
        }
        .ok_or_else(|| RegistryError::unauthorized("Invalid bearer token."))?;
        if granted.expires_at <= Instant::now() {
            self.tokens.write().await.remove(token);
            return Err(RegistryError::unauthorized("Invalid bearer token."));
        }
        Ok(granted)
    }

    async fn authenticate_basic(
        &self,
        state: &RepositoryState,
        headers: &HeaderMap,
        account: Option<&str>,
    ) -> Result<Credential, RegistryError> {
        let Some(value) = headers.get(header::AUTHORIZATION) else {
            return Ok(Credential::Anonymous);
        };
        let value = value.to_str().map_err(|_| basic_challenge())?;
        let encoded = value.strip_prefix("Basic ").ok_or_else(basic_challenge)?;
        let decoded = STANDARD.decode(encoded).map_err(|_| basic_challenge())?;
        let credentials = String::from_utf8(decoded).map_err(|_| basic_challenge())?;
        let (username, source_token) = credentials.split_once(':').ok_or_else(basic_challenge)?;
        if username.is_empty() || source_token.is_empty() {
            return Err(basic_challenge());
        }
        let authenticated = state
            .identity()
            .authenticate_token(source_token, 0)
            .await
            .map_err(|_| basic_challenge())?;
        if authenticated.user.username != username
            || account.is_some_and(|account| account != username)
        {
            return Err(basic_challenge());
        }
        Ok(Credential::Gitadel(Arc::from(source_token)))
    }

    async fn scope_permitted(
        &self,
        state: &RepositoryState,
        credential: &Credential,
        repository: Option<&crate::entity::repository::Model>,
        action: Actions,
    ) -> Result<bool, RegistryError> {
        let actor = match credential {
            Credential::Anonymous => None,
            Credential::Gitadel(source) => {
                let Ok(authenticated) = state
                    .identity()
                    .authenticate_token(source, scope_for(action))
                    .await
                else {
                    return Ok(false);
                };
                Some(authenticated.user.id)
            }
        };
        let Some(repository) = repository else {
            return Ok(actor.is_some() || action == Actions::PULL);
        };
        if action.contains(Actions::DELETE) {
            Ok(state
                .authorize(repository, actor, Permission::Write)
                .await
                .is_ok()
                && state
                    .authorize(repository, actor, Permission::Manage)
                    .await
                    .is_ok())
        } else if action.contains(Actions::PUSH) {
            Ok(state
                .authorize(repository, actor, Permission::Write)
                .await
                .is_ok())
        } else {
            Ok(state
                .can_access(repository, actor, Permission::Read)
                .await
                .map_err(|_| {
                    RegistryError::forbidden("The requested registry scope is not available.")
                })?)
        }
    }

    async fn verify_credential(
        &self,
        state: &RepositoryState,
        credential: &Credential,
        action: Actions,
    ) -> Result<Option<Uuid>, RegistryError> {
        match credential {
            Credential::Anonymous => Ok(None),
            Credential::Gitadel(source) => Ok(Some(
                state
                    .identity()
                    .authenticate_token(source, scope_for(action))
                    .await
                    .map_err(|_| {
                        RegistryError::unauthorized("The source credential is no longer valid.")
                    })?
                    .user
                    .id,
            )),
        }
    }
}

enum RequestedScope {
    Root,
    Catalog,
    Image(String, Actions),
}

fn requested_scopes(query: &TokenQuery) -> Result<Vec<RequestedScope>, RegistryError> {
    if query.scopes.len() > MAX_SCOPES {
        return Err(RegistryError::bad_request("Too many registry scopes."));
    }
    let mut values = Vec::new();
    for scope in &query.scopes {
        if scope.len() > MAX_SCOPE_LENGTH {
            return Err(RegistryError::bad_request("Registry scope is too long."));
        }
        values.extend(scope.split_whitespace());
        if values.len() > MAX_SCOPES {
            return Err(RegistryError::bad_request("Too many registry scopes."));
        }
    }
    if values.is_empty() {
        return Ok(vec![RequestedScope::Root]);
    }
    values
        .into_iter()
        .map(parse_scope)
        .collect::<Result<Vec<_>, _>>()
}

fn parse_scope(scope: &str) -> Result<RequestedScope, RegistryError> {
    if scope == "registry:catalog:*" {
        return Ok(RequestedScope::Catalog);
    }
    let mut parts = scope.split(':');
    if parts.next() != Some("repository") {
        return Err(RegistryError::bad_request("Invalid registry scope."));
    }
    let repository = parts
        .next()
        .ok_or_else(|| RegistryError::bad_request("Invalid repository scope."))?;
    let actions = parts
        .next()
        .ok_or_else(|| RegistryError::bad_request("Invalid repository scope."))?;
    if parts.next().is_some() || repository.len() > MAX_SCOPE_LENGTH {
        return Err(RegistryError::bad_request("Invalid repository scope."));
    }
    super::parse_image_name(repository)
        .map_err(|_| RegistryError::bad_request("Invalid repository scope."))?;
    let actions = Actions::from_scope(actions)
        .ok_or_else(|| RegistryError::bad_request("Invalid repository actions."))?;
    Ok(RequestedScope::Image(repository.to_owned(), actions))
}

fn merge_scope(scopes: &mut Vec<GrantedScope>, repository: String, actions: Actions) {
    if let Some(existing) = scopes
        .iter_mut()
        .find(|scope| scope.repository.as_ref() == repository)
    {
        existing.actions.0 |= actions.0;
    } else {
        scopes.push(GrantedScope {
            repository: Arc::from(repository),
            actions,
        });
    }
}

fn permission_for(action: Actions) -> Permission {
    if action.contains(Actions::DELETE) {
        Permission::Manage
    } else if action.contains(Actions::PUSH) {
        Permission::Write
    } else {
        Permission::Read
    }
}

fn scope_for(action: Actions) -> i32 {
    if action.contains(Actions::PUSH) || action.contains(Actions::DELETE) {
        SCOPE_WRITE
    } else {
        SCOPE_REPOSITORY_READ
    }
}

fn basic_challenge() -> RegistryError {
    RegistryError::challenge("Basic realm=\"gitadel\"")
}

fn quote_header_value(value: &str) -> String {
    let mut quoted = String::with_capacity(value.len());
    for character in value.chars().filter(|character| !character.is_control()) {
        if matches!(character, '\\' | '"') {
            quoted.push('\\');
        }
        quoted.push(character);
    }
    quoted
}

#[cfg(test)]
mod tests {
    use axum::{
        body::{Body, to_bytes},
        http::{Request, StatusCode},
    };
    use chrono::Utc;
    use sea_orm::{ActiveModelTrait, DatabaseConnection, EntityTrait, IntoActiveModel, Set};
    use tower::ServiceExt as _;

    use super::*;
    use crate::{
        actions::test_support,
        config::{AuthSettings, StorageSettings},
        entity::{api_token, user},
        identity::{IdentityState, SCOPE_READ, hash_secret},
        registry::test_support::digest,
    };

    fn scopes(values: &[&str]) -> TokenQuery {
        TokenQuery {
            service: Some("gitadel".to_owned()),
            scopes: values.iter().map(|value| (*value).to_owned()).collect(),
            account: None,
        }
    }

    #[test]
    fn actions_parse_and_print_round_trip() {
        assert_eq!(Actions::from_scope("pull"), Some(Actions::PULL));
        assert_eq!(
            Actions::from_scope("push,pull").map(Actions::as_scope),
            Some("pull,push")
        );
        assert_eq!(
            Actions::from_scope("pull,push,delete").map(Actions::as_scope),
            Some("pull,push,delete")
        );
        for invalid in ["", "pull,", "admin", "pull push", "*"] {
            assert_eq!(Actions::from_scope(invalid), None, "{invalid:?}");
        }
        assert!(
            Actions::from_scope("push,delete")
                .unwrap()
                .contains(Actions::PUSH)
        );
        assert!(!Actions::from_scope("push").unwrap().contains(Actions::PULL));
    }

    #[test]
    fn requested_scopes_are_parsed_split_and_bounded() {
        assert!(matches!(
            requested_scopes(&scopes(&[])).unwrap().as_slice(),
            [RequestedScope::Root]
        ));
        let parsed = requested_scopes(&scopes(&[
            "repository:alice/project:pull,push registry:catalog:*",
            "repository:alice/tools/cli:pull",
        ]))
        .unwrap();
        match parsed.as_slice() {
            [
                RequestedScope::Image(first, first_actions),
                RequestedScope::Catalog,
                RequestedScope::Image(second, second_actions),
            ] => {
                assert_eq!(first, "alice/project");
                assert_eq!(first_actions.as_scope(), "pull,push");
                assert_eq!(second, "alice/tools/cli");
                assert_eq!(*second_actions, Actions::PULL);
            }
            other => panic!("unexpected scopes: {}", other.len()),
        }
        for invalid in [
            "repository:alice/project",
            "repository:alice/project:pull:extra",
            "repository:alice/project:admin",
            "repository:ALICE:pull",
            "repository::pull",
            "image:alice/project:pull",
            "registry:catalog:pull",
        ] {
            assert!(requested_scopes(&scopes(&[invalid])).is_err(), "{invalid}");
        }
        let many = vec!["repository:alice/project:pull"; MAX_SCOPES + 1];
        assert!(requested_scopes(&scopes(&many)).is_err());
        let long = format!("repository:alice/{}:pull", "x".repeat(MAX_SCOPE_LENGTH));
        assert!(requested_scopes(&scopes(&[&long])).is_err());
    }

    struct Environment {
        root: std::path::PathBuf,
        database: DatabaseConnection,
        state: RepositoryState,
    }

    impl Drop for Environment {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    async fn environment() -> Environment {
        let root = std::env::temp_dir().join(format!("gitadel-registry-auth-{}", Uuid::new_v4()));
        let database = test_support::database().await;
        let public_url = url::Url::parse("https://gitadel.test").unwrap();
        let identity = IdentityState::new(
            database.clone(),
            AuthSettings {
                session_lifetime_hours: 24,
                invitation_lifetime_hours: 24,
            },
            public_url.clone(),
        )
        .unwrap();
        let state = RepositoryState::new(
            identity,
            StorageSettings {
                repository_root: root.join("repositories"),
                lfs_root: root.join("lfs"),
                registry_root: root.join("registry"),
                actions_artifact_root: root.join("actions-artifacts"),
            },
            public_url,
            22,
        )
        .await
        .unwrap();
        Environment {
            root,
            database,
            state,
        }
    }

    /// An API token with read and write scope; returns its id and raw value.
    async fn api_token(database: &DatabaseConnection, account: &user::Model) -> (Uuid, String) {
        let raw = format!("gat_{}", Uuid::new_v4().simple());
        let token = api_token::ActiveModel {
            id: Set(Uuid::new_v4()),
            user_id: Set(account.id),
            name: Set("registry".to_owned()),
            token_hash: Set(hash_secret(&raw)),
            scopes: Set(SCOPE_READ | SCOPE_WRITE),
            expires_at: Set(None),
            created_at: Set(Utc::now()),
            last_used_at: Set(None),
            revoked_at: Set(None),
        }
        .insert(database)
        .await
        .unwrap();
        (token.id, raw)
    }

    fn basic(username: &str, token: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_str(&format!(
                "Basic {}",
                STANDARD.encode(format!("{username}:{token}"))
            ))
            .unwrap(),
        );
        headers
    }

    async fn issued_token(response: Response) -> String {
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        body["token"].as_str().unwrap().to_owned()
    }

    #[tokio::test]
    async fn registry_tokens_re_authorise_their_source_credential_on_use() {
        let environment = environment().await;
        let alice = test_support::owner(&environment.database, "alice").await;
        test_support::repository(&environment.database, &alice, "project").await;
        let (token_id, raw) = api_token(&environment.database, &alice).await;
        let auth = RegistryAuth::new();

        // The wrong username for the token is a failed login.
        assert!(
            auth.issue(
                &environment.state,
                &basic("bob", &raw),
                &scopes(&["repository:alice/project:pull"])
            )
            .await
            .is_err()
        );
        let response = auth
            .issue(
                &environment.state,
                &basic("alice", &raw),
                &scopes(&["repository:alice/project:pull,push"]),
            )
            .await
            .unwrap();
        let token = issued_token(response).await;

        assert_eq!(
            auth.verify(&environment.state, &token, "alice/project", Actions::PUSH)
                .await
                .unwrap(),
            Some(alice.id)
        );
        assert!(
            auth.verify(&environment.state, &token, "alice/project", Actions::DELETE)
                .await
                .is_err(),
            "only granted actions pass"
        );
        assert!(
            auth.verify(&environment.state, &token, "alice/other", Actions::PULL)
                .await
                .is_err(),
            "only granted repositories pass"
        );

        // Revoking the API token behind the grant ends it before it expires.
        let mut revoked = api_token::Entity::find_by_id(token_id)
            .one(&environment.database)
            .await
            .unwrap()
            .unwrap()
            .into_active_model();
        revoked.revoked_at = Set(Some(Utc::now()));
        revoked.update(&environment.database).await.unwrap();
        assert!(
            auth.verify(&environment.state, &token, "alice/project", Actions::PULL)
                .await
                .is_err()
        );
        assert!(auth.verify_root(&environment.state, &token).await.is_err());
    }

    #[tokio::test]
    async fn anonymous_grants_cover_only_public_pulls() {
        let environment = environment().await;
        let alice = test_support::owner(&environment.database, "alice").await;
        let mut public = test_support::repository(&environment.database, &alice, "public")
            .await
            .into_active_model();
        public.visibility = Set("public".to_owned());
        public.update(&environment.database).await.unwrap();
        test_support::repository(&environment.database, &alice, "private").await;
        let auth = RegistryAuth::new();

        let response = auth
            .issue(
                &environment.state,
                &HeaderMap::new(),
                &scopes(&[
                    "repository:alice/public:pull,push",
                    "repository:alice/private:pull",
                ]),
            )
            .await
            .unwrap();
        let token = issued_token(response).await;
        assert_eq!(
            auth.verify(&environment.state, &token, "alice/public", Actions::PULL)
                .await
                .unwrap(),
            None
        );
        assert!(
            auth.verify(&environment.state, &token, "alice/public", Actions::PUSH)
                .await
                .is_err()
        );
        assert!(
            auth.verify(&environment.state, &token, "alice/private", Actions::PULL)
                .await
                .is_err()
        );
        // Nothing grantable at all is a challenge rather than an empty token.
        assert!(
            auth.issue(
                &environment.state,
                &HeaderMap::new(),
                &scopes(&["repository:alice/private:pull"]),
            )
            .await
            .is_err()
        );
    }

    /// Exchanges `scope` for a bearer token through the HTTP endpoint.
    async fn bearer_for(router: &axum::Router, username: &str, raw: &str, scope: &str) -> String {
        let uri = format!(
            "/v2/token?service=gitadel&scope={}",
            url::form_urlencoded::byte_serialize(scope.as_bytes()).collect::<String>()
        );
        let mut request = Request::builder().uri(uri);
        request.headers_mut().unwrap().extend(basic(username, raw));
        let response = router
            .clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        issued_token(response).await
    }

    #[tokio::test]
    async fn mounting_a_blob_across_repositories_needs_pull_on_the_source() {
        let environment = environment().await;
        let alice = test_support::owner(&environment.database, "alice").await;
        test_support::repository(&environment.database, &alice, "source").await;
        test_support::repository(&environment.database, &alice, "target").await;
        let (_, raw) = api_token(&environment.database, &alice).await;
        let router = crate::registry::router(environment.state.clone());
        let layer = b"layer bytes";
        let layer_digest = digest(layer);

        // Put a blob into the source image.
        let push_source =
            bearer_for(&router, "alice", &raw, "repository:alice/source:pull,push").await;
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/v2/alice/source/blobs/uploads/?digest={layer_digest}"
                    ))
                    .header(header::AUTHORIZATION, format!("Bearer {push_source}"))
                    .header(header::CONTENT_TYPE, "application/octet-stream")
                    .body(Body::from(layer.as_slice()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);

        let mount = |bearer: String| {
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/v2/alice/target/blobs/uploads/?mount={layer_digest}&from=alice/source"
                ))
                .header(header::AUTHORIZATION, format!("Bearer {bearer}"))
                .body(Body::empty())
                .unwrap()
        };

        // A token that may push to the target but was not granted pull on the
        // source cannot mount: the registry starts an ordinary upload instead.
        let target_only =
            bearer_for(&router, "alice", &raw, "repository:alice/target:pull,push").await;
        let response = router.clone().oneshot(mount(target_only)).await.unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        assert!(response.headers().get("docker-content-digest").is_none());

        // With pull on the source the blob is mounted outright.
        let both = bearer_for(
            &router,
            "alice",
            &raw,
            "repository:alice/target:pull,push repository:alice/source:pull",
        )
        .await;
        let response = router.clone().oneshot(mount(both)).await.unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        assert_eq!(
            response
                .headers()
                .get("docker-content-digest")
                .and_then(|value| value.to_str().ok()),
            Some(layer_digest.as_str())
        );

        // Another user cannot even obtain pull on the private source, so the
        // mount degrades the same way.
        let bob = test_support::owner(&environment.database, "bob").await;
        test_support::repository(&environment.database, &bob, "target").await;
        let (_, bob_raw) = api_token(&environment.database, &bob).await;
        let bob_token = bearer_for(
            &router,
            "bob",
            &bob_raw,
            "repository:bob/target:pull,push repository:alice/source:pull",
        )
        .await;
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/v2/bob/target/blobs/uploads/?mount={layer_digest}&from=alice/source"
                    ))
                    .header(header::AUTHORIZATION, format!("Bearer {bob_token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);
    }
}
