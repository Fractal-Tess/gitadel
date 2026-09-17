use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectOptions, ConnectionTrait, Database, EntityTrait,
    QueryFilter, Set,
};
use sea_orm_migration::MigratorTrait;
use uuid::Uuid;

use crate::entity::{repository, repository_icon};

#[tokio::test]
async fn nullable_default_branch_migration_preserves_repository_children_and_uniqueness() {
    let mut options = ConnectOptions::new("sqlite::memory:");
    options.max_connections(1);
    let database = Database::connect(options).await.unwrap();
    super::Migrator::up(&database, Some(51)).await.unwrap();
    let owner = crate::identity::bootstrap_admin(
        &database,
        "migration-owner",
        "migration-test-password".to_owned(),
    )
    .await
    .unwrap();
    let now = chrono::Utc::now();
    let id = Uuid::new_v4();
    let create = |id| repository::ActiveModel {
        id: Set(id),
        namespace: Set(owner.username.clone()),
        name: Set("preserved".to_owned()),
        description: Set(None),
        website_url: Set(None),
        visibility: Set("private".to_owned()),
        object_format: Set("sha1".to_owned()),
        mirrored: Set(false),
        default_branch: Set(Some("develop".to_owned())),
        issue_counter: Set(7),
        storage_key: Set(Uuid::new_v4()),
        created_by: Set(owner.id),
        archived_at: Set(None),
        deleted_at: Set(None),
        icon_updated_at: Set(Some(now)),
        icon_source: Set(Some("manual".to_owned())),
        created_at: Set(now),
        updated_at: Set(now),
    };
    create(id).insert(&database).await.unwrap();
    repository_icon::ActiveModel {
        repository_id: Set(id),
        content: Set(b"preserved image bytes".to_vec()),
    }
    .insert(&database)
    .await
    .unwrap();
    super::Migrator::up(&database, None).await.unwrap();
    let preserved = repository::Entity::find_by_id(id)
        .one(&database)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(preserved.default_branch.as_deref(), Some("develop"));
    assert_eq!(preserved.icon_source.as_deref(), Some("uploaded"));
    assert_eq!(
        repository_icon::Entity::find_by_id(id)
            .one(&database)
            .await
            .unwrap()
            .unwrap()
            .content,
        b"preserved image bytes"
    );
    assert!(create(Uuid::new_v4()).insert(&database).await.is_err());
    assert!(
        database
            .query_all_raw(sea_orm::Statement::from_string(
                sea_orm::DatabaseBackend::Sqlite,
                "PRAGMA foreign_key_check"
            ))
            .await
            .unwrap()
            .is_empty()
    );
    repository::Entity::delete_many()
        .filter(repository::Column::Id.eq(id))
        .exec(&database)
        .await
        .unwrap();
    assert!(
        repository_icon::Entity::find_by_id(id)
            .one(&database)
            .await
            .unwrap()
            .is_none()
    );
}
