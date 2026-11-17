//! Artifact assembly, storage paths, block validation, and retention cleanup.
use chrono::{Duration, Utc};
use gitadel_db::entity::{action_artifact, action_artifact_grant};
use quick_xml::{Reader, events::Event};
use sea_orm::{ActiveModelTrait, Set};
use sea_orm::{ColumnTrait, Condition, DatabaseConnection, EntityTrait, QueryFilter};
use sha2::{Digest, Sha256};
use std::sync::{LazyLock, Mutex};
use std::{
    collections::HashSet,
    io::ErrorKind,
    path::{Path, PathBuf},
};
use tokio::{
    fs,
    io::{AsyncReadExt as _, AsyncWriteExt as _},
};
use uuid::Uuid;
const MAX_BLOCK_ID: usize = 256;

pub fn artifact_path(root: &Path, row: &action_artifact::Model) -> PathBuf {
    root.join(&row.storage_key)
}
pub fn blocks_dir(root: &Path, row: &action_artifact::Model) -> PathBuf {
    root.join(format!(".blocks-{}", row.storage_key))
}

pub async fn cleanup(
    database: &DatabaseConnection,
    root: &Path,
    grant_lifetime_seconds: i64,
) -> Result<(), sea_orm::DbErr> {
    let now = Utc::now();
    let stale_pending = now - Duration::seconds(std::cmp::Ord::max(grant_lifetime_seconds, 60));
    let rows = action_artifact::Entity::find()
        .filter(
            Condition::any()
                .add(action_artifact::Column::DeletedAt.is_not_null())
                .add(action_artifact::Column::ExpiresAt.lte(now))
                .add(
                    Condition::all()
                        .add(action_artifact::Column::Status.eq("pending"))
                        .add(action_artifact::Column::UpdatedAt.lte(stale_pending)),
                ),
        )
        .all(database)
        .await?;
    for row in rows {
        let mut disk_cleanup_failed = false;
        let path = artifact_path(root, &row);
        if let Err(error) = fs::remove_file(path).await
            && error.kind() != ErrorKind::NotFound
        {
            tracing::warn!(artifact_id = row.id, %error, "could not remove Actions artifact");
            disk_cleanup_failed = true;
        }
        let staging_prefix = format!(".{}.", row.storage_key);
        if let Ok(mut entries) = fs::read_dir(root).await {
            while let Ok(Some(entry)) = entries.next_entry().await {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if name.starts_with(&staging_prefix)
                    && name.ends_with(".finalizing")
                    && let Err(error) = fs::remove_file(entry.path()).await
                    && error.kind() != ErrorKind::NotFound
                {
                    tracing::warn!(
                        artifact_id = row.id,
                        %error,
                        "could not remove staged Actions artifact"
                    );
                    disk_cleanup_failed = true;
                }
            }
        }
        if let Err(error) = fs::remove_dir_all(blocks_dir(root, &row)).await
            && error.kind() != ErrorKind::NotFound
        {
            tracing::warn!(artifact_id = row.id, %error, "could not remove Actions artifact blocks");
            disk_cleanup_failed = true;
        }
        if disk_cleanup_failed {
            continue;
        }
        action_artifact::Entity::delete_by_id(row.id)
            .exec(database)
            .await?;
    }
    action_artifact_grant::Entity::delete_many()
        .filter(action_artifact_grant::Column::ExpiresAt.lte(now))
        .exec(database)
        .await?;
    Ok(())
}

pub fn validate_block_id(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > MAX_BLOCK_ID
        || value.contains(['/', '\\'])
        || value.chars().any(char::is_control)
    {
        return Err(String::from("Block identifier is invalid."));
    }
    Ok(())
}

