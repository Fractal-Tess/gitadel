use std::collections::BTreeMap;

use chrono::{Duration, Utc};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, IntoActiveModel, QueryFilter,
    Set, TransactionTrait, sea_query::Expr,
};
use uuid::Uuid;

use crate::entity::{
    action_job, action_job_need, action_job_token, action_run, action_runner_fetch,
    action_runner_registration_token,
};

use super::{
    ActionsState,
    matrix::{StoredMatrix, base_job_key},
    tokens,
};

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

pub(crate) async fn cancel_run(
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
pub(crate) async fn aggregate_run<C: sea_orm::ConnectionTrait>(
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
            super::events::spawn_run_completed(state.repository(), run_id);
        }
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

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use chrono::{Duration, Utc};
    use russh::keys::{Algorithm, PrivateKey};
    use sea_orm::{
        ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, IntoActiveModel,
        PaginatorTrait as _, QueryFilter, Set,
    };
    use uuid::Uuid;

    use super::{StateError, TaskResult, aggregate_run, cancel_run, maintain, update_task};
    use crate::{
        actions::{
            ActionsState, REQUIRED_RUNNER_VERSION, rerun, runners, secrets::SecretCipher,
            test_support, tokens,
        },
        config::{ActionsSettings, AuthSettings, StorageSettings},
        entity::{
            action_job, action_job_need, action_job_token, action_run, action_runner,
            action_runner_registration_token, repository, user,
        },
        identity::IdentityState,
        repository::RepositoryState,
    };

    const LEASE_SECONDS: i64 = 300;

    struct Fixture {
        database: DatabaseConnection,
        owner: user::Model,
        repository: repository::Model,
        runner: action_runner::Model,
    }

    async fn fixture() -> Fixture {
        let database = test_support::database().await;
        let owner = test_support::owner(&database, "alice").await;
        let repository = test_support::repository(&database, &owner, "project").await;
        let runner = runner(&database, "runner-one").await;
        Fixture {
            database,
            owner,
            repository,
            runner,
        }
    }

    async fn runner(database: &DatabaseConnection, name: &str) -> action_runner::Model {
        let now = Utc::now();
        action_runner::ActiveModel {
            id: Default::default(),
            uuid: Set(Uuid::new_v4().to_string()),
            namespace: Set(None),
            name: Set(name.to_owned()),
            token_hash: Set(tokens::digest(name)),
            approved_labels: Set("[\"docker\"]".to_owned()),
            version: Set(REQUIRED_RUNNER_VERSION.to_owned()),
            ephemeral: Set(false),
            disabled_at: Set(None),
            deleted_at: Set(None),
            last_seen_at: Set(Some(now)),
            created_by: Set(None),
            created_at: Set(now),
        }
        .insert(database)
        .await
        .unwrap()
    }

    impl Fixture {
        async fn run(&self, status: &str) -> action_run::Model {
            let now = Utc::now();
            let number = action_run::Entity::find()
                .filter(action_run::Column::RepositoryId.eq(self.repository.id))
                .count(&self.database)
                .await
                .unwrap() as i64
                + 1;
            action_run::ActiveModel {
                id: Set(Uuid::new_v4()),
                repository_id: Set(self.repository.id),
                number: Set(number),
                workflow_path: Set(".forgejo/workflows/ci.yml".to_owned()),
                workflow_name: Set("CI".to_owned()),
                event: Set("push".to_owned()),
                ref_name: Set("refs/heads/main".to_owned()),
                before_sha: Set("0".repeat(40)),
                after_sha: Set("1".repeat(40)),
                actor_id: Set(Some(self.owner.id)),
                status: Set(status.to_owned()),
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
            }
            .insert(&self.database)
            .await
            .unwrap()
        }

        /// A job in `status`. Leased and running jobs belong to the fixture
        /// runner with a live lease; `matrix` is the stored strategy JSON.
        async fn job(
            &self,
            run_id: Uuid,
            key: &str,
            status: &str,
            matrix: Option<&str>,
        ) -> action_job::Model {
            let now = Utc::now();
            let active = matches!(status, "leased" | "running");
            let terminal = matches!(status, "success" | "failure" | "cancelled" | "skipped");
            action_job::ActiveModel {
                id: Default::default(),
                run_id: Set(run_id),
                job_key: Set(key.to_owned()),
                name: Set(key.to_owned()),
                required_labels: Set("[\"docker\"]".to_owned()),
                workflow_payload: Set(Vec::new()),
                status: Set(status.to_owned()),
                result: Set(terminal.then(|| status.to_owned())),
                runner_id: Set(active.then_some(self.runner.id)),
                request_key: Set(None),
                lease_generation: Set(i64::from(active)),
                lease_deadline: Set(active.then(|| now + Duration::seconds(LEASE_SECONDS))),
                last_report_at: Set(None),
                attempt: Set(i64::from(active)),
                step_state: Set("[]".to_owned()),
                outputs: Set("{}".to_owned()),
                expected_log_index: Set(0),
                log_bytes: Set(0),
                log_truncated: Set(false),
                failure_kind: Set(None),
                failure_summary: Set(None),
                matrix_json: Set(matrix.map(str::to_owned)),
                copied_from_job_id: Set(None),
                created_at: Set(now),
                started_at: Set(active.then_some(now)),
                completed_at: Set(terminal.then_some(now)),
            }
            .insert(&self.database)
            .await
            .unwrap()
        }

        async fn needs(&self, job: &action_job::Model, needed: &action_job::Model) {
            action_job_need::ActiveModel {
                job_id: Set(job.id),
                needed_job_id: Set(needed.id),
                needed_job_key: Set(needed.job_key.clone()),
            }
            .insert(&self.database)
            .await
            .unwrap();
        }

        /// The checkout token a runner would hold for `job`.
        async fn token_for(&self, job: &action_job::Model) -> String {
            tokens::issue_job(
                &self.database,
                job.id,
                self.repository.id,
                job.lease_generation,
            )
            .await
            .unwrap()
        }

        async fn report(
            &self,
            job: &action_job::Model,
            result: TaskResult,
        ) -> Result<(action_job::Model, Option<Uuid>), StateError> {
            update_task(
                &self.database,
                self.runner.id,
                job.id,
                result,
                BTreeMap::new(),
                "[]".to_owned(),
                LEASE_SECONDS,
            )
            .await
        }

        async fn job_now(&self, id: i64) -> action_job::Model {
            action_job::Entity::find_by_id(id)
                .one(&self.database)
                .await
                .unwrap()
                .unwrap()
        }

        async fn run_now(&self, id: Uuid) -> action_run::Model {
            action_run::Entity::find_by_id(id)
                .one(&self.database)
                .await
                .unwrap()
                .unwrap()
        }

        async fn live_tokens(&self, job_id: i64) -> usize {
            action_job_token::Entity::find()
                .filter(action_job_token::Column::JobId.eq(job_id))
                .filter(action_job_token::Column::RevokedAt.is_null())
                .count(&self.database)
                .await
                .unwrap() as usize
        }

        async fn authenticates(&self, raw: &str) -> bool {
            tokens::authenticate_job(&self.database, raw)
                .await
                .unwrap()
                .is_some()
        }
    }

    /// Everything `maintain` needs: an [`ActionsState`] over the fixture's
    /// database, with its storage under a temporary root.
    struct Environment {
        fixture: Fixture,
        state: ActionsState,
        root: std::path::PathBuf,
    }

    impl Drop for Environment {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    async fn environment() -> Environment {
        let fixture = fixture().await;
        let root = std::env::temp_dir().join(format!("gitadel-runs-{}", Uuid::new_v4()));
        let public_url = url::Url::parse("https://gitadel.test").unwrap();
        let identity = IdentityState::new(
            fixture.database.clone(),
            AuthSettings {
                session_lifetime_hours: 24,
                invitation_lifetime_hours: 24,
            },
            public_url.clone(),
        )
        .unwrap();
        let repository = RepositoryState::new(
            identity,
            StorageSettings {
                repository_root: root.join("repositories"),
                lfs_root: root.join("lfs"),
                registry_root: root.join("registry"),
                actions_artifact_root: root.join("actions-artifacts"),
            },
            public_url,
            22,
        )
        .await
        .unwrap();
        let key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap();
        let state = ActionsState::new(
            repository,
            ActionsSettings::default(),
            SecretCipher::from_host_key(&key).unwrap(),
        );
        Environment {
            fixture,
            state,
            root,
        }
    }

    const FAIL_FAST: &str = r#"{"values":{},"fail_fast":true,"max_parallel":null}"#;
    const KEEP_GOING: &str = r#"{"values":{},"fail_fast":false,"max_parallel":null}"#;

    #[tokio::test]
    async fn heartbeats_keep_a_leased_job_running_and_extend_its_lease() {
        let fixture = fixture().await;
        let run = fixture.run("queued").await;
        let job = fixture.job(run.id, "build", "leased", None).await;
        let before = Utc::now();

        let (job, completed) = fixture.report(&job, TaskResult::Heartbeat).await.unwrap();

        assert_eq!(completed, None);
        assert_eq!(job.status, "running");
        assert_eq!(job.result, None);
        let started_at = job.started_at.expect("started on the first report");
        assert!(job.last_report_at.unwrap() >= before);
        assert!(job.lease_deadline.unwrap() >= before + Duration::seconds(LEASE_SECONDS - 5));
        let run = fixture.run_now(run.id).await;
        assert_eq!(run.status, "running");
        assert!(run.started_at.is_some());

        // Later heartbeats keep the original start time.
        let (job, _) = fixture.report(&job, TaskResult::Heartbeat).await.unwrap();
        assert_eq!(job.started_at, Some(started_at));
        assert_eq!(job.status, "running");
    }

    #[tokio::test]
    async fn reports_are_accepted_only_from_the_assigned_runner() {
        let fixture = fixture().await;
        let run = fixture.run("queued").await;
        let job = fixture.job(run.id, "build", "leased", None).await;
        let other = runner(&fixture.database, "runner-two").await;

        let error = update_task(
            &fixture.database,
            other.id,
            job.id,
            TaskResult::Success,
            BTreeMap::new(),
            "[]".to_owned(),
            LEASE_SECONDS,
        )
        .await
        .unwrap_err();

        assert!(matches!(error, StateError::ForeignTask), "{error:?}");
        assert_eq!(fixture.job_now(job.id).await.status, "leased");
        assert_eq!(fixture.run_now(run.id).await.status, "queued");
    }

    #[tokio::test]
    async fn finished_jobs_do_not_regress_and_cancelled_jobs_ignore_late_reports() {
        let fixture = fixture().await;
        let run = fixture.run("running").await;
        let done = fixture.job(run.id, "done", "success", None).await;
        let mut active = done.clone().into_active_model();
        active.runner_id = Set(Some(fixture.runner.id));
        let done = active.update(&fixture.database).await.unwrap();

        let error = fixture
            .report(&done, TaskResult::Failure)
            .await
            .unwrap_err();
        assert!(matches!(error, StateError::Regression), "{error:?}");
        let error = fixture
            .report(&done, TaskResult::Heartbeat)
            .await
            .unwrap_err();
        assert!(matches!(error, StateError::Regression), "{error:?}");
        // Repeating the same outcome is a harmless retry.
        let (job, completed) = fixture.report(&done, TaskResult::Success).await.unwrap();
        assert_eq!((job.status.as_str(), completed), ("success", None));

        let cancelled = fixture.job(run.id, "late", "cancelled", None).await;
        let mut active = cancelled.into_active_model();
        active.runner_id = Set(Some(fixture.runner.id));
        let cancelled = active.update(&fixture.database).await.unwrap();
        let (job, completed) = fixture
            .report(&cancelled, TaskResult::Success)
            .await
            .unwrap();
        assert_eq!((job.status.as_str(), completed), ("cancelled", None));

        // Outputs are validated before anything is read from the database.
        let running = fixture.job(run.id, "noisy", "running", None).await;
        let mut outputs = BTreeMap::new();
        outputs.insert("k".repeat(256), "v".to_owned());
        let error = update_task(
            &fixture.database,
            fixture.runner.id,
            running.id,
            TaskResult::Success,
            outputs,
            "[]".to_owned(),
            LEASE_SECONDS,
        )
        .await
        .unwrap_err();
        assert!(matches!(error, StateError::InvalidOutput), "{error:?}");
        assert_eq!(fixture.job_now(running.id).await.status, "running");
    }

    #[tokio::test]
    async fn completing_a_job_revokes_its_token_releases_dependents_and_finishes_the_run_once() {
        let fixture = fixture().await;
        let run = fixture.run("running").await;
        let build = fixture.job(run.id, "build", "running", None).await;
        let token = fixture.token_for(&build).await;
        let deploy = fixture.job(run.id, "deploy", "waiting", None).await;
        fixture.needs(&deploy, &build).await;
        assert!(fixture.authenticates(&token).await);

        let (build, completed) = fixture.report(&build, TaskResult::Success).await.unwrap();

        assert_eq!(completed, None, "the run still has a job to do");
        assert_eq!(build.status, "success");
        assert_eq!(build.result.as_deref(), Some("success"));
        assert!(build.completed_at.is_some());
        assert_eq!(build.lease_deadline, None);
        assert_eq!(fixture.live_tokens(build.id).await, 0);
        assert!(!fixture.authenticates(&token).await);
        assert_eq!(fixture.job_now(deploy.id).await.status, "queued");
        assert_eq!(fixture.run_now(run.id).await.status, "running");

        // The dependent runs and finishes; that finishes the run, once.
        let mut active = fixture.job_now(deploy.id).await.into_active_model();
        active.status = Set("running".to_owned());
        active.runner_id = Set(Some(fixture.runner.id));
        active.lease_deadline = Set(Some(Utc::now() + Duration::seconds(LEASE_SECONDS)));
        let deploy = active.update(&fixture.database).await.unwrap();
        let (deploy, completed) = fixture.report(&deploy, TaskResult::Success).await.unwrap();
        assert_eq!(completed, Some(run.id));
        let run = fixture.run_now(run.id).await;
        assert_eq!(run.status, "success");
        assert!(run.completed_at.is_some());
        let (_, completed) = fixture.report(&deploy, TaskResult::Success).await.unwrap();
        assert_eq!(completed, None, "completion is announced exactly once");
    }

    #[tokio::test]
    async fn a_failed_matrix_job_cancels_only_its_fail_fast_siblings() {
        let fixture = fixture().await;
        let run = fixture.run("running").await;
        let failing = fixture
            .job(run.id, "test:1", "running", Some(FAIL_FAST))
            .await;
        let queued = fixture
            .job(run.id, "test:2", "queued", Some(FAIL_FAST))
            .await;
        let waiting = fixture
            .job(run.id, "test:3", "waiting", Some(FAIL_FAST))
            .await;
        fixture.needs(&waiting, &queued).await;
        let finished = fixture
            .job(run.id, "test:4", "success", Some(FAIL_FAST))
            .await;
        let tolerant = fixture
            .job(run.id, "lint:1", "queued", Some(KEEP_GOING))
            .await;
        let docs = fixture.job(run.id, "docs", "queued", None).await;
        let release = fixture.job(run.id, "release", "waiting", None).await;
        fixture.needs(&release, &waiting).await;
        let sibling_token = {
            let mut active = queued.clone().into_active_model();
            active.lease_generation = Set(1);
            let queued = active.update(&fixture.database).await.unwrap();
            fixture.token_for(&queued).await
        };

        let (_, completed) = fixture.report(&failing, TaskResult::Failure).await.unwrap();

        assert_eq!(completed, None);
        for sibling in [queued.id, waiting.id] {
            let job = fixture.job_now(sibling).await;
            assert_eq!(job.status, "cancelled", "{}", job.job_key);
            assert_eq!(job.result.as_deref(), Some("cancelled"));
            assert_eq!(job.failure_kind.as_deref(), Some("fail_fast"));
            assert_eq!(
                job.failure_summary.as_deref(),
                Some("Cancelled because another `test` matrix job failed.")
            );
            assert!(job.completed_at.is_some());
        }
        assert_eq!(fixture.live_tokens(queued.id).await, 0);
        assert!(!fixture.authenticates(&sibling_token).await);
        assert_eq!(fixture.job_now(finished.id).await.status, "success");
        assert_eq!(fixture.job_now(tolerant.id).await.status, "queued");
        assert_eq!(fixture.job_now(docs.id).await.status, "queued");
        // A job waiting on a cancelled sibling is released rather than stuck.
        assert_eq!(fixture.job_now(release.id).await.status, "queued");
        assert_eq!(fixture.run_now(run.id).await.status, "running");
    }

    #[tokio::test]
    async fn a_failed_job_without_fail_fast_leaves_its_siblings_alone() {
        let fixture = fixture().await;
        let run = fixture.run("running").await;
        let failing = fixture
            .job(run.id, "test:1", "running", Some(KEEP_GOING))
            .await;
        let sibling = fixture
            .job(run.id, "test:2", "queued", Some(KEEP_GOING))
            .await;
        let plain = fixture.job(run.id, "build", "running", None).await;
        let plain_sibling = fixture.job(run.id, "build:2", "queued", None).await;

        fixture.report(&failing, TaskResult::Failure).await.unwrap();
        fixture.report(&plain, TaskResult::Failure).await.unwrap();

        assert_eq!(fixture.job_now(sibling.id).await.status, "queued");
        assert_eq!(fixture.job_now(plain_sibling.id).await.status, "queued");
    }

    #[tokio::test]
    async fn cancelling_a_run_stops_unfinished_jobs_and_keeps_finished_results() {
        let fixture = fixture().await;
        let run = fixture.run("running").await;
        let queued = fixture.job(run.id, "queued", "queued", None).await;
        let waiting = fixture.job(run.id, "waiting", "waiting", None).await;
        let running = fixture.job(run.id, "running", "running", None).await;
        let token = fixture.token_for(&running).await;
        let succeeded = fixture.job(run.id, "succeeded", "success", None).await;
        let failed = fixture.job(run.id, "failed", "failure", None).await;
        let actor = fixture.owner.id;

        let (cancelled, changed) = cancel_run(&fixture.database, run.id, actor).await.unwrap();

        assert!(changed);
        assert_eq!(cancelled.status, "cancelled");
        assert_eq!(cancelled.cancelled_by, Some(actor));
        assert!(cancelled.cancel_requested_at.is_some());
        assert!(cancelled.completed_at.is_some());
        for id in [queued.id, waiting.id, running.id] {
            let job = fixture.job_now(id).await;
            assert_eq!(job.status, "cancelled", "{}", job.job_key);
            assert_eq!(job.result.as_deref(), Some("cancelled"));
            assert_eq!(job.lease_deadline, None);
            assert!(job.completed_at.is_some());
        }
        assert_eq!(fixture.job_now(succeeded.id).await.status, "success");
        assert_eq!(fixture.job_now(failed.id).await.status, "failure");
        assert!(!fixture.authenticates(&token).await);

        // Cancelling again, or cancelling a finished run, changes nothing.
        let (again, changed) = cancel_run(&fixture.database, run.id, actor).await.unwrap();
        assert!(!changed);
        assert_eq!(again.cancelled_by, Some(actor));
        let finished = fixture.run("success").await;
        let job = fixture.job(finished.id, "build", "queued", None).await;
        let (_, changed) = cancel_run(&fixture.database, finished.id, actor)
            .await
            .unwrap();
        assert!(!changed);
        assert_eq!(fixture.job_now(job.id).await.status, "queued");
        assert!(matches!(
            cancel_run(&fixture.database, Uuid::new_v4(), actor).await,
            Err(StateError::Missing)
        ));
    }

    #[tokio::test]
    async fn aggregate_run_reflects_its_jobs() {
        let fixture = fixture().await;
        let empty = fixture.run("queued").await;
        assert_eq!(
            aggregate_run(&fixture.database, empty.id).await.unwrap(),
            None
        );
        assert_eq!(fixture.run_now(empty.id).await.status, "queued");

        let cases: [(&[&str], &str); 6] = [
            (&["skipped", "skipped"], "success"),
            (&["success", "failure", "cancelled"], "failure"),
            (&["success", "cancelled"], "cancelled"),
            (&["queued", "waiting"], "queued"),
            (&["queued", "leased"], "running"),
            (&["success", "queued"], "running"),
        ];
        for (statuses, expected) in cases {
            let run = fixture.run("queued").await;
            for (index, status) in statuses.iter().enumerate() {
                fixture
                    .job(run.id, &format!("job{index}"), status, None)
                    .await;
            }
            let completed = aggregate_run(&fixture.database, run.id).await.unwrap();
            let run = fixture.run_now(run.id).await;
            assert_eq!(run.status, expected, "{statuses:?}");
            let terminal = matches!(expected, "success" | "failure" | "cancelled");
            assert_eq!(completed.is_some(), terminal, "{statuses:?}");
            assert_eq!(run.completed_at.is_some(), terminal, "{statuses:?}");
            assert_eq!(
                run.started_at.is_some(),
                expected == "running",
                "{statuses:?}"
            );
            // A second pass over a finished run reports nothing new.
            assert_eq!(
                aggregate_run(&fixture.database, run.id).await.unwrap(),
                None
            );
        }
    }

    #[tokio::test]
    async fn maintain_fails_lost_jobs_revokes_their_tokens_and_frees_dependents() {
        let environment = environment().await;
        let fixture = &environment.fixture;
        let run = fixture.run("running").await;
        let lost = fixture.job(run.id, "build", "running", None).await;
        let lost_token = fixture.token_for(&lost).await;
        assert!(fixture.authenticates(&lost_token).await);
        let mut active = lost.clone().into_active_model();
        active.lease_deadline = Set(Some(Utc::now() - Duration::seconds(1)));
        let lost = active.update(&fixture.database).await.unwrap();
        // An expired lease already stops the token, before any cleanup runs.
        assert!(!fixture.authenticates(&lost_token).await);
        assert_eq!(fixture.live_tokens(lost.id).await, 1);
        let healthy = fixture.job(run.id, "test", "running", None).await;
        let healthy_token = fixture.token_for(&healthy).await;
        let dependent = fixture.job(run.id, "deploy", "waiting", None).await;
        fixture.needs(&dependent, &lost).await;
        let expired = action_runner_registration_token::ActiveModel {
            id: Set(Uuid::new_v4()),
            token_hash: Set(tokens::digest("expired")),
            namespace: Set(None),
            runner_name: Set("old".to_owned()),
            approved_labels: Set("[]".to_owned()),
            expires_at: Set(Utc::now() - Duration::minutes(1)),
            used_at: Set(None),
            created_by: Set(None),
            created_at: Set(Utc::now() - Duration::hours(1)),
        }
        .insert(&fixture.database)
        .await
        .unwrap();

        maintain(&environment.state).await.unwrap();

        let lost = fixture.job_now(lost.id).await;
        assert_eq!(lost.status, "failure");
        assert_eq!(lost.result.as_deref(), Some("failure"));
        assert_eq!(lost.failure_kind.as_deref(), Some("runner_lost"));
        assert_eq!(
            lost.failure_summary.as_deref(),
            Some("The runner stopped reporting; the job was not retried.")
        );
        assert_eq!(lost.lease_deadline, None);
        assert!(lost.completed_at.is_some());
        assert_eq!(fixture.live_tokens(lost.id).await, 0);
        assert!(!fixture.authenticates(&lost_token).await);
        assert_eq!(fixture.job_now(healthy.id).await.status, "running");
        assert!(fixture.authenticates(&healthy_token).await);
        assert_eq!(fixture.job_now(dependent.id).await.status, "queued");
        assert_eq!(fixture.run_now(run.id).await.status, "running");
        assert!(
            action_runner_registration_token::Entity::find_by_id(expired.id)
                .one(&fixture.database)
                .await
                .unwrap()
                .is_none()
        );

        // A late report from the lost runner cannot resurrect the job.
        let error = fixture
            .report(&lost, TaskResult::Success)
            .await
            .unwrap_err();
        assert!(matches!(error, StateError::Regression), "{error:?}");
        // And a second pass changes nothing.
        maintain(&environment.state).await.unwrap();
        assert_eq!(fixture.job_now(healthy.id).await.status, "running");
    }

    #[tokio::test]
    async fn losing_the_last_running_job_completes_the_run() {
        let environment = environment().await;
        let fixture = &environment.fixture;
        let run = fixture.run("running").await;
        let only = fixture.job(run.id, "build", "leased", None).await;
        let mut active = only.into_active_model();
        active.lease_deadline = Set(Some(Utc::now() - Duration::seconds(1)));
        active.update(&fixture.database).await.unwrap();

        maintain(&environment.state).await.unwrap();

        let run = fixture.run_now(run.id).await;
        assert_eq!(run.status, "failure");
        assert!(run.completed_at.is_some());
    }

    #[tokio::test]
    async fn a_new_lease_generation_invalidates_the_previous_job_token() {
        let fixture = fixture().await;
        let run = fixture.run("queued").await;
        let job = fixture.job(run.id, "build", "queued", None).await;

        let first = runners::claim(
            &fixture.database,
            &fixture.runner,
            Uuid::new_v4(),
            None,
            LEASE_SECONDS,
        )
        .await
        .unwrap()
        .expect("the queued job is claimed");
        assert_eq!(first.job.id, job.id);
        assert_eq!(first.job.lease_generation, 1);
        assert!(fixture.authenticates(&first.checkout_token).await);

        // The job goes back to the queue (as a retry would) and is claimed
        // again: the old token must die with its lease.
        let mut active = first.job.into_active_model();
        active.status = Set("queued".to_owned());
        active.runner_id = Set(None);
        active.lease_deadline = Set(None);
        active.update(&fixture.database).await.unwrap();
        let second = runners::claim(
            &fixture.database,
            &fixture.runner,
            Uuid::new_v4(),
            None,
            LEASE_SECONDS,
        )
        .await
        .unwrap()
        .expect("the requeued job is claimed again");
        assert_eq!(second.job.id, job.id);
        assert_eq!(second.job.lease_generation, 2);
        assert_eq!(fixture.live_tokens(job.id).await, 1);
        assert!(!fixture.authenticates(&first.checkout_token).await);
        assert!(fixture.authenticates(&second.checkout_token).await);

        // Finishing the job ends the current token as well.
        fixture
            .report(&second.job, TaskResult::Success)
            .await
            .unwrap();
        assert_eq!(fixture.live_tokens(job.id).await, 0);
        assert!(!fixture.authenticates(&second.checkout_token).await);
    }

    #[tokio::test]
    async fn rerunning_failed_jobs_yields_a_run_that_completes_with_the_fresh_job() {
        let fixture = fixture().await;
        let original = fixture.run("failure").await;
        fixture.job(original.id, "lint", "success", None).await;
        fixture.job(original.id, "test", "failure", None).await;

        let rerun = rerun::rerun(
            &fixture.database,
            fixture.repository.id,
            original.id,
            fixture.owner.id,
            true,
        )
        .await
        .unwrap();
        let jobs = action_job::Entity::find()
            .filter(action_job::Column::RunId.eq(rerun.id))
            .all(&fixture.database)
            .await
            .unwrap();
        let fresh = jobs.iter().find(|job| job.job_key == "test").unwrap();
        assert_eq!(fresh.status, "queued");
        assert_eq!(
            jobs.iter()
                .find(|job| job.job_key == "lint")
                .unwrap()
                .status,
            "success"
        );

        let claimed = runners::claim(
            &fixture.database,
            &fixture.runner,
            Uuid::new_v4(),
            None,
            LEASE_SECONDS,
        )
        .await
        .unwrap()
        .expect("the rerun job is claimable");
        assert_eq!(claimed.job.id, fresh.id);
        let (_, completed) = fixture
            .report(&claimed.job, TaskResult::Success)
            .await
            .unwrap();
        assert_eq!(completed, Some(rerun.id));
        assert_eq!(fixture.run_now(rerun.id).await.status, "success");
        assert_eq!(fixture.run_now(original.id).await.status, "failure");

        // Nothing failed any more, so another failed-only rerun is refused.
        assert!(matches!(
            rerun::rerun(
                &fixture.database,
                fixture.repository.id,
                rerun.id,
                fixture.owner.id,
                true,
            )
            .await,
            Err(rerun::RerunError::NothingFailed)
        ));
    }
}
