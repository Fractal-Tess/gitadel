//! Claim due schedules and reserve the application maintenance queue.

use crate::archive::*;
use crate::{backup_provider, service::BackupRuntime};
use anyhow::{Result, bail};
use chrono::Utc;
use gitadel_core::audit::AuditContext;
use std::sync::Arc;
use uuid::Uuid;

const BACKUP_SCHEDULER_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);

pub async fn serve_backup_scheduler(state: Arc<BackupRuntime>) -> Result<()> {
    let mut interval = tokio::time::interval(BACKUP_SCHEDULER_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        interval.tick().await;
        if let Err(error) = schedule_due_backup(&state).await {
            tracing::warn!(%error, "scheduled automatic backup could not start");
        }
    }
}

pub async fn schedule_due_backup(runtime: &BackupRuntime) -> Result<()> {
    let state = runtime.service(AuditContext::default());
    let Some(claimed) = claim_due_backup_schedule(state.database(), Utc::now()).await? else {
        return Ok(());
    };
    let settings = state
        .runtime_settings()
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    let Some(provider) = backup_provider::load_with_storage(
        state.database(),
        settings.backup.s3.as_ref(),
        &settings.storage.lfs_root,
        claimed.provider_id,
    )
    .await?
    else {
        tracing::warn!(
            provider_id = %claimed.provider_id,
            schedule = %claimed.schedule,
            "automatic backup is due but its provider does not exist"
        );
        return Ok(());
    };
    let backup_name = "scheduled automatic backup";
    let key = new_backup_key(&provider.config, Some(backup_name))?;
    let operation_id = Uuid::new_v4();
    if let Err(error) = state
        .schedule_maintenance(MaintenanceAction::Create {
            operation_id,
            key: key.clone(),
            name: Some(backup_name.to_owned()),
            provider: provider.clone(),
        })
        .await
    {
        restore_claimed_backup_schedule(state.database(), &claimed).await?;
        bail!("could not reserve maintenance: {error}");
    }
    state
        .audit(None, "backup.create.automatic", Some(key.clone()))
        .await
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    tracing::info!(
        %operation_id,
        provider_id = %provider.id,
        %key,
        schedule = %claimed.schedule,
        next_backup_at = %claimed.next_backup_at,
        "scheduled automatic backup"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backup_provider::{
        BackupProvider, BackupProviderConfig, BackupProviderSource, FilesystemSettings,
    };
    use gitadel_config::Settings;
    use gitadel_db::entity::{audit_event, backup_provider_schedule};
    use sea_orm::{ActiveModelTrait, EntityTrait, PaginatorTrait, Set};

    async fn due_schedule() -> (sea_orm::DatabaseConnection, Uuid, chrono::DateTime<Utc>) {
        let database = gitadel_db::test_support::database().await;
        let now = Utc::now();
        let provider = BackupProvider {
            id: Uuid::new_v4(),
            name: "Scheduled fixture".into(),
            config: BackupProviderConfig::Filesystem(FilesystemSettings {
                path: std::env::temp_dir().join(format!("gitadel-scheduler-{}", Uuid::new_v4())),
            }),
            source: BackupProviderSource::Stored,
            created_at: now,
            updated_at: now,
        };
        backup_provider::save(&database, &provider).await.unwrap();
        let due_at = now - chrono::Duration::minutes(1);
        backup_provider_schedule::ActiveModel {
            provider_id: Set(provider.id),
            schedule: Set("*/15 * * * *".into()),
            next_backup_at: Set(due_at),
            updated_at: Set(now),
        }
        .insert(&database)
        .await
        .unwrap();
        (database, provider.id, due_at)
    }

    #[tokio::test]
    async fn due_schedule_enqueues_one_bound_backup_and_audits_it() {
        let (database, provider_id, _) = due_schedule().await;
        let (sender, mut receiver) = tokio::sync::mpsc::channel(1);
        let runtime = BackupRuntime::new(
            database.clone(),
            Some(Arc::new(Settings::default())),
            Some(sender),
        );
        schedule_due_backup(&runtime).await.unwrap();
        schedule_due_backup(&runtime).await.unwrap();
        let action = tokio::time::timeout(std::time::Duration::from_secs(5), receiver.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(
            matches!(action, MaintenanceAction::Create { provider, name, .. }
            if provider.id == provider_id && name.as_deref() == Some("scheduled automatic backup"))
        );
        assert!(receiver.try_recv().is_err());
        let events = audit_event::Entity::find().all(&database).await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].action, "backup.create.automatic");
    }

    #[tokio::test]
    async fn unavailable_maintenance_restores_the_claim_for_retry() {
        let (database, provider_id, due_at) = due_schedule().await;
        let runtime =
            BackupRuntime::new(database.clone(), Some(Arc::new(Settings::default())), None);
        assert!(schedule_due_backup(&runtime).await.is_err());
        let schedule = load_backup_schedule(&database, provider_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(schedule.next_backup_at, due_at);
        assert_eq!(
            audit_event::Entity::find().count(&database).await.unwrap(),
            0
        );
    }
}
