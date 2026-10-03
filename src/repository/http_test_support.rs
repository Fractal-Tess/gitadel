//! Fixtures for driving the Git, LFS, and release handlers the way an HTTP
//! client does: through the router with real credentials.

use axum::{
    Router,
    body::{Body, Bytes},
    http::{HeaderMap, HeaderValue, Request, StatusCode, header},
};
use chrono::Utc;
use sea_orm::{ActiveModelTrait, DatabaseConnection, Set};
use tower::ServiceExt as _;
use uuid::Uuid;

use super::{GitHttpState, resources::test_support::TestInstance};
use crate::{
    actions::{ActionsState, secrets::SecretCipher},
    config::ActionsSettings,
    entity::{api_token, repository, repository_collaborator, user},
    identity::{SCOPE_READ, SCOPE_WRITE, hash_secret},
};

/// The state the Git and LFS routers run with, built on a [`TestInstance`].
pub(super) fn git_http_state(instance: &TestInstance) -> GitHttpState {
    let host_key = super::ssh::load_or_create_host_key(&instance.root.join("host-key")).unwrap();
    let actions = ActionsState::new(
        instance.state.clone(),
        ActionsSettings::default(),
        SecretCipher::from_host_key(&host_key).unwrap(),
    );
    GitHttpState::new(instance.state.clone(), actions)
}

/// Stores `raw` as an API token for `account` with read and write scopes.
pub(super) async fn api_token(database: &DatabaseConnection, account: &user::Model, raw: &str) {
    scoped_api_token(database, account, raw, SCOPE_READ | SCOPE_WRITE).await;
}

pub(super) async fn scoped_api_token(
    database: &DatabaseConnection,
    account: &user::Model,
    raw: &str,
    scopes: i32,
) {
    api_token::ActiveModel {
        id: Set(Uuid::new_v4()),
        user_id: Set(account.id),
        name: Set("http test".to_owned()),
        token_hash: Set(hash_secret(raw)),
        scopes: Set(scopes),
        expires_at: Set(None),
        created_at: Set(Utc::now()),
        last_used_at: Set(None),
        revoked_at: Set(None),
    }
    .insert(database)
    .await
    .unwrap();
}

/// Adds `account` to `repository` with `role` (`read` or `write`).
pub(super) async fn collaborator(
    database: &DatabaseConnection,
    repository: &repository::Model,
    account: &user::Model,
    role: &str,
) {
    repository_collaborator::ActiveModel {
        repository_id: Set(repository.id),
        user_id: Set(account.id),
        role: Set(role.to_owned()),
        created_at: Set(Utc::now()),
    }
    .insert(database)
    .await
    .unwrap();
}

pub(super) fn bearer(token: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
    );
    headers
}

pub(super) struct HttpResponse {
    pub(super) status: StatusCode,
    pub(super) headers: HeaderMap,
    pub(super) body: Bytes,
}

impl HttpResponse {
    pub(super) fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body).unwrap_or_else(|error| {
            panic!(
                "response body is not JSON ({error}): {}",
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    pub(super) fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    /// The `error.message` of an [`ApiError`](crate::identity::ApiError) body.
    pub(super) fn error_message(&self) -> String {
        self.json()["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .to_owned()
    }
}

/// Sends one request through `router` and collects the whole response.
pub(super) async fn send(router: Router, request: Request<Body>) -> HttpResponse {
    let response = router.oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    HttpResponse {
        status,
        headers,
        body,
    }
}

/// A request builder with the Authorization header already applied.
pub(super) fn request(
    method: &str,
    uri: &str,
    headers: &HeaderMap,
) -> axum::http::request::Builder {
    let mut builder = Request::builder().method(method).uri(uri);
    for (name, value) in headers {
        builder = builder.header(name, value);
    }
    builder
}
