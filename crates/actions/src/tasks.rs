//! Runner task preparation without generated protocol types.
use crate::{ActionsService, matrix, runners, secrets};
use gitadel_db::entity::{action_job, action_job_need, action_run, repository, user};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde_json::{Value, json};
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum TaskConclusion {
    Success,
    Skipped,
    Unspecified,
    Cancelled,
    Failure,
}
impl TaskConclusion {
    pub fn from_state(state: &str) -> Self {
        match state {
            "success" => Self::Success,
            "skipped" => Self::Skipped,
            "cancelled" => Self::Cancelled,
            "failure" => Self::Failure,
            _ => Self::Unspecified,
        }
    }
    pub fn merge(self, other: Self) -> Self {
        self.max(other)
    }
}
pub struct PreparedNeed {
    pub outputs: HashMap<String, String>,
    pub result: TaskConclusion,
}
pub struct PreparedTask {
    pub id: i64,
    pub workflow_payload: Vec<u8>,
    pub context: Value,
    pub secrets: HashMap<String, String>,
    pub needs: HashMap<String, PreparedNeed>,
    pub vars: HashMap<String, String>,
}
pub async fn prepare_task(
    state: &ActionsService,
    claimed: runners::ClaimedJob,
    server_url: String,
    server_version: &str,
) -> Result<PreparedTask, TaskPreparationError> {
    let database = &state.database;
    let authenticated = crate::tokens::authenticate_job(database, &claimed.checkout_token)
        .await?
        .ok_or_else(TaskPreparationError::unauthorized)?;
    if authenticated.job.id != claimed.job.id
        || authenticated.job.lease_generation != claimed.job.lease_generation
    {
        return Err(TaskPreparationError::unauthorized());
    }
    let claimed = runners::ClaimedJob {
        job: authenticated.job,
        checkout_token: claimed.checkout_token,
    };

    let run = action_run::Entity::find_by_id(claimed.job.run_id)
        .one(database)
        .await?
        .ok_or_else(|| TaskPreparationError::internal("assigned run is missing"))?;
    let repository = repository::Entity::find_by_id(run.repository_id)
        .one(database)
        .await?
        .ok_or_else(|| TaskPreparationError::internal("assigned repository is missing"))?;
    let actor = match run.actor_id {
        Some(actor_id) => user::Entity::find_by_id(actor_id)
            .one(database)
            .await?
            .map(|actor| actor.username)
            .unwrap_or_default(),
        None => repository.namespace.clone(),
    };
    let ref_name = run
        .ref_name
        .strip_prefix("refs/heads/")
        .or_else(|| run.ref_name.strip_prefix("refs/tags/"))
        .unwrap_or(&run.ref_name);
    let ref_type = if run.ref_name.starts_with("refs/tags/") {
        "tag"
    } else {
        "branch"
    };
    let event =
        serde_json::from_str::<serde_json::Value>(&run.event_json).unwrap_or_else(|_| json!({}));
    let context_json = json!({
        "action": "", "action_path": "", "action_ref": "", "action_repository": "",
        "actor": actor, "api_url": format!("{server_url}/api/v1"), "base_ref": "",
        "event": event, "event_name": run.event, "event_path": "", "graphql_url": "",
        "head_ref": "", "job": matrix::base_job_key(&claimed.job.job_key), "ref": run.ref_name, "ref_name": ref_name,
        "ref_protected": false, "ref_type": ref_type, "repository": format!("{}/{}", repository.namespace, repository.name),
        "repository_id": repository.id.to_string(), "repository_owner": repository.namespace,
        "retention_days": state.settings().retention_days.to_string(),
        "run_attempt": run.run_attempt.to_string(), "run_id": run.id.to_string(),
        "run_number": run.number.to_string(), "runtime_token": claimed.checkout_token,
        "server_url": server_url, "sha": run.after_sha,
        "token": claimed.checkout_token, "workflow": run.workflow_name,
        "workflow_ref": format!("{}/{}/{}@{}", repository.namespace, repository.name, run.workflow_path, run.ref_name),
        "workflow_sha": run.after_sha, "workspace": "",
        "forgejo_default_actions_url": state.settings().default_actions_origin,
        "forgejo_server_version": server_version,
        "clone_url": format!("{server_url}/{}/{}.git", repository.namespace, repository.name),
    });

    let edges = action_job_need::Entity::find()
        .filter(action_job_need::Column::JobId.eq(claimed.job.id))
        .all(database)
        .await?;
    // A matrix job is needed as a whole: it fails if any expansion failed,
    // and its outputs merge those of every expansion.
    let mut needs: HashMap<String, PreparedNeed> = HashMap::new();
    for edge in edges {
        if let Some(job) = action_job::Entity::find_by_id(edge.needed_job_id)
            .one(database)
            .await?
        {
            let outputs: HashMap<String, String> =
                serde_json::from_str(&job.outputs).unwrap_or_default();
            let result = TaskConclusion::from_state(job.result.as_deref().unwrap_or(&job.status));
            let need = needs
                .entry(edge.needed_job_key)
                .or_insert_with(|| PreparedNeed {
                    outputs: HashMap::new(),
                    result,
                });
            need.outputs.extend(outputs);
            need.result = need.result.merge(result);
        }
    }
    let values = secrets::job_values(database, state.secret_cipher(), &repository).await?;
    let mut secrets = values.secrets;
    // Reserved prefixes keep user secrets from shadowing the job token.
    for name in ["GITHUB_TOKEN", "GITEA_TOKEN", "FORGEJO_TOKEN"] {
        secrets.insert(name.to_owned(), claimed.checkout_token.clone());
    }
    Ok(PreparedTask {
        id: claimed.job.id,
        workflow_payload: claimed.job.workflow_payload,
        context: context_json,
        secrets,
        needs,
        vars: values.vars,
    })
}

#[derive(Debug)]
pub enum TaskPreparationError {
    Unauthorized,
    Database(sea_orm::DbErr),
    Internal(String),
}
impl TaskPreparationError {
    fn unauthorized() -> Self {
        Self::Unauthorized
    }
    fn internal(message: impl Into<String>) -> Self {
        Self::Internal(message.into())
    }
}
impl From<sea_orm::DbErr> for TaskPreparationError {
    fn from(error: sea_orm::DbErr) -> Self {
        Self::Database(error)
    }
}
