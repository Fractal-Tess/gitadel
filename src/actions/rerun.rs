//! Reruns.
//!
//! A rerun is a new run (with the next run number) that repeats a finished
//! run's workflow, commit, event, and payload. It records the run it repeats
//! in `rerun_of` and increments `run_attempt`, which jobs see as
//! `github.run_attempt`. Keeping history in separate runs preserves every
//! attempt's logs and artifacts.
//!
//! "Re-run failed jobs" copies successful jobs whose dependencies also
//! succeeded, including their outputs, and links them to the original job
//! through `copied_from_job_id` so their logs and artifacts stay reachable.
//! Every other job, and everything that depends on it, runs again.

use std::collections::{BTreeMap, BTreeSet};

use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, Set,
    TransactionTrait,
};
use uuid::Uuid;

use crate::entity::{action_job, action_job_need, action_run};

use super::runs;

#[derive(Debug)]
pub(crate) enum RerunError {
    Missing,
    NotFinished,
    NoJobs,
    NothingFailed,
    Database(sea_orm::DbErr),
}

impl From<sea_orm::DbErr> for RerunError {
    fn from(error: sea_orm::DbErr) -> Self {
        Self::Database(error)
    }
}

/// Jobs that must run again: every unsuccessful job and its dependents.
fn jobs_to_rerun(
    jobs: &[action_job::Model],
    edges: &[action_job_need::Model],
    failed_only: bool,
) -> BTreeSet<i64> {
    if !failed_only {
        return jobs.iter().map(|job| job.id).collect();
    }
    let mut rerun: BTreeSet<i64> = jobs
        .iter()
        .filter(|job| job.status != "success")
        .map(|job| job.id)
        .collect();
    loop {
        let before = rerun.len();
        for edge in edges {
            if rerun.contains(&edge.needed_job_id) {
                rerun.insert(edge.job_id);
            }
        }
        if rerun.len() == before {
            return rerun;
        }
    }
}

pub(crate) async fn rerun(
    database: &DatabaseConnection,
    repository_id: Uuid,
    run_id: Uuid,
    actor_id: Uuid,
    failed_only: bool,
) -> Result<action_run::Model, RerunError> {
    let transaction = database.begin().await?;
    let original = action_run::Entity::find_by_id(run_id)
        .filter(action_run::Column::RepositoryId.eq(repository_id))
        .one(&transaction)
        .await?
        .ok_or(RerunError::Missing)?;
    if !matches!(
        original.status.as_str(),
        "success" | "failure" | "cancelled"
    ) {
        return Err(RerunError::NotFinished);
    }
    let jobs = action_job::Entity::find()
        .filter(action_job::Column::RunId.eq(run_id))
        .order_by_asc(action_job::Column::Id)
        .all(&transaction)
        .await?;
    if jobs.is_empty() {
        return Err(RerunError::NoJobs);
    }
    let job_ids: Vec<i64> = jobs.iter().map(|job| job.id).collect();
    let edges = action_job_need::Entity::find()
        .filter(action_job_need::Column::JobId.is_in(job_ids))
        .all(&transaction)
        .await?;
    let rerun = jobs_to_rerun(&jobs, &edges, failed_only);
    if failed_only && rerun.is_empty() {
        return Err(RerunError::NothingFailed);
    }

    let number = action_run::Entity::find()
        .filter(action_run::Column::RepositoryId.eq(repository_id))
        .order_by_desc(action_run::Column::Number)
        .one(&transaction)
        .await?
        .map_or(1, |run| run.number + 1);
    let now = Utc::now();
    let new_run_id = Uuid::new_v4();
    let run = action_run::ActiveModel {
        id: Set(new_run_id),
        repository_id: Set(repository_id),
        number: Set(number),
        workflow_path: Set(original.workflow_path.clone()),
        workflow_name: Set(original.workflow_name.clone()),
        event: Set(original.event.clone()),
        ref_name: Set(original.ref_name.clone()),
        before_sha: Set(original.before_sha.clone()),
        after_sha: Set(original.after_sha.clone()),
        actor_id: Set(Some(actor_id)),
        status: Set("queued".to_owned()),
        failure_kind: Set(None),
        failure_summary: Set(None),
        diagnostic: Set(None),
        event_json: Set(original.event_json.clone()),
        cancel_requested_at: Set(None),
        cancelled_by: Set(None),
        rerun_of: Set(Some(original.id)),
        run_attempt: Set(original.run_attempt + 1),
        created_at: Set(now),
        started_at: Set(None),
        completed_at: Set(None),
    }
    .insert(&transaction)
    .await?;

    let mut ids = BTreeMap::new();
    for job in &jobs {
        let fresh = rerun.contains(&job.id);
        let needs_pending = edges
            .iter()
            .any(|edge| edge.job_id == job.id && rerun.contains(&edge.needed_job_id));
        let mut copy = action_job::ActiveModel {
            id: Default::default(),
            run_id: Set(new_run_id),
            job_key: Set(job.job_key.clone()),
            name: Set(job.name.clone()),
            required_labels: Set(job.required_labels.clone()),
            workflow_payload: Set(job.workflow_payload.clone()),
            status: Set(if needs_pending { "waiting" } else { "queued" }.to_owned()),
            result: Set(None),
            runner_id: Set(None),
            request_key: Set(None),
            lease_generation: Set(0),
            lease_deadline: Set(None),
            last_report_at: Set(None),
            attempt: Set(0),
            step_state: Set("[]".to_owned()),
            outputs: Set("{}".to_owned()),
            expected_log_index: Set(0),
            log_bytes: Set(0),
            log_truncated: Set(false),
            failure_kind: Set(None),
            failure_summary: Set(None),
            matrix_json: Set(job.matrix_json.clone()),
            copied_from_job_id: Set(None),
            created_at: Set(now),
            started_at: Set(None),
            completed_at: Set(None),
        };
        if !fresh {
            copy.status = Set(job.status.clone());
            copy.result = Set(job.result.clone());
            copy.attempt = Set(job.attempt);
            copy.step_state = Set(job.step_state.clone());
            copy.outputs = Set(job.outputs.clone());
            copy.expected_log_index = Set(job.expected_log_index);
            copy.log_bytes = Set(job.log_bytes);
            copy.log_truncated = Set(job.log_truncated);
            // Chains of reruns keep pointing at the job that actually ran.
            copy.copied_from_job_id = Set(Some(job.copied_from_job_id.unwrap_or(job.id)));
            copy.started_at = Set(job.started_at);
            copy.completed_at = Set(job.completed_at);
        }
        let stored = copy.insert(&transaction).await?;
        ids.insert(job.id, stored.id);
    }
    for edge in &edges {
        action_job_need::ActiveModel {
            job_id: Set(ids[&edge.job_id]),
            needed_job_id: Set(ids[&edge.needed_job_id]),
            needed_job_key: Set(edge.needed_job_key.clone()),
        }
        .insert(&transaction)
        .await?;
    }
    runs::aggregate_run(&transaction, new_run_id).await?;
    transaction.commit().await?;
    Ok(action_run::Entity::find_by_id(new_run_id)
        .one(database)
        .await?
        .unwrap_or(run))
}

