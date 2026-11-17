use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let statements = [
            r#"CREATE TABLE action_artifacts (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                repository_id TEXT NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
                run_id TEXT NOT NULL REFERENCES action_runs(id) ON DELETE CASCADE,
                creating_job_id INTEGER NOT NULL REFERENCES action_jobs(id) ON DELETE CASCADE,
                name TEXT NOT NULL CHECK (length(name) > 0 AND length(CAST(name AS BLOB)) <= 255),
                storage_key TEXT NOT NULL UNIQUE,
                status TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'finalized')),
                size_bytes INTEGER NOT NULL DEFAULT 0 CHECK (size_bytes >= 0),
                sha256 TEXT CHECK (sha256 IS NULL OR (length(sha256) = 64 AND sha256 NOT GLOB '*[^0-9a-f]*')),
                expires_at TEXT NOT NULL,
                finalized_at TEXT,
                deleted_at TEXT,
                deleted_by_job_id INTEGER REFERENCES action_jobs(id) ON DELETE SET NULL,
                metadata_json TEXT NOT NULL DEFAULT '{}',
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                CHECK (status = 'pending' OR (sha256 IS NOT NULL AND finalized_at IS NOT NULL))
            )"#,
            r#"CREATE TABLE action_artifact_grants (
                id TEXT PRIMARY KEY NOT NULL,
                artifact_id INTEGER NOT NULL REFERENCES action_artifacts(id) ON DELETE CASCADE,
                issued_to_job_id INTEGER NOT NULL REFERENCES action_jobs(id) ON DELETE CASCADE,
                scope TEXT NOT NULL CHECK (scope IN ('upload', 'download')),
                token_hash TEXT NOT NULL UNIQUE,
                expires_at TEXT NOT NULL,
                revoked_at TEXT,
                last_used_at TEXT,
                use_count INTEGER NOT NULL DEFAULT 0 CHECK (use_count >= 0),
                created_at TEXT NOT NULL
            )"#,
            r#"CREATE INDEX idx_action_artifacts_run_created ON action_artifacts(run_id, created_at DESC)"#,
            r#"CREATE INDEX idx_action_artifacts_repository ON action_artifacts(repository_id, created_at DESC)"#,
            r#"CREATE INDEX idx_action_artifacts_expiry ON action_artifacts(expires_at, deleted_at)"#,
            r#"CREATE UNIQUE INDEX uq_action_artifacts_active_run_name
               ON action_artifacts(run_id, name) WHERE deleted_at IS NULL"#,
            r#"CREATE INDEX idx_action_artifact_grants_lookup ON action_artifact_grants(token_hash, expires_at, revoked_at)"#,
            r#"CREATE INDEX idx_action_artifact_grants_artifact_scope ON action_artifact_grants(artifact_id, scope, expires_at)"#,
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
        for table in ["action_artifact_grants", "action_artifacts"] {
            manager
                .get_connection()
                .execute_unprepared(&format!("DROP TABLE {table}"))
                .await?;
        }
        Ok(())
    }
}
