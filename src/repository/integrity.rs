use std::{
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

use super::RepositoryState;
use crate::{
    entity::{
        instance, issue_attachment, lfs_object, release_asset, repository, repository_release,
    },
    schedule,
};
use anyhow::{Context as _, Result, bail, ensure};
use chrono::Utc;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use sley_core::{AtomicCancel, CancelFlag, ObjectFormat};
use tokio::{fs, task, time};

const CHECK_TIMEOUT: Duration = Duration::from_secs(30 * 60);

pub(crate) async fn serve_integrity_scheduler(state: RepositoryState) -> Result<()> {
    let mut settings_changed = state.identity().subscribe_integrity_settings();

    loop {
        let settings = instance::Entity::find_by_id(1)
            .one(state.identity().database())
            .await
            .context("could not load repository integrity settings")?
            .context("instance settings row is missing")?;

        if !settings.integrity_checks_enabled {
            settings_changed
                .changed()
                .await
                .context("repository integrity settings channel closed")?;
            continue;
        }

        let now = Utc::now();
        let expression = settings.integrity_check_schedule;
        let next = schedule::next_occurrence(&expression, now).with_context(|| {
            format!("could not schedule repository integrity checks: {expression}")
        })?;
        let wait = tokio::time::sleep((next - now).to_std().unwrap_or_default());
        tokio::pin!(wait);

        tokio::select! {
            () = &mut wait => {
                if let Err(error) = check_all(&state).await {
                    tracing::error!(%error, "scheduled repository integrity check failed");
                }
            }
            changed = settings_changed.changed() => {
                changed.context("repository integrity settings channel closed")?;
            }
        }
    }
}

pub(crate) async fn check_all(state: &RepositoryState) -> Result<(usize, usize)> {
    let repositories = repository::Entity::find()
        .filter(repository::Column::DeletedAt.is_null())
        .all(state.identity().database())
        .await
        .context("could not load repositories for integrity check")?;
    let checked = repositories.len();
    let mut failed = 0;

    for repository in repositories {
        if let Err(error) = check_repository(state, &repository).await {
            failed += 1;
            tracing::error!(
                %error,
                namespace = %repository.namespace,
                repository = %repository.name,
                "repository integrity check failed"
            );
        }
    }

    state
        .identity()
        .audit(
            None,
            "repository.integrity.check",
            Some(format!("{checked} checked, {failed} failed")),
        )
        .await
        .context("could not record repository integrity result")?;
    tracing::info!(checked, failed, "repository integrity check completed");
    Ok((checked, failed))
}

async fn check_repository(state: &RepositoryState, repository: &repository::Model) -> Result<()> {
    check_git_repository(&state.repository_path(repository)).await?;
    check_lfs_objects(state, repository).await?;
    check_release_assets(state, repository).await?;
    check_issue_attachments(state, repository).await
}

async fn check_git_repository(path: &Path) -> Result<()> {
    let path = path.to_owned();
    let cancel = Arc::new(AtomicCancel::new());
    let worker_cancel = Arc::clone(&cancel);
    let mut worker = task::spawn_blocking(move || -> Result<()> {
        let format = if path.join("config").is_file() {
            sley_config::read_repo_config(&path, None)
                .and_then(|config| config.repository_object_format())
                .context("could not determine repository object format")?
        } else {
            ObjectFormat::Sha1
        };
        let report = sley_fsck::check_repository_with_options(
            &path,
            format,
            sley_fsck::RepositoryCheckOptions {
                cancel: CancelFlag::new(&worker_cancel),
                deadline: Some(Instant::now() + CHECK_TIMEOUT),
            },
        )
        .with_context(|| format!("could not check repository {}", path.display()))?;
        if report.is_ok() {
            return Ok(());
        }
        let mut failures = report
            .reference_findings
            .errors()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        failures.extend(
            report
                .object_report
                .issues
                .iter()
                .filter(|issue| issue.severity == sley_fsck::IssueSeverity::Error)
                .map(|issue| issue.message.clone()),
        );
        failures.extend(report.storage_errors);
        if failures.is_empty() {
            failures.push(format!(
                "native fsck exited with error bits {}",
                report.object_report.error_bits
            ));
        }
        bail!(
            "native repository integrity check failed: {}",
            failures.join("; ")
        );
    });
    match time::timeout(CHECK_TIMEOUT, &mut worker).await {
        Ok(result) => result.context("native repository integrity worker failed")??,
        Err(_) => {
            cancel.cancel();
            let _ = worker.await;
            bail!(
                "native repository integrity check exceeded its 30-minute limit; \
                 the background check was cancelled"
            );
        }
    }
    Ok(())
}
async fn check_lfs_objects(state: &RepositoryState, repository: &repository::Model) -> Result<()> {
    let objects = lfs_object::Entity::find()
        .filter(lfs_object::Column::RepositoryId.eq(repository.id))
        .all(state.identity().database())
        .await
        .context("could not load LFS objects")?;

    for object in objects {
        let key = state
            .lfs_object_key(repository, &object.oid)
            .with_context(|| format!("invalid LFS object ID {}", object.oid))?;
        let metadata = state
            .lfs_store()
            .stat(&key)
            .await
            .with_context(|| format!("could not inspect LFS object {}", object.oid))?
            .with_context(|| format!("LFS object {} is missing", object.oid))?;
        let expected_size = u64::try_from(object.size)
            .with_context(|| format!("LFS object {} has a negative stored size", object.oid))?;
        ensure!(
            metadata.size == expected_size,
            "LFS object {} has size {}, expected {}",
            object.oid,
            metadata.size,
            expected_size
        );
        let mut reader = state
            .lfs_store()
            .read(&key)
            .await
            .with_context(|| format!("could not read LFS object {}", object.oid))?;
        let bytes = tokio::io::copy(&mut reader, &mut tokio::io::sink())
            .await
            .with_context(|| format!("LFS object {} failed digest verification", object.oid))?;
        ensure!(
            bytes == expected_size,
            "LFS object {} yielded {bytes} bytes, expected {expected_size}",
            object.oid
        );
    }
    Ok(())
}

async fn check_release_assets(
    state: &RepositoryState,
    repository: &repository::Model,
) -> Result<()> {
    let releases = repository_release::Entity::find()
        .filter(repository_release::Column::RepositoryId.eq(repository.id))
        .all(state.identity().database())
        .await
        .context("could not load releases")?;

    for release in releases {
        let assets = release_asset::Entity::find()
            .filter(release_asset::Column::ReleaseId.eq(release.id))
            .all(state.identity().database())
            .await
            .with_context(|| format!("could not load assets for release {}", release.id))?;
        for asset in assets {
            let path = state
                .lfs_repository_path(repository)
                .join("releases")
                .join(release.id.to_string())
                .join(asset.id.to_string());
            check_stored_file(&path, asset.size_bytes, "release asset").await?;
        }
    }
    Ok(())
}

async fn check_issue_attachments(
    state: &RepositoryState,
    repository: &repository::Model,
) -> Result<()> {
    let attachments = issue_attachment::Entity::find()
        .filter(issue_attachment::Column::RepositoryId.eq(repository.id))
        .all(state.identity().database())
        .await
        .context("could not load issue attachments")?;

    for attachment in attachments {
        let path = state
            .lfs_repository_path(repository)
            .join("issue-attachments")
            .join(attachment.id.to_string());
        check_stored_file(&path, attachment.size_bytes, "issue attachment").await?;
    }
    Ok(())
}

async fn check_stored_file(path: &Path, stored_size: i64, kind: &str) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .await
        .with_context(|| format!("{kind} {} is missing", path.display()))?;
    ensure!(
        metadata.file_type().is_file(),
        "{kind} {} is not a regular file",
        path.display()
    );
    let expected_size = u64::try_from(stored_size)
        .with_context(|| format!("{kind} {} has a negative stored size", path.display()))?;
    ensure!(
        metadata.len() == expected_size,
        "{kind} {} has size {}, expected {}",
        path.display(),
        metadata.len(),
        expected_size
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    struct TestDirectory(std::path::PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("gitadel-integrity-test-{}", Uuid::new_v4()));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[tokio::test]
    async fn integrity_check_accepts_a_valid_bare_repository() {
        let directory = TestDirectory::new();
        std::fs::create_dir(directory.path().join("objects")).unwrap();
        std::fs::create_dir(directory.path().join("refs")).unwrap();
        std::fs::write(directory.path().join("HEAD"), b"ref: refs/heads/main\n").unwrap();

        check_git_repository(directory.path()).await.unwrap();
    }

    #[tokio::test]
    async fn integrity_check_rejects_incomplete_repository_layouts() {
        let directory = TestDirectory::new();
        assert!(check_git_repository(directory.path()).await.is_err());

        std::fs::create_dir(directory.path().join("refs")).unwrap();
        std::fs::write(directory.path().join("HEAD"), b"ref: refs/heads/main\n").unwrap();
        assert!(check_git_repository(directory.path()).await.is_err());
    }

    #[tokio::test]
    async fn stored_file_check_rejects_size_mismatch() {
        let directory = TestDirectory::new();
        let path = directory.path().join("asset");
        fs::write(&path, b"four").await.unwrap();

        let error = check_stored_file(&path, 5, "test asset").await.unwrap_err();

        assert!(error.to_string().contains("has size 4, expected 5"));
    }
}
