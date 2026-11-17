//! Handler-level fixtures for identity tests: a migrated in-memory database,
//! the identity routers as the server mounts them, and signed-in requests.

use std::sync::OnceLock;

use argon2::{Argon2, PasswordHasher as _};
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Method, Request, Response, StatusCode, header},
};
use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ConnectOptions, ConnectionTrait as _, Database, DatabaseConnection, Set,
};
use sea_orm_migration::MigratorTrait;
use serde_json::Value;
use tower::ServiceExt as _;
use uuid::Uuid;

use super::{IdentityState, SESSION_COOKIE};
use crate::{
    config::AuthSettings,
    entity::{namespace, user},
    migration::Migrator,
};

/// The password every harness user signs in with.
pub(crate) const PASSWORD: &str = "correct horse battery staple";

fn password_hash() -> String {
    static HASH: OnceLock<String> = OnceLock::new();
    HASH.get_or_init(|| {
        Argon2::default()
            .hash_password(PASSWORD.as_bytes())
            .unwrap()
            .to_string()
    })
    .clone()
}

pub(crate) struct Harness {
    pub(crate) state: IdentityState,
    pub(crate) database: DatabaseConnection,
}

impl Harness {
    pub(crate) async fn new() -> Self {
        let mut options = ConnectOptions::new("sqlite::memory:");
        options.max_connections(1).sqlx_logging(false);
        let database = Database::connect(options).await.unwrap();
        database
            .execute_unprepared("PRAGMA foreign_keys = ON")
            .await
            .unwrap();
        Migrator::up(&database, None).await.unwrap();
        let state = IdentityState::new(
            database.clone(),
            AuthSettings {
                session_lifetime_hours: 24,
                invitation_lifetime_hours: 24,
            },
            url::Url::parse("https://gitadel.test").unwrap(),
        )
        .unwrap();
        Self { state, database }
    }

    pub(crate) fn with_mailer(mut self, mailer: crate::mail::Mailer) -> Self {
        self.state = self.state.with_mailer(mailer);
        self
    }

    /// A user with a personal namespace who signs in with [`PASSWORD`].
    pub(crate) async fn user(&self, username: &str) -> user::Model {
        let now = Utc::now();
        let account = user::ActiveModel {
            id: Set(Uuid::new_v4()),
            username: Set(username.to_owned()),
            password_hash: Set(password_hash()),
            is_admin: Set(false),
            default_repository_visibility: Set("private".to_owned()),
            theme_preference: Set("system".to_owned()),
            motion_preference: Set("system".to_owned()),
            disabled_at: Set(None),
            avatar_updated_at: Set(None),
            created_at: Set(now),
            updated_at: Set(now),
        }
        .insert(&self.database)
        .await
        .unwrap();
        namespace::ActiveModel {
            slug: Set(username.to_owned()),
            kind: Set("user".to_owned()),
            user_id: Set(Some(account.id)),
            organization_id: Set(None),
            created_at: Set(now),
        }
        .insert(&self.database)
        .await
        .unwrap();
        account
    }

    /// A browser session for `account`, as a `Cookie` header value.
    pub(crate) async fn login(&self, account: &user::Model) -> String {
        let (raw, _) = self
            .state
            .create_session_on(&self.database, account.id)
            .await
            .unwrap();
        format!("{SESSION_COOKIE}={raw}")
    }

    /// The identity API under `/api/v1` plus the OAuth endpoints at the root,
    /// as `server.rs` mounts them.
    pub(crate) fn router(&self) -> Router {
        Router::new()
            .nest("/api/v1", super::router().with_state(self.state.clone()))
            .merge(super::oauth_router().with_state(self.state.clone()))
    }

    pub(crate) async fn send(&self, request: Request<Body>) -> Response<Body> {
        self.router().oneshot(request).await.unwrap()
    }

    /// A JSON request, optionally with a session cookie, decoded to its
    /// status and JSON body (`Null` when the body is empty).
    pub(crate) async fn request(
        &self,
        method: Method,
        uri: &str,
        cookie: Option<&str>,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut builder = Request::builder().method(method).uri(uri);
        if let Some(cookie) = cookie {
            builder = builder.header(header::COOKIE, cookie);
        }
        let request = match body {
            Some(body) => builder
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
            None => builder.body(Body::empty()).unwrap(),
        };
        let response = self.send(request).await;
        let status = response.status();
        (status, json_body(response).await)
    }
}

pub(crate) async fn json_body(response: Response<Body>) -> Value {
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()))
    }
}

/// The `token` query value of the first link in a captured message. The
/// body may be quoted-printable encoded, so soft line breaks are undone and
/// `=XX` escapes decoded first.
pub(crate) fn mailed_token(mail: &str) -> String {
    let joined = mail.replace("=\r\n", "").replace("=\n", "");
    let mut decoded = String::with_capacity(joined.len());
    let mut characters = joined.chars().peekable();
    while let Some(character) = characters.next() {
        if character == '=' {
            let high = characters.next();
            let low = characters.next();
            if let (Some(high), Some(low)) = (high, low)
                && let Some(byte) = u8::from_str_radix(&format!("{high}{low}"), 16).ok()
            {
                decoded.push(char::from(byte));
                continue;
            }
            decoded.push('=');
            decoded.extend(high);
            decoded.extend(low);
            continue;
        }
        decoded.push(character);
    }
    let start = decoded.find("token=").expect("mail contains a token link") + "token=".len();
    decoded[start..]
        .chars()
        .take_while(|character| !character.is_whitespace())
        .collect()
}
