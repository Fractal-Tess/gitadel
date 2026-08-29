use std::collections::{BTreeMap, HashMap};

use axum::{
    Router,
    body::Bytes,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, HeaderValue, StatusCode, header::CONTENT_TYPE},
    response::{IntoResponse, Response},
    routing::post,
};
use chrono::{TimeZone, Utc};
use prost::Message;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::json;
use uuid::Uuid;

use crate::entity::{action_job, action_job_need, action_run, repository, user};

use super::{
    ActionsState,
    logs::{self, IncomingLogRow},
    proto::{ping, runner},
    runners::{self, RunnerError},
    runs::{self, StateError, TaskResult},
};

const PROTO_CONTENT_TYPE: &str = "application/proto";
const JSON_CONTENT_TYPE: &str = "application/json";

pub(crate) fn router(max_request_bytes: usize) -> Router<ActionsState> {
    Router::new()
        .route("/ping.v1.PingService/Ping", post(ping_rpc))
        .route("/runner.v1.RunnerService/Register", post(register))
        .route("/runner.v1.RunnerService/Declare", post(declare))
        .route("/runner.v1.RunnerService/FetchTask", post(fetch_task))
        .route(
            "/runner.v1.RunnerService/FetchSingleTask",
            post(fetch_single_task),
        )
        .route("/runner.v1.RunnerService/UpdateTask", post(update_task))
        .route("/runner.v1.RunnerService/UpdateLog", post(update_log))
        .layer(DefaultBodyLimit::max(max_request_bytes))
}

#[derive(Clone, Copy)]
enum Encoding {
    Protobuf,
    Json,
}

async fn ping_rpc(headers: HeaderMap, body: Bytes) -> Response {
    respond::<ping::PingRequest, ping::PingResponse, _, _>(headers, body, |request| async move {
        Ok(ping::PingResponse { data: request.data })
    })
    .await
}

async fn register(State(state): State<ActionsState>, headers: HeaderMap, body: Bytes) -> Response {
    respond::<runner::RegisterRequest, runner::RegisterResponse, _, _>(
        headers,
        body,
        |request| async move {
            let registered = runners::register(
                state.repository().identity().database(),
                &request.name,
                &request.token,
                &request.labels,
                &request.version,
                request.ephemeral,
            )
            .await
            .map_err(map_runner_error)?;
            Ok(runner::RegisterResponse {
                runner: Some(runner_message(&registered.row, registered.token)),
            })
        },
    )
    .await
}

async fn declare(State(state): State<ActionsState>, headers: HeaderMap, body: Bytes) -> Response {
    let authenticated = match authenticate(&state, &headers).await {
        Ok(runner) => runner,
        Err(error) => return error.into_response(),
    };
    respond::<runner::DeclareRequest, runner::DeclareResponse, _, _>(
        headers,
        body,
        |request| async move {
            let row = runners::declare(
                state.repository().identity().database(),
                authenticated,
                &request.version,
                &request.labels,
            )
            .await
            .map_err(map_runner_error)?;
            Ok(runner::DeclareResponse {
                runner: Some(runner_message(&row, String::new())),
            })
        },
    )
    .await
}

async fn fetch_task(
    State(state): State<ActionsState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let authenticated = match authenticate(&state, &headers).await {
        Ok(runner) => runner,
        Err(error) => return error.into_response(),
    };
    let request_key = match request_key(&headers) {
        Ok(key) => key,
        Err(error) => return error.into_response(),
    };
    respond::<runner::FetchTaskRequest, runner::FetchTaskResponse, _, _>(
        headers,
        body,
        |request| async move {
            let _capacity = request.task_capacity.unwrap_or(1).clamp(1, 1);
            let claimed = claim_until(&state, &authenticated, request_key, None)
                .await
                .map_err(map_runner_error)?;
            let task = match claimed {
                Some(claimed) => Some(task_message(&state, claimed).await?),
                None => None,
            };
            let tasks_version = task.as_ref().map_or(request.tasks_version, |task| task.id);
            Ok(runner::FetchTaskResponse {
                task,
                tasks_version,
                additional_tasks: Vec::new(),
            })
        },
    )
    .await
}

