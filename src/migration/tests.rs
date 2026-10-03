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

#[tokio::test]
async fn webhook_retry_migration_leaves_queued_deliveries_due() {
    use crate::entity::repository_webhook_delivery;

    let mut options = ConnectOptions::new("sqlite::memory:");
    options.max_connections(1);
    let database = Database::connect(options).await.unwrap();
    let before = super::Migrator::migrations()
        .iter()
        .position(|migration| migration.name() == "m20261002_000076_webhook_delivery_retries")
        .unwrap();
    super::Migrator::up(&database, Some(before as u32))
        .await
        .unwrap();
    let owner = crate::actions::test_support::owner(&database, "owner").await;
    let repository = crate::actions::test_support::repository(&database, &owner, "project").await;
    let webhook_id = Uuid::new_v4();
    let delivery_id = Uuid::new_v4();
    let at = "'2026-09-01T00:00:00+00:00'";
    // The delivery entity names the new column, so the old table is written with SQL.
    for statement in [
        format!(
            "INSERT INTO repository_webhooks (id, repository_id, url, secret, active, created_at, \
             updated_at, events) VALUES (x'{}', x'{}', 'https://example.com/hook', NULL, 1, {at}, {at}, 'push')",
            webhook_id.simple(),
            repository.id.simple()
        ),
        format!(
            "INSERT INTO repository_webhook_deliveries (id, webhook_id, event, payload, duration_ms, \
             created_at, status, attempt_count) VALUES (x'{}', x'{}', 'push', '{{}}', 0, {at}, 'pending', 0)",
            delivery_id.simple(),
            webhook_id.simple()
        ),
    ] {
        database.execute_unprepared(&statement).await.unwrap();
    }

    super::Migrator::up(&database, None).await.unwrap();

    let delivery = repository_webhook_delivery::Entity::find_by_id(delivery_id)
        .one(&database)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(delivery.status, "pending");
    assert_eq!(delivery.next_attempt_at, None);
}

#[tokio::test]
async fn latest_audit_event_of_an_action_is_found_through_the_index() {
    let mut options = ConnectOptions::new("sqlite::memory:");
    options.max_connections(1);
    let database = Database::connect(options).await.unwrap();
    super::Migrator::up(&database, None).await.unwrap();

    let plan = database
        .query_all_raw(Statement::from_string(
            DbBackend::Sqlite,
            "EXPLAIN QUERY PLAN SELECT id FROM audit_events \
             WHERE action = 'repository.integrity.check' ORDER BY id DESC LIMIT 1",
        ))
        .await
        .unwrap()
        .into_iter()
        .map(|row| row.try_get::<String>("", "detail").unwrap())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(plan.contains("idx-audit-event-action-id"), "{plan}");
    assert!(!plan.contains("TEMP B-TREE"), "{plan}");
}

