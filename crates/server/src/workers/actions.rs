//! Application worker cadence and cancellation.

/// Scheduler ticks between full schedule scans (15 s x 20 = 5 minutes).
const SCHEDULE_SCAN_TICKS: u32 = 20;

pub(crate) async fn serve_actions_scheduler(
    service: gitadel_actions::ActionsService,
    shutdown: tokio_util::sync::CancellationToken,
) -> anyhow::Result<()> {
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(15));
    let mut ticks = 0u32;
    loop {
        tokio::select! {
            () = shutdown.cancelled() => return Ok(()),
            _ = interval.tick() => {
                // A transient database error must not end this future: the
                // server treats that as fatal. The next tick retries.
                if let Err(error) = service.maintain().await {
                    tracing::error!(%error, "actions run maintenance failed");
                }
                if let Err(error) = service.cleanup_artifacts().await {
                    tracing::warn!(%error, "actions artifact cleanup failed");
                }
                // Default branches can also move without a push (settings,
                // mirrors, restores), so rescan periodically.
                if ticks.is_multiple_of(SCHEDULE_SCAN_TICKS)
                    && let Err(error) = gitadel_actions::service_schedule::sync_all(&service).await
                {
                    tracing::warn!(%error, "actions schedule scan failed");
                }
                ticks = ticks.wrapping_add(1);
                if let Err(error) = gitadel_actions::service_schedule::fire_due(&service).await {
                    tracing::warn!(%error, "actions schedule dispatch failed");
                }
            }
        }
    }
}
