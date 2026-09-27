use sea_orm::{ConnectionTrait, DbBackend, Statement};
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        rebuild_runs(manager, "('push', 'workflow_dispatch', 'schedule')").await?;
        let statements = [
            // One row per cron entry of a workflow on the default branch.
            // next_fire_at is persisted so restarts neither skip nor repeat runs.
            r#"CREATE TABLE action_schedules (
                repository_id TEXT NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
                workflow_path TEXT NOT NULL,
                cron TEXT NOT NULL,
                last_fired_at TEXT,
                next_fire_at TEXT NOT NULL,
                created_at TEXT NOT NULL,
                PRIMARY KEY (repository_id, workflow_path, cron)
            )"#,
            r#"CREATE INDEX idx_action_schedules_next ON action_schedules(next_fire_at)"#,
            // The default-branch commit whose workflows populated action_schedules.
            r#"CREATE TABLE action_schedule_scans (
                repository_id TEXT PRIMARY KEY NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
                default_branch TEXT NOT NULL,
                head_sha TEXT NOT NULL,
                scanned_at TEXT NOT NULL
            )"#,
        ];
        for statement in statements {
            manager
                .get_connection()
                .execute_unprepared(statement)
                .await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for table in ["action_schedule_scans", "action_schedules"] {
            manager
                .get_connection()
                .execute_unprepared(&format!("DROP TABLE {table}"))
                .await?;
        }
        manager
            .get_connection()
            .execute_unprepared("DELETE FROM action_runs WHERE event <> 'push'")
            .await?;
        rebuild_runs(manager, "('push')").await
    }
}

/// Recreates `action_runs` with a new event constraint. Child tables keep
/// referencing `action_runs` by name because legacy_alter_table is enabled.
async fn rebuild_runs(manager: &SchemaManager<'_>, events: &str) -> Result<(), DbErr> {
    let database = manager.get_connection();
    database
        .execute_unprepared("PRAGMA foreign_keys = OFF")
        .await?;
    database
        .execute_unprepared("PRAGMA legacy_alter_table = ON")
        .await?;
    if let Err(error) = database.execute_unprepared("BEGIN IMMEDIATE").await {
        restore_pragmas(database).await;
        return Err(error);
    }
    let migration = async {
        database
            .execute_unprepared("ALTER TABLE action_runs RENAME TO action_runs_previous")
            .await?;
        database
            .execute_unprepared(&format!(
                r#"CREATE TABLE action_runs (
                    id TEXT PRIMARY KEY NOT NULL,
                    repository_id TEXT NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
                    number INTEGER NOT NULL CHECK (number > 0),
                    workflow_path TEXT NOT NULL,
                    workflow_name TEXT NOT NULL,
                    event TEXT NOT NULL CHECK (event IN {events}),
                    ref_name TEXT NOT NULL,
                    before_sha TEXT NOT NULL,
                    after_sha TEXT NOT NULL,
                    actor_id TEXT REFERENCES users(id) ON DELETE SET NULL,
                    status TEXT NOT NULL CHECK (status IN ('queued','running','success','failure','cancelled')),
                    failure_kind TEXT,
                    failure_summary TEXT,
                    diagnostic TEXT,
                    event_json TEXT NOT NULL,
                    cancel_requested_at TEXT,
                    cancelled_by TEXT REFERENCES users(id) ON DELETE SET NULL,
                    created_at TEXT NOT NULL,
                    started_at TEXT,
                    completed_at TEXT,
                    UNIQUE (repository_id, number)
                )"#
            ))
            .await?;
        database
            .execute_unprepared(
                r#"INSERT INTO action_runs (
                    id, repository_id, number, workflow_path, workflow_name, event, ref_name,
                    before_sha, after_sha, actor_id, status, failure_kind, failure_summary,
                    diagnostic, event_json, cancel_requested_at, cancelled_by, created_at,
                    started_at, completed_at
                )
                SELECT id, repository_id, number, workflow_path, workflow_name, event, ref_name,
                       before_sha, after_sha, actor_id, status, failure_kind, failure_summary,
                       diagnostic, event_json, cancel_requested_at, cancelled_by, created_at,
                       started_at, completed_at
                FROM action_runs_previous"#,
            )
            .await?;
        database
            .execute_unprepared("DROP TABLE action_runs_previous")
            .await?;
        for index in [
            "CREATE INDEX idx_action_runs_repository_created ON action_runs(repository_id, created_at DESC)",
            "CREATE INDEX idx_action_runs_repository_sha ON action_runs(repository_id, after_sha)",
            "CREATE INDEX idx_action_runs_status ON action_runs(status)",
        ] {
            database.execute_unprepared(index).await?;
        }
        Ok::<(), DbErr>(())
    }
    .await;
    let result = match migration {
        Ok(()) => match database
            .query_all_raw(Statement::from_string(
                DbBackend::Sqlite,
                "SELECT * FROM pragma_foreign_key_check WHERE \"table\" IN (\
                 'action_runs', 'action_jobs', 'action_artifacts')",
            ))
            .await
        {
            Ok(violations) if violations.is_empty() => {
                database.execute_unprepared("COMMIT").await.map(|_| ())
            }
            Ok(_) => {
                let _ = database.execute_unprepared("ROLLBACK").await;
                Err(DbErr::Migration(
                    "action run migration violated foreign keys".to_owned(),
                ))
            }
            Err(error) => {
                let _ = database.execute_unprepared("ROLLBACK").await;
                Err(error)
            }
        },
        Err(error) => {
            let _ = database.execute_unprepared("ROLLBACK").await;
            Err(error)
        }
    };
    restore_pragmas(database).await;
    result
}

async fn restore_pragmas(database: &impl ConnectionTrait) {
    let _ = database
        .execute_unprepared("PRAGMA legacy_alter_table = OFF")
        .await;
    let _ = database
        .execute_unprepared("PRAGMA foreign_keys = ON")
        .await;
}