async fn fetch_single_task(
    State(state): State<ActionsState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let authenticated = match authenticate(&state, &headers).await {
        Ok(runner) => runner,
        Err(error) => return error.into_response(),
    };
    let request_key = match request_key(&headers) {
        Ok(key) => key,
        Err(error) => return error.into_response(),
    };
    respond::<runner::FetchSingleTaskRequest, runner::FetchSingleTaskResponse, _, _>(
        headers,
        body,
        |request| async move {
            let handle = request
                .handle
                .as_deref()
                .map(str::parse)
                .transpose()
                .map_err(|_| ConnectError::invalid("task handle must be a numeric task ID"))?;
            let claimed = claim_until(&state, &authenticated, request_key, handle)
                .await
                .map_err(map_runner_error)?;
            let task = match claimed {
                Some(claimed) => Some(task_message(&state, claimed).await?),
                None => None,
            };
            let tasks_version = task.as_ref().map_or(request.tasks_version, |task| task.id);
            Ok(runner::FetchSingleTaskResponse {
                tasks_version,
                task,
            })
        },
    )
    .await
}

async fn claim_until(
    state: &ActionsState,
    runner: &crate::entity::action_runner::Model,
    request_key: Uuid,
    handle: Option<i64>,
) -> Result<Option<runners::ClaimedJob>, RunnerError> {
    let deadline = tokio::time::Instant::now()
        + std::time::Duration::from_secs(state.settings().fetch_timeout_seconds);
    loop {
        let claimed = runners::claim(
            state.repository().identity().database(),
            runner,
            request_key,
            handle,
            state.settings().runner_loss_seconds,
        )
        .await?;
        if claimed.is_some() || tokio::time::Instant::now() >= deadline {
            return Ok(claimed);
        }
        tokio::select! {
            () = state.shutdown.cancelled() => return Ok(None),
            () = tokio::time::sleep(std::time::Duration::from_millis(250)) => {}
        }
    }
}

async fn update_task(
    State(state): State<ActionsState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let authenticated = match authenticate(&state, &headers).await {
        Ok(runner) => runner,
        Err(error) => return error.into_response(),
    };
    respond::<runner::UpdateTaskRequest, runner::UpdateTaskResponse, _, _>(
        headers,
        body,
        |request| async move {
            let task_state = request
                .state
                .ok_or_else(|| ConnectError::invalid("task state is required"))?;
            let result = match runner::Result::try_from(task_state.result)
                .unwrap_or(runner::Result::Unspecified)
            {
                runner::Result::Unspecified => TaskResult::Heartbeat,
                runner::Result::Success => TaskResult::Success,
                runner::Result::Failure => TaskResult::Failure,
                runner::Result::Cancelled => TaskResult::Cancelled,
                runner::Result::Skipped => TaskResult::Skipped,
            };
            let outputs = request.outputs.into_iter().collect::<BTreeMap<_, _>>();
            let sent_outputs = outputs.keys().cloned().collect();
            let steps_json = serde_json::to_string(&task_state.steps)
                .map_err(|error| ConnectError::internal(error.to_string()))?;
            let job = runs::update_task(
                state.repository().identity().database(),
                authenticated.id,
                task_state.id,
                result,
                outputs,
                steps_json,
                state.settings().runner_loss_seconds,
            )
            .await
            .map_err(map_state_error)?;
            let mut response_state = task_state;
            if job.status == "cancelled" {
                response_state.result = runner::Result::Cancelled as i32;
            }
            Ok(runner::UpdateTaskResponse {
                state: Some(response_state),
                sent_outputs,
            })
        },
    )
    .await
}

