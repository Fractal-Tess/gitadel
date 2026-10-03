use std::time::Duration;

use chrono::{DateTime, Utc};
use sea_orm::{
    ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter, QueryOrder, QuerySelect,
    sea_query::Query,
};

use super::{RepositoryState, issues, mirrors, webhooks};
use crate::entity::{repository, repository_mirror};

const SCHEDULER_INTERVAL: Duration = Duration::from_secs(30);
const SCHEDULER_BATCH_SIZE: u64 = 16;

pub(crate) async fn serve_mirror_scheduler(state: RepositoryState) -> Result<(), anyhow::Error> {
    let mut interval = tokio::time::interval(SCHEDULER_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        interval.tick().await;
        tick(&state).await;
    }
}

/// One scheduler pass: periodic housekeeping, then every due mirror is handed
/// to its own task. Failures are logged and left for the next pass.
async fn tick(state: &RepositoryState) {
    if let Err(error) = mirrors::cleanup_temporary_files(&state.repository_root).await {
        tracing::warn!(%error, "could not clean stale mirror credential files");
    }
    if let Err(error) = issues::cleanup_expired_attachments(state).await {
        tracing::warn!(%error, "could not clean expired issue attachments");
    }
    if let Err(error) = webhooks::process_pending_deliveries(state).await {
        tracing::warn!(%error, "could not process pending webhook deliveries");
    }
    let due = match due_mirrors(
        state.identity().database(),
        Utc::now(),
        SCHEDULER_BATCH_SIZE,
    )
    .await
    {
        Ok(due) => due,
        Err(error) => {
            tracing::error!(%error, "could not load scheduled repository mirrors");
            return;
        }
    };
    for mirror in due {
        let repository = match repository::Entity::find_by_id(mirror.repository_id)
            .one(state.identity().database())
            .await
        {
            Ok(Some(repository)) => repository,
            Ok(None) => continue,
            Err(error) => {
                // Transient; the mirror stays due and the next tick retries.
                tracing::error!(%error, repository_id = %mirror.repository_id, "could not load scheduled mirror repository");
                continue;
            }
        };
        let worker_state = state.clone();
        let task_state = worker_state.clone();
        task_state.spawn_task(async move {
            // A mirror that is already synchronizing is not a failure.
            if let Err(error) = mirrors::try_synchronize(&worker_state, &repository, None).await {
                tracing::warn!(
                    %error,
                    repository_id = %repository.id,
                    "scheduled repository mirror failed"
                );
            }
        });
    }
}

