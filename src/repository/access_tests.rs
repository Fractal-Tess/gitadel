//! Access decisions that span the repository modules.

use chrono::Utc;
use sea_orm::{ActiveModelTrait, DatabaseConnection, IntoActiveModel, Set};
use uuid::Uuid;

use super::{LfsPermission, RepositoryState};
use crate::{
    actions::test_support,
    config::{AuthSettings, StorageSettings},
    entity::user,
    identity::IdentityState,
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
