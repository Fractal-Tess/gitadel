//! Git LFS object storage: the LFS [`StorageDomain`] and the offline
//! `gitadel lfs` commands.

use std::{collections::HashMap, path::PathBuf, sync::Arc};

use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::Utc;
use sea_orm::{
    ColumnTrait as _, DatabaseConnection, DatabaseTransaction, EntityTrait as _, QueryFilter as _,
    QuerySelect as _, Set, sea_query::OnConflict,
};
use uuid::Uuid;

use crate::{
    blob_store::{
        BlobDigest, BlobMetadata, DomainStorage, DomainUsage, ObjectKey, ObjectPrefix,
        RepositoryUsage, StorageDomain, UsageContext,
        targets::{self, StorageTargetConfiguration},
    },
    config::{LfsCommand, LfsTargetCommand, S3Settings, Settings, StorageSettings},
    database,
    entity::{lfs_object, repository},
};

/// Git LFS objects, keyed `<storage-key>/<oid[0..2]>/<oid[2..4]>/<oid>`.
/// The `lfs_objects` catalog follows the objects: migrations backfill it and
/// cutover reattributes it to the destination.
pub(crate) struct LfsDomain;

#[async_trait]
impl StorageDomain for LfsDomain {
    fn name(&self) -> &'static str {
        "lfs"
    }

    fn label(&self) -> &'static str {
        "LFS"
    }

    fn key_prefix(&self) -> &'static str {
        ""
    }

    fn repository_storage_key(&self, key: &ObjectKey) -> Option<Uuid> {
        let mut parts = key.as_str().split('/');
        let storage_key = Uuid::parse_str(parts.next()?).ok()?;
        let (first, second, oid) = (parts.next()?, parts.next()?, parts.next()?);
        (parts.next().is_none()
            && first.len() == 2
            && second.len() == 2
            && oid.parse::<BlobDigest>().is_ok())
        .then_some(storage_key)
    }

    fn repository_prefix(&self, storage_key: Uuid) -> Result<ObjectPrefix> {
        ObjectPrefix::new(storage_key.to_string())
    }

    fn local_root(&self, settings: &StorageSettings) -> PathBuf {
        settings.lfs_root.clone()
    }

    async fn object_copied(
        &self,
        database: &DatabaseConnection,
        object: &BlobMetadata,
        source_target_id: Option<Uuid>,
    ) -> Result<()> {
        catalog_object(database, object, source_target_id).await
    }

    async fn cutover(
        &self,
        transaction: &DatabaseTransaction,
        source: Option<Uuid>,
        destination: Option<Uuid>,
    ) -> Result<()> {
        let objects = lfs_object::Entity::update_many().col_expr(
            lfs_object::Column::StorageTargetId,
            sea_orm::sea_query::Expr::value(destination),
        );
        match source {
            Some(source) => objects.filter(lfs_object::Column::StorageTargetId.eq(source)),
            None => objects.filter(lfs_object::Column::StorageTargetId.is_null()),
        }
        .exec(transaction)
        .await?;
        Ok(())
    }
}

/// Opens the LFS storage for a running server, failing any online migration
/// a restart interrupted.
pub(crate) async fn start(
    database: &DatabaseConnection,
    settings: &StorageSettings,
) -> Result<Arc<DomainStorage>> {
    DomainStorage::start(
        database,
        Arc::new(LfsDomain),
        LfsDomain.local_root(settings),
    )
    .await
}

pub async fn run(command: &LfsCommand, settings: &Settings) -> Result<()> {
    let database = database::connect_and_migrate(&settings.database).await?;
    let result = match command {
        LfsCommand::Target { command } => run_target(command, &database, settings).await,
        LfsCommand::Migrate {
            target_id,
            batch_size,
        } => {
            let result = migrate(&database, settings, *target_id, *batch_size).await;
            if result.is_ok() {
                println!("LFS storage migration completed; target {target_id} is active.");
            }
            result
        }
    };
    database.close().await?;
    result
}

