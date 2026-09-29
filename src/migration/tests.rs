use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectOptions, ConnectionTrait, Database, DbBackend,
    EntityTrait, QueryFilter, Set, Statement,
};
use sea_orm_migration::MigratorTrait;
use uuid::Uuid;

use crate::entity::{repository, repository_icon};

/// The owner a partly migrated database's rows point at.
struct LegacyOwner {
    id: Uuid,
    username: String,
}

/// Inserts an account with plain SQL rather than through the `users` entity,
/// which names columns that later migrations add and an older schema lacks.
async fn legacy_owner(database: &sea_orm::DatabaseConnection) -> LegacyOwner {
    let owner = LegacyOwner {
        id: Uuid::new_v4(),
        username: "migration-owner".to_owned(),
    };
    let now = chrono::Utc::now();
    database
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "INSERT INTO users (id, username, password_hash, is_admin, \
             default_repository_visibility, theme_preference, created_at, updated_at) \
             VALUES (?, ?, 'unused', 1, 'private', 'system', ?, ?)",
            [
                owner.id.into(),
                owner.username.clone().into(),
                now.into(),
                now.into(),
            ],
        ))
        .await
        .unwrap();
    database
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "INSERT INTO namespaces (slug, kind, user_id, organization_id, created_at) \
             VALUES (?, 'user', ?, NULL, ?)",
            [owner.username.clone().into(), owner.id.into(), now.into()],
        ))
        .await
        .unwrap();
    owner
}

#[tokio::test]
async fn nullable_default_branch_migration_preserves_repository_children_and_uniqueness() {
    let mut options = ConnectOptions::new("sqlite::memory:");
    options.max_connections(1);
    let database = Database::connect(options).await.unwrap();
    super::Migrator::up(&database, Some(51)).await.unwrap();
    let owner = legacy_owner(&database).await;
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
    let owner = legacy_owner(&database).await;
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
        rerun_of: Set(None),
        run_attempt: Set(1),
        created_at: Set(now),
        started_at: Set(None),
        completed_at: Set(None),
    };
    // The entity tracks the latest schema, so the old table is written with SQL.
    let insert_old = |id: Uuid, number: i64, event: &str| {
        format!(
            "INSERT INTO action_runs (id, repository_id, number, workflow_path, workflow_name, event, \
             ref_name, before_sha, after_sha, status, event_json, created_at) \
             VALUES (x'{}', x'{}', {number}, 'ci.yml', 'CI', '{event}', 'refs/heads/main', '', '', \
             'queued', '{{}}', '2026-01-01 00:00:00')",
            id.simple(),
            repository_id.simple()
        )
    };
    database
        .execute_unprepared(&insert_old(run_id, 1, "push"))
        .await
        .unwrap();
    database
        .execute_unprepared(
            "INSERT INTO action_jobs (run_id, job_key, name, required_labels, workflow_payload, status, created_at) \
             SELECT id, 'build', 'build', '[]', x'', 'queued', created_at FROM action_runs WHERE number = 1",
        )
        .await
        .unwrap();
    assert!(
        database
            .execute_unprepared(&insert_old(Uuid::new_v4(), 2, "schedule"))
            .await
            .is_err()
    );

    super::Migrator::up(&database, None).await.unwrap();

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

#[tokio::test]
async fn storage_domain_migration_keeps_selected_targets_and_history() {
    use crate::entity::{storage_domain_state, storage_migration, storage_target};

    let mut options = ConnectOptions::new("sqlite::memory:");
    options.max_connections(1);
    let database = Database::connect(options).await.unwrap();
    let before = super::Migrator::migrations()
        .iter()
        .position(|migration| migration.name() == "m20260929_000069_storage_domains")
        .unwrap();
    super::Migrator::up(&database, Some(before as u32))
        .await
        .unwrap();
    let lfs_target = Uuid::new_v4();
    let registry_target = Uuid::new_v4();
    let lfs_migration = Uuid::new_v4();
    let registry_migration = Uuid::new_v4();
    let at = "'2026-09-01T00:00:00+00:00'";
    // The entities track the new schema, so the old tables are written with SQL.
    for statement in [
        format!(
            "INSERT INTO lfs_storage_targets (id, name, kind, configuration, created_at, updated_at) \
             VALUES (x'{}', 'LFS', 'filesystem', '{{}}', {at}, {at}), \
                    (x'{}', 'Registry', 'filesystem', '{{}}', {at}, {at})",
            lfs_target.simple(),
            registry_target.simple()
        ),
        format!(
            "UPDATE lfs_storage_state SET active_target_id = x'{}'",
            lfs_target.simple()
        ),
        format!(
            "UPDATE registry_storage_state SET active_target_id = x'{}'",
            registry_target.simple()
        ),
        format!(
            "INSERT INTO lfs_storage_migrations (id, source_target_id, target_id, state, phase, \
             last_key, copied_objects, copied_bytes, error, started_at, updated_at, completed_at) \
             VALUES (x'{}', NULL, x'{}', 'completed', 'complete', 'last', 3, 42, NULL, {at}, {at}, {at})",
            lfs_migration.simple(),
            lfs_target.simple()
        ),
        format!(
            "INSERT INTO registry_storage_migrations (id, source_target_id, target_id, state, phase, \
             last_key, copied_objects, copied_bytes, total_bytes, error, started_at, updated_at, completed_at) \
             VALUES (x'{}', NULL, x'{}', 'failed', 'copying_registry', NULL, 1, 7, 9, 'boom', {at}, {at}, NULL)",
            registry_migration.simple(),
            registry_target.simple()
        ),
    ] {
        database.execute_unprepared(&statement).await.unwrap();
    }

    super::Migrator::up(&database, None).await.unwrap();

    let selected = |domain: &'static str| {
        let database = &database;
        async move {
            storage_domain_state::Entity::find_by_id(domain)
                .one(database)
                .await
                .unwrap()
                .unwrap()
                .active_target_id
        }
    };
    assert_eq!(selected("lfs").await, Some(lfs_target));
    assert_eq!(selected("registry").await, Some(registry_target));
    assert_eq!(
        storage_target::Entity::find()
            .all(&database)
            .await
            .unwrap()
            .len(),
        2
    );
    let lfs = storage_migration::Entity::find_by_id(lfs_migration)
        .one(&database)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (
            lfs.domain.as_str(),
            lfs.state.as_str(),
            lfs.phase.as_str(),
            lfs.copied_bytes,
            lfs.target_id
        ),
        ("lfs", "completed", "completed", 42, Some(lfs_target))
    );
    let registry = storage_migration::Entity::find_by_id(registry_migration)
        .one(&database)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (
            registry.domain.as_str(),
            registry.phase.as_str(),
            registry.total_bytes,
            registry.error.as_deref()
        ),
        ("registry", "copying", Some(9), Some("boom"))
    );
    // References to the renamed target table still hold: a selected target
    // cannot be deleted.
    assert!(
        storage_target::Entity::delete_by_id(registry_target)
            .exec(&database)
            .await
            .is_err()
    );
    let lfs_object_references = database
        .query_all_raw(sea_orm::Statement::from_string(
            sea_orm::DatabaseBackend::Sqlite,
            "SELECT \"table\" FROM pragma_foreign_key_list('lfs_objects') WHERE \"from\" = 'storage_target_id'",
        ))
        .await
        .unwrap();
    assert_eq!(
        lfs_object_references[0]
            .try_get::<String>("", "table")
            .unwrap(),
        "storage_targets"
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
