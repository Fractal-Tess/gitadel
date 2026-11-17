use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // A rerun is a new run that points at the run it repeats. Jobs kept
        // from that run by "re-run failed jobs" point at the job whose logs,
        // outputs, and artifacts they reuse.
        let statements = [
            "ALTER TABLE action_runs ADD COLUMN rerun_of TEXT REFERENCES action_runs(id) ON DELETE SET NULL",
            "ALTER TABLE action_runs ADD COLUMN run_attempt INTEGER NOT NULL DEFAULT 1 CHECK (run_attempt > 0)",
            "ALTER TABLE action_jobs ADD COLUMN copied_from_job_id INTEGER REFERENCES action_jobs(id) ON DELETE SET NULL",
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
        let statements = [
            "ALTER TABLE action_jobs DROP COLUMN copied_from_job_id",
            "ALTER TABLE action_runs DROP COLUMN run_attempt",
            "ALTER TABLE action_runs DROP COLUMN rerun_of",
        ];
        for statement in statements {
            manager
                .get_connection()
                .execute_unprepared(statement)
                .await?;
        }
        Ok(())
    }
}