async fn run_target(
    command: &LfsTargetCommand,
    database: &DatabaseConnection,
    settings: &Settings,
) -> Result<()> {
    match command {
        LfsTargetCommand::List => {
            println!(
                "{}",
                serde_json::to_string_pretty(
                    // The CLI holds no scan cache, so targets report their
                    // database usage and the volume they sit on.
                    &targets::list(database, &settings.storage.lfs_root, &Default::default())
                        .await?,
                )?
            );
        }
        LfsTargetCommand::AddFilesystem { name, path } => {
            let target = targets::create(
                database,
                name.clone(),
                StorageTargetConfiguration::Filesystem { path: path.clone() },
            )
            .await?;
            println!("{}", serde_json::to_string_pretty(&target)?);
        }
        LfsTargetCommand::AddS3 {
            name,
            endpoint,
            bucket,
            access_key,
            secret_key,
            region,
            prefix,
        } => {
            let target = targets::create(
                database,
                name.clone(),
                StorageTargetConfiguration::S3 {
                    s3: S3Settings {
                        endpoint: endpoint.clone(),
                        bucket: bucket.clone(),
                        access_key: access_key.clone(),
                        secret_key: secret_key.clone(),
                        region: region.clone(),
                        prefix: prefix.clone(),
                    },
                },
            )
            .await?;
            println!("{}", serde_json::to_string_pretty(&target)?);
        }
        LfsTargetCommand::Test { target_id } => {
            let (_, configuration) = targets::find(database, *target_id).await?;
            targets::test_configuration(&configuration).await?;
            println!("Storage target {target_id} passed write, stat, read, and delete checks.");
        }
    }
    Ok(())
}

/// Offline migration for `gitadel lfs migrate`; Gitadel must be stopped.
pub(crate) async fn migrate(
    database: &DatabaseConnection,
    settings: &Settings,
    target_id: Uuid,
    batch_size: usize,
) -> Result<()> {
    DomainStorage::load(
        database,
        Arc::new(LfsDomain),
        LfsDomain.local_root(&settings.storage),
    )
    .await?
    .migrate_offline(database, target_id, batch_size)
    .await
}

/// Records an object in `lfs_objects`, attributed to the store it was copied
/// from until cutover reattributes it.
async fn catalog_object(
    database: &DatabaseConnection,
    metadata: &BlobMetadata,
    target_id: Option<Uuid>,
) -> Result<()> {
    let storage_key = LfsDomain
        .repository_storage_key(&metadata.key)
        .context("LFS object key has no repository storage key")?;
    let repository_id = repository::Entity::find()
        .filter(repository::Column::StorageKey.eq(storage_key))
        .one(database)
        .await?
        .context("LFS object belongs to an unknown repository storage key")?
        .id;
    let oid = metadata
        .key
        .as_str()
        .rsplit('/')
        .next()
        .context("LFS object key has no digest")?;
    lfs_object::Entity::insert(lfs_object::ActiveModel {
        repository_id: Set(repository_id),
        oid: Set(oid.to_owned()),
        size: Set(i64::try_from(metadata.size)?),
        storage_target_id: Set(target_id),
        created_at: Set(Utc::now()),
    })
    .on_conflict(
        OnConflict::columns([lfs_object::Column::RepositoryId, lfs_object::Column::Oid])
            .update_columns([lfs_object::Column::Size])
            .to_owned(),
    )
    .exec(database)
    .await?;
    Ok(())
}

/// LFS usage, read from the `lfs_objects` catalog. An object shared by
/// repositories counts once for each repository that owns it.
pub(crate) struct LfsUsage;

