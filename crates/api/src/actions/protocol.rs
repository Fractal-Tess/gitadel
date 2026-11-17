use std::collections::BTreeMap;

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
use serde::{Serialize, de::DeserializeOwned};
#[cfg(test)]
use serde_json::json;
use uuid::Uuid;

use super::{
    ActionsState,
    logs::{self, IncomingLogRow},
    proto::{ping, runner},
    runners::{self, RunnerError},
    runs::{StateError, TaskResult},
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
            let row = state
                .service()
                .declare_runner(&authenticated, &request.version, &request.labels)
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
    runner: &gitadel_actions::runner_service::RunnerCredential,
    request_key: Uuid,
    handle: Option<i64>,
) -> Result<Option<runners::ClaimedJob>, RunnerError> {
    let deadline = tokio::time::Instant::now()
        + std::time::Duration::from_secs(state.settings().fetch_timeout_seconds);
    loop {
        let claimed = state
            .service()
            .claim_task(runner, request_key, handle)
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
            let job = state
                .service()
                .update_runner_task(&authenticated, task_state.id, result, outputs, steps_json)
                .await
                .map_err(|error| match error {
                    gitadel_actions::runner_service::RunnerUpdateError::Credential(error) => {
                        map_runner_error(error)
                    }
                    gitadel_actions::runner_service::RunnerUpdateError::State(error) => {
                        map_state_error(error)
                    }
                })?;
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
            let ack_index = state
                .service()
                .append_logs(&authenticated, request.task_id, request.index, rows)
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
    let task = gitadel_actions::tasks::prepare_task(
        state.service(),
        claimed,
        state.job_origin(),
        env!("CARGO_PKG_VERSION"),
    )
    .await
    .map_err(|error| match error {
        gitadel_actions::tasks::TaskPreparationError::Unauthorized => {
            ConnectError::unauthenticated("The job lease is no longer valid.")
        }
        gitadel_actions::tasks::TaskPreparationError::Database(error) => database_error(error),
        gitadel_actions::tasks::TaskPreparationError::Internal(message) => {
            ConnectError::internal(message)
        }
    })?;
    Ok(runner::Task {
        id: task.id,
        workflow_payload: Some(task.workflow_payload),
        context: Some(
            serde_json::from_value(task.context)
                .map_err(|error| ConnectError::internal(error.to_string()))?,
        ),
        secrets: task.secrets,
        needs: task
            .needs
            .into_iter()
            .map(|(key, need)| {
                (
                    key,
                    runner::TaskNeed {
                        outputs: need.outputs,
                        result: task_conclusion(need.result) as i32,
                    },
                )
            })
            .collect(),
        vars: task.vars,
        machine: String::new(),
    })
}
fn task_conclusion(value: gitadel_actions::tasks::TaskConclusion) -> runner::Result {
    use gitadel_actions::tasks::TaskConclusion;
    match value {
        TaskConclusion::Success => runner::Result::Success,
        TaskConclusion::Skipped => runner::Result::Skipped,
        TaskConclusion::Unspecified => runner::Result::Unspecified,
        TaskConclusion::Cancelled => runner::Result::Cancelled,
        TaskConclusion::Failure => runner::Result::Failure,
    }
}

#[cfg(test)]
fn merge_results(current: i32, next: i32) -> i32 {
    let rank = |result: i32| match runner::Result::try_from(result) {
        Ok(runner::Result::Failure) => 4,
        Ok(runner::Result::Cancelled) => 3,
        Ok(runner::Result::Unspecified) | Err(_) => 2,
        Ok(runner::Result::Skipped) => 1,
        Ok(runner::Result::Success) => 0,
    };
    if rank(next) > rank(current) {
        next
    } else {
        current
    }
}

async fn authenticate(
    state: &ActionsState,
    headers: &HeaderMap,
) -> Result<gitadel_actions::runner_service::RunnerCredential, ConnectError> {
    let uuid = required_header(headers, "x-runner-uuid")?;
    let token = required_header(headers, "x-runner-token")?;
    let credential =
        gitadel_actions::runner_service::RunnerCredential::new(uuid.to_owned(), token.to_owned());
    credential
        .authenticate(state.repository().identity().database())
        .await
        .map_err(map_runner_error)?;
    Ok(credential)
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
        logs::LogError::Credential(error) => map_runner_error(error),
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
    fn matrix_needs_report_the_worst_result() {
        let success = runner::Result::Success as i32;
        let failure = runner::Result::Failure as i32;
        let skipped = runner::Result::Skipped as i32;
        assert_eq!(merge_results(success, failure), failure);
        assert_eq!(merge_results(failure, success), failure);
        assert_eq!(merge_results(skipped, success), skipped);
    }

    #[test]
    fn runner_version_pin_is_exact() {
        assert_eq!(super::super::REQUIRED_RUNNER_VERSION, "13.0.0");
    }
}
