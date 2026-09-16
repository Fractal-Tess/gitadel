use chrono::{Duration, Utc};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, IntoActiveModel, QueryFilter,
    QueryOrder, Set, TransactionTrait,
};
use uuid::Uuid;

use crate::entity::{action_job, action_run, action_runner, action_runner_fetch, repository};

use super::{REQUIRED_RUNNER_VERSION, tokens};

#[derive(Debug)]
pub(crate) enum RunnerError {
    Unauthorized,
    InvalidRegistration(String),
    Version(String),
    Conflict(String),
    Database(sea_orm::DbErr),
}

impl From<sea_orm::DbErr> for RunnerError {
    fn from(error: sea_orm::DbErr) -> Self {
        Self::Database(error)
    }
}

#[derive(Debug)]
pub(crate) struct RegisteredRunner {
    pub(crate) row: action_runner::Model,
    pub(crate) token: String,
}

fn canonical_runner_version(version: &str) -> Option<&'static str> {
    (version == REQUIRED_RUNNER_VERSION
        || version.strip_prefix('v') == Some(REQUIRED_RUNNER_VERSION))
    .then_some(REQUIRED_RUNNER_VERSION)
}

pub(crate) fn validate_registration(name: &str, labels: &[String]) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty()
        || name.len() > 100
        || labels.is_empty()
        || labels.len() > 16
        || labels.iter().any(|label| {
            label.is_empty()
                || label.len() > 100
                || !label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
        })
    {
        return Err("Provide a runner name and 1 to 16 plain label names.".to_owned());
    }
    Ok(name.to_owned())
}

pub(crate) async fn register(
    database: &DatabaseConnection,
    name: &str,
    raw_registration: &str,
    labels: &[String],
    version: &str,
    ephemeral: bool,
) -> Result<RegisteredRunner, RunnerError> {
    if ephemeral {
        return Err(RunnerError::InvalidRegistration(
            "ephemeral runners are not supported".to_owned(),
        ));
    }
    let version = canonical_runner_version(version).ok_or_else(|| {
        RunnerError::Version(format!(
            "runner version {REQUIRED_RUNNER_VERSION} is required"
        ))
    })?;
    let transaction = database.begin().await?;
    let grant = tokens::consume_registration(&transaction, raw_registration)
        .await?
        .ok_or(RunnerError::Unauthorized)?;
    let approved: Vec<String> = serde_json::from_str(&grant.approved_labels).map_err(|_| {
        RunnerError::InvalidRegistration("stored runner labels are invalid".to_owned())
    })?;
    if name != grant.runner_name || labels != approved {
        return Err(RunnerError::InvalidRegistration(
            "runner name and labels must exactly match the issued grant".to_owned(),
        ));
    }
    let mut duplicate = action_runner::Entity::find()
        .filter(action_runner::Column::Name.eq(name))
        .filter(action_runner::Column::DeletedAt.is_null())
        .filter(action_runner::Column::DisabledAt.is_null());
    duplicate = match grant.namespace.as_deref() {
        Some(namespace) => duplicate.filter(action_runner::Column::Namespace.eq(namespace)),
        None => duplicate.filter(action_runner::Column::Namespace.is_null()),
    };
    if duplicate.one(&transaction).await?.is_some() {
        let scope = grant.namespace.as_deref().unwrap_or("the system");
        return Err(RunnerError::Conflict(format!(
            "{scope} already has an active runner with that name"
        )));
    }
    let raw_token = tokens::runner_token();
    let row = action_runner::ActiveModel {
        id: Default::default(),
        uuid: Set(Uuid::new_v4().to_string()),
        namespace: Set(grant.namespace),
        name: Set(name.to_owned()),
        token_hash: Set(tokens::digest(&raw_token)),
        approved_labels: Set(grant.approved_labels),
        version: Set(version.to_owned()),
        ephemeral: Set(false),
        disabled_at: Set(None),
        deleted_at: Set(None),
        last_seen_at: Set(Some(Utc::now())),
        created_by: Set(grant.created_by),
        created_at: Set(Utc::now()),
    }
    .insert(&transaction)
    .await?;
    transaction.commit().await?;
    Ok(RegisteredRunner {
        row,
        token: raw_token,
    })
}

