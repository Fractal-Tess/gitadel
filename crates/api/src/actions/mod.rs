pub(crate) mod api;
pub(crate) mod artifacts;
pub(crate) mod dispatch;
pub(crate) mod events;
pub(crate) mod logs;
pub(crate) mod protocol;
pub(crate) mod rerun;
pub(crate) mod runners;
pub(crate) mod runs;
pub(crate) mod secrets;
#[cfg(test)]
pub(crate) mod test_support;
pub(crate) mod tokens;
pub(crate) mod workflow;

use std::sync::Arc;

use axum::Router;
use tokio_util::sync::CancellationToken;

use crate::{config::ActionsSettings, repository::RepositoryState};

pub(crate) use gitadel_actions::REQUIRED_RUNNER_VERSION;

#[derive(Clone)]
pub struct ActionsState {
    repository: RepositoryState,
    service: Arc<gitadel_actions::ActionsService>,
    shutdown: CancellationToken,
}

impl ActionsState {
    pub fn new(
        repository: RepositoryState,
        settings: ActionsSettings,
        secret_cipher: secrets::SecretCipher,
    ) -> Self {
        let service = gitadel_actions::ActionsService::new(
            repository.identity().database().clone(),
            gitadel_git::content::RepositoryContent::new(
                repository.repository_root().to_path_buf(),
            ),
            settings,
            secret_cipher,
            repository.actions_artifact_root().to_path_buf(),
            Arc::new(LegacyRunEffects(repository.clone())),
        );
        Self::from_service(repository, service)
    }
    pub fn from_service(
        repository: RepositoryState,
        service: gitadel_actions::ActionsService,
    ) -> Self {
        Self {
            repository,
            service: Arc::new(service),
            shutdown: CancellationToken::new(),
        }
    }
    /// Use the same server-owned shutdown signal for worker ticks and job long polls.
    pub fn with_shutdown(mut self, shutdown: CancellationToken) -> Self {
        self.shutdown = shutdown;
        self
    }
    pub fn service(&self) -> &gitadel_actions::ActionsService {
        &self.service
    }
    pub(crate) fn secret_cipher(&self) -> &secrets::SecretCipher {
        self.service.secret_cipher()
    }

    pub(crate) fn repository(&self) -> &RepositoryState {
        &self.repository
    }

    pub(crate) fn settings(&self) -> &ActionsSettings {
        self.service.settings()
    }

    /// Origin used for every URL handed to jobs, without a trailing slash.
    ///
    /// Runners and job containers often reach Gitadel over a private network,
    /// so `actions.internal_url` takes precedence over the public URL.
    pub(crate) fn job_origin(&self) -> String {
        self.settings()
            .internal_url
            .as_deref()
            .unwrap_or_else(|| self.repository.public_url().as_str())
            .trim_end_matches('/')
            .to_owned()
    }

    pub(crate) fn protocol_router(&self) -> Router {
        protocol::router(self.settings().max_log_request_bytes).with_state(self.clone())
    }

    pub(crate) fn artifact_router(&self) -> Router {
        artifacts::router().with_state(self.clone())
    }

    pub(crate) fn api_router(&self) -> Router {
        contract_router().with_state(self.clone()).into()
    }
}

pub(crate) fn contract_router() -> utoipa_axum::router::OpenApiRouter<ActionsState> {
    api::router()
        .merge(secrets::router())
        .merge(dispatch::router())
}

pub async fn bootstrap_system_runner(state: &ActionsState) -> anyhow::Result<()> {
    state.service().bootstrap_system_runner().await
}

// pbjson-build emits formatting borrows that are valid but trip current Clippy.
#[allow(clippy::useless_borrows_in_formatting)]
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

struct LegacyRunEffects(RepositoryState);
impl gitadel_actions::RunEffects for LegacyRunEffects {
    fn requested(&self, id: uuid::Uuid) {
        events::spawn_run_requested(&self.0, id);
    }
    fn completed(&self, id: uuid::Uuid) {
        events::spawn_run_completed(&self.0, id);
    }
    fn requested_and_completed(&self, id: uuid::Uuid) {
        events::spawn_run_requested_and_completed(&self.0, id);
    }
}
