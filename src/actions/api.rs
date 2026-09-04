use std::collections::BTreeMap;

use axum::{
    Json, Router,
    body::Body,
    extract::{Path, Query, State},
    http::HeaderMap,
    response::{IntoResponse, Response},
    routing::{delete, get, post},
};
use axum_extra::extract::cookie::CookieJar;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{Duration, Utc};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, IntoActiveModel, QueryFilter, QueryOrder,
    QuerySelect, Set,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    entity::{action_artifact, action_job, action_job_log, action_run, action_runner, repository},
    identity::{ApiError, AuthenticatedUser, SCOPE_READ, SCOPE_WRITE},
    repository::Permission,
};

use super::{ActionsState, REQUIRED_RUNNER_VERSION, artifacts, runners, runs, tokens};

const MAX_LOG_QUERY_ROWS: u64 = 64;

pub(crate) fn router() -> Router<ActionsState> {
    Router::new()
        .route(
            "/repositories/{namespace}/{name}/actions/runs",
            get(list_runs),
        )
        .route(
            "/repositories/{namespace}/{name}/actions/statuses",
            get(statuses),
        )
        .route(
            "/repositories/{namespace}/{name}/actions/runs/{run_id}",
            get(run_detail),
        )
        .route(
            "/repositories/{namespace}/{name}/actions/runs/{run_id}/cancel",
            post(cancel_run),
        )
        .route(
            "/repositories/{namespace}/{name}/actions/runs/{run_id}/jobs/{job_id}/logs",
            get(job_logs),
        )
        .route(
            "/repositories/{namespace}/{name}/actions/runs/{run_id}/artifacts",
            get(list_run_artifacts),
        )
        .route(
            "/repositories/{namespace}/{name}/actions/runs/{run_id}/artifacts/{artifact_id}",
            get(download_run_artifact),
        )
        .route(
            "/namespaces/{namespace}/actions/runners",
            get(list_namespace_runners),
        )
        .route(
            "/namespaces/{namespace}/actions/runner-registration-tokens",
            post(issue_namespace_registration),
        )
        .route(
            "/namespaces/{namespace}/actions/runners/{runner_id}",
            delete(remove_namespace_runner),
        )
        .route("/admin/actions/runners", get(list_system_runners))
        .route(
            "/admin/actions/runner-registration-tokens",
            post(issue_system_registration),
        )
        .route(
            "/admin/actions/runners/{runner_id}",
            delete(remove_system_runner),
        )
}

#[derive(Deserialize)]
struct RunsQuery {
    page: Option<usize>,
    per_page: Option<usize>,
    commit: Option<String>,
}

#[derive(Serialize)]
struct RunSummary {
    id: Uuid,
    number: i64,
    workflow_name: String,
    workflow_path: String,
    status: String,
    failure_kind: Option<String>,
    failure_summary: Option<String>,
    reference: String,
    before_sha: String,
    after_sha: String,
    created_at: chrono::DateTime<Utc>,
    started_at: Option<chrono::DateTime<Utc>>,
    completed_at: Option<chrono::DateTime<Utc>>,
}

impl From<action_run::Model> for RunSummary {
    fn from(run: action_run::Model) -> Self {
        Self {
            id: run.id,
            number: run.number,
            workflow_name: run.workflow_name,
            workflow_path: run.workflow_path,
            status: run.status,
            failure_kind: run.failure_kind,
            failure_summary: run.failure_summary,
            reference: run.ref_name,
            before_sha: run.before_sha,
            after_sha: run.after_sha,
            created_at: run.created_at,
            started_at: run.started_at,
            completed_at: run.completed_at,
        }
    }
}

#[derive(Serialize)]
struct RunListResponse {
    runs: Vec<RunSummary>,
    page: usize,
    per_page: usize,
    has_more: bool,
}

async fn list_runs(
    State(state): State<ActionsState>,
    Path((namespace, name)): Path<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
    Query(query): Query<RunsQuery>,
) -> Result<Json<RunListResponse>, ApiError> {
    let (repository, _) = readable_repository(&state, &headers, &jar, &namespace, &name).await?;
    let page = query.page.unwrap_or(1).max(1);
    let per_page = query.per_page.unwrap_or(25).clamp(1, 100);
    let mut select =
        action_run::Entity::find().filter(action_run::Column::RepositoryId.eq(repository.id));
    if let Some(commit) = query.commit {
        select = select.filter(action_run::Column::AfterSha.eq(commit));
    }
    let start = (page - 1).saturating_mul(per_page);
    let mut rows = select
        .order_by_desc(action_run::Column::CreatedAt)
        .offset(start as u64)
        .limit((per_page + 1) as u64)
        .all(state.repository().identity().database())
        .await?;
    let has_more = rows.len() > per_page;
    rows.truncate(per_page);
    let runs = rows.into_iter().map(Into::into).collect();
    Ok(Json(RunListResponse {
        runs,
        page,
        per_page,
        has_more,
    }))
}