#[tokio::test]
async fn cache_kind_migration_drops_borrowed_kinds_and_admits_the_new_ones() {
    use crate::entity::repository_cache_entry;

    let mut options = ConnectOptions::new("sqlite::memory:");
    options.max_connections(1);
    let database = Database::connect(options).await.unwrap();
    let before = super::Migrator::migrations()
        .iter()
        .position(|migration| migration.name() == "m20261003_000081_repository_cache_kinds")
        .unwrap();
    super::Migrator::up(&database, Some(before as u32))
        .await
        .unwrap();
    let owner = crate::actions::test_support::owner(&database, "owner").await;
    let repository = crate::actions::test_support::repository(&database, &owner, "project").await;
    let now = chrono::Utc::now();
    let entry = |kind: &str, key: &str| repository_cache_entry::ActiveModel {
        repository_id: Set(repository.id),
        kind: Set(kind.to_owned()),
        cache_key: Set(key.to_owned()),
        payload_json: Set("[]".to_owned()),
        expires_at: Set(now + chrono::Duration::days(1)),
        created_at: Set(now),
    };
    // Rows the day-commit and line-stat caches wrote under borrowed kinds,
    // beside the rows those kinds legitimately own.
    for (kind, key) in [
        ("overview", "digest:v3:main"),
        ("overview", "digest:day:2026-09-01"),
        ("commit_count", "0123abcd"),
        ("commit_count", "diff:0123abcd"),
    ] {
        entry(kind, key).insert(&database).await.unwrap();
    }
    for kind in ["day_commits", "commit_stats"] {
        assert!(
            entry(kind, "early").insert(&database).await.is_err(),
            "{kind} is refused before the migration"
        );
    }

    super::Migrator::up(&database, None).await.unwrap();

    let mut remaining = repository_cache_entry::Entity::find()
        .filter(repository_cache_entry::Column::RepositoryId.eq(repository.id))
        .all(&database)
        .await
        .unwrap()
        .into_iter()
        .map(|row| (row.kind, row.cache_key))
        .collect::<Vec<_>>();
    remaining.sort();
    assert_eq!(
        remaining,
        vec![
            ("commit_count".to_owned(), "0123abcd".to_owned()),
            ("overview".to_owned(), "digest:v3:main".to_owned()),
        ]
    );
    for kind in ["activity", "day_commits", "commit_stats"] {
        entry(kind, "late").insert(&database).await.unwrap();
    }
    assert!(entry("bogus", "late").insert(&database).await.is_err());
    let index = database
        .query_one_raw(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT name FROM sqlite_master WHERE type = 'index' \
             AND name = 'idx_repository_cache_expiry'",
        ))
        .await
        .unwrap();
    assert!(index.is_some(), "the expiry index is recreated");
}

#[tokio::test]
async fn commit_email_verification_migration_grandfathers_existing_addresses() {
    use crate::entity::user_commit_email;

    let mut options = ConnectOptions::new("sqlite::memory:");
    options.max_connections(1);
    let database = Database::connect(options).await.unwrap();
    // Every migration before 000078, which rebuilds the table.
    super::Migrator::up(&database, Some(75)).await.unwrap();
    let owner = legacy_owner(&database).await;
    database
        .execute_unprepared(&format!(
            "INSERT INTO user_commit_emails (user_id, email, created_at) \
             VALUES (x'{}', 'old@example.com', '2026-09-30 10:00:00.000000 +00:00')",
            owner.id.simple()
        ))
        .await
        .unwrap();

    super::Migrator::up(&database, None).await.unwrap();

    let row = user_commit_email::Entity::find()
        .filter(user_commit_email::Column::Email.eq("old@example.com"))
        .one(&database)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.user_id, owner.id);
    assert_eq!(
        row.verified_at,
        Some(row.created_at),
        "pre-existing rows keep counting"
    );

    // Two accounts may hold an unverified claim; only verified ones are unique.
    let other = Uuid::new_v4();
    database
        .execute_unprepared(&format!(
            "INSERT INTO users (id, username, password_hash, is_admin, \
             default_repository_visibility, theme_preference, motion_preference, \
             created_at, updated_at) \
             VALUES (x'{}', 'claimant', 'unused', 0, 'private', 'system', 'system', \
             '2026-09-30 10:00:00.000000 +00:00', '2026-09-30 10:00:00.000000 +00:00')",
            other.simple()
        ))
        .await
        .unwrap();
    let claim = |verified: &str| {
        format!(
            "INSERT INTO user_commit_emails (user_id, email, verified_at, created_at) \
             VALUES (x'{}', 'old@example.com', {verified}, '2026-10-01 10:00:00.000000 +00:00')",
            other.simple()
        )
    };
    assert!(
        database
            .execute_unprepared(&claim("'2026-10-01 10:00:00.000000 +00:00'"))
            .await
            .is_err(),
        "a second verified claim on one address is refused"
    );
    database.execute_unprepared(&claim("NULL")).await.unwrap();
}
