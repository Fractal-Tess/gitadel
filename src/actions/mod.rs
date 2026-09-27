pub(crate) mod api;
pub(crate) mod artifacts;
pub(crate) mod dispatch;
pub(crate) mod events;
pub(crate) mod logs;
pub(crate) mod matrix;
pub(crate) mod protocol;
pub(crate) mod rerun;
pub(crate) mod runners;
pub(crate) mod runs;
pub(crate) mod schedule;
pub(crate) mod secrets;
#[cfg(test)]
pub(crate) mod test_support;
pub(crate) mod tokens;
pub(crate) mod workflow;

use std::{os::unix::fs::PermissionsExt, sync::Arc, time::Duration};

use axum::Router;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use tokio_util::sync::CancellationToken;

use crate::{
    config::ActionsSettings, entity::action_runner, filesystem::create_private_directory_async,
    repository::RepositoryState,
};

pub(crate) const REQUIRED_RUNNER_VERSION: &str = "13.0.0";

#[derive(Clone)]
pub(crate) struct ActionsState {
    repository: RepositoryState,
    settings: Arc<ActionsSettings>,
    secret_cipher: Arc<secrets::SecretCipher>,
    shutdown: CancellationToken,
}

impl ActionsState {
    pub(crate) fn new(
        repository: RepositoryState,
        settings: ActionsSettings,
        secret_cipher: secrets::SecretCipher,
    ) -> Self {
        Self {
            repository,
            settings: Arc::new(settings),
            secret_cipher: Arc::new(secret_cipher),
            shutdown: CancellationToken::new(),
        }
    }

    pub(crate) fn secret_cipher(&self) -> &secrets::SecretCipher {
        &self.secret_cipher
    }

    pub(crate) fn repository(&self) -> &RepositoryState {
        &self.repository
    }

    pub(crate) fn settings(&self) -> &ActionsSettings {
        &self.settings
    }

    /// Origin used for every URL handed to jobs, without a trailing slash.
    ///
    /// Runners and job containers often reach Gitadel over a private network,
    /// so `actions.internal_url` takes precedence over the public URL.
    pub(crate) fn job_origin(&self) -> String {
        self.settings
            .internal_url
            .as_deref()
            .unwrap_or_else(|| self.repository.public_url().as_str())
            .trim_end_matches('/')
            .to_owned()
    }

    pub(crate) fn protocol_router(&self) -> Router {
        protocol::router(self.settings.max_log_request_bytes).with_state(self.clone())
    }

    pub(crate) fn artifact_router(&self) -> Router {
        artifacts::router().with_state(self.clone())
    }

    pub(crate) fn api_router(&self) -> Router {
        api::router()
            .merge(secrets::router())
            .merge(dispatch::router())
            .with_state(self.clone())
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
    create_private_directory_async(parent).await?;
    tokio::fs::write(&settings.registration_token_file, raw).await?;
    tokio::fs::set_permissions(
        &settings.registration_token_file,
        std::fs::Permissions::from_mode(0o600),
    )
    .await?;
    Ok(())
}

/// Scheduler ticks between full schedule scans (15 s x 20 = 5 minutes).
const SCHEDULE_SCAN_TICKS: u32 = 20;

pub(crate) async fn serve_actions_scheduler(state: ActionsState) -> anyhow::Result<()> {
    let mut interval = tokio::time::interval(Duration::from_secs(15));
    let mut ticks = 0u32;
    loop {
        tokio::select! {
            () = state.shutdown.cancelled() => return Ok(()),
            _ = interval.tick() => {
                runs::maintain(&state).await?;
                if let Err(error) = artifacts::cleanup(&state).await {
                    tracing::warn!(%error, "actions artifact cleanup failed");
                }
                // Default branches can also move without a push (settings,
                // mirrors, restores), so rescan periodically.
                if ticks.is_multiple_of(SCHEDULE_SCAN_TICKS)
                    && let Err(error) = schedule::sync_all(&state).await
                {
                    tracing::warn!(%error, "actions schedule scan failed");
                }
                ticks = ticks.wrapping_add(1);
                if let Err(error) = schedule::fire_due(&state).await {
                    tracing::warn!(%error, "actions schedule dispatch failed");
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
