//! Installation settings, branding and audit administration.
use chrono::{DateTime, Utc};
use gitadel_core::audit::AuditContext;
use gitadel_db::entity::{audit_event, instance, instance_asset, user};
use gitadel_identity::{AccountService, IdentityError as ServiceError, Principal};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, QueryFilter,
    QueryOrder, QuerySelect, Set, TransactionTrait,
};
use std::time::Duration;
use tokio::sync::watch;
use uuid::Uuid;
pub struct InstanceService<'a> {
    database: &'a DatabaseConnection,
    audit: AuditContext,
    integrity_version: &'a watch::Sender<u64>,
}
impl<'a> InstanceService<'a> {
    pub fn new(
        database: &'a DatabaseConnection,
        audit: AuditContext,
        integrity_version: &'a watch::Sender<u64>,
    ) -> Self {
        Self {
            database,
            audit,
            integrity_version,
        }
    }
    pub fn database(&self) -> &DatabaseConnection {
        self.database
    }
    pub async fn require_admin(
        &self,
        actor: &Principal,
        scope: i32,
    ) -> Result<gitadel_identity::AuthenticatedUser, ServiceError> {
        AccountService::new(self.database, self.audit)
            .require_actor(actor, true, scope)
            .await
    }
    pub async fn audit_on<C: ConnectionTrait>(
        &self,
        c: &C,
        actor: Option<Uuid>,
        action: impl Into<String>,
        target: Option<String>,
    ) -> Result<(), ServiceError> {
        gitadel_db::audit::record(c, actor, action, target, self.audit).await?;
        Ok(())
    }
    pub fn notify_integrity_settings_changed(&self) {
        self.integrity_version
            .send_modify(|version| *version = version.wrapping_add(1));
    }
}
pub const MAX_FAVICON_BYTES: usize = 512 * 1024;
const MIN_FAVICON_EDGE: u32 = 16;
const MAX_FAVICON_EDGE: u32 = 1024;
const PNG_SIGNATURE: [u8; 8] = [137, 80, 78, 71, 13, 10, 26, 10];
pub struct IntegritySettingsResponse {
    pub enabled: bool,
    pub schedule: String,
    pub last_checked_at: Option<DateTime<Utc>>,
    pub last_result: Option<String>,
}
pub struct UpdateIntegritySettingsRequest {
    pub enabled: bool,
    pub schedule: String,
}
pub struct UpdateInstanceSettingsRequest {
    pub site_name: String,
    pub site_description: Option<String>,
}
pub async fn integrity_settings_response(
    state: &InstanceService<'_>,
) -> Result<IntegritySettingsResponse, ServiceError> {
    let settings = instance::Entity::find_by_id(1)
        .one(state.database())
        .await?
        .ok_or_else(|| ServiceError::internal("instance settings row is missing"))?;
    let last_check = audit_event::Entity::find()
        .filter(audit_event::Column::Action.eq("repository.integrity.check"))
        .order_by_desc(audit_event::Column::Id)
        .one(state.database())
        .await?;

    Ok(IntegritySettingsResponse {
        enabled: settings.integrity_checks_enabled,
        schedule: settings.integrity_check_schedule,
        last_checked_at: last_check.as_ref().map(|event| event.created_at),
        last_result: last_check.and_then(|event| event.target),
    })
}
pub fn validate_png(bytes: &[u8]) -> Result<(), ServiceError> {
    if bytes.len() > MAX_FAVICON_BYTES {
        return Err(ServiceError::bad_request(
            "Favicon files cannot exceed 512 KiB.",
        ));
    }
    if bytes.len() < 24
        || !bytes.starts_with(&PNG_SIGNATURE)
        || bytes.get(12..16) != Some(b"IHDR".as_slice())
    {
        return Err(invalid_png());
    }

    let width = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
    let height = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
    if width != height || !(MIN_FAVICON_EDGE..=MAX_FAVICON_EDGE).contains(&width) {
        return Err(ServiceError::bad_request(
            "Favicons must be square and between 16 and 1024 pixels.",
        ));
    }

    if !gitadel_identity::images::valid_png(bytes) {
        return Err(invalid_png());
    }
    Ok(())
}
pub fn invalid_png() -> ServiceError {
    ServiceError::bad_request("The uploaded file is not a valid PNG.")
}
pub async fn touch_instance<C: ConnectionTrait>(
    connection: &C,
    now: chrono::DateTime<Utc>,
) -> Result<(), ServiceError> {
    let settings = instance::Entity::find_by_id(1)
        .one(connection)
        .await?
        .ok_or_else(|| ServiceError::internal("instance settings row is missing"))?;
    let mut active: instance::ActiveModel = settings.into();
    active.updated_at = Set(now);
    active.update(connection).await?;
    Ok(())
}
pub async fn prune_audit_events<C: ConnectionTrait>(
    connection: &C,
    retention_days: i64,
    now: DateTime<Utc>,
) -> Result<u64, sea_orm::DbErr> {
    if retention_days <= 0 {
        return Ok(0);
    }
    let cutoff = now - chrono::Duration::days(retention_days);
    Ok(audit_event::Entity::delete_many()
        .filter(audit_event::Column::CreatedAt.lt(cutoff))
        .exec(connection)
        .await?
        .rows_affected)
}
pub async fn public_instance_settings(
    state: &InstanceService<'_>,
) -> Result<instance::Model, ServiceError> {
    let settings = instance::Entity::find_by_id(1)
        .one(state.database())
        .await?
        .ok_or_else(|| ServiceError::internal("instance settings row is missing"))?;
    Ok(settings)
}
pub async fn get_instance_settings(
    state: &InstanceService<'_>,
    actor: &Principal,
) -> Result<instance::Model, ServiceError> {
    let _actor = state
        .require_admin(actor, gitadel_identity::SCOPE_READ)
        .await?;
    let settings = instance::Entity::find_by_id(1)
        .one(state.database())
        .await?
        .ok_or_else(|| ServiceError::internal("instance settings row is missing"))?;
    Ok(settings)
}
pub async fn update_instance_settings(
    state: &InstanceService<'_>,
    actor: &Principal,
    request: UpdateInstanceSettingsRequest,
) -> Result<instance::Model, ServiceError> {
    let actor = state
        .require_admin(actor, gitadel_identity::SCOPE_WRITE)
        .await?;
    let site_name = request.site_name.trim();
    if site_name.is_empty() || site_name.len() > 80 {
        return Err(ServiceError::bad_request(
            "Site name must contain between 1 and 80 characters.",
        ));
    }
    let site_description = request
        .site_description
        .map(|description| description.trim().to_owned())
        .filter(|description| !description.is_empty());
    if site_description
        .as_ref()
        .is_some_and(|description| description.len() > 280)
    {
        return Err(ServiceError::bad_request(
            "Site description cannot exceed 280 characters.",
        ));
    }

    let settings = instance::Entity::find_by_id(1)
        .one(state.database())
        .await?
        .ok_or_else(|| ServiceError::internal("instance settings row is missing"))?;
    let now = Utc::now();
    let mut active: instance::ActiveModel = settings.into();
    active.site_name = Set(site_name.to_owned());
    active.site_description = Set(site_description);
    active.updated_at = Set(now);
    let settings = active.update(state.database()).await?;

    gitadel_db::audit::record(
        state.database(),
        Some(actor.user.id),
        "instance.settings.update",
        None,
        AuditContext::default(),
    )
    .await?;

    Ok(settings)
}
pub async fn get_integrity_settings(
    state: &InstanceService<'_>,
    actor: &Principal,
) -> Result<IntegritySettingsResponse, ServiceError> {
    let _actor = state
        .require_admin(actor, gitadel_identity::SCOPE_READ)
        .await?;
    integrity_settings_response(state).await
}
pub async fn update_integrity_settings(
    state: &InstanceService<'_>,
    actor: &Principal,
    request: UpdateIntegritySettingsRequest,
) -> Result<IntegritySettingsResponse, ServiceError> {
    let actor = state
        .require_admin(actor, gitadel_identity::SCOPE_WRITE)
        .await?;
    let schedule = request.schedule.trim();
    if schedule.is_empty() || schedule.len() > 128 {
        return Err(ServiceError::bad_request(
            "Schedule must contain between 1 and 128 characters.",
        ));
    }
    gitadel_core::schedule::next_occurrence(schedule, Utc::now()).map_err(|error| {
        ServiceError::bad_request(format!("Invalid UTC cron schedule: {error}"))
    })?;

    let settings = instance::Entity::find_by_id(1)
        .one(state.database())
        .await?
        .ok_or_else(|| ServiceError::internal("instance settings row is missing"))?;
    let now = Utc::now();
    let mut active: instance::ActiveModel = settings.into();
    active.integrity_checks_enabled = Set(request.enabled);
    active.integrity_check_schedule = Set(schedule.to_owned());
    active.updated_at = Set(now);
    active.update(state.database()).await?;

    gitadel_db::audit::record(
        state.database(),
        Some(actor.user.id),
        "instance.integrity.update",
        Some(format!(
            "{}; {schedule}",
            if request.enabled {
                "enabled"
            } else {
                "disabled"
            }
        )),
        AuditContext::default(),
    )
    .await?;

    state.notify_integrity_settings_changed();
    integrity_settings_response(state).await
}
pub async fn update_instance_favicon(
    state: &InstanceService<'_>,
    actor: &Principal,
    theme: String,
    content_type: &str,
    body: &[u8],
) -> Result<(), ServiceError> {
    let actor = state
        .require_admin(actor, gitadel_identity::SCOPE_WRITE)
        .await?;
    let name = favicon_name(&theme)?;
    if content_type
        .split(';')
        .next()
        .is_none_or(|value| value.trim() != "image/png")
    {
        return Err(ServiceError::bad_request("Favicons must be PNG images."));
    }
    validate_png(body)?;

    let now = Utc::now();
    let transaction = state.database().begin().await?;
    match instance_asset::Entity::find_by_id(name)
        .one(&transaction)
        .await?
    {
        Some(asset) => {
            let mut active: instance_asset::ActiveModel = asset.into();
            active.content_type = Set("image/png".to_owned());
            active.content = Set(body.to_vec());
            active.updated_at = Set(now);
            active.update(&transaction).await?;
        }
        None => {
            instance_asset::ActiveModel {
                name: Set(name.to_owned()),
                content_type: Set("image/png".to_owned()),
                content: Set(body.to_vec()),
                updated_at: Set(now),
            }
            .insert(&transaction)
            .await?;
        }
    }
    touch_instance(&transaction, now).await?;
    state
        .audit_on(
            &transaction,
            Some(actor.user.id),
            "instance.favicon.update",
            Some(name.to_owned()),
        )
        .await?;
    transaction.commit().await?;

    Ok(())
}
pub async fn delete_instance_favicon(
    state: &InstanceService<'_>,
    actor: &Principal,
    theme: String,
) -> Result<(), ServiceError> {
    let actor = state
        .require_admin(actor, gitadel_identity::SCOPE_WRITE)
        .await?;
    let name = favicon_name(&theme)?;
    let transaction = state.database().begin().await?;
    instance_asset::Entity::delete_by_id(name)
        .exec(&transaction)
        .await?;
    let now = Utc::now();
    touch_instance(&transaction, now).await?;
    state
        .audit_on(
            &transaction,
            Some(actor.user.id),
            "instance.favicon.delete",
            Some(name.to_owned()),
        )
        .await?;
    transaction.commit().await?;
    Ok(())
}
pub fn favicon_name(theme: &str) -> Result<&'static str, ServiceError> {
    match theme {
        "light" => Ok("favicon-light"),
        "dark" => Ok("favicon-dark"),
        _ => Err(ServiceError::not_found()),
    }
}
pub async fn favicon(
    database: &DatabaseConnection,
    theme: &str,
) -> Result<Option<instance_asset::Model>, ServiceError> {
    Ok(instance_asset::Entity::find_by_id(favicon_name(theme)?)
        .one(database)
        .await?)
}
const AUDIT_RETENTION_INTERVAL: Duration = Duration::from_secs(60 * 60);
pub async fn serve_audit_retention(database: DatabaseConnection, retention_days: i64) {
    if retention_days <= 0 {
        return;
    }
    let mut interval = tokio::time::interval(AUDIT_RETENTION_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        interval.tick().await;
        match prune_audit_events(&database, retention_days, Utc::now()).await {
            Ok(0) => {}
            Ok(removed) => tracing::info!(removed, retention_days, "removed expired audit events"),
            Err(error) => tracing::warn!(%error, "could not remove expired audit events"),
        }
    }
}
pub async fn audit_page(
    database: &DatabaseConnection,
    limit: u64,
    offset: u64,
    before: Option<i64>,
) -> Result<Vec<(audit_event::Model, Option<user::Model>)>, sea_orm::DbErr> {
    let mut query = audit_event::Entity::find()
        .find_also_related(user::Entity)
        .order_by_desc(audit_event::Column::Id);
    if let Some(before) = before {
        query = query.filter(audit_event::Column::Id.lt(before));
    }
    query.limit(limit).offset(offset).all(database).await
}
pub async fn list_audit(
    state: &InstanceService<'_>,
    actor: &Principal,
    limit: Option<u64>,
    offset: Option<u64>,
    before: Option<i64>,
) -> Result<Vec<(audit_event::Model, Option<user::Model>)>, ServiceError> {
    let actor = AccountService::new(state.database, state.audit)
        .require_actor(actor, false, gitadel_identity::SCOPE_READ)
        .await?;
    if !actor.user.is_admin {
        return Err(ServiceError::forbidden(
            "Only an administrator can read the audit history.",
        ));
    }
    Ok(audit_page(
        state.database,
        limit.unwrap_or(100).clamp(1, 500),
        offset.unwrap_or(0),
        before,
    )
    .await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn direct_instance_updates_require_a_current_administrator() {
        let database = gitadel_db::test_support::database().await;
        let user = gitadel_db::test_support::owner(&database, "alice").await;
        let raw =
            gitadel_identity::create_session_on(&database, user.id, chrono::Duration::hours(1))
                .await
                .unwrap();
        let principal = gitadel_identity::IdentityService::new(&database)
            .authenticate_session_principal(&raw)
            .await
            .unwrap();
        let version = watch::channel(0).0;
        let service = InstanceService::new(&database, AuditContext::default(), &version);
        assert!(matches!(
            update_instance_settings(
                &service,
                &principal,
                UpdateInstanceSettingsRequest {
                    site_name: "changed".into(),
                    site_description: None
                }
            )
            .await,
            Err(ServiceError::Forbidden(_))
        ));
    }
}