pub(crate) async fn authenticate(
    database: &DatabaseConnection,
    uuid: &str,
    token: &str,
) -> Result<action_runner::Model, RunnerError> {
    let row = action_runner::Entity::find()
        .filter(action_runner::Column::Uuid.eq(uuid))
        .filter(action_runner::Column::DeletedAt.is_null())
        .filter(action_runner::Column::DisabledAt.is_null())
        .one(database)
        .await?
        .filter(|row| tokens::digest_matches(token, &row.token_hash))
        .ok_or(RunnerError::Unauthorized)?;
    let mut active = row.clone().into_active_model();
    active.last_seen_at = Set(Some(Utc::now()));
    active.update(database).await?;
    Ok(row)
}

pub(crate) async fn declare(
    database: &DatabaseConnection,
    runner: action_runner::Model,
    version: &str,
    labels: &[String],
) -> Result<action_runner::Model, RunnerError> {
    let approved: Vec<String> = serde_json::from_str(&runner.approved_labels).map_err(|_| {
        RunnerError::InvalidRegistration("stored runner labels are invalid".to_owned())
    })?;
    let version = canonical_runner_version(version);
    if version.is_none() || labels != approved {
        return Err(RunnerError::Version(format!(
            "runner must declare version {REQUIRED_RUNNER_VERSION} and its approved labels"
        )));
    }
    let mut active = runner.into_active_model();
    active.version = Set(version.expect("validated runner version").to_owned());
    Ok(active.update(database).await?)
}

#[derive(Debug)]
pub(crate) struct ClaimedJob {
    pub(crate) job: action_job::Model,
    pub(crate) checkout_token: String,
}

