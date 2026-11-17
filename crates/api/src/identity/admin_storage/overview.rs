//! One answer to "how full are my disks, and how much of that is Gitadel":
//! every place Gitadel keeps data, what each holds, and the volume it sits on.

use axum::{Json, extract::State, http::HeaderMap};
use axum_extra::extract::cookie::CookieJar;
use chrono::{DateTime, Utc};
use serde::Serialize;

use super::super::{ApiError, IdentityState, SCOPE_READ, require_admin_principal};

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct StorageOverview {
    volumes: Vec<Volume>,
    categories: Vec<Category>,
    measured_at: DateTime<Utc>,
}

/// A mounted filesystem holding at least one category. `used_bytes` counts
/// everything on it, Gitadel included; space the filesystem keeps back for
/// root is neither used nor available, so it is reported on its own instead
/// of making the other figures fail to add up.
#[derive(Debug, Serialize, utoipa::ToSchema)]
struct Volume {
    id: String,
    mount_point: Option<String>,
    total_bytes: u64,
    used_bytes: u64,
    available_bytes: u64,
    reserved_bytes: u64,
    gitadel_bytes: u64,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
struct Category {
    key: &'static str,
    label: &'static str,
    location: String,
    bytes: u64,
    /// `None` for object storage and external databases, which sit on no
    /// volume this host can see.
    volume_id: Option<String>,
}

#[utoipa::path(
    get, path = "/admin/storage/overview",
    operation_id = "get_admin_storage_overview",
    summary = "Storage overview",
    tag = "Storage",
    responses(
        (status = 200, description = "Success", body = StorageOverview, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn storage_overview(
    State(state): State<IdentityState>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<StorageOverview>, ApiError> {
    let actor = require_admin_principal(&state, &headers, &jar, SCOPE_READ).await?;
    Ok(Json(
        domain::storage_overview(&state.storage_service(), &actor)
            .await?
            .into(),
    ))
}

use gitadel_storage::overview as domain;
impl From<domain::StorageOverview> for StorageOverview {
    fn from(v: domain::StorageOverview) -> Self {
        Self {
            volumes: v.volumes.into_iter().map(Into::into).collect(),
            categories: v.categories.into_iter().map(Into::into).collect(),
            measured_at: v.measured_at,
        }
    }
}
impl From<domain::Volume> for Volume {
    fn from(v: domain::Volume) -> Self {
        Self {
            id: v.id,
            mount_point: v.mount_point,
            total_bytes: v.total_bytes,
            used_bytes: v.used_bytes,
            available_bytes: v.available_bytes,
            reserved_bytes: v.reserved_bytes,
            gitadel_bytes: v.gitadel_bytes,
        }
    }
}
impl From<domain::Category> for Category {
    fn from(v: domain::Category) -> Self {
        Self {
            key: v.key,
            label: v.label,
            location: v.location,
            bytes: v.bytes,
            volume_id: v.volume_id,
        }
    }
}
