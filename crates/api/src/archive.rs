//! Connect backup scheduling to the running application's maintenance queue.

use crate::{backup_provider, identity::IdentityState};
use anyhow::{Result, bail};
use chrono::Utc;
pub use gitadel_backups::archive::*;
use uuid::Uuid;

const BACKUP_SCHEDULER_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);

pub async fn serve_backup_scheduler(state: IdentityState) -> Result<()> {
    let mut interval = tokio::time::interval(BACKUP_SCHEDULER_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        interval.tick().await;
        if let Err(error) = schedule_due_backup(&state).await {
            tracing::warn!(%error, "scheduled automatic backup could not start");
        }
    }
}

pub(super) async fn schedule_due_backup(state: &IdentityState) -> Result<()> {
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