#[cfg(test)]
mod tests {
    use sea_orm::ConnectionTrait as _;

    use super::*;
    use crate::actions::test_support;

    async fn finished_run(database: &DatabaseConnection) -> (Uuid, Uuid, Vec<i64>) {
        let owner = test_support::owner(database, "alice").await;
        let repository = test_support::repository(database, &owner, "project").await;
        let run_id = Uuid::new_v4();
        let now = Utc::now();
        action_run::ActiveModel {
            id: Set(run_id),
            repository_id: Set(repository.id),
            number: Set(1),
            workflow_path: Set(".forgejo/workflows/ci.yml".to_owned()),
            workflow_name: Set("CI".to_owned()),
            event: Set("push".to_owned()),
            ref_name: Set("refs/heads/main".to_owned()),
            before_sha: Set("0".repeat(40)),
            after_sha: Set("1".repeat(40)),
            actor_id: Set(Some(owner.id)),
            status: Set("failure".to_owned()),
            failure_kind: Set(None),
            failure_summary: Set(None),
            diagnostic: Set(None),
            event_json: Set("{\"ref\":\"refs/heads/main\"}".to_owned()),
            cancel_requested_at: Set(None),
            cancelled_by: Set(None),
            rerun_of: Set(None),
            run_attempt: Set(1),
            created_at: Set(now),
            started_at: Set(Some(now)),
            completed_at: Set(Some(now)),
        }
        .insert(database)
        .await
        .unwrap();
        // lint -> test -> deploy, plus an independent docs job.
        let mut ids = Vec::new();
        for (key, status) in [
            ("lint", "success"),
            ("test", "failure"),
            ("deploy", "skipped"),
            ("docs", "success"),
        ] {
            let job = action_job::ActiveModel {
                id: Default::default(),
                run_id: Set(run_id),
                job_key: Set(key.to_owned()),
                name: Set(key.to_owned()),
                required_labels: Set("[\"docker\"]".to_owned()),
                workflow_payload: Set(b"jobs: {}".to_vec()),
                status: Set(status.to_owned()),
                result: Set(Some(status.to_owned())),
                runner_id: Set(None),
                request_key: Set(None),
                lease_generation: Set(1),
                lease_deadline: Set(None),
                last_report_at: Set(None),
                attempt: Set(1),
                step_state: Set("[]".to_owned()),
                outputs: Set(format!("{{\"from\":\"{key}\"}}")),
                expected_log_index: Set(3),
                log_bytes: Set(30),
                log_truncated: Set(false),
                failure_kind: Set(None),
                failure_summary: Set(None),
                matrix_json: Set(None),
                copied_from_job_id: Set(None),
                created_at: Set(now),
                started_at: Set(Some(now)),
                completed_at: Set(Some(now)),
            }
            .insert(database)
            .await
            .unwrap();
            ids.push(job.id);
        }
        for (job, needed, key) in [(1, 0, "lint"), (2, 1, "test")] {
            action_job_need::ActiveModel {
                job_id: Set(ids[job]),
                needed_job_id: Set(ids[needed]),
                needed_job_key: Set(key.to_owned()),
            }
            .insert(database)
            .await
            .unwrap();
        }
        (repository.id, owner.id, ids)
    }

