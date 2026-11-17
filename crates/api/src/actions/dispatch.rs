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
use serde_json::json;
use yaml_serde::Value;

use crate::{
    identity::{ApiError, SCOPE_READ, SCOPE_WRITE},
    repository::{Permission, read_git},
};

use super::{
    ActionsState,
    api::RunSummary,
    workflow::{self, RunTrigger},
};

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

pub(crate) fn parse_inputs(config: Option<&Value>) -> Result<Vec<DispatchInput>, String> {
    gitadel_actions::dispatch::parse_inputs(config)
        .map(|inputs| inputs.into_iter().map(Into::into).collect())
}

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

/// Rejects revision syntax so a ref names exactly one branch or tag.
fn valid_reference(reference: &str) -> bool {
    !reference.is_empty()
        && reference.len() <= 255
        && !reference.contains("..")
        && !reference.contains("@{")
        && !reference.starts_with('-')
        && !reference.ends_with('/')
        && !reference.chars().any(|character| {
            character.is_control()
                || matches!(character, ' ' | '~' | '^' | ':' | '?' | '*' | '[' | '\\')
        })
}

/// Resolves a branch, tag, or full ref name to `(full ref, commit OID)`.
async fn resolve_reference(
    state: &ActionsState,
    repository: &crate::entity::repository::Model,
    reference: Option<&str>,
) -> Result<(String, String), ApiError> {
    let reference = match reference.map(str::trim).filter(|value| !value.is_empty()) {
        Some(reference) => reference.to_owned(),
        None => repository
            .default_branch
            .clone()
            .ok_or_else(|| ApiError::bad_request("The repository has no default branch."))?,
    };
    if !valid_reference(&reference) {
        return Err(ApiError::bad_request(
            "The ref is not a valid branch or tag.",
        ));
    }
    let candidates = if reference.starts_with("refs/heads/") || reference.starts_with("refs/tags/")
    {
        vec![reference.clone()]
    } else {
        vec![
            format!("refs/heads/{reference}"),
            format!("refs/tags/{reference}"),
        ]
    };
    let path = state.repository().repository_path(repository);
    read_git(path, move |git| {
        for candidate in candidates {
            if let Ok(oid) = git.rev_parse(&candidate)
                && let Ok(commit) = git.peel_to_commit_oid(oid)
            {
                return Ok(Some((candidate, commit.to_hex())));
            }
        }
        Ok(None)
    })
    .await?
    .ok_or_else(|| ApiError::bad_request("The branch or tag does not exist."))
}

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
    let user_id = state
        .repository()
        .identity()
        .optional_user(&headers, &jar, SCOPE_READ)
        .await?
        .map(|user| user.id);
    state
        .repository()
        .authorize(&repository, user_id, Permission::Read)
        .await?;
    let can_dispatch = user_id.is_some()
        && repository.archived_at.is_none()
        && !repository.mirrored
        && state
            .repository()
            .can_access(&repository, user_id, Permission::Write)
            .await?;
    let (reference, commit) =
        resolve_reference(&state, &repository, query.reference.as_deref()).await?;
    let workflows = workflow::discover(&state, &repository, commit.clone())
        .await?
        .into_iter()
        .map(|discovered| match discovered {
            Ok(plan) => {
                let dispatchable = plan.has_trigger("workflow_dispatch");
                let inputs = if dispatchable {
                    parse_inputs(plan.trigger_config("workflow_dispatch")).unwrap_or_default()
                } else {
                    Vec::new()
                };
                WorkflowResponse {
                    path: plan.path,
                    name: plan.name,
                    dispatchable,
                    inputs,
                    error: None,
                }
            }
            Err(diagnostic) => WorkflowResponse {
                name: diagnostic.path.clone(),
                path: diagnostic.path,
                dispatchable: false,
                inputs: Vec::new(),
                error: Some(diagnostic.summary),
            },
        })
        .collect();
    Ok(Json(WorkflowsResponse {
        reference,
        commit,
        can_dispatch,
        workflows,
    }))
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
    let (reference, commit) =
        resolve_reference(&state, &repository, request.reference.as_deref()).await?;
    let wanted = request.workflow.trim();
    let discovered = workflow::discover(&state, &repository, commit.clone())
        .await?
        .into_iter()
        .find(|discovered| {
            let path = match discovered {
                Ok(plan) => &plan.path,
                Err(diagnostic) => &diagnostic.path,
            };
            path == wanted || path.rsplit('/').next() == Some(wanted)
        })
        .ok_or_else(|| ApiError::bad_request("The workflow does not exist at that ref."))?;
    let plan = discovered.map_err(|diagnostic| {
        ApiError::bad_request(format!("The workflow is invalid: {}", diagnostic.summary))
    })?;
    if !plan.has_trigger("workflow_dispatch") {
        return Err(ApiError::bad_request(
            "The workflow does not declare the workflow_dispatch trigger.",
        ));
    }
    let declared =
        parse_inputs(plan.trigger_config("workflow_dispatch")).map_err(ApiError::bad_request)?;
    let inputs = resolve_inputs(&declared, &request.inputs).map_err(ApiError::bad_request)?;
    let event_json = json!({
        "inputs": inputs,
        "ref": reference,
        "workflow": plan.path,
        "repository": {
            "full_name": format!("{}/{}", repository.namespace, repository.name),
            "default_branch": repository.default_branch,
        },
        "sender": { "login": actor.user.username },
    });
    let trigger = RunTrigger {
        event: "workflow_dispatch",
        reference: &reference,
        before: "",
        after: &commit,
        actor: Some(&actor.user),
        event_json,
    };
    let run = workflow::enqueue_plan(
        state.repository().identity().database(),
        &repository,
        &trigger,
        plan,
    )
    .await?;
    super::events::spawn_run_requested(state.repository(), run.id);
    state
        .repository()
        .identity()
        .audit(
            Some(actor.user.id),
            "actions.run.dispatch",
            Some(run.id.to_string()),
        )
        .await?;
    Ok((StatusCode::CREATED, Json(run.into())))
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
