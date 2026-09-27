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

#[tokio::test]
async fn action_run_event_migration_preserves_runs_and_job_references() {
    use crate::entity::{action_job, action_run};

    let mut options = ConnectOptions::new("sqlite::memory:");
    options.max_connections(1);
    let database = Database::connect(options).await.unwrap();
    // Apply everything before the action run event migration.
    let before = super::Migrator::migrations()
        .iter()
        .position(|migration| migration.name() == "m20260927_000061_action_dispatch_schedule")
        .unwrap();
    super::Migrator::up(&database, Some(before as u32))
        .await
        .unwrap();
    let owner = crate::identity::bootstrap_admin(
        &database,
        "migration-owner",
        "migration-test-password".to_owned(),
    )
    .await
    .unwrap();
    let now = chrono::Utc::now();
    let repository_id = Uuid::new_v4();
    repository::ActiveModel {
        id: Set(repository_id),
        namespace: Set(owner.username.clone()),
        name: Set("runs".to_owned()),
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
    .insert(&database)
    .await
    .unwrap();
    let run_id = Uuid::new_v4();
    let run = |id: Uuid, number: i64, event: &str| action_run::ActiveModel {
        id: Set(id),
        repository_id: Set(repository_id),
        number: Set(number),
        workflow_path: Set(".forgejo/workflows/ci.yml".to_owned()),
        workflow_name: Set("CI".to_owned()),
        event: Set(event.to_owned()),
        ref_name: Set("refs/heads/main".to_owned()),
        before_sha: Set(String::new()),
        after_sha: Set("1".repeat(40)),
        actor_id: Set(Some(owner.id)),
        status: Set("queued".to_owned()),
        failure_kind: Set(None),
        failure_summary: Set(None),
        diagnostic: Set(None),
        event_json: Set("{}".to_owned()),
        cancel_requested_at: Set(None),
        cancelled_by: Set(None),
        created_at: Set(now),
        started_at: Set(None),
        completed_at: Set(None),
    };
    run(run_id, 1, "push").insert(&database).await.unwrap();
    database
        .execute_unprepared(
            "INSERT INTO action_jobs (run_id, job_key, name, required_labels, workflow_payload, status, created_at) \
             SELECT id, 'build', 'build', '[]', x'', 'queued', created_at FROM action_runs WHERE number = 1",
        )
        .await
        .unwrap();
    assert!(
        run(Uuid::new_v4(), 2, "schedule")
            .insert(&database)
            .await
            .is_err()
    );

    super::Migrator::up(&database, Some(1)).await.unwrap();

    run(Uuid::new_v4(), 2, "schedule")
        .insert(&database)
        .await
        .unwrap();
    run(Uuid::new_v4(), 3, "workflow_dispatch")
        .insert(&database)
        .await
        .unwrap();
    assert!(
        run(Uuid::new_v4(), 4, "issues")
            .insert(&database)
            .await
            .is_err()
    );
    let job = action_job::Entity::find()
        .one(&database)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(job.run_id, run_id);
    action_run::Entity::delete_by_id(run_id)
        .exec(&database)
        .await
        .unwrap();
    assert!(
        action_job::Entity::find()
            .one(&database)
            .await
            .unwrap()
            .is_none()
    );
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
}
