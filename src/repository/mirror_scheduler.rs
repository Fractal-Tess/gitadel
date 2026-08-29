use std::time::Duration;

use chrono::Utc;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder, QuerySelect, sea_query::Query};

use super::{RepositoryState, mirrors};
use crate::entity::{repository, repository_mirror};

const SCHEDULER_INTERVAL: Duration = Duration::from_secs(30);
const SCHEDULER_BATCH_SIZE: u64 = 16;

pub(crate) async fn serve_mirror_scheduler(state: RepositoryState) -> Result<(), anyhow::Error> {
    let mut interval = tokio::time::interval(SCHEDULER_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        interval.tick().await;
        if let Err(error) = mirrors::cleanup_temporary_files(&state.repository_root).await {
            tracing::warn!(%error, "could not clean stale mirror credential files");
        }
        let due = repository_mirror::Entity::find()
            .filter(repository_mirror::Column::NextSyncAt.is_not_null())
            .filter(repository_mirror::Column::NextSyncAt.lte(Utc::now()))
            .filter(
                repository_mirror::Column::RepositoryId.in_subquery(
                    Query::select()
                        .column(repository::Column::Id)
                        .from(repository::Entity)
                        .and_where(repository::Column::DeletedAt.is_null())
                        .to_owned(),
                ),
            )
            .order_by_asc(repository_mirror::Column::NextSyncAt)
            .limit(SCHEDULER_BATCH_SIZE)
            .all(state.identity().database())
            .await;
        let due = match due {
            Ok(due) => due,
            Err(error) => {
                tracing::error!(%error, "could not load scheduled repository mirrors");
                continue;
            }
        };
        for mirror in due {
            let Some(repository) = repository::Entity::find_by_id(mirror.repository_id)
                .one(state.identity().database())
                .await
                .inspect_err(|error| {
                    tracing::error!(%error, repository_id = %mirror.repository_id, "could not load scheduled mirror repository");
                })?
            else {
                continue;
            };
            let worker_state = state.clone();
            let task_state = worker_state.clone();
            task_state.spawn_task(async move {
                if let Err(error) = mirrors::synchronize(&worker_state, &repository, None).await
                    && error.to_string() != "Mirror synchronization is already running."
                {
                    tracing::warn!(
                        %error,
                        repository_id = %repository.id,
                        "scheduled repository mirror failed"
                    );
                }
            });
        }
    }
}