#[async_trait]
impl DomainUsage for LfsUsage {
    async fn by_repository(
        &self,
        context: UsageContext<'_>,
    ) -> Result<HashMap<Uuid, RepositoryUsage>> {
        let rows = lfs_object::Entity::find()
            .select_only()
            .column(lfs_object::Column::RepositoryId)
            .column_as(lfs_object::Column::Oid.count(), "object_count")
            .column_as(lfs_object::Column::Size.sum(), "total_bytes")
            .group_by(lfs_object::Column::RepositoryId)
            .into_tuple::<(Uuid, i64, i64)>()
            .all(context.database)
            .await?;
        rows.into_iter()
            .map(|(repository_id, object_count, total_bytes)| {
                let usage = RepositoryUsage {
                    object_count: u64::try_from(object_count)?,
                    total_bytes: u64::try_from(total_bytes)?,
                    details: Default::default(),
                };
                Ok((repository_id, usage))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        blob_store::{BlobStore as _, FilesystemBlobStore, lfs_object_key, manager},
        entity::{storage_migration, storage_target},
        identity,
    };
    use sea_orm::ActiveModelTrait as _;
    use sha2::{Digest as _, Sha256};

    #[test]
    fn lfs_domain_recognizes_only_object_keys() {
        let storage_key = Uuid::new_v4();
        let oid = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let object = lfs_object_key(storage_key, oid).unwrap();
        assert_eq!(LfsDomain.repository_storage_key(&object), Some(storage_key));
        for key in [
            format!("{storage_key}/issue-attachments/{}", Uuid::new_v4()),
            format!("registry/{storage_key}/{oid}/blobs/01/23/{oid}"),
            format!(".gitadel/target/{storage_key}/{oid}"),
        ] {
            assert_eq!(
                LfsDomain.repository_storage_key(&ObjectKey::new(key).unwrap()),
                None
            );
        }
    }

    #[tokio::test]
    async fn migration_copies_verifies_cuts_over_and_can_return_local() {
        let root =
            std::env::temp_dir().join(format!("gitadel-storage-migration-{}", Uuid::new_v4()));
        let mut settings = Settings::default();
        settings.database.url = format!("sqlite://{}?mode=rwc", root.join("gitadel.db").display());
        settings.storage.repository_root = root.join("repositories");
        settings.storage.lfs_root = root.join("lfs");
        settings.storage.registry_root = root.join("registry");
        settings.storage.actions_artifact_root = root.join("actions");
        tokio::fs::create_dir_all(&root).await.unwrap();
        let database = database::connect_and_migrate(&settings.database)
            .await
            .unwrap();
        let user = identity::bootstrap_admin(
            &database,
            "storage-test",
            "correct-horse-battery".to_owned(),
        )
        .await
        .unwrap();
        let repository_id = Uuid::new_v4();
        let storage_key = Uuid::new_v4();
        let now = Utc::now();
        repository::ActiveModel {
            id: Set(repository_id),
            namespace: Set(user.username.clone()),
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

        let payload = b"migration payload";
        let digest = BlobDigest::from_bytes(Sha256::digest(payload).into());
        let key = lfs_object_key(storage_key, &digest.to_hex()).unwrap();
        let source = FilesystemBlobStore::new(settings.storage.lfs_root.clone())
            .await
            .unwrap();
        source
            .put_verified(
                &key,
                digest,
                Box::pin(std::io::Cursor::new(payload.as_slice())),
            )
            .await
            .unwrap();

        let target_path = root.join("secondary-lfs");
        let target = targets::create(
            &database,
            "Secondary".to_owned(),
            StorageTargetConfiguration::Filesystem {
                path: target_path.clone(),
            },
        )
        .await
        .unwrap();
        migrate(&database, &settings, target.id, 1).await.unwrap();
        assert_eq!(
            manager::active_target_id(&database, "lfs").await.unwrap(),
            Some(target.id)
        );
        let progress = storage_migration::Entity::find()
            .one(&database)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            (
                progress.domain.as_str(),
                progress.copied_objects,
                progress.copied_bytes,
                progress.last_key.as_deref()
            ),
            ("lfs", 1, payload.len() as i64, Some(key.as_str()))
        );

        // Reported usage follows the objects: cutover reattributes the bytes to
        // the destination, and the configured local path goes back to nothing.
        let views = targets::list(&database, &settings.storage.lfs_root, &Default::default())
            .await
            .unwrap();
        let usage_of = |id: Uuid| views.iter().find(|view| view.id == id).unwrap().usage;
        assert_eq!(
            (
                usage_of(target.id).lfs_object_count,
                usage_of(target.id).lfs_bytes
            ),
            (1, payload.len() as u64)
        );
        assert_eq!(usage_of(Uuid::nil()).lfs_bytes, 0);
        assert!(
            views.iter().all(|view| view
                .capacity
                .is_some_and(|capacity| capacity.total_bytes > 0)),
            "filesystem targets should report the volume they sit on"
        );

        // Scanning reads the destination itself, and leaves out the ownership
        // marker Gitadel wrote when it claimed the target.
        let measured = targets::measure(&StorageTargetConfiguration::Filesystem {
            path: target_path.clone(),
        })
        .await
        .unwrap();
        assert_eq!(
            (measured.object_count, measured.total_bytes),
            (1, payload.len() as u64)
        );

        let destination = FilesystemBlobStore::new(target_path).await.unwrap();
        assert_eq!(
            destination.stat(&key).await.unwrap().unwrap().size,
            payload.len() as u64
        );
        assert!(source.stat(&key).await.unwrap().is_some());

        migrate(&database, &settings, Uuid::nil(), 1).await.unwrap();
        assert_eq!(
            manager::active_target_id(&database, "lfs").await.unwrap(),
            None
        );
        targets::delete(&database, target.id).await.unwrap();
        assert!(
            storage_target::Entity::find_by_id(target.id)
                .one(&database)
                .await
                .unwrap()
                .is_none()
        );
        database.close().await.unwrap();
        tokio::fs::remove_dir_all(root).await.unwrap();
    }
}
