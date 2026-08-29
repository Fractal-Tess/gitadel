use std::collections::BTreeMap;

use chrono::{Duration, Utc};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, IntoActiveModel, QueryFilter,
    Set, TransactionTrait,
};
use uuid::Uuid;

use crate::entity::{
    action_job, action_job_need, action_job_token, action_run, action_runner_fetch,
    action_runner_registration_token,
};

use super::{ActionsState, tokens};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TaskResult {
    Heartbeat,
    Success,
    Failure,
    Cancelled,
    Skipped,
}

impl TaskResult {
    fn status(self) -> Option<&'static str> {
        match self {
            Self::Heartbeat => None,
            Self::Success => Some("success"),
            Self::Failure => Some("failure"),
            Self::Cancelled => Some("cancelled"),
            Self::Skipped => Some("skipped"),
        }
    }
}

pub(crate) async fn update_task(
    database: &DatabaseConnection,
    runner_id: i64,
    task_id: i64,
    result: TaskResult,
    outputs: BTreeMap<String, String>,
    steps_json: String,
    lease_seconds: i64,
) -> Result<action_job::Model, StateError> {
    validate_outputs(&outputs)?;
    let transaction = database.begin().await?;
    let job = action_job::Entity::find_by_id(task_id)
        .one(&transaction)
        .await?
        .filter(|job| job.runner_id == Some(runner_id))
        .ok_or(StateError::ForeignTask)?;
    if job.status == "cancelled" {
        transaction.commit().await?;
        return Ok(job);
    }
    if matches!(job.status.as_str(), "success" | "failure" | "skipped") {
        if result.status() == Some(job.status.as_str()) {
            transaction.commit().await?;
            return Ok(job);
        }
        return Err(StateError::Regression);
    }
    let now = Utc::now();
    let started_at = job.started_at.unwrap_or(now);
    let mut active = job.into_active_model();
    active.last_report_at = Set(Some(now));
    active.step_state = Set(steps_json);
    active.outputs = Set(serde_json::to_string(&outputs).expect("outputs serialize"));
    match result.status() {
        None => {
            active.status = Set("running".to_owned());
            active.started_at = Set(Some(started_at));
            active.lease_deadline = Set(Some(now + Duration::seconds(lease_seconds)));
        }
        Some(status) => {
            active.status = Set(status.to_owned());
            active.result = Set(Some(status.to_owned()));
            active.completed_at = Set(Some(now));
            active.lease_deadline = Set(None);
        }
    }
    let job = active.update(&transaction).await?;
    if result.status().is_some() {
        tokens::revoke_job(&transaction, job.id).await?;
        release_dependents(&transaction, job.id).await?;
    }
    aggregate_run(&transaction, job.run_id).await?;
    transaction.commit().await?;
    Ok(job)
}

pub(crate) async fn cancel_run(
    database: &DatabaseConnection,
    run_id: Uuid,
    actor_id: Uuid,
) -> Result<action_run::Model, StateError> {
    let transaction = database.begin().await?;
    let run = action_run::Entity::find_by_id(run_id)
        .one(&transaction)
        .await?
        .ok_or(StateError::Missing)?;
    if matches!(run.status.as_str(), "success" | "failure" | "cancelled") {
        transaction.commit().await?;
        return Ok(run);
    }
    let now = Utc::now();
    let jobs = action_job::Entity::find()
        .filter(action_job::Column::RunId.eq(run_id))
        .all(&transaction)
        .await?;
    for job in jobs {
        if !matches!(
            job.status.as_str(),
            "success" | "failure" | "cancelled" | "skipped"
        ) {
            let id = job.id;
            let mut active = job.into_active_model();
            active.status = Set("cancelled".to_owned());
            active.result = Set(Some("cancelled".to_owned()));
            active.completed_at = Set(Some(now));
            active.lease_deadline = Set(None);
            active.update(&transaction).await?;
            tokens::revoke_job(&transaction, id).await?;
        }
    }
    let mut active = run.into_active_model();
    active.status = Set("cancelled".to_owned());
    active.cancel_requested_at = Set(Some(now));
    active.cancelled_by = Set(Some(actor_id));
    active.completed_at = Set(Some(now));
    let run = active.update(&transaction).await?;
    transaction.commit().await?;
    Ok(run)
}

