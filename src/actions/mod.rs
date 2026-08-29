pub(crate) mod api;
pub(crate) mod artifacts;
pub(crate) mod logs;
pub(crate) mod protocol;
pub(crate) mod runners;
pub(crate) mod runs;
pub(crate) mod tokens;
pub(crate) mod workflow;

use std::{sync::Arc, time::Duration};

use axum::Router;
use tokio_util::sync::CancellationToken;

use crate::{config::ActionsSettings, repository::RepositoryState};

pub(crate) const REQUIRED_RUNNER_VERSION: &str = "13.0.0";

#[derive(Clone)]
pub(crate) struct ActionsState {
    repository: RepositoryState,
    settings: Arc<ActionsSettings>,
    shutdown: CancellationToken,
}

impl ActionsState {
    pub(crate) fn new(repository: RepositoryState, settings: ActionsSettings) -> Self {
        Self {
            repository,
            settings: Arc::new(settings),
            shutdown: CancellationToken::new(),
        }
    }

    pub(crate) fn repository(&self) -> &RepositoryState {
        &self.repository
    }

    pub(crate) fn settings(&self) -> &ActionsSettings {
        &self.settings
    }

    pub(crate) fn protocol_router(&self) -> Router {
        protocol::router(self.settings.max_log_request_bytes).with_state(self.clone())
    }

    pub(crate) fn artifact_router(&self) -> Router {
        artifacts::router().with_state(self.clone())
    }

    pub(crate) fn api_router(&self) -> Router {
        api::router().with_state(self.clone())
    }

    pub(crate) fn cancel(&self) {
        self.shutdown.cancel();
    }
}

pub(crate) async fn serve_actions_scheduler(state: ActionsState) -> anyhow::Result<()> {
    let mut interval = tokio::time::interval(Duration::from_secs(15));
    loop {
        tokio::select! {
            () = state.shutdown.cancelled() => return Ok(()),
            _ = interval.tick() => {
                runs::maintain(&state).await?;
                if let Err(error) = artifacts::cleanup(&state).await {
                    tracing::warn!(%error, "actions artifact cleanup failed");
                }
            }
        }
    }
}

pub(crate) mod proto {
    pub(crate) mod ping {
        include!(concat!(env!("OUT_DIR"), "/ping.v1.rs"));
        include!(concat!(env!("OUT_DIR"), "/ping.v1.serde.rs"));
    }

    pub(crate) mod runner {
        include!(concat!(env!("OUT_DIR"), "/runner.v1.rs"));
        include!(concat!(env!("OUT_DIR"), "/runner.v1.serde.rs"));
    }

    pub(crate) mod artifact {
        include!(concat!(
            env!("OUT_DIR"),
            "/github.actions.results.api.v1.rs"
        ));
        include!(concat!(
            env!("OUT_DIR"),
            "/github.actions.results.api.v1.serde.rs"
        ));
    }
}