    async fn jobs_of(database: &DatabaseConnection, run_id: Uuid) -> Vec<action_job::Model> {
        action_job::Entity::find()
            .filter(action_job::Column::RunId.eq(run_id))
            .order_by_asc(action_job::Column::Id)
            .all(database)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn rerunning_failed_jobs_keeps_successes_and_requeues_dependents() {
        let database = test_support::database().await;
        let (repository_id, actor, original) = finished_run(&database).await;
        let original_run = action_job::Entity::find_by_id(original[0])
            .one(&database)
            .await
            .unwrap()
            .unwrap()
            .run_id;

        let run = rerun(&database, repository_id, original_run, actor, true)
            .await
            .unwrap();

        assert_eq!(run.number, 2);
        assert_eq!(run.run_attempt, 2);
        assert_eq!(run.rerun_of, Some(original_run));
        assert_eq!(run.status, "running");
        let jobs = jobs_of(&database, run.id).await;
        let states: Vec<_> = jobs
            .iter()
            .map(|job| {
                (
                    job.job_key.as_str(),
                    job.status.as_str(),
                    job.copied_from_job_id,
                )
            })
            .collect();
        assert_eq!(
            states,
            [
                ("lint", "success", Some(original[0])),
                ("test", "queued", None),
                ("deploy", "waiting", None),
                ("docs", "success", Some(original[3])),
            ]
        );
        assert_eq!(jobs[0].outputs, "{\"from\":\"lint\"}");
        let edges = action_job_need::Entity::find()
            .filter(action_job_need::Column::JobId.eq(jobs[2].id))
            .all(&database)
            .await
            .unwrap();
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].needed_job_id, jobs[1].id);

        // A rerun of the rerun still points kept jobs at the job that ran.
        database
            .execute_unprepared(&format!(
                "UPDATE action_jobs SET status = 'success', result = 'success' WHERE run_id = (SELECT id FROM action_runs WHERE number = 2) AND job_key = 'test'; \
                 UPDATE action_jobs SET status = 'failure', result = 'failure' WHERE run_id = (SELECT id FROM action_runs WHERE number = 2) AND job_key = 'deploy'; \
                 UPDATE action_runs SET status = 'failure' WHERE number = {}",
                run.number
            ))
            .await
            .unwrap();
        let again = rerun(&database, repository_id, run.id, actor, true)
            .await
            .unwrap();
        assert_eq!(again.run_attempt, 3);
        let jobs = jobs_of(&database, again.id).await;
        assert_eq!(jobs[0].copied_from_job_id, Some(original[0]));
        assert_eq!(jobs[2].status, "queued");
    }

    #[tokio::test]
    async fn full_reruns_requeue_everything_and_unfinished_runs_are_rejected() {
        let database = test_support::database().await;
        let (repository_id, actor, original) = finished_run(&database).await;
        let run_id = action_job::Entity::find_by_id(original[0])
            .one(&database)
            .await
            .unwrap()
            .unwrap()
            .run_id;

        let run = rerun(&database, repository_id, run_id, actor, false)
            .await
            .unwrap();
        let statuses: Vec<_> = jobs_of(&database, run.id)
            .await
            .into_iter()
            .map(|job| job.status)
            .collect();
        assert_eq!(statuses, ["queued", "waiting", "waiting", "queued"]);
        assert!(matches!(
            rerun(&database, repository_id, run.id, actor, false).await,
            Err(RerunError::NotFinished)
        ));
        assert!(matches!(
            rerun(&database, Uuid::new_v4(), run_id, actor, false).await,
            Err(RerunError::Missing)
        ));
    }
}
