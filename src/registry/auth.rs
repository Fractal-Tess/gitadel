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
use rand::RngCore;
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
        rand::rng().fill_bytes(&mut raw);
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