async fn update_log(
    State(state): State<ActionsState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let authenticated = match authenticate(&state, &headers).await {
        Ok(runner) => runner,
        Err(error) => return error.into_response(),
    };
    respond::<runner::UpdateLogRequest, runner::UpdateLogResponse, _, _>(
        headers,
        body,
        |request| async move {
            let rows = request
                .rows
                .into_iter()
                .map(|row| IncomingLogRow {
                    timestamp: row
                        .time
                        .and_then(|time| {
                            Utc.timestamp_opt(time.seconds, time.nanos as u32).single()
                        })
                        .unwrap_or_else(Utc::now),
                    content: row.content,
                })
                .collect();
            let ack_index = logs::append(
                state.repository().identity().database(),
                authenticated.id,
                request.task_id,
                request.index,
                rows,
                state.settings().max_log_row_bytes,
                state.settings().max_job_log_bytes,
            )
            .await
            .map_err(map_log_error)?;
            Ok(runner::UpdateLogResponse { ack_index })
        },
    )
    .await
}

#[expect(
    deprecated,
    reason = "the pinned Forgejo schema still requires the field"
)]
async fn task_message(
    state: &ActionsState,
    claimed: runners::ClaimedJob,
) -> Result<runner::Task, ConnectError> {
    let database = state.repository().identity().database();
    let run = action_run::Entity::find_by_id(claimed.job.run_id)
        .one(database)
        .await
        .map_err(database_error)?
        .ok_or_else(|| ConnectError::internal("assigned run is missing"))?;
    let repository = repository::Entity::find_by_id(run.repository_id)
        .one(database)
        .await
        .map_err(database_error)?
        .ok_or_else(|| ConnectError::internal("assigned repository is missing"))?;
    let actor = match run.actor_id {
        Some(actor_id) => user::Entity::find_by_id(actor_id)
            .one(database)
            .await
            .map_err(database_error)?
            .map(|actor| actor.username)
            .unwrap_or_default(),
        None => String::new(),
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
    let server_url = state
        .repository()
        .public_url()
        .to_string()
        .trim_end_matches('/')
        .to_owned();
    let context_json = json!({
        "action": "", "action_path": "", "action_ref": "", "action_repository": "",
        "actor": actor, "api_url": format!("{server_url}/api/v1"), "base_ref": "",
        "event": event, "event_name": "push", "event_path": "", "graphql_url": "",
        "head_ref": "", "job": claimed.job.job_key, "ref": run.ref_name, "ref_name": ref_name,
        "ref_protected": false, "ref_type": ref_type, "repository": format!("{}/{}", repository.namespace, repository.name),
        "repository_id": repository.id.to_string(), "repository_owner": repository.namespace,
        "retention_days": state.settings().retention_days.to_string(),
        "run_attempt": claimed.job.attempt.to_string(), "run_id": run.id.to_string(),
        "run_number": run.number.to_string(), "runtime_token": claimed.checkout_token,
        "server_url": server_url, "sha": run.after_sha,
        "token": claimed.checkout_token, "workflow": run.workflow_name,
        "workflow_ref": format!("{}/{}/{}@{}", repository.namespace, repository.name, run.workflow_path, run.ref_name),
        "workflow_sha": run.after_sha, "workspace": "",
        "forgejo_default_actions_url": state.settings().default_actions_origin,
        "forgejo_server_version": env!("CARGO_PKG_VERSION"),
        "clone_url": state.repository().http_clone_url(&repository),
    });
    let context = serde_json::from_value(context_json)
        .map_err(|error| ConnectError::internal(error.to_string()))?;
    let edges = action_job_need::Entity::find()
        .filter(action_job_need::Column::JobId.eq(claimed.job.id))
        .all(database)
        .await
        .map_err(database_error)?;
    let mut needs = HashMap::new();
    for edge in edges {
        if let Some(job) = action_job::Entity::find_by_id(edge.needed_job_id)
            .one(database)
            .await
            .map_err(database_error)?
        {
            let outputs = serde_json::from_str(&job.outputs).unwrap_or_default();
            needs.insert(
                edge.needed_job_key,
                runner::TaskNeed {
                    outputs,
                    result: protocol_result(job.result.as_deref().unwrap_or(&job.status)) as i32,
                },
            );
        }
    }
    let mut secrets = HashMap::new();
    for name in ["GITHUB_TOKEN", "GITEA_TOKEN", "FORGEJO_TOKEN"] {
        secrets.insert(name.to_owned(), claimed.checkout_token.clone());
    }
    Ok(runner::Task {
        id: claimed.job.id,
        workflow_payload: Some(claimed.job.workflow_payload),
        context: Some(context),
        secrets,
        machine: String::new(),
        needs,
        vars: HashMap::new(),
    })
}

fn protocol_result(result: &str) -> runner::Result {
    match result {
        "success" => runner::Result::Success,
        "failure" => runner::Result::Failure,
        "cancelled" => runner::Result::Cancelled,
        "skipped" => runner::Result::Skipped,
        _ => runner::Result::Unspecified,
    }
}

async fn authenticate(
    state: &ActionsState,
    headers: &HeaderMap,
) -> Result<crate::entity::action_runner::Model, ConnectError> {
    let uuid = required_header(headers, "x-runner-uuid")?;
    let token = required_header(headers, "x-runner-token")?;
    runners::authenticate(state.repository().identity().database(), uuid, token)
        .await
        .map_err(map_runner_error)
}

fn request_key(headers: &HeaderMap) -> Result<Uuid, ConnectError> {
    required_header(headers, "x-runner-request-key")?
        .parse()
        .map_err(|_| ConnectError::invalid("x-runner-request-key must be a UUID"))
}

fn required_header<'a>(headers: &'a HeaderMap, name: &str) -> Result<&'a str, ConnectError> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ConnectError::unauthenticated(format!("{name} is required")))
}