/// Up to `limit` scheduled mirrors whose next run is at or before `now`,
/// oldest first, skipping repositories that are being deleted.
async fn due_mirrors(
    database: &DatabaseConnection,
    now: DateTime<Utc>,
    limit: u64,
) -> Result<Vec<repository_mirror::Model>, DbErr> {
    repository_mirror::Entity::find()
        .filter(repository_mirror::Column::NextSyncAt.is_not_null())
        .filter(repository_mirror::Column::NextSyncAt.lte(now))
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
        .limit(limit)
        .all(database)
        .await
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use chrono::{Duration, Utc};
    use sea_orm::{ActiveModelTrait, EntityTrait, IntoActiveModel, Set};
    use uuid::Uuid;

    use super::{SCHEDULER_BATCH_SIZE, due_mirrors, tick};
    use crate::{
        entity::{repository, repository_mirror},
        repository::resources::test_support::{TestInstance, instance},
    };

    #[derive(Clone, Default)]
    struct Capture(Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for Capture {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Capture {
        type Writer = Self;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    /// Everything logged at WARN or above while the guard lives. Tests run
    /// on one thread, so tasks the tick spawns log here too.
    struct Logs {
        capture: Capture,
        _guard: tracing::subscriber::DefaultGuard,
    }

    fn capture_logs() -> Logs {
        let capture = Capture::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(capture.clone())
            .with_ansi(false)
            .with_max_level(tracing::Level::WARN)
            .finish();
        Logs {
            capture,
            _guard: tracing::subscriber::set_default(subscriber),
        }
    }

    impl Logs {
        fn text(&self) -> String {
            String::from_utf8(self.capture.0.lock().unwrap().clone()).unwrap()
        }
    }

    async fn mirror(
        instance: &TestInstance,
        repository: &repository::Model,
        remote_url: &str,
        next_sync_at: Option<chrono::DateTime<Utc>>,
    ) -> repository_mirror::Model {
        let now = Utc::now();
        repository_mirror::ActiveModel {
            repository_id: Set(repository.id),
            remote_url: Set(remote_url.to_owned()),
            identity_id: Set(None),
            github_owner: Set(None),
            github_repository: Set(None),
            schedule: Set(Some("0 * * * *".to_owned())),
            last_attempted_at: Set(None),
            last_synced_at: Set(None),
            last_error: Set(None),
            metadata_last_synced_at: Set(None),
            metadata_error: Set(None),
            next_sync_at: Set(next_sync_at),
            created_at: Set(now),
            updated_at: Set(now),
        }
        .insert(&instance.database)
        .await
        .unwrap()
    }

    async fn stored(instance: &TestInstance, repository_id: Uuid) -> repository_mirror::Model {
        repository_mirror::Entity::find_by_id(repository_id)
            .one(&instance.database)
            .await
            .unwrap()
            .unwrap()
    }

    const UPSTREAM: &str = "https://github.com/octocat/hello-world.git";

    #[tokio::test]
    async fn due_mirrors_are_the_overdue_live_ones_oldest_first_up_to_the_batch_size() {
        let instance = instance().await;
        let now = Utc::now();
        let older = instance.repository("older").await;
        mirror(
            &instance,
            &older,
            UPSTREAM,
            Some(now - Duration::minutes(10)),
        )
        .await;
        let newer = instance.repository("newer").await;
        mirror(
            &instance,
            &newer,
            UPSTREAM,
            Some(now - Duration::minutes(5)),
        )
        .await;
        let paused = instance.repository("paused").await;
        mirror(&instance, &paused, UPSTREAM, None).await;
        let future = instance.repository("future").await;
        mirror(
            &instance,
            &future,
            UPSTREAM,
            Some(now + Duration::minutes(10)),
        )
        .await;
        let deleted = instance.repository("deleted").await;
        mirror(
            &instance,
            &deleted,
            UPSTREAM,
            Some(now - Duration::hours(1)),
        )
        .await;
        let mut active = deleted.clone().into_active_model();
        active.deleted_at = Set(Some(now));
        active.update(&instance.database).await.unwrap();

        let ids = |mirrors: Vec<repository_mirror::Model>| {
            mirrors
                .into_iter()
                .map(|mirror| mirror.repository_id)
                .collect::<Vec<_>>()
        };
        let due = due_mirrors(&instance.database, now, SCHEDULER_BATCH_SIZE)
            .await
            .unwrap();
        assert_eq!(ids(due), [older.id, newer.id]);
        let due = due_mirrors(&instance.database, now, 1).await.unwrap();
        assert_eq!(ids(due), [older.id]);
        // Exactly due counts as due.
        let exact = due_mirrors(&instance.database, now + Duration::minutes(10), 10)
            .await
            .unwrap();
        assert_eq!(ids(exact), [older.id, newer.id, future.id]);

        for index in 0..SCHEDULER_BATCH_SIZE + 4 {
            let repository = instance.repository(&format!("bulk-{index}")).await;
            mirror(
                &instance,
                &repository,
                UPSTREAM,
                Some(now - Duration::seconds(i64::try_from(index).unwrap())),
            )
            .await;
        }
        let batch = due_mirrors(&instance.database, now, SCHEDULER_BATCH_SIZE)
            .await
            .unwrap();
        assert_eq!(batch.len(), 16);
        assert!(
            batch
                .windows(2)
                .all(|pair| pair[0].next_sync_at <= pair[1].next_sync_at)
        );
    }

    #[tokio::test]
    async fn a_tick_logs_a_database_failure_and_returns() {
        let instance = instance().await;
        let repository = instance.repository("project").await;
        mirror(
            &instance,
            &repository,
            UPSTREAM,
            Some(Utc::now() - Duration::minutes(1)),
        )
        .await;
        let logs = capture_logs();
        instance.database.clone().close().await.unwrap();

        tick(&instance.state).await;
        instance.state.close_tasks();
        instance.state.wait_for_tasks().await;

        let text = logs.text();
        assert!(
            text.contains("could not load scheduled repository mirrors"),
            "{text}"
        );
        assert!(
            !text.contains("scheduled repository mirror failed"),
            "{text}"
        );
    }

    #[tokio::test]
    async fn a_tick_skips_busy_mirrors_quietly_and_reports_failing_ones() {
        let instance = instance().await;
        let due = Utc::now() - Duration::minutes(1);
        let busy = instance.repository("busy").await;
        let busy_mirror = mirror(&instance, &busy, UPSTREAM, Some(due)).await;
        let failing = instance.repository("failing").await;
        // An insecure remote fails validation before any network access.
        mirror(
            &instance,
            &failing,
            "http://insecure.example/repository.git",
            Some(due),
        )
        .await;
        let claim = instance.state.mirror_syncing.claim(busy.id).unwrap();
        let logs = capture_logs();
        let started = Utc::now();

        tick(&instance.state).await;
        instance.state.close_tasks();
        instance.state.wait_for_tasks().await;

        let text = logs.text();
        assert_eq!(
            text.matches("scheduled repository mirror failed").count(),
            1,
            "{text}"
        );
        assert!(text.contains(&failing.id.to_string()), "{text}");
        assert!(!text.contains(&busy.id.to_string()), "{text}");

        let failed = stored(&instance, failing.id).await;
        assert!(failed.last_error.is_some());
        assert!(failed.last_attempted_at.unwrap() >= started);
        assert!(failed.next_sync_at.unwrap() > started);
        // The busy mirror was left exactly as it was, still due.
        let untouched = stored(&instance, busy.id).await;
        assert_eq!(untouched.last_attempted_at, None);
        assert_eq!(untouched.last_error, None);
        assert_eq!(untouched.next_sync_at, busy_mirror.next_sync_at);
        assert!(instance.state.mirror_syncing.contains(busy.id));
        drop(claim);
        assert!(!instance.state.mirror_syncing.contains(busy.id));
    }
}
