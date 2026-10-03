//! Backup schedules: the stored S3 settings, the per-provider cron schedule,
//! and the scheduler loop that claims due schedules and runs them.

use super::*;

pub async fn load_s3_settings(
    database: &DatabaseConnection,
    fallback: Option<&S3Settings>,
) -> Result<Option<S3Settings>> {
    Ok(backup_provider::list(database, fallback)
        .await?
        .into_iter()
        .find_map(|provider| match provider.config {
            BackupProviderConfig::S3(settings) => Some(settings),
            BackupProviderConfig::Filesystem(_) => None,
        }))
}

pub async fn load_backup_schedule(
    database: &DatabaseConnection,
    provider_id: Uuid,
) -> Result<Option<BackupSchedule>> {
    Ok(backup_provider_schedule::Entity::find_by_id(provider_id)
        .one(database)
        .await?
        .map(|stored| BackupSchedule {
            schedule: stored.schedule,
            next_backup_at: stored.next_backup_at,
        }))
}

pub async fn list_backup_schedules(
    database: &DatabaseConnection,
) -> Result<BTreeMap<Uuid, BackupSchedule>> {
    Ok(backup_provider_schedule::Entity::find()
        .all(database)
        .await?
        .into_iter()
        .map(|stored| {
            (
                stored.provider_id,
                BackupSchedule {
                    schedule: stored.schedule,
                    next_backup_at: stored.next_backup_at,
                },
            )
        })
        .collect())
}

pub fn validate_backup_schedule(value: Option<&str>) -> Result<()> {
    prepare_backup_schedule(value, Utc::now()).map(drop)
}

pub(super) fn prepare_backup_schedule(
    value: Option<&str>,
    after: DateTime<Utc>,
) -> Result<Option<BackupSchedule>> {
    let schedule = value.map(str::trim).filter(|value| !value.is_empty());
    let Some(schedule) = schedule else {
        return Ok(None);
    };
    ensure!(
        schedule.len() <= MAX_BACKUP_SCHEDULE_LENGTH,
        "backup schedules cannot exceed {MAX_BACKUP_SCHEDULE_LENGTH} characters"
    );
    let next_backup_at = crate::schedule::next_occurrence(schedule, after)
        .with_context(|| format!("backup schedule {schedule:?}"))?;
    Ok(Some(BackupSchedule {
        schedule: schedule.to_owned(),
        next_backup_at,
    }))
}

pub async fn save_backup_schedule(
    database: &DatabaseConnection,
    provider_id: Uuid,
    value: Option<&str>,
) -> Result<Option<BackupSchedule>> {
    let now = Utc::now();
    let Some(prepared) = prepare_backup_schedule(value, now)? else {
        backup_provider_schedule::Entity::delete_by_id(provider_id)
            .exec(database)
            .await?;
        return Ok(None);
    };
    let schedule = &prepared.schedule;
    let next_backup_at = prepared.next_backup_at;
    match backup_provider_schedule::Entity::find_by_id(provider_id)
        .one(database)
        .await?
    {
        Some(stored) => {
            let mut active: backup_provider_schedule::ActiveModel = stored.into();
            active.schedule = Set(schedule.clone());
            active.next_backup_at = Set(next_backup_at);
            active.updated_at = Set(now);
            active.update(database).await?;
        }
        None => {
            backup_provider_schedule::ActiveModel {
                provider_id: Set(provider_id),
                schedule: Set(schedule.clone()),
                next_backup_at: Set(next_backup_at),
                updated_at: Set(now),
            }
            .insert(database)
            .await?;
        }
    }
    Ok(Some(prepared))
}

pub async fn serve_backup_scheduler(state: IdentityState) -> Result<()> {
    let mut interval = tokio::time::interval(BACKUP_SCHEDULER_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        interval.tick().await;
        if let Err(error) = schedule_due_backup(&state).await {
            tracing::warn!(%error, "scheduled automatic backup could not start");
        }
    }
}