#[expect(
    deprecated,
    reason = "the pinned Forgejo schema still requires legacy label fields"
)]
fn runner_message(row: &crate::entity::action_runner::Model, token: String) -> runner::Runner {
    runner::Runner {
        id: row.id,
        uuid: row.uuid.clone(),
        token,
        name: row.name.clone(),
        status: runner::RunnerStatus::Idle as i32,
        agent_labels: Vec::new(),
        custom_labels: Vec::new(),
        version: row.version.clone(),
        labels: serde_json::from_str(&row.approved_labels).unwrap_or_default(),
        ephemeral: false,
    }
}

async fn respond<Request, Reply, F, Fut>(headers: HeaderMap, body: Bytes, handler: F) -> Response
where
    Request: Message + Default + DeserializeOwned,
    Reply: Message + Serialize,
    F: FnOnce(Request) -> Fut,
    Fut: Future<Output = Result<Reply, ConnectError>>,
{
    let encoding = match request_encoding(&headers) {
        Ok(encoding) => encoding,
        Err(error) => return error.into_response(),
    };
    let request = match decode(encoding, &body) {
        Ok(request) => request,
        Err(error) => return error.into_response(),
    };
    match handler(request).await {
        Ok(reply) => encode(encoding, &reply),
        Err(error) => error.into_response(),
    }
}

fn request_encoding(headers: &HeaderMap) -> Result<Encoding, ConnectError> {
    let content_type = headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| {
            ConnectError::unsupported("content-type must be application/proto or application/json")
        })?;
    match content_type.split(';').next().unwrap_or_default().trim() {
        PROTO_CONTENT_TYPE => Ok(Encoding::Protobuf),
        JSON_CONTENT_TYPE => Ok(Encoding::Json),
        _ => Err(ConnectError::unsupported(
            "unsupported Connect content type",
        )),
    }
}

fn decode<T: Message + Default + DeserializeOwned>(
    encoding: Encoding,
    body: &[u8],
) -> Result<T, ConnectError> {
    match encoding {
        Encoding::Protobuf => {
            T::decode(body).map_err(|error| ConnectError::invalid(error.to_string()))
        }
        Encoding::Json => {
            serde_json::from_slice(body).map_err(|error| ConnectError::invalid(error.to_string()))
        }
    }
}

fn encode<T: Message + Serialize>(encoding: Encoding, message: &T) -> Response {
    let (content_type, body) = match encoding {
        Encoding::Protobuf => (PROTO_CONTENT_TYPE, message.encode_to_vec()),
        Encoding::Json => match serde_json::to_vec(message) {
            Ok(body) => (JSON_CONTENT_TYPE, body),
            Err(error) => return ConnectError::internal(error.to_string()).into_response(),
        },
    };
    let mut response = (StatusCode::OK, body).into_response();
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static(content_type));
    response
}

