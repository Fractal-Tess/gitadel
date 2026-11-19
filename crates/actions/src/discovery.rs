//! Discovery and push-triggered workflow operations.
use super::ActionsService;
use crate::service::read_git;
pub use crate::workflow::{RunTrigger, WorkflowDiagnostic, WorkflowPlan, enqueue_plan};
use crate::workflow::{matches_push, validate_workflow};
use gitadel_db::entity::{repository, user};
use gitadel_identity::IdentityError;
use sea_orm::EntityTrait;
use serde_json::json;
use sley::GitObjectType;
use uuid::Uuid;
const WORKFLOW_DIRECTORIES: [&str; 3] = [
    ".forgejo/workflows",
    ".gitea/workflows",
    ".github/workflows",
];
const MAX_WORKFLOWS: usize = 64;
#[expect(
    clippy::too_many_arguments,
    reason = "push fields mirror the external Git event contract"
)]
pub async fn ingest_push(
    state: &ActionsService,
    repository: &repository::Model,
    actor_id: Uuid,
    reference: &str,
    before: &str,
    after: &str,
    zero: &str,
    changed_paths: Vec<String>,
) -> Result<(), IdentityError> {
    if after == zero
        || (!reference.starts_with("refs/heads/") && !reference.starts_with("refs/tags/"))
    {
        return Ok(());
    }
    let after = workflow_commit_oid(state, repository, reference, after).await?;
    let plans = discover(state, repository, after.clone()).await?;
    let actor = user::Entity::find_by_id(actor_id)
        .one(&state.database)
        .await?
        .ok_or_else(IdentityError::not_found)?;
    for discovered in plans {
        match discovered {
            Ok(plan) => {
                if matches_push(&plan.root, reference, &changed_paths).unwrap_or(false) {
                    let trigger = RunTrigger {
                        event: "push",
                        reference,
                        before,
                        after: &after,
                        actor: Some(&actor),
                        event_json: json!({
                            "ref": reference,
                            "before": before,
                            "after": after,
                            "actor": actor.username,
                        }),
                    };
                    let run = enqueue_plan(&state.database, repository, &trigger, plan).await?;
                    state.effects.requested(run.id);
                }
            }
            Err(diagnostic) => {
                record_failure(
                    state,
                    repository,
                    Some(actor_id),
                    reference,
                    before,
                    &after,
                    diagnostic,
                )
                .await?;
            }
        }
    }
    if repository
        .default_branch
        .as_deref()
        .is_some_and(|branch| reference.strip_prefix("refs/heads/") == Some(branch))
        && let Err(error) = super::service_schedule::sync_repository(state, repository).await
    {
        tracing::warn!(%error, repository_id = %repository.id, "could not refresh Actions schedules");
    }
    Ok(())
}

async fn workflow_commit_oid(
    state: &ActionsService,
    repository: &repository::Model,
    reference: &str,
    after: &str,
) -> Result<String, IdentityError> {
    if !reference.starts_with("refs/tags/") {
        return Ok(after.to_owned());
    }
    let path = state.content.repository_path(repository.storage_key);
    let after = after.to_owned();
    read_git(path, move |git| {
        let oid = git.peel_to_commit_oid(git.rev_parse(&after)?)?;
        Ok(oid.to_hex())
    })
    .await
}

/// Reads and validates every workflow file at `revision`.
pub async fn discover(
    state: &ActionsService,
    repository: &repository::Model,
    revision: String,
) -> Result<Vec<Result<WorkflowPlan, WorkflowDiagnostic>>, IdentityError> {
    let path = state.content.repository_path(repository.storage_key);
    let files = read_git(path, move |git| {
        let mut selected = None;
        for directory in WORKFLOW_DIRECTORIES {
            if let Ok(resolved) = git.resolve_path(&revision, directory)
                && resolved.object_type == GitObjectType::Tree
            {
                selected = Some((directory, resolved.oid));
                break;
            }
        }
        let Some((directory, tree_oid)) = selected else {
            return Ok(Vec::new());
        };
        let tree = git.read_tree(&tree_oid)?;
        let mut files = Vec::new();
        for entry in tree.entries {
            if files.len() >= MAX_WORKFLOWS {
                break;
            }
            let Ok(name) = std::str::from_utf8(entry.name.as_bytes()) else {
                continue;
            };
            if entry.is_tree() || entry.is_symlink() || entry.is_gitlink() {
                continue;
            }
            if !(name.ends_with(".yml") || name.ends_with(".yaml")) {
                continue;
            }
            let bytes = git.blobs().read(entry.oid)?;
            files.push((format!("{directory}/{name}"), bytes));
        }
        Ok(files)
    })
    .await?;
    Ok(files
        .into_iter()
        .map(|(path, bytes)| validate_workflow(&state.settings, path, bytes))
        .collect())
}

async fn record_failure(
    state: &ActionsService,
    repository: &repository::Model,
    actor_id: Option<Uuid>,
    reference: &str,
    before: &str,
    after: &str,
    diagnostic: WorkflowDiagnostic,
) -> Result<(), IdentityError> {
    let run_id = crate::workflow::record_failure(
        &state.database,
        repository,
        actor_id,
        reference,
        before,
        after,
        diagnostic,
    )
    .await?;
    state.effects.requested_and_completed(run_id);
    Ok(())
}
