use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let statements = [
            r#"CREATE TABLE action_repository_settings (
                repository_id TEXT PRIMARY KEY NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
                enabled INTEGER NOT NULL DEFAULT 0 CHECK (enabled IN (0, 1)),
                trust_acknowledged_at TEXT,
                trust_acknowledged_by TEXT REFERENCES users(id) ON DELETE SET NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            )"#,
            r#"CREATE TABLE action_runs (
                id TEXT PRIMARY KEY NOT NULL,
                repository_id TEXT NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
                number INTEGER NOT NULL CHECK (number > 0),
                workflow_path TEXT NOT NULL,
                workflow_name TEXT NOT NULL,
                event TEXT NOT NULL CHECK (event = 'push'),
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
            )"#,
            r#"CREATE TABLE action_runners (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                uuid TEXT NOT NULL UNIQUE,
                repository_id TEXT NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
                name TEXT NOT NULL,
                token_hash TEXT NOT NULL UNIQUE,
                approved_labels TEXT NOT NULL,
                version TEXT NOT NULL,
                ephemeral INTEGER NOT NULL DEFAULT 0 CHECK (ephemeral = 0),
                disabled_at TEXT,
                deleted_at TEXT,
                last_seen_at TEXT,
                created_by TEXT REFERENCES users(id) ON DELETE SET NULL,
                created_at TEXT NOT NULL
            )"#,
            r#"CREATE UNIQUE INDEX uq_action_runners_active_repository ON action_runners(repository_id) WHERE deleted_at IS NULL AND disabled_at IS NULL"#,
            r#"CREATE TABLE action_jobs (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                run_id TEXT NOT NULL REFERENCES action_runs(id) ON DELETE CASCADE,
                job_key TEXT NOT NULL,
                name TEXT NOT NULL,
                required_labels TEXT NOT NULL,
                workflow_payload BLOB NOT NULL,
                status TEXT NOT NULL CHECK (status IN ('waiting','queued','leased','running','success','failure','cancelled','skipped')),
                result TEXT,
                runner_id INTEGER REFERENCES action_runners(id) ON DELETE SET NULL,
                request_key TEXT,
                lease_generation INTEGER NOT NULL DEFAULT 0,
                lease_deadline TEXT,
                last_report_at TEXT,
                attempt INTEGER NOT NULL DEFAULT 0,
                step_state TEXT NOT NULL DEFAULT '[]',
                outputs TEXT NOT NULL DEFAULT '{}',
                expected_log_index INTEGER NOT NULL DEFAULT 0,
                log_bytes INTEGER NOT NULL DEFAULT 0,
                log_truncated INTEGER NOT NULL DEFAULT 0 CHECK (log_truncated IN (0, 1)),
                failure_kind TEXT,
                failure_summary TEXT,
                created_at TEXT NOT NULL,
                started_at TEXT,
                completed_at TEXT,
                UNIQUE (run_id, job_key)
            )"#,
            r#"CREATE INDEX idx_action_jobs_queue ON action_jobs(status, created_at)"#,
            r#"CREATE INDEX idx_action_jobs_runner_status ON action_jobs(runner_id, status)"#,
            r#"CREATE TABLE action_job_needs (
                job_id INTEGER NOT NULL REFERENCES action_jobs(id) ON DELETE CASCADE,
                needed_job_id INTEGER NOT NULL REFERENCES action_jobs(id) ON DELETE CASCADE,
                needed_job_key TEXT NOT NULL,
                PRIMARY KEY (job_id, needed_job_id),
                CHECK (job_id <> needed_job_id)
            )"#,
            r#"CREATE TABLE action_job_logs (
                job_id INTEGER NOT NULL REFERENCES action_jobs(id) ON DELETE CASCADE,
                row_index INTEGER NOT NULL CHECK (row_index >= 0),
                timestamp TEXT NOT NULL,
                content TEXT NOT NULL,
                byte_count INTEGER NOT NULL CHECK (byte_count >= 0),
                PRIMARY KEY (job_id, row_index)
            )"#,
            r#"CREATE TABLE action_runner_registration_tokens (
                id TEXT PRIMARY KEY NOT NULL,
                token_hash TEXT NOT NULL UNIQUE,
                repository_id TEXT NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
                runner_name TEXT NOT NULL,
                approved_labels TEXT NOT NULL,
                expires_at TEXT NOT NULL,
                used_at TEXT,
                created_by TEXT REFERENCES users(id) ON DELETE SET NULL,
                created_at TEXT NOT NULL
            )"#,
            r#"CREATE TABLE action_runner_fetches (
                runner_id INTEGER NOT NULL REFERENCES action_runners(id) ON DELETE CASCADE,
                request_key TEXT NOT NULL,
                job_id INTEGER NOT NULL REFERENCES action_jobs(id) ON DELETE CASCADE,
                lease_generation INTEGER NOT NULL,
                token_generation TEXT NOT NULL,
                created_at TEXT NOT NULL,
                PRIMARY KEY (runner_id, request_key)
            )"#,
            r#"CREATE TABLE action_job_tokens (
                id TEXT PRIMARY KEY NOT NULL,
                job_id INTEGER NOT NULL REFERENCES action_jobs(id) ON DELETE CASCADE,
                repository_id TEXT NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
                lease_generation INTEGER NOT NULL,
                token_hash TEXT NOT NULL UNIQUE,
                expires_at TEXT NOT NULL,
                revoked_at TEXT,
                created_at TEXT NOT NULL
            )"#,
            r#"CREATE INDEX idx_action_runs_repository_created ON action_runs(repository_id, created_at DESC)"#,
            r#"CREATE INDEX idx_action_runs_repository_sha ON action_runs(repository_id, after_sha)"#,
            r#"CREATE INDEX idx_action_runs_status ON action_runs(status)"#,
            r#"CREATE INDEX idx_action_registration_expiry ON action_runner_registration_tokens(expires_at, used_at)"#,
            r#"CREATE INDEX idx_action_job_token_hash ON action_job_tokens(token_hash, revoked_at, expires_at)"#,
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
        for table in [
            "action_job_tokens",
            "action_runner_fetches",
            "action_runner_registration_tokens",
            "action_job_logs",
            "action_job_needs",
            "action_jobs",
            "action_runners",
            "action_runs",
            "action_repository_settings",
        ] {
            manager
                .get_connection()
                .execute_unprepared(&format!("DROP TABLE {table}"))
                .await?;
        }
        Ok(())
    }
}
