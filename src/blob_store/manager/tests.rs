//! Every scenario runs once per storage domain, so LFS and the registry are
//! held to the same migration guarantees.

use std::path::PathBuf;

use sha2::{Digest as _, Sha256};

use super::*;
use crate::{
    blob_store::{
        BlobDigest, FilesystemBlobStore, ObjectKey, lfs_object_key,
        targets::{self, StorageTargetConfiguration},
    },
    config::Settings,
    database,
    entity::lfs_object,
    identity,
    registry::storage::RegistryDomain,
    storage::LfsDomain,
};

fn domains() -> [Arc<dyn StorageDomain>; 2] {
    [Arc::new(LfsDomain), Arc::new(RegistryDomain)]
}

struct Fixture {
    root: PathBuf,
    settings: Settings,
    database: DatabaseConnection,
    storage_key: Uuid,
}

impl Fixture {
    async fn new() -> Self {
        let root = std::env::temp_dir().join(format!("gitadel-domain-storage-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(&root).await.unwrap();
        let mut settings = Settings::default();
        settings.database.url = format!("sqlite://{}?mode=rwc", root.join("gitadel.db").display());
        settings.storage.repository_root = root.join("repositories");
        settings.storage.lfs_root = root.join("lfs");
        settings.storage.registry_root = root.join("registry");
        settings.storage.actions_artifact_root = root.join("actions");
        let database = database::connect_and_migrate(&settings.database)
            .await
            .unwrap();
        let user = identity::bootstrap_admin(&database, "storage-test", "correct-horse".to_owned())
            .await
            .unwrap();
        let storage_key = Uuid::new_v4();
        let now = Utc::now();
        repository::ActiveModel {
            id: Set(Uuid::new_v4()),
            namespace: Set(user.username),
            name: Set("repository".to_owned()),
            description: Set(None),
            website_url: Set(None),
            visibility: Set("private".to_owned()),
            object_format: Set("sha1".to_owned()),
            mirrored: Set(false),
            default_branch: Set(Some("main".to_owned())),
            issue_counter: Set(0),
            storage_key: Set(storage_key),
            created_by: Set(user.id),
            archived_at: Set(None),
            deleted_at: Set(None),
            icon_updated_at: Set(None),
            icon_source: Set(None),
            created_at: Set(now),
            updated_at: Set(now),
        }
        .insert(&database)
        .await
        .unwrap();
        Self {
            root,
            settings,
            database,
            storage_key,
        }
    }

    fn local_root(&self, domain: &dyn StorageDomain) -> PathBuf {
        domain.local_root(&self.settings.storage)
    }

    async fn start(&self, domain: &Arc<dyn StorageDomain>) -> Arc<DomainStorage> {
        DomainStorage::start(
            &self.database,
            domain.clone(),
            self.local_root(domain.as_ref()),
        )
        .await
        .unwrap()
    }

    /// Writes `payload` under the domain's canonical key into `store`.
    async fn put(
        &self,
        domain: &dyn StorageDomain,
        store: &dyn BlobStore,
        payload: &'static [u8],
    ) -> ObjectKey {
        let digest = BlobDigest::from_bytes(Sha256::digest(payload).into());
        let key = object_key(domain, self.storage_key, digest);
        store
            .put_verified(&key, digest, Box::pin(std::io::Cursor::new(payload)))
            .await
            .unwrap();
        key
    }

    async fn target(&self, domain: &dyn StorageDomain) -> (Uuid, PathBuf) {
        let path = self.root.join(format!("target-{}", domain.name()));
        let target = targets::create(
            &self.database,
            format!("{} target", domain.name()),
            StorageTargetConfiguration::Filesystem { path: path.clone() },
        )
        .await
        .unwrap();
        (target.id, path)
    }

    async fn migrate(&self, storage: &DomainStorage, target_id: Uuid) -> Result<Uuid> {
        let operation_id = Uuid::new_v4();
        storage
            .reserve_migration(&self.database, target_id, operation_id)
            .await?;
        let guard = storage.try_lock_migration().unwrap();
        storage
            .migrate_online(&self.database, operation_id, 1, guard)
            .await?;
        Ok(operation_id)
    }

    async fn migration(&self, id: Uuid) -> storage_migration::Model {
        storage_migration::Entity::find_by_id(id)
            .one(&self.database)
            .await
            .unwrap()
            .unwrap()
    }

    async fn close(self) {
        self.database.close().await.unwrap();
        tokio::fs::remove_dir_all(self.root).await.unwrap();
    }
}

fn object_key(domain: &dyn StorageDomain, storage_key: Uuid, digest: BlobDigest) -> ObjectKey {
    let hex = digest.to_hex();
    match domain.name() {
        "lfs" => lfs_object_key(storage_key, &hex).unwrap(),
        "registry" => crate::registry::storage::object_key(
            storage_key,
            crate::registry::storage::ObjectKind::Blob,
            &hex,
        )
        .unwrap(),
        other => panic!("no test key layout for {other}"),
    }
}

#[tokio::test]
async fn migration_moves_every_domain_to_a_target_and_back() {
    for domain in domains() {
        let fixture = Fixture::new().await;
        let storage = fixture.start(&domain).await;
        let first = fixture
            .put(domain.as_ref(), storage.store().as_ref(), b"first object")
            .await;
        let (target_id, target_path) = fixture.target(domain.as_ref()).await;

        let operation = fixture.migrate(&storage, target_id).await.unwrap();
        let migration = fixture.migration(operation).await;
        assert_eq!(
            (
                migration.domain.as_str(),
                migration.state.as_str(),
                migration.copied_objects,
                migration.total_bytes,
            ),
            (domain.name(), state::COMPLETED, 1, Some(12)),
            "{}",
            domain.name()
        );
        assert_eq!(storage.target_id(), Some(target_id));
        assert_eq!(
            active_target_id(&fixture.database, domain.name())
                .await
                .unwrap(),
            Some(target_id)
        );
        // Targets hold canonical keys, whatever the local layout is.
        let target = FilesystemBlobStore::new(target_path).await.unwrap();
        assert!(target.stat(&first).await.unwrap().is_some());

        let second = fixture
            .put(domain.as_ref(), storage.store().as_ref(), b"second object")
            .await;
        fixture.migrate(&storage, Uuid::nil()).await.unwrap();
        assert_eq!(storage.target_id(), None);
        assert_eq!(
            active_target_id(&fixture.database, domain.name())
                .await
                .unwrap(),
            None
        );
        let mut keys = [first, second];
        keys.sort_by(|left, right| left.as_str().cmp(right.as_str()));
        assert_eq!(
            storage
                .inventory(&fixture.database)
                .await
                .unwrap()
                .into_iter()
                .map(|object| object.key)
                .collect::<Vec<_>>(),
            keys
        );
        if domain.name() == "lfs" {
            // Cutover reattributes the catalog to the store now holding the objects.
            let cataloged = lfs_object::Entity::find()
                .filter(lfs_object::Column::StorageTargetId.is_null())
                .all(&fixture.database)
                .await
                .unwrap();
            assert_eq!(cataloged.len(), 2);
        }
        fixture.close().await;
    }
}

#[tokio::test]
async fn startup_fails_interrupted_migrations_of_its_own_domain_only() {
    let fixture = Fixture::new().await;
    let [lfs, registry] = domains();
    let lfs_storage = fixture.start(&lfs).await;
    let registry_storage = fixture.start(&registry).await;
    let (target_id, _) = fixture.target(lfs.as_ref()).await;
    let lfs_operation = Uuid::new_v4();
    lfs_storage
        .reserve_migration(&fixture.database, target_id, lfs_operation)
        .await
        .unwrap();
    let registry_operation = Uuid::new_v4();
    registry_storage
        .reserve_migration(&fixture.database, target_id, registry_operation)
        .await
        .unwrap();
    // A domain allows one unfinished migration at a time.
    assert!(
        lfs_storage
            .reserve_migration(&fixture.database, target_id, Uuid::new_v4())
            .await
            .is_err()
    );

    let restarted = fixture.start(&lfs).await;
    let interrupted = fixture.migration(lfs_operation).await;
    assert_eq!(
        (interrupted.state.as_str(), interrupted.phase.as_str()),
        (state::FAILED, phase::FAILED)
    );
    assert!(interrupted.error.unwrap().contains("restarted"));
    assert_eq!(restarted.target_id(), None);
    assert_eq!(
        fixture.migration(registry_operation).await.state,
        state::PENDING
    );
    restarted
        .reserve_migration(&fixture.database, target_id, Uuid::new_v4())
        .await
        .unwrap();
    fixture.close().await;
}

#[tokio::test]
async fn failed_verification_leaves_the_source_selected() {
    for domain in domains() {
        let fixture = Fixture::new().await;
        let storage = fixture.start(&domain).await;
        let key = fixture
            .put(
                domain.as_ref(),
                storage.store().as_ref(),
                b"original object",
            )
            .await;
        let (target_id, target_path) = fixture.target(domain.as_ref()).await;
        // The destination already holds different bytes under the object's digest.
        let corrupt = target_path.join(key.as_str());
        tokio::fs::create_dir_all(corrupt.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(&corrupt, b"tampered").await.unwrap();

        let operation_id = Uuid::new_v4();
        storage
            .reserve_migration(&fixture.database, target_id, operation_id)
            .await
            .unwrap();
        let guard = storage.try_lock_migration().unwrap();
        assert!(
            storage
                .migrate_online(&fixture.database, operation_id, 1, guard)
                .await
                .is_err(),
            "{}",
            domain.name()
        );
        let migration = fixture.migration(operation_id).await;
        assert_eq!(migration.state, state::FAILED);
        assert!(migration.error.is_some());
        assert_eq!(storage.target_id(), None);
        assert_eq!(
            active_target_id(&fixture.database, domain.name())
                .await
                .unwrap(),
            None
        );
        assert!(storage.store().stat(&key).await.unwrap().is_some());
        fixture.close().await;
    }
}
