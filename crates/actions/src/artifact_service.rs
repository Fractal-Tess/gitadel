//! Artifact lifecycle, upload grants and quota reservations.
use crate::{
    artifacts::{ArtifactError, artifact_path, blocks_dir},
    tokens::AuthenticatedJob,
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{Duration, Utc};
use futures_util::TryStreamExt as _;
use gitadel_db::entity::{action_artifact, action_artifact_grant, action_job};
use gitadel_identity::IdentityError;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, Condition, EntityTrait, QueryFilter, QueryOrder, Set,
    sea_query::{Expr, ExprTrait as _},
};
use sha2::{Digest, Sha256};
use std::{io::ErrorKind, path::Path};
use tokio::{fs, io::AsyncWriteExt as _};
use uuid::Uuid;
pub struct UploadGrant {
    row: action_artifact::Model,
    grant: action_artifact_grant::Model,
}
pub struct ArtifactService<'a> {
    database: &'a sea_orm::DatabaseConnection,
    root: &'a Path,
    settings: &'a gitadel_config::ActionsSettings,
}
impl<'a> ArtifactService<'a> {
    pub fn new(
        database: &'a sea_orm::DatabaseConnection,
        root: &'a Path,
        settings: &'a gitadel_config::ActionsSettings,
    ) -> Self {
        Self {
            database,
            root,
            settings,
        }
    }
    pub async fn create(
        &self,
        credential: &str,
        run: Uuid,
        job_id: i64,
        name: &str,
        expires_at: Option<chrono::DateTime<Utc>>,
    ) -> Result<String, IdentityError> {
        let auth = &crate::tokens::authenticate_job(self.database, credential)
            .await?
            .ok_or_else(IdentityError::unauthorized)?;
        check_ids(run, job_id, auth)?;
        valid_name(name, self.settings.max_artifact_name_bytes)?;
        let now = Utc::now();
        let maximum_expiry = now + Duration::days(self.settings.retention_days);
        let expires_at = match expires_at {
            Some(requested) => {
                if requested <= now {
                    return Err(IdentityError::bad_request(
                        "Artifact expiry must be in the future.",
                    ));
                }
                std::cmp::Ord::min(requested, maximum_expiry)
            }
            None => maximum_expiry,
        };
        let row = action_artifact::ActiveModel {
            id: Default::default(),
            repository_id: Set(auth.repository.id),
            run_id: Set(run),
            creating_job_id: Set(job_id),
            name: Set(name.to_owned()),
            storage_key: Set(Uuid::new_v4().to_string()),
            status: Set("pending".to_owned()),
            size_bytes: Set(0),
            sha256: Set(None),
            expires_at: Set(expires_at),
            finalized_at: Set(None),
            deleted_at: Set(None),
            deleted_by_job_id: Set(None),
            metadata_json: Set("{}".to_owned()),
            created_at: Set(now),
            updated_at: Set(now),
        }
        .insert(self.database)
        .await
        .map_err(|error| {
            if matches!(error, sea_orm::DbErr::Exec(_)) {
                IdentityError::conflict("An artifact with this name already exists.")
            } else {
                error.into()
            }
        })?;
        let grant_token = format!(
            "gta_art_{}",
            URL_SAFE_NO_PAD.encode(rand::random::<[u8; 32]>())
        );
        action_artifact_grant::ActiveModel {
            id: Set(Uuid::new_v4()),
            artifact_id: Set(row.id),
            issued_to_job_id: Set(job_id),
            scope: Set("upload".to_owned()),
            token_hash: Set(crate::tokens::digest(&grant_token)),
            expires_at: Set(now + Duration::seconds(self.settings.artifact_grant_lifetime_seconds)),
            revoked_at: Set(None),
            last_used_at: Set(None),
            use_count: Set(0),
            created_at: Set(now),
        }
        .insert(self.database)
        .await?;
        Ok(grant_token)
    }
    pub async fn finalize(
        &self,
        credential: &str,
        run: Uuid,
        job_id: i64,
        name: &str,
        size: i64,
        hash: Option<&str>,
    ) -> Result<i64, IdentityError> {
        let auth = &crate::tokens::authenticate_job(self.database, credential)
            .await?
            .ok_or_else(IdentityError::unauthorized)?;
        check_ids(run, job_id, auth)?;
        valid_name(name, self.settings.max_artifact_name_bytes)?;
        let row = action_artifact::Entity::find()
            .filter(action_artifact::Column::RunId.eq(run))
            .filter(action_artifact::Column::CreatingJobId.eq(job_id))
            .filter(action_artifact::Column::Name.eq(name))
            .filter(action_artifact::Column::DeletedAt.is_null())
            .one(self.database)
            .await?
            .ok_or_else(IdentityError::not_found)?;
        if row.status == "finalized" {
            return Ok(row.id);
        }
        let artifact_id =
            crate::artifacts::finalize(self.database, self.root, self.settings, &row, size, hash)
                .await
                .map_err(map_artifact_error)?;
        Ok(artifact_id)
    }
    pub async fn list(
        &self,
        credential: &str,
        run: Uuid,
        name_filter: Option<String>,
        id_filter: Option<i64>,
    ) -> Result<Vec<action_artifact::Model>, IdentityError> {
        let auth = &crate::tokens::authenticate_job(self.database, credential)
            .await?
            .ok_or_else(IdentityError::unauthorized)?;
        check_run(run, auth)?;
        let mut select = action_artifact::Entity::find()
            .filter(run_scope(self.database, run).await?)
            .filter(action_artifact::Column::RepositoryId.eq(auth.repository.id))
            .filter(action_artifact::Column::Status.eq("finalized"))
            .filter(action_artifact::Column::DeletedAt.is_null())
            .filter(action_artifact::Column::ExpiresAt.gt(Utc::now()))
            .order_by_asc(action_artifact::Column::Id);
        if let Some(filter) = name_filter {
            select = select.filter(action_artifact::Column::Name.eq(filter));
        }
        if let Some(filter) = id_filter {
            select = select.filter(action_artifact::Column::Id.eq(filter));
        }
        Ok(select.all(self.database).await?)
    }
    pub async fn signed_download(
        &self,
        credential: &str,
        run: Uuid,
        name: &str,
    ) -> Result<String, IdentityError> {
        let auth = &crate::tokens::authenticate_job(self.database, credential)
            .await?
            .ok_or_else(IdentityError::unauthorized)?;
        check_run(run, auth)?;
        // Prefer this run's own artifact over one inherited from a kept job.
        let row = action_artifact::Entity::find()
            .filter(run_scope(self.database, run).await?)
            .filter(action_artifact::Column::RepositoryId.eq(auth.repository.id))
            .filter(action_artifact::Column::Name.eq(name))
            .filter(action_artifact::Column::Status.eq("finalized"))
            .order_by_desc(Expr::col(action_artifact::Column::RunId).eq(run))
            .order_by_desc(action_artifact::Column::Id)
            .filter(action_artifact::Column::DeletedAt.is_null())
            .filter(action_artifact::Column::ExpiresAt.gt(Utc::now()))
            .one(self.database)
            .await?
            .ok_or_else(IdentityError::not_found)?;
        let grant_token = format!(
            "gta_art_{}",
            URL_SAFE_NO_PAD.encode(rand::random::<[u8; 32]>())
        );
        let now = Utc::now();
        action_artifact_grant::ActiveModel {
            id: Set(Uuid::new_v4()),
            artifact_id: Set(row.id),
            issued_to_job_id: Set(auth.job.id),
            scope: Set("download".to_owned()),
            token_hash: Set(crate::tokens::digest(&grant_token)),
            expires_at: Set(now + Duration::seconds(self.settings.artifact_grant_lifetime_seconds)),
            revoked_at: Set(None),
            last_used_at: Set(None),
            use_count: Set(0),
            created_at: Set(now),
        }
        .insert(self.database)
        .await?;
        Ok(grant_token)
    }
    pub async fn delete(
        &self,
        credential: &str,
        run: Uuid,
        name: &str,
    ) -> Result<i64, IdentityError> {
        let auth = &crate::tokens::authenticate_job(self.database, credential)
            .await?
            .ok_or_else(IdentityError::unauthorized)?;
        check_run(run, auth)?;
        let row = action_artifact::Entity::find()
            .filter(action_artifact::Column::RunId.eq(run))
            .filter(action_artifact::Column::RepositoryId.eq(auth.repository.id))
            .filter(action_artifact::Column::Name.eq(name))
            .filter(action_artifact::Column::DeletedAt.is_null())
            .one(self.database)
            .await?
            .ok_or_else(IdentityError::not_found)?;
        let now = Utc::now();
        let mut active: action_artifact::ActiveModel = row.clone().into();
        active.deleted_at = Set(Some(now));
        active.deleted_by_job_id = Set(Some(auth.job.id));
        active.updated_at = Set(now);
        active.update(self.database).await?;
        let _ = fs::remove_file(artifact_path(self.root, &row)).await;
        Ok(row.id)
    }
    async fn grant(
        &self,
        token: &str,
        scope: &str,
    ) -> Result<(action_artifact::Model, action_artifact_grant::Model), IdentityError> {
        if token.split('.').count() == 3 {
            return Err(IdentityError::unauthorized());
        }
        let now = Utc::now();
        let grant = action_artifact_grant::Entity::find()
            .filter(action_artifact_grant::Column::TokenHash.eq(crate::tokens::digest(token)))
            .filter(action_artifact_grant::Column::Scope.eq(scope))
            .filter(action_artifact_grant::Column::RevokedAt.is_null())
            .filter(action_artifact_grant::Column::ExpiresAt.gt(now))
            .one(self.database)
            .await?
            .ok_or_else(IdentityError::unauthorized)?;
        let issued_job = action_job::Entity::find_by_id(grant.issued_to_job_id)
            .filter(action_job::Column::Status.eq("running"))
            .filter(action_job::Column::LeaseDeadline.gt(now))
            .one(self.database)
            .await?
            .ok_or_else(IdentityError::unauthorized)?;
        let row = action_artifact::Entity::find_by_id(grant.artifact_id)
            .one(self.database)
            .await?
            .ok_or_else(IdentityError::not_found)?;
        if issued_job.run_id != row.run_id {
            // Downloads may target artifacts inherited from kept jobs.
            let inherited = scope == "download"
                && action_job::Entity::find()
                    .filter(action_job::Column::RunId.eq(issued_job.run_id))
                    .filter(action_job::Column::CopiedFromJobId.eq(row.creating_job_id))
                    .one(self.database)
                    .await?
                    .is_some();
            if !inherited {
                return Err(IdentityError::unauthorized());
            }
        }
        Ok((row, grant))
    }
    async fn touch_grant(&self, grant: action_artifact_grant::Model) -> Result<(), IdentityError> {
        let mut active: action_artifact_grant::ActiveModel = grant.into();
        active.last_used_at = Set(Some(Utc::now()));
        active.use_count = Set(active.use_count.as_ref() + 1);
        active.update(self.database).await?;
        Ok(())
    }
    pub async fn authorize_upload(&self, token: &str) -> Result<UploadGrant, IdentityError> {
        let (row, grant) = self.grant(token, "upload").await?;
        if row.status != "pending" || row.deleted_at.is_some() {
            return Err(IdentityError::conflict(
                "Artifact is no longer accepting uploads.",
            ));
        }
        Ok(UploadGrant { row, grant })
    }
    pub async fn set_block_order(
        &self,
        authorization: UploadGrant,
        order: Vec<String>,
    ) -> Result<(), IdentityError> {
        let UploadGrant { row, grant } = authorization;
        let mut metadata: serde_json::Value =
            serde_json::from_str(&row.metadata_json).unwrap_or_else(|_| serde_json::json!({}));
        metadata["order"] = serde_json::json!(order);
        let mut active: action_artifact::ActiveModel = row.into();
        active.metadata_json =
            Set(serde_json::to_string(&metadata).map_err(IdentityError::internal)?);
        active.updated_at = Set(Utc::now());
        active.update(self.database).await?;
        self.touch_grant(grant).await
    }
    pub async fn upload<S, E>(
        &self,
        authorization: UploadGrant,
        block_id: String,
        append: bool,
        stream: S,
    ) -> Result<(), IdentityError>
    where
        S: futures_util::Stream<Item = Result<bytes::Bytes, E>> + Unpin,
        E: std::fmt::Display,
    {
        let UploadGrant { row, grant } = authorization;
        validate_block_id(&block_id)?;
        let directory = blocks_dir(self.root, &row);
        fs::create_dir_all(&directory)
            .await
            .map_err(IdentityError::internal)?;
        let path = directory.join(hex::encode(Sha256::digest(block_id.as_bytes())));
        let temporary = directory.join(format!(".upload-{}", Uuid::new_v4()));
        let old_metadata = match fs::metadata(&path).await {
            Ok(metadata) => Some(metadata),
            Err(error) if error.kind() == ErrorKind::NotFound => None,
            Err(error) => return Err(IdentityError::internal(error)),
        };
        let old_size = old_metadata.as_ref().map_or(0, std::fs::Metadata::len);
        if old_metadata.is_none() {
            let mut entries = fs::read_dir(&directory)
                .await
                .map_err(IdentityError::internal)?;
            let mut block_count = 0usize;
            while let Some(entry) = entries
                .next_entry()
                .await
                .map_err(IdentityError::internal)?
            {
                if !entry.file_name().to_string_lossy().starts_with('.') {
                    block_count += 1;
                }
                if block_count >= self.settings.max_artifact_blocks {
                    return Err(IdentityError::bad_request(
                        "Artifact has too many uploaded blocks.",
                    ));
                }
            }
        }
        if append && old_size > 0 {
            fs::copy(&path, &temporary)
                .await
                .map_err(IdentityError::internal)?;
        }
        let uploaded = write_upload_file(
            stream,
            &temporary,
            append && old_size > 0,
            self.settings.max_artifact_upload_request_bytes,
        )
        .await;
        if let Err(error) = uploaded {
            let _ = fs::remove_file(&temporary).await;
            return Err(error);
        }
        let new_size = fs::metadata(&temporary)
            .await
            .map_err(IdentityError::internal)?
            .len();
        let additional_bytes = new_size.saturating_sub(old_size);
        let additional_bytes = i64::try_from(additional_bytes)
            .map_err(|_| IdentityError::bad_request("Artifact size is invalid."))?;
        let maximum_bytes = self.settings.max_artifact_bytes;
        let reservation = action_artifact::Entity::update_many()
            .col_expr(
                action_artifact::Column::SizeBytes,
                Expr::col(action_artifact::Column::SizeBytes).add(additional_bytes),
            )
            .filter(action_artifact::Column::Id.eq(row.id))
            .filter(action_artifact::Column::Status.eq("pending"))
            .filter(
                Expr::col(action_artifact::Column::SizeBytes)
                    .add(additional_bytes)
                    .lte(maximum_bytes),
            )
            .exec(self.database)
            .await?;
        if reservation.rows_affected != 1 {
            let _ = fs::remove_file(&temporary).await;
            return Err(IdentityError::bad_request(
                "Artifacts exceed the configured size limit.",
            ));
        }
        if let Err(error) = fs::rename(&temporary, &path).await {
            if let Err(release_error) = action_artifact::Entity::update_many()
                .col_expr(
                    action_artifact::Column::SizeBytes,
                    Expr::col(action_artifact::Column::SizeBytes).sub(additional_bytes),
                )
                .filter(action_artifact::Column::Id.eq(row.id))
                .exec(self.database)
                .await
            {
                tracing::error!(
                    artifact_id = %row.id,
                    error = %release_error,
                    "could not release reserved artifact size after a failed upload"
                );
            }
            let _ = fs::remove_file(&temporary).await;
            return Err(IdentityError::internal(error));
        }
        self.touch_grant(grant).await?;
        Ok(())
    }
    pub async fn download(
        &self,
        token: &str,
    ) -> Result<(action_artifact::Model, fs::File), IdentityError> {
        let (row, grant) = self.grant(token, "download").await?;
        if row.status != "finalized" || row.expires_at <= Utc::now() || row.deleted_at.is_some() {
            return Err(IdentityError::not_found());
        }
        let file = fs::File::open(artifact_path(self.root, &row))
            .await
            .map_err(|_| IdentityError::not_found())?;
        self.touch_grant(grant).await?;
        Ok((row, file))
    }
}
pub fn check_run(run: Uuid, auth: &AuthenticatedJob) -> Result<(), IdentityError> {
    if auth.run.id != run {
        return Err(IdentityError::forbidden(
            "The Actions token does not belong to this workflow run.",
        ));
    }
    Ok(())
}
pub fn check_ids(run: Uuid, job_id: i64, auth: &AuthenticatedJob) -> Result<(), IdentityError> {
    check_run(run, auth)?;
    if auth.job.id != job_id {
        return Err(IdentityError::forbidden(
            "The Actions token does not belong to this workflow job.",
        ));
    }
    Ok(())
}
fn valid_name(name: &str, max_bytes: usize) -> Result<(), IdentityError> {
    if name.is_empty()
        || name.len() > max_bytes
        || name == "."
        || name == ".."
        || name.contains(['/', '\\'])
        || name.chars().any(char::is_control)
    {
        return Err(IdentityError::bad_request("Artifact name is invalid."));
    }
    Ok(())
}
/// Artifacts visible to a run: its own, plus those uploaded by the original
/// jobs that a "re-run failed jobs" run kept instead of running again.
pub async fn run_scope(
    database: &sea_orm::DatabaseConnection,
    run_id: Uuid,
) -> Result<Condition, sea_orm::DbErr> {
    let kept: Vec<i64> = action_job::Entity::find()
        .filter(action_job::Column::RunId.eq(run_id))
        .filter(action_job::Column::CopiedFromJobId.is_not_null())
        .all(database)
        .await?
        .into_iter()
        .filter_map(|job| job.copied_from_job_id)
        .collect();
    let mut scope = Condition::any().add(action_artifact::Column::RunId.eq(run_id));
    if !kept.is_empty() {
        scope = scope.add(action_artifact::Column::CreatingJobId.is_in(kept));
    }
    Ok(scope)
}

