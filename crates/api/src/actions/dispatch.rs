//! Manually dispatched runs (`on: workflow_dispatch`) and workflow listing.

use crate::openapi::ContractRouterExt;

use std::collections::BTreeMap;

use axum::{
    Json,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
};
use axum_extra::extract::cookie::CookieJar;
use serde::{Deserialize, Serialize};
#[cfg(test)]
use serde_json::json;
#[cfg(test)]
use yaml_serde::Value;

use crate::{
    identity::{ApiError, SCOPE_READ, SCOPE_WRITE},
    repository::Permission,
};

use super::{ActionsState, api::RunSummary};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub(crate) enum InputKind {
    String,
    Boolean,
    Choice,
    Number,
}

#[derive(Clone, Debug, Serialize, utoipa::ToSchema)]
pub(crate) struct DispatchInput {
    pub(crate) name: String,
    pub(crate) description: Option<String>,
    pub(crate) required: bool,
    #[serde(rename = "type")]
    pub(crate) kind: InputKind,
    pub(crate) default: Option<String>,
    pub(crate) options: Vec<String>,
}

impl From<gitadel_actions::dispatch::DispatchInput> for DispatchInput {
    fn from(input: gitadel_actions::dispatch::DispatchInput) -> Self {
        Self {
            name: input.name,
            description: input.description,
            required: input.required,
            kind: match input.kind {
                gitadel_actions::dispatch::InputKind::String => InputKind::String,
                gitadel_actions::dispatch::InputKind::Boolean => InputKind::Boolean,
                gitadel_actions::dispatch::InputKind::Choice => InputKind::Choice,
                gitadel_actions::dispatch::InputKind::Number => InputKind::Number,
            },
            default: input.default,
            options: input.options,
        }
    }
}

#[cfg(test)]
pub(crate) fn parse_inputs(config: Option<&Value>) -> Result<Vec<DispatchInput>, String> {
    gitadel_actions::dispatch::parse_inputs(config)
        .map(|inputs| inputs.into_iter().map(Into::into).collect())
}

#[cfg(test)]
fn resolve_inputs(
    declared: &[DispatchInput],
    provided: &BTreeMap<String, serde_json::Value>,
) -> Result<BTreeMap<String, String>, String> {
    let declared: Vec<_> = declared
        .iter()
        .map(|input| gitadel_actions::dispatch::DispatchInput {
            name: input.name.clone(),
            description: input.description.clone(),
            required: input.required,
            kind: match input.kind {
                InputKind::String => gitadel_actions::dispatch::InputKind::String,
                InputKind::Boolean => gitadel_actions::dispatch::InputKind::Boolean,
                InputKind::Choice => gitadel_actions::dispatch::InputKind::Choice,
                InputKind::Number => gitadel_actions::dispatch::InputKind::Number,
            },
            default: input.default.clone(),
            options: input.options.clone(),
        })
        .collect();
    gitadel_actions::dispatch::resolve_inputs(&declared, provided)
}

pub(crate) fn router() -> utoipa_axum::router::OpenApiRouter<ActionsState> {
    utoipa_axum::router::OpenApiRouter::new()
        .api_routes(crate::openapi::routes!(
            crate::actions::dispatch::list_workflows
        ))
        .api_routes(crate::openapi::routes!(crate::actions::dispatch::dispatch))
}

#[cfg(test)]
use gitadel_actions::service_dispatch::valid_reference;
#[derive(Deserialize, utoipa::ToSchema, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct WorkflowsQuery {
    #[serde(rename = "ref")]
    reference: Option<String>,
}

#[derive(Serialize, utoipa::ToSchema)]
struct WorkflowResponse {
    path: String,
    name: String,
    dispatchable: bool,
    inputs: Vec<DispatchInput>,
    error: Option<String>,
}

#[derive(Serialize, utoipa::ToSchema)]
struct WorkflowsResponse {
    reference: String,
    commit: String,
    can_dispatch: bool,
    workflows: Vec<WorkflowResponse>,
}

