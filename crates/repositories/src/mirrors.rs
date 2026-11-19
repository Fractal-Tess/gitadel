//! Persistent mirror scheduling.

use chrono::{DateTime, Utc};
use gitadel_core::schedule;
use gitadel_db::entity::{repository, repository_mirror};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, DbErr, EntityTrait, IntoActiveModel,
    QueryFilter, QueryOrder, QuerySelect, Set, TransactionTrait, sea_query::Query,
};
use std::{collections::HashSet, path::Path, sync::Arc, time::Duration};
use uuid::Uuid;

/// Up to `limit` scheduled mirrors whose next run is at or before `now`,
/// oldest first, skipping repositories that are being deleted.
pub async fn due_mirrors(
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

const TEMPORARY_FILE_MAX_AGE: Duration = Duration::from_secs(12 * 60);
const LFS_IMPORT_STAGING_PREFIX: &str = ".gitadel-import-";
pub const MAX_ERROR_LENGTH: usize = 2_048;

/// Repositories with a mirror operation in flight.
#[derive(Clone, Default)]
pub struct MirrorBusySet(Arc<std::sync::Mutex<HashSet<Uuid>>>);

impl MirrorBusySet {
    fn ids(&self) -> std::sync::MutexGuard<'_, HashSet<Uuid>> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn contains(&self, repository_id: Uuid) -> bool {
        self.ids().contains(&repository_id)
    }

    /// Marks the repository busy until the returned claim is dropped, which
    /// also covers a handler future cancelled in the middle of the operation.
    pub fn claim(&self, repository_id: Uuid) -> Option<MirrorBusy> {
        self.ids().insert(repository_id).then(|| MirrorBusy {
            set: self.clone(),
            repository_id,
        })
    }
}

pub struct MirrorBusy {
    set: MirrorBusySet,
    repository_id: Uuid,
}

impl Drop for MirrorBusy {
    fn drop(&mut self) {
        self.set.ids().remove(&self.repository_id);
    }
}

pub async fn cleanup_temporary_files(directory: &Path) -> Result<(), std::io::Error> {
    let mut entries = tokio::fs::read_dir(directory).await?;
    while let Some(entry) = entries.next_entry().await? {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let is_mirror_credential = name.starts_with(".gitadel-mirror-")
            && (name.ends_with(".key") || name.ends_with(".known-hosts"));
        let metadata = entry.metadata().await?;
        let is_stale = metadata
            .modified()?
            .elapsed()
            .is_ok_and(|age| age > TEMPORARY_FILE_MAX_AGE);
        if is_mirror_credential && metadata.is_file() && is_stale {
            tokio::fs::remove_file(entry.path()).await?;
        }
    }
    Ok(())
}

pub async fn cleanup_lfs_staging(directory: &Path) -> Result<(), std::io::Error> {
    let mut entries = tokio::fs::read_dir(directory).await?;
    while let Some(entry) = entries.next_entry().await? {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !name.starts_with(LFS_IMPORT_STAGING_PREFIX) {
            continue;
        }
        let metadata = entry.metadata().await?;
        let is_stale = metadata
            .modified()?
            .elapsed()
            .is_ok_and(|age| age > TEMPORARY_FILE_MAX_AGE);
        if metadata.is_dir() && is_stale {
            tokio::fs::remove_dir_all(entry.path()).await?;
        }
    }
    Ok(())
}

/// The mirror's next scheduled run after `after`. A stored expression that no
/// longer parses yields no next run rather than leaving the mirror due on
/// every scheduler tick.
pub fn next_sync_after(
    mirror: &repository_mirror::Model,
    after: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
    let expression = mirror.schedule.as_deref()?;
    match schedule::next_occurrence(expression, after) {
        Ok(next) => Some(next),
        Err(error) => {
            tracing::error!(
                %error,
                repository_id = %mirror.repository_id,
                "mirror schedule is invalid; scheduled synchronization is paused"
            );
            None
        }
    }
}

pub async fn record_preflight_failure(
    database: &DatabaseConnection,
    mirror: repository_mirror::Model,
    attempted_at: DateTime<Utc>,
    error: &str,
) {
    let repository_id = mirror.repository_id;
    let now = Utc::now();
    let next_sync_at = next_sync_after(&mirror, now);
    let mut active = mirror.into_active_model();
    active.last_attempted_at = Set(Some(attempted_at));
    active.next_sync_at = Set(next_sync_at);
    active.updated_at = Set(now);
    active.last_error = Set(Some(truncate(error, MAX_ERROR_LENGTH)));
    if let Err(update_error) = active.update(database).await {
        tracing::error!(
            error = %update_error,
            %repository_id,
            "could not record mirror synchronization failure"
        );
    }
}

pub fn truncate(value: &str, maximum: usize) -> String {
    if value.len() <= maximum {
        return value.to_owned();
    }
    let mut end = maximum;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}

pub struct MirrorCompletion {
    pub attempted_at: DateTime<Utc>,
    pub git: Result<(), String>,
    pub metadata: Option<Result<(), String>>,
}

/// Persist the outcome and next run together with the successful Git update.
pub async fn finish_synchronization(
    database: &DatabaseConnection,
    repository: &repository::Model,
    mirror: repository_mirror::Model,
    completion: MirrorCompletion,
    actor_user_id: Option<Uuid>,
    audit: gitadel_core::audit::AuditContext,
) -> Result<repository_mirror::Model, DbErr> {
    let completed_at = Utc::now();
    let next_sync_at = next_sync_after(&mirror, completed_at);
    let mut active = mirror.into_active_model();
    active.last_attempted_at = Set(Some(completion.attempted_at));
    active.next_sync_at = Set(next_sync_at);
    active.updated_at = Set(completed_at);
    match &completion.git {
        Ok(()) => {
            active.last_synced_at = Set(Some(completed_at));
            active.last_error = Set(None);
        }
        Err(error) => {
            active.last_error = Set(Some(truncate(error, MAX_ERROR_LENGTH)));
        }
    }
    if let Some(metadata_result) = &completion.metadata {
        match metadata_result {
            Ok(()) => {
                active.metadata_last_synced_at = Set(Some(completed_at));
                active.metadata_error = Set(None);
            }
            Err(error) => {
                active.metadata_error = Set(Some(truncate(error, MAX_ERROR_LENGTH)));
            }
        }
    }

    let transaction = database.begin().await?;
    let mirror = active.update(&transaction).await?;
    if completion.git.is_ok() {
        repository::Entity::update_many()
            .col_expr(
                repository::Column::UpdatedAt,
                sea_orm::sea_query::Expr::value(completed_at),
            )
            .filter(repository::Column::Id.eq(repository.id))
            .exec(&transaction)
            .await?;
        gitadel_db::audit::record(
            &transaction,
            actor_user_id,
            "repository.mirror.sync",
            Some(format!("{}/{}", repository.namespace, repository.name)),
            audit,
        )
        .await?;
    }
    transaction.commit().await?;
    Ok(mirror)
}