async fn write_upload_file<S, E>(
    mut stream: S,
    path: &std::path::Path,
    append: bool,
    limit: usize,
) -> Result<u64, IdentityError>
where
    S: futures_util::Stream<Item = Result<bytes::Bytes, E>> + Unpin,
    E: std::fmt::Display,
{
    let mut output = if append {
        fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .await
            .map_err(IdentityError::internal)?
    } else {
        fs::File::create(path)
            .await
            .map_err(IdentityError::internal)?
    };
    let mut total = 0u64;
    while let Some(chunk) = stream.try_next().await.map_err(IdentityError::internal)? {
        total = total
            .checked_add(chunk.len() as u64)
            .ok_or_else(|| IdentityError::bad_request("Upload size overflow."))?;
        if total > limit as u64 {
            return Err(IdentityError::bad_request(
                "Artifact upload block is too large.",
            ));
        }
        output
            .write_all(&chunk)
            .await
            .map_err(IdentityError::internal)?;
    }
    output.flush().await.map_err(IdentityError::internal)?;
    Ok(total)
}
fn map_artifact_error(error: ArtifactError) -> IdentityError {
    match error {
        ArtifactError::Invalid(message) => IdentityError::bad_request(message),
        ArtifactError::Conflict(message) => IdentityError::conflict(message),
        ArtifactError::Internal(message) => IdentityError::internal(message),
        ArtifactError::Database(error) => error.into(),
    }
}
fn validate_block_id(value: &str) -> Result<(), IdentityError> {
    crate::artifacts::validate_block_id(value).map_err(IdentityError::bad_request)
}