#[derive(Serialize)]
struct JobResponse {
    id: i64,
    key: String,
    name: String,
    status: String,
    result: Option<String>,
    labels: Vec<String>,
    runner_id: Option<i64>,
    attempt: i64,
    failure_kind: Option<String>,
    failure_summary: Option<String>,
    created_at: chrono::DateTime<Utc>,
    started_at: Option<chrono::DateTime<Utc>>,
    completed_at: Option<chrono::DateTime<Utc>>,
}

#[derive(Serialize)]
struct RunDetailResponse {
    run: RunSummary,
    jobs: Vec<JobResponse>,
    can_cancel: bool,
    diagnostic: Option<String>,
}

async fn run_detail(
    State(state): State<ActionsState>,
    Path((namespace, name, run_id)): Path<(String, String, Uuid)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<RunDetailResponse>, ApiError> {
    let (repository, user_id) =
        readable_repository(&state, &headers, &jar, &namespace, &name).await?;
    let run = owned_run(&state, repository.id, run_id).await?;
    let diagnostic = run.diagnostic.clone();
    let can_cancel = state
        .repository()
        .can_access(&repository, user_id, Permission::Write)
        .await?
        && matches!(run.status.as_str(), "queued" | "running");
    let jobs = action_job::Entity::find()
        .filter(action_job::Column::RunId.eq(run_id))
        .order_by_asc(action_job::Column::Id)
        .all(state.repository().identity().database())
        .await?
        .into_iter()
        .map(job_response)
        .collect();
    Ok(Json(RunDetailResponse {
        run: run.into(),
        jobs,
        can_cancel,
        diagnostic,
    }))
}

fn job_response(job: action_job::Model) -> JobResponse {
    JobResponse {
        id: job.id,
        key: job.job_key,
        name: job.name,
        status: job.status,
        result: job.result,
        labels: serde_json::from_str(&job.required_labels).unwrap_or_default(),
        runner_id: job.runner_id,
        attempt: job.attempt,
        failure_kind: job.failure_kind,
        failure_summary: job.failure_summary,
        created_at: job.created_at,
        started_at: job.started_at,
        completed_at: job.completed_at,
    }
}

async fn cancel_run(
    State(state): State<ActionsState>,
    Path((namespace, name, run_id)): Path<(String, String, Uuid)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<RunSummary>, ApiError> {
    let (actor, repository) = state
        .repository()
        .authenticated_repository(
            &headers,
            &jar,
            &namespace,
            &name,
            Permission::Write,
            SCOPE_WRITE,
        )
        .await?;
    owned_run(&state, repository.id, run_id).await?;
    let run = runs::cancel_run(
        state.repository().identity().database(),
        run_id,
        actor.user.id,
    )
    .await
    .map_err(map_state_error)?;
    state
        .repository()
        .identity()
        .audit(
            Some(actor.user.id),
            "actions.run.cancel",
            Some(run_id.to_string()),
        )
        .await?;
    Ok(Json(run.into()))
}

#[derive(Deserialize)]
struct LogsQuery {
    cursor: Option<String>,
}
#[derive(Serialize)]
struct LogsResponse {
    text: String,
    next_cursor: Option<String>,
    complete: bool,
}

async fn job_logs(
    State(state): State<ActionsState>,
    Path((namespace, name, run_id, job_id)): Path<(String, String, Uuid, i64)>,
    headers: HeaderMap,
    jar: CookieJar,
    Query(query): Query<LogsQuery>,
) -> Result<Json<LogsResponse>, ApiError> {
    let (repository, _) = readable_repository(&state, &headers, &jar, &namespace, &name).await?;
    owned_run(&state, repository.id, run_id).await?;
    let job = action_job::Entity::find_by_id(job_id)
        .filter(action_job::Column::RunId.eq(run_id))
        .one(state.repository().identity().database())
        .await?
        .ok_or_else(ApiError::not_found)?;
    let start = decode_cursor(query.cursor.as_deref()).unwrap_or(0);
    let row_limit = MAX_LOG_QUERY_ROWS;
    let rows = action_job_log::Entity::find()
        .filter(action_job_log::Column::JobId.eq(job_id))
        .filter(action_job_log::Column::RowIndex.gte(start))
        .order_by_asc(action_job_log::Column::RowIndex)
        .limit(row_limit)
        .all(state.repository().identity().database())
        .await?;
    let mut text = String::new();
    let mut next = None;
    for row in rows {
        let separator = usize::from(!text.is_empty());
        if !text.is_empty()
            && text.len() + separator + row.content.len() > state.settings().max_log_response_bytes
        {
            break;
        }
        if separator == 1 {
            text.push('\n');
        }
        text.push_str(&row.content);
        next = Some(row.row_index + 1);
    }
    let terminal = matches!(
        job.status.as_str(),
        "success" | "failure" | "cancelled" | "skipped"
    );
    let complete = terminal && next.unwrap_or(start) >= job.expected_log_index;
    Ok(Json(LogsResponse {
        text,
        next_cursor: next.map(encode_cursor),
        complete,
    }))
}

#[derive(Deserialize)]
struct StatusesQuery {
    #[serde(default)]
    oids: String,
}

fn parse_status_oids(raw: &str) -> Result<Vec<&str>, ApiError> {
    let oids: Vec<_> = raw.split(',').filter(|oid| !oid.is_empty()).collect();
    if oids.len() > 50 {
        return Err(ApiError::bad_request(
            "At most 50 commit OIDs may be requested.",
        ));
    }
    if oids.iter().any(|oid| {
        !matches!(oid.len(), 40 | 64) || !oid.bytes().all(|byte| byte.is_ascii_hexdigit())
    }) {
        return Err(ApiError::bad_request("Commit OIDs must be hexadecimal."));
    }
    Ok(oids)
}
#[derive(Serialize)]
struct CommitStatus {
    oid: String,
    status: String,
    total: usize,
    success: usize,
    failure: usize,
    running: usize,
    queued: usize,
    cancelled: usize,
}
#[derive(Serialize)]
struct StatusesResponse {
    statuses: Vec<CommitStatus>,
}

async fn statuses(
    State(state): State<ActionsState>,
    Path((namespace, name)): Path<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
    Query(query): Query<StatusesQuery>,
) -> Result<Json<StatusesResponse>, ApiError> {
    let (repository, _) = readable_repository(&state, &headers, &jar, &namespace, &name).await?;
    let oids = parse_status_oids(&query.oids)?;
    let mut statuses = Vec::new();
    for oid in oids {
        let runs = action_run::Entity::find()
            .filter(action_run::Column::RepositoryId.eq(repository.id))
            .filter(action_run::Column::AfterSha.eq(oid))
            .all(state.repository().identity().database())
            .await?;
        if runs.is_empty() {
            continue;
        }
        let counts = runs
            .iter()
            .fold(BTreeMap::<&str, usize>::new(), |mut counts, run| {
                *counts.entry(&run.status).or_default() += 1;
                counts
            });
        let status = if counts.get("failure").copied().unwrap_or(0) > 0 {
            "failure"
        } else if counts.get("running").copied().unwrap_or(0) > 0 {
            "running"
        } else if counts.get("queued").copied().unwrap_or(0) > 0 {
            "queued"
        } else if counts.get("cancelled").copied().unwrap_or(0) > 0 {
            "cancelled"
        } else {
            "success"
        };
        statuses.push(CommitStatus {
            oid: oid.to_owned(),
            status: status.to_owned(),
            total: runs.len(),
            success: *counts.get("success").unwrap_or(&0),
            failure: *counts.get("failure").unwrap_or(&0),
            running: *counts.get("running").unwrap_or(&0),
            queued: *counts.get("queued").unwrap_or(&0),
            cancelled: *counts.get("cancelled").unwrap_or(&0),
        });
    }
    Ok(Json(StatusesResponse { statuses }))
}

#[derive(Serialize)]
struct RunnerResponse {
    id: i64,
    name: String,
    labels: Vec<String>,
    version: String,
    status: String,
    last_seen_at: Option<chrono::DateTime<Utc>>,
    incompatibility: Option<String>,
}
#[derive(Serialize)]
struct RunnersResponse {
    runners: Vec<RunnerResponse>,
}

#[derive(Clone)]
enum RunnerScope {
    Namespace(String),
    System,
}

impl RunnerScope {
    fn namespace(&self) -> Option<&str> {
        match self {
            Self::Namespace(namespace) => Some(namespace),
            Self::System => None,
        }
    }

    fn audit_target(&self) -> &str {
        self.namespace().unwrap_or("system")
    }
}

async fn authorize_runner_scope(
    state: &ActionsState,
    scope: &RunnerScope,
    headers: &HeaderMap,
    jar: &CookieJar,
    permission: i32,
) -> Result<AuthenticatedUser, ApiError> {
    let identity = state.repository().identity();
    match scope {
        RunnerScope::Namespace(namespace) => {
            let actor = identity.authenticate(headers, jar, permission).await?;
            crate::identity::authorize_namespace(identity, &actor, namespace).await?;
            Ok(actor)
        }
        RunnerScope::System => {
            crate::identity::require_admin(identity, headers, jar, permission).await
        }
    }
}

async fn list_namespace_runners(
    State(state): State<ActionsState>,
    Path(namespace): Path<String>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<RunnersResponse>, ApiError> {
    list_runners_for_scope(state, RunnerScope::Namespace(namespace), headers, jar).await
}

async fn list_system_runners(
    State(state): State<ActionsState>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<RunnersResponse>, ApiError> {
    list_runners_for_scope(state, RunnerScope::System, headers, jar).await
}

async fn list_runners_for_scope(
    state: ActionsState,
    scope: RunnerScope,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<RunnersResponse>, ApiError> {
    authorize_runner_scope(&state, &scope, &headers, &jar, SCOPE_READ).await?;
    let mut query =
        action_runner::Entity::find().filter(action_runner::Column::DeletedAt.is_null());
    query = match scope.namespace() {
        Some(namespace) => query.filter(action_runner::Column::Namespace.eq(namespace)),
        None => query.filter(action_runner::Column::Namespace.is_null()),
    };
    let runners = query
        .all(state.repository().identity().database())
        .await?
        .into_iter()
        .map(|runner| runner_response(&state, runner))
        .collect();
    Ok(Json(RunnersResponse { runners }))
}

fn runner_response(state: &ActionsState, runner: action_runner::Model) -> RunnerResponse {
    let online = runner.disabled_at.is_none()
        && runner.last_seen_at.is_some_and(|seen| {
            seen > Utc::now() - Duration::seconds(state.settings().runner_loss_seconds)
        });
    let compatible = runner.version == REQUIRED_RUNNER_VERSION;
    RunnerResponse {
        id: runner.id,
        name: runner.name,
        labels: serde_json::from_str(&runner.approved_labels).unwrap_or_default(),
        version: runner.version.clone(),
        status: if online { "online" } else { "offline" }.to_owned(),
        last_seen_at: runner.last_seen_at,
        incompatibility: (!compatible)
            .then(|| format!("Runner version {REQUIRED_RUNNER_VERSION} is required.")),
    }
}

#[derive(Deserialize)]
struct RegistrationRequest {
    name: String,
    labels: Vec<String>,
}
#[derive(Serialize)]
struct RegistrationResponse {
    token: String,
    expires_at: chrono::DateTime<Utc>,
    server_url: String,
    required_version: &'static str,
    runner_name: String,
    labels: Vec<String>,
}

async fn issue_namespace_registration(
    State(state): State<ActionsState>,
    Path(namespace): Path<String>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<RegistrationRequest>,
) -> Result<Json<RegistrationResponse>, ApiError> {
    issue_registration_for_scope(
        state,
        RunnerScope::Namespace(namespace),
        headers,
        jar,
        request,
    )
    .await
}

async fn issue_system_registration(
    State(state): State<ActionsState>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<RegistrationRequest>,
) -> Result<Json<RegistrationResponse>, ApiError> {
    issue_registration_for_scope(state, RunnerScope::System, headers, jar, request).await
}

async fn issue_registration_for_scope(
    state: ActionsState,
    scope: RunnerScope,
    headers: HeaderMap,
    jar: CookieJar,
    request: RegistrationRequest,
) -> Result<Json<RegistrationResponse>, ApiError> {
    let identity = state.repository().identity();
    let actor = authorize_runner_scope(&state, &scope, &headers, &jar, SCOPE_WRITE).await?;
    if actor.via_api_token {
        return Err(ApiError::forbidden(
            "Runner registration requires an interactive session.",
        ));
    }
    let runner_name = runners::validate_registration(&request.name, &request.labels)
        .map_err(ApiError::bad_request)?;
    let audit_target = scope.audit_target().to_owned();
    let raw = tokens::issue_registration(
        identity.database(),
        scope.namespace().map(str::to_owned),
        Some(actor.user.id),
        runner_name.clone(),
        request.labels.clone(),
    )
    .await?;
    identity
        .audit(
            Some(actor.user.id),
            "actions.runner.registration.issue",
            Some(audit_target),
        )
        .await?;
    Ok(Json(RegistrationResponse {
        token: raw,
        expires_at: Utc::now() + Duration::minutes(10),
        server_url: state.repository().public_url().to_string(),
        required_version: REQUIRED_RUNNER_VERSION,
        runner_name,
        labels: request.labels,
    }))
}

async fn remove_namespace_runner(
    State(state): State<ActionsState>,
    Path((namespace, runner_id)): Path<(String, i64)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<RunnerResponse>, ApiError> {
    remove_runner_for_scope(
        state,
        RunnerScope::Namespace(namespace),
        runner_id,
        headers,
        jar,
    )
    .await
}

async fn remove_system_runner(
    State(state): State<ActionsState>,
    Path(runner_id): Path<i64>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<RunnerResponse>, ApiError> {
    remove_runner_for_scope(state, RunnerScope::System, runner_id, headers, jar).await
}

async fn remove_runner_for_scope(
    state: ActionsState,
    scope: RunnerScope,
    runner_id: i64,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<RunnerResponse>, ApiError> {
    let identity = state.repository().identity();
    let actor = authorize_runner_scope(&state, &scope, &headers, &jar, SCOPE_WRITE).await?;
    if actor.via_api_token {
        return Err(ApiError::forbidden(
            "Runner removal requires an interactive session.",
        ));
    }
    let mut query = action_runner::Entity::find_by_id(runner_id)
        .filter(action_runner::Column::DeletedAt.is_null());
    query = match scope.namespace() {
        Some(namespace) => query.filter(action_runner::Column::Namespace.eq(namespace)),
        None => query.filter(action_runner::Column::Namespace.is_null()),
    };
    let runner = query
        .one(identity.database())
        .await?
        .ok_or_else(ApiError::not_found)?;
    let jobs = action_job::Entity::find()
        .filter(action_job::Column::RunnerId.eq(runner_id))
        .filter(action_job::Column::Status.is_in(["leased", "running"]))
        .all(identity.database())
        .await?;
    for job in jobs {
        runs::cancel_run(identity.database(), job.run_id, actor.user.id)
            .await
            .map_err(map_state_error)?;
    }
    let mut active = runner.into_active_model();
    active.deleted_at = Set(Some(Utc::now()));
    let runner = active.update(identity.database()).await?;
    identity
        .audit(
            Some(actor.user.id),
            "actions.runner.remove",
            Some(format!("{}:{runner_id}", scope.audit_target())),
        )
        .await?;
    Ok(Json(runner_response(&state, runner)))
}

async fn readable_repository(
    state: &ActionsState,
    headers: &HeaderMap,
    jar: &CookieJar,
    namespace: &str,
    name: &str,
) -> Result<(repository::Model, Option<Uuid>), ApiError> {
    let repository = state.repository().find(namespace, name).await?;
    let user_id = state
        .repository()
        .identity()
        .optional_user(headers, jar, SCOPE_READ)
        .await?
        .map(|user| user.id);
    state
        .repository()
        .authorize(&repository, user_id, Permission::Read)
        .await?;
    Ok((repository, user_id))
}

async fn owned_run(
    state: &ActionsState,
    repository_id: Uuid,
    run_id: Uuid,
) -> Result<action_run::Model, ApiError> {
    action_run::Entity::find_by_id(run_id)
        .filter(action_run::Column::RepositoryId.eq(repository_id))
        .one(state.repository().identity().database())
        .await?
        .ok_or_else(ApiError::not_found)
}

#[derive(Serialize)]
struct ArtifactListItem {
    download_url: String,
    id: i64,
    name: String,
    size_bytes: i64,
    sha256: Option<String>,
    creating_job_id: i64,
    created_at: chrono::DateTime<Utc>,
    expires_at: chrono::DateTime<Utc>,
}

#[derive(Serialize)]
struct ArtifactListResponse {
    artifacts: Vec<ArtifactListItem>,
}

async fn list_run_artifacts(
    State(state): State<ActionsState>,
    Path((namespace, name, run_id)): Path<(String, String, Uuid)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<ArtifactListResponse>, ApiError> {
    let (repository, _) = readable_repository(&state, &headers, &jar, &namespace, &name).await?;
    let run = owned_run(&state, repository.id, run_id).await?;
    let rows = action_artifact::Entity::find()
        .filter(action_artifact::Column::RepositoryId.eq(repository.id))
        .filter(action_artifact::Column::RunId.eq(run.id))
        .filter(action_artifact::Column::Status.eq("finalized"))
        .filter(action_artifact::Column::DeletedAt.is_null())
        .filter(action_artifact::Column::ExpiresAt.gt(Utc::now()))
        .order_by_asc(action_artifact::Column::Id)
        .all(state.repository().identity().database())
        .await?;
    let artifacts = rows
        .into_iter()
        .map(|artifact| ArtifactListItem {
            download_url: format!(
                "{}/api/v1/repositories/{}/{}/actions/runs/{}/artifacts/{}",
                state
                    .repository()
                    .public_url()
                    .to_string()
                    .trim_end_matches('/'),
                namespace,
                name,
                run.id,
                artifact.id
            ),
            id: artifact.id,
            name: artifact.name,
            size_bytes: artifact.size_bytes,
            sha256: artifact.sha256,
            creating_job_id: artifact.creating_job_id,
            created_at: artifact.created_at,
            expires_at: artifact.expires_at,
        })
        .collect();
    Ok(Json(ArtifactListResponse { artifacts }))
}

async fn download_run_artifact(
    State(state): State<ActionsState>,
    Path((namespace, name, run_id, artifact_id)): Path<(String, String, Uuid, i64)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Response, ApiError> {
    let (repository, _) = readable_repository(&state, &headers, &jar, &namespace, &name).await?;
    let _run = owned_run(&state, repository.id, run_id).await?;
    let artifact = action_artifact::Entity::find_by_id(artifact_id)
        .filter(action_artifact::Column::RepositoryId.eq(repository.id))
        .filter(action_artifact::Column::RunId.eq(run_id))
        .filter(action_artifact::Column::Status.eq("finalized"))
        .filter(action_artifact::Column::DeletedAt.is_null())
        .filter(action_artifact::Column::ExpiresAt.gt(Utc::now()))
        .one(state.repository().identity().database())
        .await?
        .ok_or_else(ApiError::not_found)?;
    let file = tokio::fs::File::open(
        state
            .repository()
            .actions_artifact_root()
            .join(&artifact.storage_key),
    )
    .await
    .map_err(|_| ApiError::not_found())?;
    let mut response = Body::from_stream(tokio_util::io::ReaderStream::new(file)).into_response();
    artifacts::set_download_headers(&mut response, &artifact);
    Ok(response)
}

fn encode_cursor(index: i64) -> String {
    URL_SAFE_NO_PAD.encode(index.to_be_bytes())
}
fn decode_cursor(cursor: Option<&str>) -> Option<i64> {
    let bytes = URL_SAFE_NO_PAD.decode(cursor?).ok()?;
    Some(i64::from_be_bytes(bytes.try_into().ok()?)).filter(|index| *index >= 0)
}

fn map_state_error(error: runs::StateError) -> ApiError {
    match error {
        runs::StateError::Missing => ApiError::not_found(),
        runs::StateError::ForeignTask => {
            ApiError::forbidden("The run is not owned by this repository.")
        }
        runs::StateError::Regression => ApiError::conflict("The run is already terminal."),
        runs::StateError::InvalidOutput => ApiError::bad_request("Invalid Actions output."),
        runs::StateError::Database(error) => error.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commit_status_query_accepts_sha1_and_sha256_lists() {
        let sha1 = "a".repeat(40);
        let sha256 = "b".repeat(64);
        let raw = format!("{sha1},{sha256}");
        let Ok(oids) = parse_status_oids(&raw) else {
            panic!("valid commit OIDs must parse");
        };
        assert_eq!(oids, [sha1.as_str(), sha256.as_str()]);
    }

    #[test]
    fn commit_status_query_rejects_invalid_or_excessive_oids() {
        assert!(parse_status_oids("not-an-oid").is_err());
        let excessive = std::iter::repeat_n("a".repeat(40), 51).collect::<Vec<_>>();
        assert!(parse_status_oids(&excessive.join(",")).is_err());
    }
}