#[derive(Debug, Serialize)]
struct ConnectErrorBody<'a> {
    code: &'a str,
    message: String,
}

#[derive(Debug)]
pub(crate) struct ConnectError {
    status: StatusCode,
    code: &'static str,
    message: String,
}

impl ConnectError {
    fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }
    fn invalid(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "invalid_argument", message)
    }
    fn unauthenticated(message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "unauthenticated", message)
    }
    fn unsupported(message: impl Into<String>) -> Self {
        Self::new(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "invalid_argument",
            message,
        )
    }
    fn internal(message: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", message)
    }
}

impl IntoResponse for ConnectError {
    fn into_response(self) -> Response {
        let body = ConnectErrorBody {
            code: self.code,
            message: self.message,
        };
        (
            self.status,
            [(CONTENT_TYPE, JSON_CONTENT_TYPE)],
            axum::Json(body),
        )
            .into_response()
    }
}

fn map_runner_error(error: RunnerError) -> ConnectError {
    match error {
        RunnerError::Unauthorized => ConnectError::unauthenticated("invalid runner credentials"),
        RunnerError::InvalidRegistration(message) => ConnectError::invalid(message),
        RunnerError::Version(message) => ConnectError::new(
            StatusCode::PRECONDITION_FAILED,
            "failed_precondition",
            message,
        ),
        RunnerError::Conflict(message) => {
            ConnectError::new(StatusCode::CONFLICT, "already_exists", message)
        }
        RunnerError::Database(error) => database_error(error),
    }
}

fn map_state_error(error: StateError) -> ConnectError {
    match error {
        StateError::Missing | StateError::ForeignTask => ConnectError::new(
            StatusCode::NOT_FOUND,
            "not_found",
            "task is not assigned to this runner",
        ),
        StateError::Regression => ConnectError::new(
            StatusCode::CONFLICT,
            "failed_precondition",
            "task state cannot regress",
        ),
        StateError::InvalidOutput => ConnectError::invalid("task outputs exceed protocol limits"),
        StateError::Database(error) => database_error(error),
    }
}

fn map_log_error(error: logs::LogError) -> ConnectError {
    match error {
        logs::LogError::ForeignTask => ConnectError::new(
            StatusCode::NOT_FOUND,
            "not_found",
            "task is not assigned to this runner",
        ),
        logs::LogError::Invalid(message) => ConnectError::invalid(message),
        logs::LogError::Database(error) => database_error(error),
    }
}

fn database_error(error: sea_orm::DbErr) -> ConnectError {
    tracing::error!(%error, "Actions protocol database operation failed");
    ConnectError::new(
        StatusCode::SERVICE_UNAVAILABLE,
        "unavailable",
        "Actions state is temporarily unavailable",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binary_ping_round_trips() {
        let request = ping::PingRequest {
            data: "gitadel".to_owned(),
        };
        let decoded: ping::PingRequest =
            decode(Encoding::Protobuf, &request.encode_to_vec()).unwrap();
        assert_eq!(decoded.data, "gitadel");
    }

    #[test]
    fn json_ping_uses_proto_field_names() {
        let decoded: ping::PingRequest = decode(Encoding::Json, br#"{"data":"gitadel"}"#).unwrap();
        assert_eq!(decoded.data, "gitadel");
    }

    #[test]
    fn runner_json_uses_protojson_for_int64_and_struct() {
        let request: runner::FetchTaskRequest = decode(
            Encoding::Json,
            br#"{"tasksVersion":"42","taskCapacity":"1"}"#,
        )
        .unwrap();
        assert_eq!(request.tasks_version, 42);
        assert_eq!(request.task_capacity, Some(1));

        let expected = json!({
            "repository": "owner/repository",
            "run_id": "c30211c9-a054-44e6-93dc-f0bfc6120341"
        });
        let context: pbjson_types::Struct = serde_json::from_value(expected.clone()).unwrap();
        assert_eq!(serde_json::to_value(context).unwrap(), expected);
    }

    #[test]
    fn runner_version_pin_is_exact() {
        assert_eq!(super::super::REQUIRED_RUNNER_VERSION, "13.0.0");
    }
}
