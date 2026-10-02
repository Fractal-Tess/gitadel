//! Access decisions that span the repository modules.

use chrono::Utc;
use sea_orm::{ActiveModelTrait, DatabaseConnection, IntoActiveModel, Set};
use uuid::Uuid;

use axum::{
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::IntoResponse as _,
};

use super::{GitHttpState, LfsPermission, Permission, RepositoryState, git_http, lfs};
use crate::{
    actions::{ActionsState, secrets::SecretCipher, test_support},
    config::{ActionsSettings, AuthSettings, StorageSettings},
    entity::{api_token, repository, user},
    identity::{IdentityState, SCOPE_READ, SCOPE_WRITE, hash_secret},
};

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
    let root = std::env::temp_dir().join(format!("gitadel-access-{}", Uuid::new_v4()));
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

async fn disable(database: &DatabaseConnection, account: &user::Model) {
    let mut active = account.clone().into_active_model();
    active.disabled_at = Set(Some(Utc::now()));
    active.update(database).await.unwrap();
}

#[tokio::test]
async fn lfs_tokens_stop_working_once_the_account_is_disabled() {
    let environment = environment().await;
    let alice = test_support::owner(&environment.database, "alice").await;
    let repository = test_support::repository(&environment.database, &alice, "project").await;
    let token = environment
        .state
        .issue_lfs_token(repository.id, alice.id, LfsPermission::Write)
        .await;

    assert_eq!(
        environment
            .state
            .authenticate_lfs_token(&token, repository.id, LfsPermission::Write)
            .await,
        Some(alice.id)
    );
    // A token never authorizes another repository.
    assert_eq!(
        environment
            .state
            .authenticate_lfs_token(&token, Uuid::new_v4(), LfsPermission::Read)
            .await,
        None
    );

    disable(&environment.database, &alice).await;

    assert_eq!(
        environment
            .state
            .authenticate_lfs_token(&token, repository.id, LfsPermission::Read)
            .await,
        None
    );
    assert!(
        !environment
            .state
            .identity()
            .user_is_enabled(alice.id)
            .await
            .unwrap()
    );
}

async fn token(database: &DatabaseConnection, account: &user::Model, raw: &str) {
    api_token::ActiveModel {
        id: Set(Uuid::new_v4()),
        user_id: Set(account.id),
        name: Set("access test".to_owned()),
        token_hash: Set(hash_secret(raw)),
        scopes: Set(SCOPE_READ | SCOPE_WRITE),
        expires_at: Set(None),
        created_at: Set(Utc::now()),
        last_used_at: Set(None),
        revoked_at: Set(None),
    }
    .insert(database)
    .await
    .unwrap();
}

fn git_http_state(environment: &Environment) -> GitHttpState {
    let host_key = super::ssh::load_or_create_host_key(&environment.root.join("host-key")).unwrap();
    let actions = ActionsState::new(
        environment.state.clone(),
        ActionsSettings::default(),
        SecretCipher::from_host_key(&host_key).unwrap(),
    );
    GitHttpState::new(environment.state.clone(), actions)
}

fn credentials(value: Option<&str>) -> HeaderMap {
    let mut headers = HeaderMap::new();
    if let Some(value) = value {
        headers.insert(header::AUTHORIZATION, HeaderValue::from_str(value).unwrap());
    }
    headers
}

#[tokio::test]
async fn repository_state_is_not_revealed_to_callers_without_access() {
    let environment = environment().await;
    let alice = test_support::owner(&environment.database, "alice").await;
    let mallory = test_support::owner(&environment.database, "mallory").await;
    let archived = test_support::repository(&environment.database, &alice, "archived").await;
    let mut active = archived.into_active_model();
    active.archived_at = Set(Some(Utc::now()));
    let archived = active.update(&environment.database).await.unwrap();
    let mirrored = test_support::repository(&environment.database, &alice, "mirrored").await;
    let mut active = mirrored.into_active_model();
    active.mirrored = Set(true);
    let mirrored = active.update(&environment.database).await.unwrap();
    let missing = repository::Model {
        id: Uuid::new_v4(),
        name: "plain".to_owned(),
        archived_at: None,
        ..archived.clone()
    };

    let status = |user: Option<Uuid>, repository: repository::Model| {
        let state = environment.state.clone();
        async move {
            state
                .authorize(&repository, user, Permission::Write)
                .await
                .map_or_else(|error| error.into_response().status(), |()| StatusCode::OK)
        }
    };

    // Without access, every private repository looks the same.
    for user in [None, Some(mallory.id)] {
        assert_eq!(status(user, archived.clone()).await, StatusCode::NOT_FOUND);
        assert_eq!(status(user, mirrored.clone()).await, StatusCode::NOT_FOUND);
        assert_eq!(status(user, missing.clone()).await, StatusCode::NOT_FOUND);
    }
    // The owner still learns why a write is refused.
    assert_eq!(
        status(Some(alice.id), archived).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        status(Some(alice.id), mirrored).await,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn git_and_lfs_answer_alike_for_private_and_missing_repositories() {
    let environment = environment().await;
    let alice = test_support::owner(&environment.database, "alice").await;
    let mallory = test_support::owner(&environment.database, "mallory").await;
    test_support::repository(&environment.database, &alice, "secret").await;
    token(&environment.database, &alice, "alice-token").await;
    token(&environment.database, &mallory, "mallory-token").await;
    let state = git_http_state(&environment);

    for (authorization, expected) in [
        (None, StatusCode::UNAUTHORIZED),
        (Some("Bearer bogus"), StatusCode::UNAUTHORIZED),
        (Some("Basic bm90LWEtdG9rZW4="), StatusCode::UNAUTHORIZED),
        (Some("Bearer mallory-token"), StatusCode::NOT_FOUND),
    ] {
        let headers = credentials(authorization);
        for name in ["secret.git", "missing.git"] {
            let git = git_http::authorized_repository(&state, &headers, "alice", name)
                .await
                .unwrap_err()
                .into_response()
                .status();
            assert_eq!(git, expected, "git {name} with {authorization:?}");
            for permission in [LfsPermission::Read, LfsPermission::Write] {
                let lfs = lfs::authorized_repository(&state, &headers, "alice", name, permission)
                    .await
                    .unwrap_err()
                    .into_response()
                    .status();
                assert_eq!(lfs, expected, "lfs {name} with {authorization:?}");
            }
        }
    }

    // The owner is unaffected.
    let headers = credentials(Some("Bearer alice-token"));
    assert!(
        git_http::authorized_repository(&state, &headers, "alice", "secret.git")
            .await
            .is_ok()
    );
    assert!(
        lfs::authorized_repository(
            &state,
            &headers,
            "alice",
            "secret.git",
            LfsPermission::Write
        )
        .await
        .is_ok()
    );
}
