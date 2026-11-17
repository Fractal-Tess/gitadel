//! Kinds of stored data that share the storage target machinery.

use std::{path::PathBuf, sync::Arc};

use anyhow::Result;
use async_trait::async_trait;
use sea_orm::{DatabaseConnection, DatabaseTransaction};
use uuid::Uuid;

use super::{BlobMetadata, BlobStore, FilesystemBlobStore, ObjectKey, ObjectPrefix};
use crate::config::StorageSettings;

/// Describes one kind of content-addressed data, such as Git LFS objects or
/// container registry payloads, so a single [`DomainStorage`] can select,
/// migrate, measure, and clean its storage.
///
/// Object keys are canonical: they are identical on the domain's local root
/// and on every storage target, which is what lets migrations copy keys
/// verbatim.
///
/// [`DomainStorage`]: super::manager::DomainStorage
#[async_trait]
pub trait StorageDomain: Send + Sync + 'static {
    /// Stable identifier persisted in `storage_domain_state.domain` and
    /// `storage_migrations.domain`.
    fn name(&self) -> &'static str;

    /// Human-readable name used in log lines and error messages.
    fn label(&self) -> &'static str;

    /// Prefix every canonical key of the domain lives under. Listing it on a
    /// shared target may also return other domains' keys, which
    /// [`Self::repository_storage_key`] rejects.
    fn key_prefix(&self) -> &'static str;

    /// The repository storage key that owns `key`, or `None` when the key does
    /// not belong to this domain.
    fn repository_storage_key(&self, key: &ObjectKey) -> Option<Uuid>;

    /// Prefix of every object one repository owns in this domain.
    fn repository_prefix(&self, storage_key: Uuid) -> Result<ObjectPrefix>;

    /// Where the domain stores objects when no storage target is selected.
    fn local_root(&self, settings: &StorageSettings) -> PathBuf;

    /// Opens the domain's local storage rooted at `root`, which is
    /// [`Self::local_root`] at runtime and a staging directory in backups.
    async fn open_local(&self, root: PathBuf) -> Result<Arc<dyn BlobStore>> {
        Ok(Arc::new(FilesystemBlobStore::new(root).await?))
    }

    /// Runs after a migration copied and verified `object`, before cutover.
    async fn object_copied(
        &self,
        _database: &DatabaseConnection,
        _object: &BlobMetadata,
        _source_target_id: Option<Uuid>,
    ) -> Result<()> {
        Ok(())
    }

    /// Runs inside the cutover transaction that selects `destination`.
    async fn cutover(
        &self,
        _transaction: &DatabaseTransaction,
        _source: Option<Uuid>,
        _destination: Option<Uuid>,
    ) -> Result<()> {
        Ok(())
    }
}
