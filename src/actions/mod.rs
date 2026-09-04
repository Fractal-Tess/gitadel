pub(crate) mod api;
pub(crate) mod artifacts;
pub(crate) mod logs;
pub(crate) mod protocol;
pub(crate) mod runners;
pub(crate) mod runs;
pub(crate) mod tokens;
pub(crate) mod workflow;

use std::{os::unix::fs::PermissionsExt, sync::Arc, time::Duration};

use axum::Router;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use tokio_util::sync::CancellationToken;

use crate::{config::ActionsSettings, entity::action_runner, repository::RepositoryState};

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

pub(crate) async fn bootstrap_system_runner(state: &ActionsState) -> anyhow::Result<()> {
    let Some(settings) = state.settings.system_runner.as_ref() else {
        return Ok(());
    };
    let name = runners::validate_registration(&settings.name, &settings.labels)
        .map_err(anyhow::Error::msg)?;
    let database = state.repository.identity().database();
    let registered = action_runner::Entity::find()
        .filter(action_runner::Column::Namespace.is_null())
        .filter(action_runner::Column::Name.eq(&name))
        .filter(action_runner::Column::DisabledAt.is_null())
        .filter(action_runner::Column::DeletedAt.is_null())
        .one(database)
        .await?
        .is_some();
    if registered {
        if tokio::fs::try_exists(&settings.registration_token_file).await? {
            tokio::fs::remove_file(&settings.registration_token_file).await?;
        }
        return Ok(());
    }

    let raw =
        tokens::issue_registration(database, None, None, name, settings.labels.clone()).await?;
    let parent = settings
        .registration_token_file
        .parent()
        .ok_or_else(|| anyhow::anyhow!("system runner token file requires a parent directory"))?;
    tokio::fs::create_dir_all(parent).await?;
    tokio::fs::write(&settings.registration_token_file, raw).await?;
    tokio::fs::set_permissions(
        &settings.registration_token_file,
        std::fs::Permissions::from_mode(0o600),
    )
    .await?;
    Ok(())
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
