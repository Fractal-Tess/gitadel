//! Default-branch schedule discovery and explicitly driven claims.
use crate::{ActionsService, discovery, schedule::parse_crons, workflow::RunTrigger};
use chrono::Utc;
use gitadel_db::entity::{action_schedule, action_schedule_scan, repository};
use gitadel_identity::IdentityError;
use gitadel_repositories::content::read_git;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde_json::json;
use std::collections::BTreeSet;
/// Resolves the default branch head as `(branch, commit)`.
async fn default_head(
    state: &ActionsService,
    repository: &repository::Model,
) -> Result<Option<(String, String)>, IdentityError> {
    let Some(branch) = repository.default_branch.clone() else {
        return Ok(None);
    };
    let path = state.content.repository_path(repository);
    let reference = format!("refs/heads/{branch}");
    let commit = read_git(path, move |git| {
        Ok(git
            .rev_parse(&reference)
            .ok()
            .and_then(|oid| git.peel_to_commit_oid(oid).ok())
            .map(|oid| oid.to_hex()))
    })
    .await?;
    Ok(commit.map(|commit| (branch, commit)))
}

/// Refreshes a repository's schedules from its default branch head.
pub async fn sync_repository(
    state: &ActionsService,
    repository: &repository::Model,
) -> Result<(), IdentityError> {
    let database = &state.database;
    let eligible =
        repository.deleted_at.is_none() && repository.archived_at.is_none() && !repository.mirrored;
    let head = if eligible {
        default_head(state, repository).await?
    } else {
        None
    };
    let Some((branch, commit)) = head else {
        crate::schedule::clear_schedules(database, repository.id).await?;
        return Ok(());
    };
    let scan = action_schedule_scan::Entity::find_by_id(repository.id)
        .one(database)
        .await?;
    if scan
        .as_ref()
        .is_some_and(|scan| scan.default_branch == branch && scan.head_sha == commit)
    {
        return Ok(());
    }
    let mut wanted = BTreeSet::new();
    for plan in discovery::discover(state, repository, commit.clone())
        .await?
        .into_iter()
        .flatten()
    {
        if let Some(schedule) = plan.trigger_config("schedule")
            && let Ok(crons) = parse_crons(schedule)
        {
            for cron in crons {
                wanted.insert((plan.path.clone(), cron));
            }
        }
    }
    crate::schedule::sync_schedules(database, repository.id, branch, commit, wanted, scan).await?;
    Ok(())
}

/// Scans every repository; unchanged default-branch heads are skipped cheaply.
pub async fn sync_all(state: &ActionsService) -> anyhow::Result<()> {
    let repositories = repository::Entity::find()
        .filter(repository::Column::DeletedAt.is_null())
        .all(&state.database)
        .await?;
    for repository in repositories {
        if let Err(error) = sync_repository(state, &repository).await {
            tracing::warn!(%error, repository_id = %repository.id, "could not scan Actions schedules");
        }
    }
    Ok(())
}

/// Starts runs for every schedule whose next occurrence has passed.
pub async fn fire_due(state: &ActionsService) -> anyhow::Result<()> {
    let database = &state.database;
    let now = Utc::now();
    let due = crate::schedule::due_schedules(database, now).await?;
    for schedule in due {
        if !crate::schedule::claim_due(database, &schedule, now).await? {
            continue;
        }
        if let Err(error) = start(state, &schedule).await {
            tracing::warn!(
                %error,
                repository_id = %schedule.repository_id,
                workflow = schedule.workflow_path,
                "could not start a scheduled Actions run"
            );
        }
    }
    Ok(())
}

async fn start(
    state: &ActionsService,
    schedule: &action_schedule::Model,
) -> Result<(), IdentityError> {
    let database = &state.database;
    let Some(repository) = repository::Entity::find_by_id(schedule.repository_id)
        .one(database)
        .await?
        .filter(|repository| {
            repository.deleted_at.is_none()
                && repository.archived_at.is_none()
                && !repository.mirrored
        })
    else {
        return Ok(());
    };
    let Some((branch, commit)) = default_head(state, &repository).await? else {
        return Ok(());
    };
    // Run the workflow as it exists now, and only if it still has this cron.
    let Some(plan) = discovery::discover(state, &repository, commit.clone())
        .await?
        .into_iter()
        .flatten()
        .find(|plan| {
            plan.path == schedule.workflow_path
                && plan
                    .trigger_config("schedule")
                    .and_then(|value| parse_crons(value).ok())
                    .is_some_and(|crons| crons.contains(&schedule.cron))
        })
    else {
        return Ok(());
    };
    let reference = format!("refs/heads/{branch}");
    let trigger = RunTrigger {
        event: "schedule",
        reference: &reference,
        before: "",
        after: &commit,
        actor: None,
        event_json: json!({ "schedule": schedule.cron }),
    };
    let run = crate::workflow::enqueue_plan(database, &repository, &trigger, plan).await?;
    state.effects.requested(run.id);
    Ok(())
}