async fn release_dependents<C: sea_orm::ConnectionTrait>(
    database: &C,
    completed_job: i64,
) -> Result<(), sea_orm::DbErr> {
    let edges = action_job_need::Entity::find()
        .filter(action_job_need::Column::NeededJobId.eq(completed_job))
        .all(database)
        .await?;
    for edge in edges {
        let waiting = action_job::Entity::find_by_id(edge.job_id)
            .one(database)
            .await?;
        let Some(waiting) = waiting.filter(|job| job.status == "waiting") else {
            continue;
        };
        let needs = action_job_need::Entity::find()
            .filter(action_job_need::Column::JobId.eq(waiting.id))
            .all(database)
            .await?;
        let mut all_terminal = true;
        for need in needs {
            let terminal = action_job::Entity::find_by_id(need.needed_job_id)
                .one(database)
                .await?
                .is_some_and(|job| {
                    matches!(
                        job.status.as_str(),
                        "success" | "failure" | "cancelled" | "skipped"
                    )
                });
            all_terminal &= terminal;
        }
        if all_terminal {
            let mut active = waiting.into_active_model();
            active.status = Set("queued".to_owned());
            active.update(database).await?;
        }
    }
    Ok(())
}

async fn aggregate_run<C: sea_orm::ConnectionTrait>(
    database: &C,
    run_id: Uuid,
) -> Result<(), sea_orm::DbErr> {
    let jobs = action_job::Entity::find()
        .filter(action_job::Column::RunId.eq(run_id))
        .all(database)
        .await?;
    if jobs.is_empty() {
        return Ok(());
    }
    let terminal = jobs.iter().all(|job| {
        matches!(
            job.status.as_str(),
            "success" | "failure" | "cancelled" | "skipped"
        )
    });
    let status = if terminal && jobs.iter().any(|job| job.status == "failure") {
        "failure"
    } else if terminal && jobs.iter().any(|job| job.status == "cancelled") {
        "cancelled"
    } else if terminal {
        "success"
    } else if jobs.iter().any(|job| {
        matches!(
            job.status.as_str(),
            "leased" | "running" | "success" | "failure" | "cancelled" | "skipped"
        )
    }) {
        "running"
    } else {
        "queued"
    };
    if let Some(run) = action_run::Entity::find_by_id(run_id).one(database).await? {
        let now = Utc::now();
        let mut active = run.into_active_model();
        active.status = Set(status.to_owned());
        if status == "running" && active.started_at.as_ref().is_none() {
            active.started_at = Set(Some(now));
        }
        if terminal {
            active.completed_at = Set(Some(now));
        }
        active.update(database).await?;
    }
    Ok(())
}

fn validate_outputs(outputs: &BTreeMap<String, String>) -> Result<(), StateError> {
    for (key, value) in outputs {
        if key.len() > 255 || value.len() > 1_048_576 {
            return Err(StateError::InvalidOutput);
        }
    }
    Ok(())
}

pub(crate) async fn maintain(state: &ActionsState) -> anyhow::Result<()> {
    let database = state.repository().identity().database();
    let now = Utc::now();
    action_runner_registration_token::Entity::delete_many()
        .filter(action_runner_registration_token::Column::ExpiresAt.lt(now))
        .exec(database)
        .await?;
    action_job_token::Entity::update_many()
        .col_expr(action_job_token::Column::RevokedAt, now.into())
        .filter(action_job_token::Column::ExpiresAt.lt(now))
        .filter(action_job_token::Column::RevokedAt.is_null())
        .exec(database)
        .await?;
    let lost = action_job::Entity::find()
        .filter(action_job::Column::Status.is_in(["leased", "running"]))
        .filter(action_job::Column::LeaseDeadline.lt(now))
        .all(database)
        .await?;
    for job in lost {
        let transaction = database.begin().await?;
        let id = job.id;
        let run_id = job.run_id;
        let mut active = job.into_active_model();
        active.status = Set("failure".to_owned());
        active.result = Set(Some("failure".to_owned()));
        active.failure_kind = Set(Some("runner_lost".to_owned()));
        active.failure_summary = Set(Some(
            "The runner stopped reporting; the job was not retried.".to_owned(),
        ));
        active.completed_at = Set(Some(now));
        active.lease_deadline = Set(None);
        active.update(&transaction).await?;
        tokens::revoke_job(&transaction, id).await?;
        release_dependents(&transaction, id).await?;
        aggregate_run(&transaction, run_id).await?;
        transaction.commit().await?;
    }
    action_runner_fetch::Entity::delete_many()
        .filter(action_runner_fetch::Column::CreatedAt.lt(now - Duration::hours(6)))
        .exec(database)
        .await?;
    Ok(())
}

#[derive(Debug)]
pub(crate) enum StateError {
    Missing,
    ForeignTask,
    Regression,
    InvalidOutput,
    Database(sea_orm::DbErr),
}

impl From<sea_orm::DbErr> for StateError {
    fn from(error: sea_orm::DbErr) -> Self {
        Self::Database(error)
    }
}