pub(super) async fn schedule_due_backup(state: &IdentityState) -> Result<()> {
    let Some(claimed) = claim_due_backup_schedule(state.database(), Utc::now()).await? else {
        return Ok(());
    };
    let settings = state
        .runtime_settings()
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    let Some(provider) = backup_provider::load_with_storage(
        state.database(),
        settings.backup.s3.as_ref(),
        &settings.storage.lfs_root,
        claimed.provider_id,
    )
    .await?
    else {
        tracing::warn!(
            provider_id = %claimed.provider_id,
            schedule = %claimed.schedule,
            "automatic backup is due but its provider does not exist"
        );
        return Ok(());
    };
    let backup_name = "scheduled automatic backup";
    let key = new_backup_key(&provider.config, Some(backup_name))?;
    let operation_id = Uuid::new_v4();
    if let Err(error) = state
        .schedule_maintenance(MaintenanceAction::Create {
            operation_id,
            key: key.clone(),
            name: Some(backup_name.to_owned()),
            provider: provider.clone(),
        })
        .await
    {
        restore_claimed_backup_schedule(state.database(), &claimed).await?;
        bail!("could not reserve maintenance: {error}");
    }
    state
        .audit(None, "backup.create.automatic", Some(key.clone()))
        .await
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    tracing::info!(
        %operation_id,
        provider_id = %provider.id,
        %key,
        schedule = %claimed.schedule,
        next_backup_at = %claimed.next_backup_at,
        "scheduled automatic backup"
    );
    Ok(())
}

pub(super) async fn claim_due_backup_schedule(
    database: &DatabaseConnection,
    now: DateTime<Utc>,
) -> Result<Option<ClaimedBackupSchedule>> {
    let Some(stored) = backup_provider_schedule::Entity::find()
        .filter(backup_provider_schedule::Column::NextBackupAt.lte(now))
        .order_by_asc(backup_provider_schedule::Column::NextBackupAt)
        .one(database)
        .await?
    else {
        return Ok(None);
    };
    let next_backup_at = crate::schedule::next_occurrence(&stored.schedule, now)
        .with_context(|| format!("stored backup schedule {:?}", stored.schedule))?;
    let updated = backup_provider_schedule::Entity::update_many()
        .col_expr(
            backup_provider_schedule::Column::NextBackupAt,
            Expr::value(next_backup_at),
        )
        .col_expr(
            backup_provider_schedule::Column::UpdatedAt,
            Expr::value(now),
        )
        .filter(backup_provider_schedule::Column::ProviderId.eq(stored.provider_id))
        .filter(backup_provider_schedule::Column::Schedule.eq(&stored.schedule))
        .filter(backup_provider_schedule::Column::NextBackupAt.eq(stored.next_backup_at))
        .exec(database)
        .await?;
    if updated.rows_affected == 0 {
        return Ok(None);
    }
    Ok(Some(ClaimedBackupSchedule {
        provider_id: stored.provider_id,
        schedule: stored.schedule,
        previous_next_backup_at: stored.next_backup_at,
        next_backup_at,
    }))
}

pub(super) async fn restore_claimed_backup_schedule(
    database: &DatabaseConnection,
    claimed: &ClaimedBackupSchedule,
) -> Result<()> {
    backup_provider_schedule::Entity::update_many()
        .col_expr(
            backup_provider_schedule::Column::NextBackupAt,
            Expr::value(claimed.previous_next_backup_at),
        )
        .col_expr(
            backup_provider_schedule::Column::UpdatedAt,
            Expr::value(Utc::now()),
        )
        .filter(backup_provider_schedule::Column::ProviderId.eq(claimed.provider_id))
        .filter(backup_provider_schedule::Column::Schedule.eq(&claimed.schedule))
        .filter(backup_provider_schedule::Column::NextBackupAt.eq(claimed.next_backup_at))
        .exec(database)
        .await?;
    Ok(())
}

pub(super) async fn resolve_s3_settings(settings: &Settings) -> Result<S3Settings> {
    let database_path = sqlite_path(&settings.database.url)?;
    if database_path.is_file() {
        let database = database::connect_and_migrate(&settings.database).await?;
        let resolved = load_s3_settings(&database, settings.backup.s3.as_ref()).await;
        database
            .close()
            .await
            .context("could not close backup settings connection")?;
        return resolved?.context("backup S3 storage is not configured");
    }
    settings
        .backup
        .s3
        .clone()
        .context("backup S3 storage is not configured")
}
