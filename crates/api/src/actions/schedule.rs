//! `on: schedule` triggers.
//!
//! Workflows on each repository's default branch are scanned when that
//! branch is pushed and periodically by the Actions scheduler. Every cron
//! entry becomes an `action_schedules` row whose `next_fire_at` is persisted,
//! and a compare-and-swap on that column makes each occurrence fire at most
//! once, even across restarts. Occurrences missed while Gitadel was down are
//! coalesced into a single run.

use std::collections::BTreeSet;

use chrono::Utc;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde_json::json;

use crate::{
    entity::{action_schedule, action_schedule_scan, repository},
    identity::ApiError,
    repository::read_git,
};

use super::{
    ActionsState,
    workflow::{self, RunTrigger},
};

use gitadel_actions::schedule::parse_crons;

/// Resolves the default branch head as `(branch, commit)`.
async fn default_head(
    state: &ActionsState,
    repository: &repository::Model,
) -> Result<Option<(String, String)>, ApiError> {
    let Some(branch) = repository.default_branch.clone() else {
        return Ok(None);
    };
    let path = state.repository().repository_path(repository);
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
pub(crate) async fn sync_repository(
    state: &ActionsState,
    repository: &repository::Model,
) -> Result<(), ApiError> {
    let database = state.repository().identity().database();
    let eligible =
        repository.deleted_at.is_none() && repository.archived_at.is_none() && !repository.mirrored;
    let head = if eligible {
        default_head(state, repository).await?
    } else {
        None
    };
    let Some((branch, commit)) = head else {
        gitadel_actions::schedule::clear_schedules(database, repository.id).await?;
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
    for plan in workflow::discover(state, repository, commit.clone())
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
    gitadel_actions::schedule::sync_schedules(
        database,
        repository.id,
        branch,
        commit,
        wanted,
        scan,
    )
    .await?;
    Ok(())
}

/// Scans every repository; unchanged default-branch heads are skipped cheaply.
pub(crate) async fn sync_all(state: &ActionsState) -> anyhow::Result<()> {
    let repositories = repository::Entity::find()
        .filter(repository::Column::DeletedAt.is_null())
        .all(state.repository().identity().database())
        .await?;
    for repository in repositories {
        if let Err(error) = sync_repository(state, &repository).await {
            tracing::warn!(%error, repository_id = %repository.id, "could not scan Actions schedules");
        }
    }
    Ok(())
}

/// Starts runs for every schedule whose next occurrence has passed.
pub(crate) async fn fire_due(state: &ActionsState) -> anyhow::Result<()> {
    let database = state.repository().identity().database();
    let now = Utc::now();
    let due = gitadel_actions::schedule::due_schedules(database, now).await?;
    for schedule in due {
        if !gitadel_actions::schedule::claim_due(database, &schedule, now).await? {
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

async fn start(state: &ActionsState, schedule: &action_schedule::Model) -> Result<(), ApiError> {
    let database = state.repository().identity().database();
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
    let Some(plan) = workflow::discover(state, &repository, commit.clone())
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
    let run = workflow::enqueue_plan(database, &repository, &trigger, plan).await?;
    super::events::spawn_run_requested(state.repository(), run.id);
    Ok(())
}