#[utoipa::path(
    get, path = "/repositories/{namespace}/{name}/actions/workflows",
    operation_id = "get_repositories_namespace_name_actions_workflows",
    summary = "List workflows",
    tag = "Actions",
    params(
        ("namespace" = String, Path),
        ("name" = String, Path),
        WorkflowsQuery,
    ),
    security((), ("token" = []), ("session" = [])),
    responses(
        (status = 200, description = "Success", body = WorkflowsResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
async fn list_workflows(
    State(state): State<ActionsState>,
    Path((namespace, name)): Path<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
    Query(query): Query<WorkflowsQuery>,
) -> Result<Json<WorkflowsResponse>, ApiError> {
    let repository = state.repository().find(&namespace, &name).await?;
    let _user_id = state
        .repository()
        .identity()
        .optional_user(&headers, &jar, SCOPE_READ)
        .await?
        .map(|user| user.id);
    let result = gitadel_actions::service_dispatch::list_workflows(
        state.service(),
        repository.id,
        &crate::repository::principal(&headers, &jar)?,
        query.reference.as_deref(),
    )
    .await?;
    Ok(Json(result.into()))
}
#[derive(Deserialize, utoipa::ToSchema)]
struct DispatchRequest {
    /// Workflow path, such as `.forgejo/workflows/ci.yml`, or its file name.
    workflow: String,
    #[serde(rename = "ref")]
    reference: Option<String>,
    #[serde(default)]
    inputs: BTreeMap<String, serde_json::Value>,
}

#[utoipa::path(
    post, path = "/repositories/{namespace}/{name}/actions/workflows/dispatch",
    operation_id = "post_repositories_namespace_name_actions_workflows_dispatch",
    summary = "Dispatch",
    tag = "Actions",
    params(
        ("namespace" = String, Path),
        ("name" = String, Path),
    ),
    responses(
        (status = 201, description = "Success", body = RunSummary, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
async fn dispatch(
    State(state): State<ActionsState>,
    Path((namespace, name)): Path<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(request): Json<DispatchRequest>,
) -> Result<(StatusCode, Json<RunSummary>), ApiError> {
    let (_, repository) = state
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
    let run = gitadel_actions::service_dispatch::dispatch(
        state.service(),
        &crate::repository::principal(&headers, &jar)?,
        repository.id,
        gitadel_actions::service_dispatch::DispatchRequest {
            workflow: request.workflow,
            reference: request.reference,
            inputs: request.inputs,
        },
        gitadel_core::audit::AuditContext {
            remote_address: crate::identity::remote_address(),
        },
    )
    .await?;
    Ok((StatusCode::CREATED, Json(run.into())))
}
impl From<gitadel_actions::service_dispatch::WorkflowResponse> for WorkflowResponse {
    fn from(value: gitadel_actions::service_dispatch::WorkflowResponse) -> Self {
        Self {
            path: value.path,
            name: value.name,
            dispatchable: value.dispatchable,
            inputs: value.inputs.into_iter().map(Into::into).collect(),
            error: value.error,
        }
    }
}

impl From<gitadel_actions::service_dispatch::WorkflowsResponse> for WorkflowsResponse {
    fn from(value: gitadel_actions::service_dispatch::WorkflowsResponse) -> Self {
        Self {
            reference: value.reference,
            commit: value.commit,
            can_dispatch: value.can_dispatch,
            workflows: value.workflows.into_iter().map(Into::into).collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inputs(source: &str) -> Result<Vec<DispatchInput>, String> {
        let value: Value = yaml_serde::from_str(source).unwrap();
        parse_inputs(Some(&value))
    }

    #[test]
    fn inputs_are_parsed_and_validated() {
        let declared = inputs(
            "inputs:\n  target:\n    required: true\n  dry-run:\n    type: boolean\n    default: true\n  \
             level:\n    type: choice\n    options: [low, high]\n    default: low\n",
        )
        .unwrap();
        assert_eq!(declared.len(), 3);
        assert_eq!(declared[1].default.as_deref(), Some("true"));
        for invalid in [
            "inputs:\n  x:\n    type: choice\n",
            "inputs:\n  x:\n    type: environment\n",
            "inputs:\n  x:\n    type: boolean\n    default: maybe\n",
            "inputs:\n  x:\n    type: choice\n    options: [a]\n    default: b\n",
            "inputs:\n  9x: {}\n",
            "branches: [main]\n",
        ] {
            assert!(inputs(invalid).is_err(), "{invalid}");
        }
        assert!(parse_inputs(None).unwrap().is_empty());
        assert!(parse_inputs(Some(&Value::Null)).unwrap().is_empty());
    }

    #[test]
    fn provided_inputs_merge_with_defaults() {
        let declared = inputs(
            "inputs:\n  target:\n    required: true\n  dry-run:\n    type: boolean\n  \
             level:\n    type: choice\n    options: [low, high]\n    default: low\n",
        )
        .unwrap();
        let provided = BTreeMap::from([
            ("target".to_owned(), json!("prod")),
            ("dry-run".to_owned(), json!(true)),
        ]);
        let resolved = resolve_inputs(&declared, &provided).unwrap();
        assert_eq!(resolved["target"], "prod");
        assert_eq!(resolved["dry-run"], "true");
        assert_eq!(resolved["level"], "low");

        assert!(resolve_inputs(&declared, &BTreeMap::new()).is_err());
        let unknown = BTreeMap::from([
            ("target".to_owned(), json!("prod")),
            ("other".to_owned(), json!("x")),
        ]);
        assert!(resolve_inputs(&declared, &unknown).is_err());
        let bad_choice = BTreeMap::from([
            ("target".to_owned(), json!("prod")),
            ("level".to_owned(), json!("max")),
        ]);
        assert!(resolve_inputs(&declared, &bad_choice).is_err());
    }

    #[test]
    fn references_reject_revision_syntax() {
        for valid in ["main", "release/1.0", "refs/tags/v1", "v1.2.3"] {
            assert!(valid_reference(valid), "{valid}");
        }
        for invalid in [
            "", "main~1", "main^", "a..b", "HEAD@{1}", "-x", "a b", "x:y",
        ] {
            assert!(!valid_reference(invalid), "{invalid}");
        }
    }
}
