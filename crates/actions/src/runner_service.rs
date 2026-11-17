//! Credential-preserving runner operations over the existing state machines.
use crate::{ActionsService, logs, runners, runs};
use gitadel_db::entity::{action_job, action_runner};
use std::collections::BTreeMap;
use uuid::Uuid;

#[derive(Clone)]
pub struct RunnerCredential {
    uuid: String,
    token: String,
}
impl RunnerCredential {
    pub fn new(uuid: String, token: String) -> Self {
        Self { uuid, token }
    }
    pub async fn authenticate(
        &self,
        database: &sea_orm::DatabaseConnection,
    ) -> Result<action_runner::Model, runners::RunnerError> {
        runners::authenticate(database, &self.uuid, &self.token).await
    }
}

#[derive(Debug)]
pub enum RunnerUpdateError {
    Credential(runners::RunnerError),
    State(runs::StateError),
}

impl ActionsService {
    pub async fn declare_runner(
        &self,
        credential: &RunnerCredential,
        version: &str,
        labels: &[String],
    ) -> Result<action_runner::Model, runners::RunnerError> {
        let runner = credential.authenticate(&self.database).await?;
        runners::declare(&self.database, runner, version, labels).await
    }
    pub async fn claim_task(
        &self,
        credential: &RunnerCredential,
        request_key: Uuid,
        handle: Option<i64>,
    ) -> Result<Option<runners::ClaimedJob>, runners::RunnerError> {
        let runner = credential.authenticate(&self.database).await?;
        runners::claim(
            &self.database,
            &runner,
            request_key,
            handle,
            self.settings.runner_loss_seconds,
        )
        .await
    }
    pub async fn update_runner_task(
        &self,
        credential: &RunnerCredential,
        task_id: i64,
        result: runs::TaskResult,
        outputs: BTreeMap<String, String>,
        steps_json: String,
    ) -> Result<action_job::Model, RunnerUpdateError> {
        let runner = credential
            .authenticate(&self.database)
            .await
            .map_err(RunnerUpdateError::Credential)?;
        let (job, completed) = runs::update_task(
            &self.database,
            runner.id,
            task_id,
            result,
            outputs,
            steps_json,
            self.settings.runner_loss_seconds,
        )
        .await
        .map_err(RunnerUpdateError::State)?;
        if let Some(run_id) = completed {
            self.effects.completed(run_id);
        }
        Ok(job)
    }
    pub async fn append_logs(
        &self,
        credential: &RunnerCredential,
        task_id: i64,
        start_index: i64,
        rows: Vec<logs::IncomingLogRow>,
    ) -> Result<i64, logs::LogError> {
        let runner = credential
            .authenticate(&self.database)
            .await
            .map_err(logs::LogError::Credential)?;
        logs::append(
            &self.database,
            runner.id,
            task_id,
            start_index,
            rows,
            self.settings.max_log_row_bytes,
            self.settings.max_job_log_bytes,
        )
        .await
    }
}
