//! Database fixtures shared by Actions unit tests.

use chrono::Utc;
use sea_orm::{ActiveModelTrait, ConnectOptions, Database, DatabaseConnection, Set};
use sea_orm_migration::MigratorTrait;
use uuid::Uuid;

use crate::{
    entity::{namespace, repository, user},
    migration::Migrator,
};

pub(crate) async fn database() -> DatabaseConnection {
    let mut options = ConnectOptions::new("sqlite::memory:");
    options.max_connections(1).sqlx_logging(false);
    let database = Database::connect(options).await.unwrap();
    Migrator::up(&database, None).await.unwrap();
    database
}

/// Creates a user with a personal namespace of the same name.
pub(crate) async fn owner(database: &DatabaseConnection, username: &str) -> user::Model {
    let now = Utc::now();
    let owner = user::ActiveModel {
        id: Set(Uuid::new_v4()),
        username: Set(username.to_owned()),
        password_hash: Set("unused".to_owned()),
        is_admin: Set(false),
        default_repository_visibility: Set("private".to_owned()),
        theme_preference: Set("system".to_owned()),
        motion_preference: Set("system".to_owned()),
        disabled_at: Set(None),
        avatar_updated_at: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(database)
    .await
    .unwrap();
    namespace::ActiveModel {
        slug: Set(username.to_owned()),
        kind: Set("user".to_owned()),
        user_id: Set(Some(owner.id)),
        organization_id: Set(None),
        created_at: Set(now),
    }
    .insert(database)
    .await
    .unwrap();
    owner
}

pub(crate) async fn repository(
    database: &DatabaseConnection,
    owner: &user::Model,
    name: &str,
) -> repository::Model {
    let now = Utc::now();
    repository::ActiveModel {
        id: Set(Uuid::new_v4()),
        namespace: Set(owner.username.clone()),
        name: Set(name.to_owned()),
        description: Set(None),
        website_url: Set(None),
        visibility: Set("private".to_owned()),
        object_format: Set("sha1".to_owned()),
        mirrored: Set(false),
        default_branch: Set(Some("main".to_owned())),
        issue_counter: Set(0),
        storage_key: Set(Uuid::new_v4()),
        created_by: Set(owner.id),
        archived_at: Set(None),
        deleted_at: Set(None),
        icon_updated_at: Set(None),
        icon_source: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(database)
    .await
    .unwrap()
}
