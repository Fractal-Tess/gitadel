//! Compatibility adapters for server-owned scheduler supervision.
use super::ActionsState;
pub(crate) async fn sync_all(state: &ActionsState) -> anyhow::Result<()> {
    gitadel_actions::service_schedule::sync_all(state.service()).await
}
pub(crate) async fn fire_due(state: &ActionsState) -> anyhow::Result<()> {
    gitadel_actions::service_schedule::fire_due(state.service()).await
}
