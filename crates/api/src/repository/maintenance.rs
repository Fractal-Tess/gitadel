use std::sync::LazyLock;

use tokio::sync::Mutex;

static MAINTENANCE_LOCK: LazyLock<Mutex<()>> = LazyLock::new(Mutex::default);

/// Run native automatic repository maintenance after a successful receive.
///
/// Automatic maintenance is deliberately best effort: transport success must
/// not be turned into a failed push merely because housekeeping could not run.
/// The process-wide lock complements Sley's on-disk `gc.pid` lock, which cannot
/// distinguish two tasks in this process before either has acquired it.
pub(super) async fn run(path: &std::path::Path, repository: &str) {
    let _guard = MAINTENANCE_LOCK.lock().await;
    let path = path.to_path_buf();
    let result = tokio::task::spawn_blocking(move || sley_gc::embedder::run_auto(&path)).await;
    match result {
        Ok(Ok(())) => {}
        Ok(Err(error)) => tracing::warn!(%error, %repository, "automatic Git maintenance failed"),
        Err(error) => tracing::warn!(%error, %repository, "automatic Git maintenance task failed"),
    }
}