pub(crate) async fn claim(
    database: &DatabaseConnection,
    runner: &action_runner::Model,
    request_key: Uuid,
    handle: Option<i64>,
    lease_seconds: i64,
) -> Result<Option<ClaimedJob>, RunnerError> {
    let transaction = database.begin().await?;
    if let Some(fetch) =
        action_runner_fetch::Entity::find_by_id((runner.id, request_key.to_string()))
            .one(&transaction)
            .await?
    {
        let job = action_job::Entity::find_by_id(fetch.job_id)
            .one(&transaction)
            .await?;
        let Some(job) = job.filter(|job| {
            matches!(job.status.as_str(), "leased" | "running")
                && handle.is_none_or(|handle| job.id == handle)
        }) else {
            transaction.commit().await?;
            return Ok(None);
        };
        let run = action_run::Entity::find_by_id(job.run_id)
            .one(&transaction)
            .await?
            .ok_or_else(|| RunnerError::Conflict("assigned run is missing".to_owned()))?;
        let checkout_token = tokens::issue_job(
            &transaction,
            job.id,
            run.repository_id,
            job.lease_generation,
        )
        .await?;
        transaction.commit().await?;
        return Ok(Some(ClaimedJob {
            job,
            checkout_token,
        }));
    }

    let approved: Vec<String> = serde_json::from_str(&runner.approved_labels).map_err(|_| {
        RunnerError::InvalidRegistration("stored runner labels are invalid".to_owned())
    })?;
    let repository_ids = if let Some(namespace) = runner.namespace.as_deref() {
        repository::Entity::find()
            .filter(repository::Column::Namespace.eq(namespace))
            .filter(repository::Column::DeletedAt.is_null())
            .all(&transaction)
            .await?
            .into_iter()
            .map(|repository| repository.id)
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    if runner.namespace.is_some() && repository_ids.is_empty() {
        transaction.commit().await?;
        return Ok(None);
    }
    let run_ids = if runner.namespace.is_some() {
        action_run::Entity::find()
            .filter(action_run::Column::RepositoryId.is_in(repository_ids))
            .all(&transaction)
            .await?
            .into_iter()
            .map(|run| run.id)
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    if runner.namespace.is_some() && run_ids.is_empty() {
        transaction.commit().await?;
        return Ok(None);
    }
    let mut select = action_job::Entity::find().filter(action_job::Column::Status.eq("queued"));
    if runner.namespace.is_some() {
        select = select.filter(action_job::Column::RunId.is_in(run_ids));
    }
    if let Some(handle) = handle {
        select = select.filter(action_job::Column::Id.eq(handle));
    }
    let candidates = select
        .order_by_asc(action_job::Column::CreatedAt)
        .all(&transaction)
        .await?;
    let mut selected = None;
    for job in candidates {
        let labels: Vec<String> = serde_json::from_str(&job.required_labels).map_err(|_| {
            RunnerError::InvalidRegistration("stored job labels are invalid".to_owned())
        })?;
        if labels.iter().all(|label| approved.contains(label)) {
            selected = Some(job);
            break;
        }
    }
    let Some(job) = selected else {
        transaction.commit().await?;
        return Ok(None);
    };
    let generation = job.lease_generation + 1;
    let now = Utc::now();
    let deadline = now + Duration::seconds(lease_seconds);
    let mut active = job.into_active_model();
    active.status = Set("leased".to_owned());
    active.runner_id = Set(Some(runner.id));
    active.request_key = Set(Some(request_key.to_string()));
    active.lease_generation = Set(generation);
    active.lease_deadline = Set(Some(deadline));
    active.last_report_at = Set(Some(now));
    active.attempt = Set(active.attempt.as_ref() + 1);
    let job = active.update(&transaction).await?;
    let run = action_run::Entity::find_by_id(job.run_id)
        .one(&transaction)
        .await?
        .ok_or_else(|| RunnerError::Conflict("assigned run is missing".to_owned()))?;
    let checkout_token =
        tokens::issue_job(&transaction, job.id, run.repository_id, generation).await?;
    action_runner_fetch::ActiveModel {
        runner_id: Set(runner.id),
        request_key: Set(request_key.to_string()),
        job_id: Set(job.id),
        lease_generation: Set(generation),
        token_generation: Set(Uuid::new_v4().to_string()),
        created_at: Set(now),
    }
    .insert(&transaction)
    .await?;
    transaction.commit().await?;
    Ok(Some(ClaimedJob {
        job,
        checkout_token,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::action_job_token;
    use sea_orm::{ActiveModelTrait, ConnectOptions, Database};
    use sea_orm_migration::MigratorTrait;

    use crate::{
        entity::{namespace, user},
        migration::Migrator,
    };

    #[test]
    fn canonicalizes_the_official_runner_version() {
        assert_eq!(
            canonical_runner_version("v13.0.0"),
            Some(REQUIRED_RUNNER_VERSION)
        );
        assert_eq!(
            canonical_runner_version("13.0.0"),
            Some(REQUIRED_RUNNER_VERSION)
        );
        assert_eq!(canonical_runner_version("v13.0.1"), None);
        assert_eq!(canonical_runner_version("vv13.0.0"), None);
    }

    async fn database() -> DatabaseConnection {
        let mut options = ConnectOptions::new("sqlite::memory:");
        options.max_connections(1).sqlx_logging(false);
        let database = Database::connect(options).await.unwrap();
        Migrator::up(&database, None).await.unwrap();
        database
    }

    async fn owner(database: &DatabaseConnection, username: &str) -> Uuid {
        let now = Utc::now();
        let id = Uuid::new_v4();
        user::ActiveModel {
            id: Set(id),
            username: Set(username.to_owned()),
            password_hash: Set("unused".to_owned()),
            is_admin: Set(false),
            default_repository_visibility: Set("private".to_owned()),
            theme_preference: Set("system".to_owned()),
            disabled_at: Set(None),
            avatar_updated_at: Set(None),
            created_at: Set(now),
            updated_at: Set(now),
        }
        .insert(database)
        .await
        .unwrap();
        namespace::ActiveModel {
            slug: Set(username.to_owned()),
            kind: Set("user".to_owned()),
            user_id: Set(Some(id)),
            organization_id: Set(None),
            created_at: Set(now),
        }
        .insert(database)
        .await
        .unwrap();
        id
    }

    async fn queued_job(
        database: &DatabaseConnection,
        namespace: &str,
        owner_id: Uuid,
        created_at: chrono::DateTime<Utc>,
    ) -> (Uuid, action_job::Model) {
        let repository_id = Uuid::new_v4();
        repository::ActiveModel {
            id: Set(repository_id),
            namespace: Set(namespace.to_owned()),
            name: Set("project".to_owned()),
            description: Set(None),
            website_url: Set(None),
            visibility: Set("private".to_owned()),
            object_format: Set("sha1".to_owned()),
            mirrored: Set(false),
            default_branch: Set("main".to_owned()),
            issue_counter: Set(0),
            storage_key: Set(Uuid::new_v4()),
            created_by: Set(owner_id),
            archived_at: Set(None),
            deleted_at: Set(None),
            icon_updated_at: Set(None),
            icon_source: Set(None),
            created_at: Set(created_at),
            updated_at: Set(created_at),
        }
        .insert(database)
        .await
        .unwrap();
        let run_id = Uuid::new_v4();
        action_run::ActiveModel {
            id: Set(run_id),
            repository_id: Set(repository_id),
            number: Set(1),
            workflow_path: Set(".forgejo/workflows/ci.yml".to_owned()),
            workflow_name: Set("CI".to_owned()),
            event: Set("push".to_owned()),
            ref_name: Set("refs/heads/main".to_owned()),
            before_sha: Set("0".repeat(40)),
            after_sha: Set("1".repeat(40)),
            actor_id: Set(Some(owner_id)),
            status: Set("queued".to_owned()),
            failure_kind: Set(None),
            failure_summary: Set(None),
            diagnostic: Set(None),
            event_json: Set("{}".to_owned()),
            cancel_requested_at: Set(None),
            cancelled_by: Set(None),
            created_at: Set(created_at),
            started_at: Set(None),
            completed_at: Set(None),
        }
        .insert(database)
        .await
        .unwrap();
        let job = action_job::ActiveModel {
            id: Default::default(),
            run_id: Set(run_id),
            job_key: Set("build".to_owned()),
            name: Set("Build".to_owned()),
            required_labels: Set("[\"docker\"]".to_owned()),
            workflow_payload: Set(Vec::new()),
            status: Set("queued".to_owned()),
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
            created_at: Set(created_at),
            started_at: Set(None),
            completed_at: Set(None),
        }
        .insert(database)
        .await
        .unwrap();
        (run_id, job)
    }

    #[tokio::test]
    async fn namespace_runner_only_claims_jobs_from_its_owner_pool() {
        let database = database().await;
        let alice = owner(&database, "alice").await;
        let bob = owner(&database, "bob").await;
        let now = Utc::now();
        let (bob_run, bob_job) =
            queued_job(&database, "bob", bob, now - Duration::seconds(1)).await;
        let (alice_run, _) = queued_job(&database, "alice", alice, now).await;
        let runner = action_runner::ActiveModel {
            id: Default::default(),
            uuid: Set(Uuid::new_v4().to_string()),
            namespace: Set(Some("alice".to_owned())),
            name: Set("alice-runner".to_owned()),
            token_hash: Set(tokens::digest("runner-token")),
            approved_labels: Set("[\"docker\"]".to_owned()),
            version: Set(REQUIRED_RUNNER_VERSION.to_owned()),
            ephemeral: Set(false),
            disabled_at: Set(None),
            deleted_at: Set(None),
            last_seen_at: Set(Some(now)),
            created_by: Set(Some(alice)),
            created_at: Set(now),
        }
        .insert(&database)
        .await
        .unwrap();

        let claimed = claim(&database, &runner, Uuid::new_v4(), None, 300)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(claimed.job.run_id, alice_run);
        assert_ne!(claimed.job.run_id, bob_run);
        assert_eq!(
            action_job::Entity::find_by_id(bob_job.id)
                .one(&database)
                .await
                .unwrap()
                .unwrap()
                .status,
            "queued"
        );
    }

    #[tokio::test]
    async fn system_runner_claims_the_oldest_job_across_namespaces() {
        let database = database().await;
        let alice = owner(&database, "alice").await;
        let bob = owner(&database, "bob").await;
        let now = Utc::now();
        let (bob_run, _) = queued_job(&database, "bob", bob, now - Duration::seconds(1)).await;
        let (_alice_run, _) = queued_job(&database, "alice", alice, now).await;
        let runner = action_runner::ActiveModel {
            id: Default::default(),
            uuid: Set(Uuid::new_v4().to_string()),
            namespace: Set(None),
            name: Set("system-runner".to_owned()),
            token_hash: Set(tokens::digest("system-runner-token")),
            approved_labels: Set("[\"docker\"]".to_owned()),
            version: Set(REQUIRED_RUNNER_VERSION.to_owned()),
            ephemeral: Set(false),
            disabled_at: Set(None),
            deleted_at: Set(None),
            last_seen_at: Set(Some(now)),
            created_by: Set(Some(alice)),
            created_at: Set(now),
        }
        .insert(&database)
        .await
        .unwrap();

        let claimed = claim(&database, &runner, Uuid::new_v4(), None, 300)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(claimed.job.run_id, bob_run);
        let token = action_job_token::Entity::find()
            .filter(action_job_token::Column::JobId.eq(claimed.job.id))
            .one(&database)
            .await
            .unwrap()
            .unwrap();
        assert!(token.expires_at > now + Duration::hours(5));
    }
}
