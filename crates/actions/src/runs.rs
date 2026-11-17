use std::collections::BTreeMap;

use chrono::{Duration, Utc};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, IntoActiveModel, QueryFilter,
    Set, TransactionTrait, sea_query::Expr,
};
use uuid::Uuid;

use gitadel_db::entity::{
    action_job, action_job_need, action_job_token, action_run, action_runner_fetch,
    action_runner_registration_token,
};

use super::{
    matrix::{StoredMatrix, base_job_key},
    tokens,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskResult {
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

pub async fn update_task(
    database: &DatabaseConnection,
    runner_id: i64,
    task_id: i64,
    result: TaskResult,
    outputs: BTreeMap<String, String>,
    steps_json: String,
    lease_seconds: i64,
) -> Result<(action_job::Model, Option<Uuid>), StateError> {
    validate_outputs(&outputs)?;
    let transaction = database.begin().await?;
    let job = action_job::Entity::find_by_id(task_id)
        .one(&transaction)
        .await?
        .filter(|job| job.runner_id == Some(runner_id))
        .ok_or(StateError::ForeignTask)?;
    if job.status == "cancelled" {
        transaction.commit().await?;
        return Ok((job, None));
    }
    if matches!(job.status.as_str(), "success" | "failure" | "skipped") {
        if result.status() == Some(job.status.as_str()) {
            transaction.commit().await?;
            return Ok((job, None));
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
        fail_fast(&transaction, &job).await?;
    }
    let completed = aggregate_run(&transaction, job.run_id).await?;
    transaction.commit().await?;
    Ok((job, completed))
}

pub async fn cancel_run(
    database: &DatabaseConnection,
    run_id: Uuid,
    actor_id: Uuid,
) -> Result<(action_run::Model, bool), StateError> {
    let transaction = database.begin().await?;
    let run = action_run::Entity::find_by_id(run_id)
        .one(&transaction)
        .await?
        .ok_or(StateError::Missing)?;
    if is_terminal_run(&run.status) {
        transaction.commit().await?;
        return Ok((run, false));
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
    Ok((run, true))
}

fn is_terminal_run(status: &str) -> bool {
    matches!(status, "success" | "failure" | "cancelled")
}

/// Cancels the unfinished siblings of a failed matrix job whose strategy
/// uses `fail-fast` (the default).
async fn fail_fast<C: sea_orm::ConnectionTrait>(
    database: &C,
    failed: &action_job::Model,
) -> Result<(), sea_orm::DbErr> {
    if failed.status != "failure" {
        return Ok(());
    }
    let Some(matrix) = StoredMatrix::parse(failed.matrix_json.as_deref()) else {
        return Ok(());
    };
    if !matrix.fail_fast {
        return Ok(());
    }
    let base = base_job_key(&failed.job_key);
    let siblings = action_job::Entity::find()
        .filter(action_job::Column::RunId.eq(failed.run_id))
        .filter(action_job::Column::Status.is_in(["waiting", "queued", "leased", "running"]))
        .all(database)
        .await?;
    let now = Utc::now();
    for sibling in siblings
        .into_iter()
        .filter(|sibling| sibling.id != failed.id && base_job_key(&sibling.job_key) == base)
    {
        let id = sibling.id;
        let mut active = sibling.into_active_model();
        active.status = Set("cancelled".to_owned());
        active.result = Set(Some("cancelled".to_owned()));
        active.failure_kind = Set(Some("fail_fast".to_owned()));
        active.failure_summary = Set(Some(format!(
            "Cancelled because another `{base}` matrix job failed."
        )));
        active.completed_at = Set(Some(now));
        active.lease_deadline = Set(None);
        active.update(database).await?;
        tokens::revoke_job(database, id).await?;
        release_dependents(database, id).await?;
    }
    Ok(())
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

/// Recompute a run's status from its jobs.
///
/// Returns the run ID when this call moved the run into a terminal state, so
/// the caller can announce completion exactly once after committing.
pub async fn aggregate_run<C: sea_orm::ConnectionTrait>(
    database: &C,
    run_id: Uuid,
) -> Result<Option<Uuid>, sea_orm::DbErr> {
    let jobs = action_job::Entity::find()
        .filter(action_job::Column::RunId.eq(run_id))
        .all(database)
        .await?;
    if jobs.is_empty() {
        return Ok(None);
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
    let mut completed = None;
    if let Some(run) = action_run::Entity::find_by_id(run_id).one(database).await? {
        if terminal && !is_terminal_run(&run.status) {
            completed = Some(run.id);
        }
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
    Ok(completed)
}

fn validate_outputs(outputs: &BTreeMap<String, String>) -> Result<(), StateError> {
    for (key, value) in outputs {
        if key.len() > 255 || value.len() > 1_048_576 {
            return Err(StateError::InvalidOutput);
        }
    }
    Ok(())
}

/// Expires credentials and lost leases, reporting completed runs after each commit.
pub async fn maintain(
    database: &DatabaseConnection,
    mut on_completed: impl FnMut(Uuid),
) -> Result<(), sea_orm::DbErr> {
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
        let transition = action_job::Entity::update_many()
            .col_expr(action_job::Column::Status, Expr::value("failure"))
            .col_expr(
                action_job::Column::Result,
                Expr::value(Some("failure".to_owned())),
            )
            .col_expr(
                action_job::Column::FailureKind,
                Expr::value(Some("runner_lost".to_owned())),
            )
            .col_expr(
                action_job::Column::FailureSummary,
                Expr::value(Some(
                    "The runner stopped reporting; the job was not retried.".to_owned(),
                )),
            )
            .col_expr(action_job::Column::CompletedAt, Expr::value(Some(now)))
            .col_expr(
                action_job::Column::LeaseDeadline,
                Expr::value(Option::<chrono::DateTime<Utc>>::None),
            )
            .filter(action_job::Column::Id.eq(id))
            .filter(action_job::Column::Status.is_in(["leased", "running"]))
            .filter(action_job::Column::LeaseGeneration.eq(job.lease_generation))
            .filter(action_job::Column::LeaseDeadline.eq(job.lease_deadline))
            .exec(&transaction)
            .await?;
        if transition.rows_affected == 0 {
            transaction.commit().await?;
            continue;
        }
        tokens::revoke_job(&transaction, id).await?;
        release_dependents(&transaction, id).await?;
        if let Some(failed) = action_job::Entity::find_by_id(id).one(&transaction).await? {
            fail_fast(&transaction, &failed).await?;
        }
        let completed = aggregate_run(&transaction, run_id).await?;
        transaction.commit().await?;
        if let Some(run_id) = completed {
            on_completed(run_id);
        }
    }
    action_runner_fetch::Entity::delete_many()
        .filter(action_runner_fetch::Column::CreatedAt.lt(now - Duration::hours(6)))
        .exec(database)
        .await?;
    Ok(())
}

#[derive(Debug)]
pub enum StateError {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{REQUIRED_RUNNER_VERSION, runners, test_support, workflow};
    use gitadel_db::entity::{action_run, action_runner, user};

    async fn leased_run() -> (
        DatabaseConnection,
        user::Model,
        action_run::Model,
        action_runner::Model,
        runners::ClaimedJob,
    ) {
        let database = test_support::database().await;
        let owner = test_support::owner(&database, "alice").await;
        let repository = test_support::repository(&database, &owner, "project").await;
        let plan = workflow::validate_workflow(
            &gitadel_config::ActionsSettings::default(),
            "ci.yml".to_owned(),
            b"on: push\njobs:\n  build:\n    runs-on: docker\n    steps: []\n".to_vec(),
        )
        .unwrap();
        let trigger = workflow::RunTrigger {
            event: "push",
            reference: "refs/heads/main",
            before: "",
            after: "abc",
            actor: Some(&owner),
            event_json: serde_json::json!({}),
        };
        let run = workflow::enqueue_plan(&database, &repository, &trigger, plan)
            .await
            .unwrap();
        let labels = vec!["docker".to_owned()];
        let registration = tokens::issue_registration(
            &database,
            Some(owner.username.clone()),
            Some(owner.id),
            "worker".to_owned(),
            labels.clone(),
        )
        .await
        .unwrap();
        let runner = runners::register(
            &database,
            "worker",
            &registration,
            &labels,
            REQUIRED_RUNNER_VERSION,
            false,
        )
        .await
        .unwrap()
        .row;
        let claimed = runners::claim(&database, &runner, Uuid::new_v4(), None, 300)
            .await
            .unwrap()
            .unwrap();
        (database, owner, run, runner, claimed)
    }

    #[tokio::test]
    async fn cancelled_run_revokes_credentials_and_late_heartbeat_cannot_revive_it() {
        let (database, owner, run, runner, claimed) = leased_run().await;
        update_task(
            &database,
            runner.id,
            claimed.job.id,
            TaskResult::Heartbeat,
            BTreeMap::new(),
            "[]".to_owned(),
            300,
        )
        .await
        .unwrap();
        assert!(
            tokens::authenticate_job(&database, &claimed.checkout_token)
                .await
                .unwrap()
                .is_some()
        );
        let (cancelled, changed) = cancel_run(&database, run.id, owner.id).await.unwrap();
        assert!(changed);
        assert_eq!(cancelled.status, "cancelled");
        assert!(
            tokens::authenticate_job(&database, &claimed.checkout_token)
                .await
                .unwrap()
                .is_none()
        );
        let (late, completed) = update_task(
            &database,
            runner.id,
            claimed.job.id,
            TaskResult::Heartbeat,
            BTreeMap::new(),
            "[]".to_owned(),
            300,
        )
        .await
        .unwrap();
        assert_eq!(late.status, "cancelled");
        assert_eq!(completed, None);
        assert!(!cancel_run(&database, run.id, owner.id).await.unwrap().1);
    }

    #[tokio::test]
    async fn expired_lease_fails_run_and_reports_completion_once_after_commit() {
        let (database, _, run, _, claimed) = leased_run().await;
        let mut expired = claimed.job.into_active_model();
        expired.lease_deadline = Set(Some(Utc::now() - Duration::seconds(1)));
        let expired = expired.update(&database).await.unwrap();
        let mut completions = Vec::new();
        maintain(&database, |id| completions.push(id))
            .await
            .unwrap();
        maintain(&database, |id| completions.push(id))
            .await
            .unwrap();
        assert_eq!(completions, [run.id]);
        let failed = action_job::Entity::find_by_id(expired.id)
            .one(&database)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(failed.failure_kind.as_deref(), Some("runner_lost"));
        assert_eq!(failed.lease_deadline, None);
        assert_eq!(
            action_run::Entity::find_by_id(run.id)
                .one(&database)
                .await
                .unwrap()
                .unwrap()
                .status,
            "failure"
        );
        assert!(
            tokens::authenticate_job(&database, &claimed.checkout_token)
                .await
                .unwrap()
                .is_none()
        );
    }
}
