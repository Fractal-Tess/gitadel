//! Application worker cadence and cancellation.

mod actions;
pub(super) use actions::serve_actions_scheduler;

pub(super) async fn serve_mirror_scheduler(
    state: gitadel_api::RepositoryState,
) -> anyhow::Result<()> {
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        interval.tick().await;
        gitadel_api::tick_repository_mirrors(&state).await;
    }
}
