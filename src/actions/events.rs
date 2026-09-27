//! Side effects announced when workflow runs are requested or complete:
//! `workflow_run` webhooks and failure emails to the user who pushed.
//!
//! Everything here runs on the repository task tracker so runner protocol
//! responses and push handling never wait on webhook or notification work.

use sea_orm::EntityTrait;
use uuid::Uuid;

use crate::{
    entity::action_run,
    repository::{RepositoryState, notifications, webhook_events},
};

/// Announce a newly stored run.
pub(crate) fn spawn_run_requested(state: &RepositoryState, run_id: Uuid) {
    spawn(state, run_id, &[RunEvent::Requested]);
}

/// Announce a run that just reached a terminal state.
pub(crate) fn spawn_run_completed(state: &RepositoryState, run_id: Uuid) {
    spawn(state, run_id, &[RunEvent::Completed]);
}

/// Announce a run that was stored already finished, such as an invalid
/// workflow file. Both events are sent in order from one task.
pub(crate) fn spawn_run_requested_and_completed(state: &RepositoryState, run_id: Uuid) {
    spawn(state, run_id, &[RunEvent::Requested, RunEvent::Completed]);
}

#[derive(Clone, Copy)]
enum RunEvent {
    Requested,
    Completed,
}

fn spawn(state: &RepositoryState, run_id: Uuid, events: &'static [RunEvent]) {
    let task_state = state.clone();
    state.spawn_task(async move {
        let run = match action_run::Entity::find_by_id(run_id)
            .one(task_state.identity().database())
            .await
        {
            Ok(Some(run)) => run,
            Ok(None) => return,
            Err(error) => {
                tracing::warn!(%error, %run_id, "could not load workflow run for events");
                return;
            }
        };
        for event in events {
            match event {
                RunEvent::Requested => {
                    webhook_events::workflow_run(&task_state, &run, "requested").await;
                }
                RunEvent::Completed => {
                    webhook_events::workflow_run(&task_state, &run, "completed").await;
                    notifications::run_failed(&task_state, &run).await;
                }
            }
        }
    });
}
