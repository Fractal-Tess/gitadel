//! Manual workflow discovery and dispatch, including direct-call authorization.
use crate::{
    ActionsService, discovery,
    dispatch::{DispatchInput, parse_inputs, resolve_inputs},
    workflow::RunTrigger,
};
use gitadel_core::audit::AuditContext;
use gitadel_db::entity::action_run;
use gitadel_identity::{
    IdentityError,
    permissions::{
        RepositoryPermission as Permission, authorize_repository, can_access_repository,
    },
};
use gitadel_repositories::content::read_git;
use serde_json::json;
use std::collections::BTreeMap;
pub struct DispatchRequest {
    pub workflow: String,
    pub reference: Option<String>,
    pub inputs: BTreeMap<String, serde_json::Value>,
}
pub struct WorkflowResponse {
    pub path: String,
    pub name: String,
    pub dispatchable: bool,
    pub inputs: Vec<DispatchInput>,
    pub error: Option<String>,
}

pub struct WorkflowsResponse {
    pub reference: String,
    pub commit: String,
    pub can_dispatch: bool,
    pub workflows: Vec<WorkflowResponse>,
}

/// Rejects revision syntax so a ref names exactly one branch or tag.
pub fn valid_reference(reference: &str) -> bool {
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
    state: &ActionsService,
    repository: &gitadel_db::entity::repository::Model,
    reference: Option<&str>,
) -> Result<(String, String), IdentityError> {
    let reference = match reference.map(str::trim).filter(|value| !value.is_empty()) {
        Some(reference) => reference.to_owned(),
        None => repository
            .default_branch
            .clone()
            .ok_or_else(|| IdentityError::bad_request("The repository has no default branch."))?,
    };
    if !valid_reference(&reference) {
        return Err(IdentityError::bad_request(
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
    let path = state.content.repository_path(repository);
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
    .ok_or_else(|| IdentityError::bad_request("The branch or tag does not exist."))
}

pub async fn list_workflows(
    state: &ActionsService,
    repository_id: uuid::Uuid,
    principal: &gitadel_identity::permissions::RepositoryPrincipal,
    reference: Option<&str>,
) -> Result<WorkflowsResponse, IdentityError> {
    let user_id = principal
        .optional_user(&state.database, gitadel_identity::SCOPE_READ)
        .await?
        .map(|user| user.id);
    let repository =
        authorize_repository(&state.database, repository_id, user_id, Permission::Read).await?;
    let can_dispatch = user_id.is_some()
        && repository.archived_at.is_none()
        && !repository.mirrored
        && can_access_repository(&state.database, repository.id, user_id, Permission::Write)
            .await?;
    let (reference, commit) = resolve_reference(state, &repository, reference).await?;
    let workflows = discovery::discover(state, &repository, commit.clone())
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
    Ok(WorkflowsResponse {
        reference,
        commit,
        can_dispatch,
        workflows,
    })
}
pub async fn dispatch(
    state: &ActionsService,
    principal: &gitadel_identity::permissions::RepositoryPrincipal,
    repository_id: uuid::Uuid,
    request: DispatchRequest,
    audit: AuditContext,
) -> Result<action_run::Model, IdentityError> {
    let actor = &principal
        .authenticate(&state.database, gitadel_identity::SCOPE_WRITE)
        .await?;
    let repository = authorize_repository(
        &state.database,
        repository_id,
        Some(actor.user.id),
        Permission::Write,
    )
    .await?;
    let (reference, commit) =
        resolve_reference(state, &repository, request.reference.as_deref()).await?;
    let wanted = request.workflow.trim();
    let discovered = discovery::discover(state, &repository, commit.clone())
        .await?
        .into_iter()
        .find(|discovered| {
            let path = match discovered {
                Ok(plan) => &plan.path,
                Err(diagnostic) => &diagnostic.path,
            };
            path == wanted || path.rsplit('/').next() == Some(wanted)
        })
        .ok_or_else(|| IdentityError::bad_request("The workflow does not exist at that ref."))?;
    let plan = discovered.map_err(|diagnostic| {
        IdentityError::bad_request(format!("The workflow is invalid: {}", diagnostic.summary))
    })?;
    if !plan.has_trigger("workflow_dispatch") {
        return Err(IdentityError::bad_request(
            "The workflow does not declare the workflow_dispatch trigger.",
        ));
    }
    let declared = parse_inputs(plan.trigger_config("workflow_dispatch"))
        .map_err(IdentityError::bad_request)?;
    let inputs = resolve_inputs(&declared, &request.inputs).map_err(IdentityError::bad_request)?;
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
    let run = crate::workflow::enqueue_plan(&state.database, &repository, &trigger, plan).await?;
    state.effects.requested(run.id);
    gitadel_db::audit::record(
        &state.database,
        Some(actor.user.id),
        "actions.run.dispatch",
        Some(run.id.to_string()),
        audit,
    )
    .await?;
    Ok(run)
}
