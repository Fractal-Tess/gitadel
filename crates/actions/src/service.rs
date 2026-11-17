//! Owned Actions handles and explicit runtime entry points.
use gitadel_config::ActionsSettings;
use gitadel_repositories::content::RepositoryContent;
use gitadel_secrets::SecretCipher;
use sea_orm::DatabaseConnection;
use std::{path::PathBuf, sync::Arc};
use uuid::Uuid;

/// Announce committed run transitions using the server's repository coordinator.
/// Implementations enqueue work and do not block the triggering operation.
pub trait RunEffects: Send + Sync {
    fn requested(&self, run_id: Uuid);
    fn completed(&self, run_id: Uuid);
    fn requested_and_completed(&self, run_id: Uuid);
}
#[derive(Clone)]
pub struct ActionsService {
    pub(crate) database: DatabaseConnection,
    pub(crate) content: RepositoryContent,
    pub(crate) settings: Arc<ActionsSettings>,
    cipher: Arc<SecretCipher>,
    artifact_root: Arc<PathBuf>,
    pub(crate) effects: Arc<dyn RunEffects>,
}
impl ActionsService {
    /// Construct without recovery, scans, timers, or spawned workers.
    pub fn new(
        database: DatabaseConnection,
        content: RepositoryContent,
        settings: ActionsSettings,
        cipher: SecretCipher,
        artifact_root: PathBuf,
        effects: Arc<dyn RunEffects>,
    ) -> Self {
        Self {
            database,
            content,
            settings: Arc::new(settings),
            cipher: Arc::new(cipher),
            artifact_root: Arc::new(artifact_root),
            effects,
        }
    }
    pub fn settings(&self) -> &ActionsSettings {
        &self.settings
    }
    pub fn secret_cipher(&self) -> &SecretCipher {
        &self.cipher
    }
    /// One finite maintenance pass; the server owns cadence and cancellation.
    pub async fn maintain(&self) -> Result<(), sea_orm::DbErr> {
        crate::runs::maintain(&self.database, |run_id| self.effects.completed(run_id)).await
    }
    pub async fn cleanup_artifacts(&self) -> Result<(), sea_orm::DbErr> {
        crate::artifacts::cleanup(
            &self.database,
            &self.artifact_root,
            self.settings.artifact_grant_lifetime_seconds,
        )
        .await
    }
}

use gitadel_core::filesystem::create_private_directory_async;
use gitadel_db::entity::action_runner;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use std::os::unix::fs::PermissionsExt;
impl ActionsService {
    pub async fn bootstrap_system_runner(&self) -> anyhow::Result<()> {
        let Some(settings) = self.settings().system_runner.as_ref() else {
            return Ok(());
        };
        let name = crate::runners::validate_registration(&settings.name, &settings.labels)
            .map_err(anyhow::Error::msg)?;
        let database = &self.database;
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
            crate::tokens::issue_registration(database, None, None, name, settings.labels.clone())
                .await?;
        let parent = settings.registration_token_file.parent().ok_or_else(|| {
            anyhow::anyhow!("system runner token file requires a parent directory")
        })?;
        create_private_directory_async(parent).await?;
        tokio::fs::write(&settings.registration_token_file, raw).await?;
        tokio::fs::set_permissions(
            &settings.registration_token_file,
            std::fs::Permissions::from_mode(0o600),
        )
        .await?;
        Ok(())
    }
}