#[cfg(test)]
mod direct_boundaries_tests {
    use super::*;
    use crate::{runners, runs, test_support, tokens, workflow};
    use std::{collections::BTreeMap, path::PathBuf};
    use tokio::io::AsyncReadExt;

    struct TestDirectory(PathBuf);
    struct NoRunEffects;
    impl crate::RunEffects for NoRunEffects {
        fn requested(&self, _: Uuid) {}
        fn completed(&self, _: Uuid) {}
        fn requested_and_completed(&self, _: Uuid) {}
    }
    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[tokio::test]
    async fn upload_retries_preserve_quotas_and_grants_stop_after_job_revocation() {
        let database = test_support::database().await;
        let owner = test_support::owner(&database, "owner").await;
        let repository = test_support::repository(&database, &owner, "project").await;
        let settings = gitadel_config::ActionsSettings {
            max_artifact_bytes: 5,
            ..Default::default()
        };
        let labels = vec!["docker".to_owned()];
        let registration =
            tokens::issue_registration(&database, None, None, "runner".to_owned(), labels.clone())
                .await
                .unwrap();
        let runner = runners::register(
            &database,
            "runner",
            &registration,
            &labels,
            crate::REQUIRED_RUNNER_VERSION,
            false,
        )
        .await
        .unwrap();
        let plan = workflow::validate_workflow(
            &settings,
            "ci.yml".to_owned(),
            b"on: push\njobs:\n  build:\n    runs-on: docker\n    steps: []\n".to_vec(),
        )
        .unwrap();
        let run = workflow::enqueue_plan(
            &database,
            &repository,
            &workflow::RunTrigger {
                event: "push",
                reference: "refs/heads/main",
                before: "",
                after: "abc",
                actor: Some(&owner),
                event_json: serde_json::json!({}),
            },
            plan,
        )
        .await
        .unwrap();
        let claimed = runners::claim(&database, &runner.row, Uuid::new_v4(), None, 300)
            .await
            .unwrap()
            .unwrap();
        runs::update_task(
            &database,
            runner.row.id,
            claimed.job.id,
            runs::TaskResult::Heartbeat,
            BTreeMap::new(),
            "[]".to_owned(),
            300,
        )
        .await
        .unwrap();
        let directory = TestDirectory(
            std::env::temp_dir().join(format!("gitadel-artifact-service-test-{}", Uuid::new_v4())),
        );
        let service = ArtifactService::new(&database, &directory.0, &settings);
        let actions = crate::ActionsService::new(
            database.clone(),
            gitadel_git::content::RepositoryContent::new(PathBuf::from("unused")),
            settings.clone(),
            gitadel_secrets::SecretCipher::from_material(b"test host key").unwrap(),
            directory.0.clone(),
            std::sync::Arc::new(NoRunEffects),
        );
        let invalid_runner = crate::runner_service::RunnerCredential::new(
            runner.row.uuid.clone(),
            "invalid".to_owned(),
        );
        assert!(matches!(
            actions
                .append_logs(&invalid_runner, claimed.job.id, 0, vec![])
                .await,
            Err(crate::logs::LogError::Credential(
                runners::RunnerError::Unauthorized
            ))
        ));
        let runner_credential =
            crate::runner_service::RunnerCredential::new(runner.row.uuid.clone(), runner.token);
        let rows = || {
            vec![crate::logs::IncomingLogRow {
                timestamp: Utc::now(),
                content: "gta_job_secret".to_owned(),
            }]
        };
        assert_eq!(
            actions
                .append_logs(&runner_credential, claimed.job.id, 0, rows())
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            actions
                .append_logs(&runner_credential, claimed.job.id, 0, rows())
                .await
                .unwrap(),
            1
        );
        assert!(matches!(
            service
                .create(
                    &claimed.checkout_token,
                    Uuid::new_v4(),
                    claimed.job.id,
                    "report",
                    None
                )
                .await,
            Err(IdentityError::Forbidden(_))
        ));
        let upload = service
            .create(
                &claimed.checkout_token,
                run.id,
                claimed.job.id,
                "report",
                None,
            )
            .await
            .unwrap();
        let grant = service.authorize_upload(&upload).await.unwrap();
        let oversized = futures_util::stream::iter([Ok::<_, std::io::Error>(
            bytes::Bytes::from_static(b"123456"),
        )]);
        assert!(matches!(
            service
                .upload(grant, "full".to_owned(), false, oversized)
                .await,
            Err(IdentityError::BadRequest(_))
        ));
        let grant = service.authorize_upload(&upload).await.unwrap();
        let content = futures_util::stream::iter([Ok::<_, std::io::Error>(
            bytes::Bytes::from_static(b"12345"),
        )]);
        service
            .upload(grant, "full".to_owned(), false, content)
            .await
            .unwrap();
        let hash = format!("sha256:{}", hex::encode(Sha256::digest(b"12345")));
        let artifact_id = service
            .finalize(
                &claimed.checkout_token,
                run.id,
                claimed.job.id,
                "report",
                5,
                Some(&hash),
            )
            .await
            .unwrap();
        let download = service
            .signed_download(&claimed.checkout_token, run.id, "report")
            .await
            .unwrap();
        let (artifact, mut file) = service.download(&download).await.unwrap();
        assert_eq!(artifact.id, artifact_id);
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).await.unwrap();
        assert_eq!(bytes, b"12345");
        tokens::revoke_job(&database, claimed.job.id).await.unwrap();
        assert!(matches!(
            service.download(&download).await,
            Err(IdentityError::Unauthorized)
        ));
        assert!(matches!(
            service
                .signed_download(&claimed.checkout_token, run.id, "report")
                .await,
            Err(IdentityError::Unauthorized)
        ));
    }
}