pub fn parse_block_list(bytes: &[u8], max_blocks: usize) -> Result<Vec<String>, String> {
    let mut reader = Reader::from_reader(bytes);
    reader.config_mut().trim_text(true);
    let mut buffer = Vec::new();
    let mut order = Vec::new();
    let mut seen = HashSet::new();
    loop {
        match reader
            .read_event_into(&mut buffer)
            .map_err(|_| String::from("Malformed artifact block list."))?
        {
            Event::Start(event) if event.name().as_ref() == "Latest" => {
                let value = match reader
                    .read_event_into(&mut buffer)
                    .map_err(|_| String::from("Malformed artifact block list."))?
                {
                    Event::Text(text) => text.xml10_content().into_owned(),
                    _ => return Err(String::from("Malformed artifact block list.")),
                };
                validate_block_id(&value)?;
                if !seen.insert(value.clone()) {
                    return Err(String::from(
                        "Artifact block list contains a duplicate block.",
                    ));
                }
                order.push(value);
                if order.len() > max_blocks {
                    return Err(String::from("Artifact block list has too many blocks."));
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    if order.is_empty() {
        return Err(String::from("Artifact block list is empty."));
    }
    Ok(order)
}

/// Failure while assembling and finalizing an artifact.
#[derive(Debug)]
pub enum ArtifactError {
    Invalid(&'static str),
    Conflict(&'static str),
    Internal(String),
    Database(sea_orm::DbErr),
}
impl ArtifactError {
    fn bad_request(message: &'static str) -> Self {
        Self::Invalid(message)
    }
    fn conflict(message: &'static str) -> Self {
        Self::Conflict(message)
    }
    fn internal(error: impl std::fmt::Display) -> Self {
        Self::Internal(error.to_string())
    }
}
impl From<sea_orm::DbErr> for ArtifactError {
    fn from(error: sea_orm::DbErr) -> Self {
        Self::Database(error)
    }
}

static FINALIZING_ARTIFACTS: LazyLock<Mutex<HashSet<i64>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

struct FinalizationGuard {
    artifact_id: i64,
}

impl FinalizationGuard {
    fn acquire(artifact_id: i64) -> Result<Self, ArtifactError> {
        let mut finalizing = FINALIZING_ARTIFACTS
            .lock()
            .map_err(|_| ArtifactError::internal("artifact finalization lock is poisoned"))?;
        if !finalizing.insert(artifact_id) {
            return Err(ArtifactError::conflict(
                "Artifact finalization is already in progress.",
            ));
        }
        Ok(Self { artifact_id })
    }
}

impl Drop for FinalizationGuard {
    fn drop(&mut self) {
        if let Ok(mut finalizing) = FINALIZING_ARTIFACTS.lock() {
            finalizing.remove(&self.artifact_id);
        }
    }
}

/// Assembles stored blocks, verifies their size and digest, then persists finalization.
pub async fn finalize(
    database: &DatabaseConnection,
    root: &Path,
    settings: &gitadel_config::ActionsSettings,
    row: &action_artifact::Model,
    size: i64,
    hash: Option<&str>,
) -> Result<i64, ArtifactError> {
    let _finalization = FinalizationGuard::acquire(row.id)?;
    let expected_size =
        u64::try_from(size).map_err(|_| ArtifactError::bad_request("Artifact size is invalid."))?;
    let max_artifact = u64::try_from(settings.max_artifact_bytes)
        .map_err(|_| ArtifactError::internal("invalid artifact size setting"))?;
    if expected_size > max_artifact {
        return Err(ArtifactError::bad_request(
            "Artifacts exceed the configured size limit.",
        ));
    }
    let expected_hash = hash
        .and_then(|value| value.strip_prefix("sha256:"))
        .map(str::to_ascii_lowercase)
        .filter(|value| value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .ok_or_else(|| ArtifactError::bad_request("Artifact SHA-256 is required."))?;
    let source = blocks_dir(root, row);
    let destination = artifact_path(root, row);
    fs::create_dir_all(root)
        .await
        .map_err(ArtifactError::internal)?;
    let temporary = root.join(format!(
        ".{}.{}.finalizing",
        row.storage_key,
        Uuid::new_v4()
    ));
    let mut output = fs::File::create(&temporary)
        .await
        .map_err(ArtifactError::internal)?;
    let mut hasher = Sha256::new();
    let mut total = 0u64;
    let metadata: serde_json::Value =
        serde_json::from_str(&row.metadata_json).unwrap_or_else(|_| serde_json::json!({}));
    let ordered_ids: Vec<String> = metadata
        .get("order")
        .and_then(|value| value.as_array())
        .map(|values| {
            values
                .iter()
                .filter_map(|value| value.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    let block_paths = if ordered_ids.is_empty() {
        let mut entries = fs::read_dir(&source)
            .await
            .map_err(ArtifactError::internal)?;
        let mut paths = Vec::new();
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(ArtifactError::internal)?
        {
            if !entry.file_name().to_string_lossy().starts_with('.') {
                paths.push(entry.path());
            }
            if paths.len() > settings.max_artifact_blocks {
                let _ = fs::remove_file(&temporary).await;
                return Err(ArtifactError::bad_request(
                    "Artifact has too many uploaded blocks.",
                ));
            }
        }
        paths.sort();
        paths
    } else {
        ordered_ids
            .iter()
            .map(|id| source.join(hex::encode(Sha256::digest(id.as_bytes()))))
            .collect()
    };
    for path in block_paths {
        let mut input = fs::File::open(path).await.map_err(|_| {
            ArtifactError::bad_request("Artifact block list references a missing block.")
        })?;
        loop {
            let mut buf = [0u8; 64 * 1024];
            let n = input
                .read(&mut buf)
                .await
                .map_err(ArtifactError::internal)?;
            if n == 0 {
                break;
            }
            total = total
                .checked_add(n as u64)
                .ok_or_else(|| ArtifactError::bad_request("Artifact size overflow."))?;
            if total > max_artifact {
                let _ = fs::remove_file(&temporary).await;
                return Err(ArtifactError::bad_request(
                    "Artifacts exceed the configured size limit.",
                ));
            }
            hasher.update(&buf[..n]);
            output
                .write_all(&buf[..n])
                .await
                .map_err(ArtifactError::internal)?;
        }
    }
    output.flush().await.map_err(ArtifactError::internal)?;
    drop(output);
    let actual = hex::encode(hasher.finalize());
    if total != expected_size || actual != expected_hash {
        let _ = fs::remove_file(&temporary).await;
        return Err(ArtifactError::bad_request(
            "Artifact size or SHA-256 does not match.",
        ));
    }
    fs::rename(&temporary, &destination)
        .await
        .map_err(ArtifactError::internal)?;
    let now = Utc::now();
    let mut active: action_artifact::ActiveModel = row.clone().into();
    active.status = Set("finalized".to_owned());
    active.size_bytes = Set(i64::try_from(total)
        .map_err(|_| ArtifactError::bad_request("Artifact size is invalid."))?);
    active.sha256 = Set(Some(actual));
    active.finalized_at = Set(Some(now));
    active.updated_at = Set(now);
    active.update(database).await?;
    let _ = fs::remove_dir_all(source).await;
    Ok(row.id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{test_support, workflow};
    use gitadel_config::ActionsSettings;

    struct TestDirectory(PathBuf);

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[tokio::test]
    async fn finalize_verifies_ordered_blocks_and_cleanup_removes_expired_data() {
        let database = test_support::database().await;
        let owner = test_support::owner(&database, "alice").await;
        let repository = test_support::repository(&database, &owner, "project").await;
        let settings = ActionsSettings::default();
        let plan = workflow::validate_workflow(
            &settings,
            "ci.yml".to_owned(),
            b"on: push\njobs:\n  build:\n    runs-on: docker\n    steps: []\n".to_vec(),
        )
        .unwrap();
        let trigger = workflow::RunTrigger {
            event: "push",
            reference: "refs/heads/main",
            before: "",
            after: "abc",
            actor: Some(&owner),
            event_json: serde_json::json!({}),
        };
        let run = workflow::enqueue_plan(&database, &repository, &trigger, plan)
            .await
            .unwrap();
        let job = gitadel_db::entity::action_job::Entity::find()
            .one(&database)
            .await
            .unwrap()
            .unwrap();
        let now = Utc::now();
        let row = action_artifact::ActiveModel {
            id: Default::default(),
            repository_id: Set(repository.id),
            run_id: Set(run.id),
            creating_job_id: Set(job.id),
            name: Set("report".to_owned()),
            storage_key: Set(Uuid::new_v4().to_string()),
            status: Set("pending".to_owned()),
            size_bytes: Set(0),
            sha256: Set(None),
            expires_at: Set(now + Duration::days(1)),
            finalized_at: Set(None),
            deleted_at: Set(None),
            deleted_by_job_id: Set(None),
            metadata_json: Set(serde_json::json!({"order": ["second", "first"]}).to_string()),
            created_at: Set(now),
            updated_at: Set(now),
        }
        .insert(&database)
        .await
        .unwrap();
        let root = TestDirectory(
            std::env::temp_dir().join(format!("gitadel-actions-artifact-{}", Uuid::new_v4())),
        );
        let blocks = blocks_dir(&root.0, &row);
        fs::create_dir_all(&blocks).await.unwrap();
        fs::write(blocks.join(hex::encode(Sha256::digest(b"first"))), b"world")
            .await
            .unwrap();
        fs::write(
            blocks.join(hex::encode(Sha256::digest(b"second"))),
            b"hello ",
        )
        .await
        .unwrap();
        let wrong_hash = format!("sha256:{}", "0".repeat(64));
        assert!(matches!(
            finalize(&database, &root.0, &settings, &row, 11, Some(&wrong_hash)).await,
            Err(ArtifactError::Invalid(
                "Artifact size or SHA-256 does not match."
            ))
        ));
        assert!(!fs::try_exists(artifact_path(&root.0, &row)).await.unwrap());
        let hash = format!("sha256:{}", hex::encode(Sha256::digest(b"hello world")));
        assert_eq!(
            finalize(&database, &root.0, &settings, &row, 11, Some(&hash))
                .await
                .unwrap(),
            row.id
        );
        assert_eq!(
            fs::read(artifact_path(&root.0, &row)).await.unwrap(),
            b"hello world"
        );
        assert!(!fs::try_exists(blocks).await.unwrap());
        let finalized = action_artifact::Entity::find_by_id(row.id)
            .one(&database)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(finalized.status, "finalized");
        let mut expired: action_artifact::ActiveModel = finalized.into();
        expired.expires_at = Set(now - Duration::seconds(1));
        expired.update(&database).await.unwrap();
        cleanup(&database, &root.0, settings.artifact_grant_lifetime_seconds)
            .await
            .unwrap();
        assert!(
            action_artifact::Entity::find_by_id(row.id)
                .one(&database)
                .await
                .unwrap()
                .is_none()
        );
        assert!(!fs::try_exists(artifact_path(&root.0, &row)).await.unwrap());
    }
}
