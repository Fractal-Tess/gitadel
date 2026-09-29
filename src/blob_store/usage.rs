//! Per-repository usage reporting for storage domains.
//!
//! Each [`StorageDomain`](super::StorageDomain) that is shown to
//! administrators pairs with a [`DomainUsage`] provider. The provider only
//! measures; filtering, sorting, and pagination are shared by every domain, so
//! a provider can move from scanning storage to reading a catalog table
//! without touching the HTTP API.

use std::{
    collections::{BTreeMap, HashMap},
    ops::AddAssign,
};

use anyhow::Result;
use async_trait::async_trait;
use sea_orm::DatabaseConnection;
use serde::Serialize;
use uuid::Uuid;

use super::DomainStorage;
use crate::config::StorageSettings;

/// How a usage detail is measured, so clients can format it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageUnit {
    Count,
    Bytes,
}

/// A domain-specific counter reported beside object count and bytes, such as
/// the registry's tag count.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct UsageDetail {
    pub key: &'static str,
    pub label: &'static str,
    pub unit: UsageUnit,
    /// Whether the counter is meaningful per repository, or only as a total.
    pub per_repository: bool,
}

/// Stored data one repository owns in one domain. `total_bytes` counts
/// persisted objects; temporary data belongs in `details`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct RepositoryUsage {
    pub object_count: u64,
    pub total_bytes: u64,
    pub details: BTreeMap<&'static str, u64>,
}

impl AddAssign<&RepositoryUsage> for RepositoryUsage {
    fn add_assign(&mut self, other: &RepositoryUsage) {
        self.object_count = self.object_count.saturating_add(other.object_count);
        self.total_bytes = self.total_bytes.saturating_add(other.total_bytes);
        for (key, value) in &other.details {
            let total = self.details.entry(key).or_default();
            *total = total.saturating_add(*value);
        }
    }
}

/// What a provider may consult while measuring.
#[derive(Clone, Copy)]
pub struct UsageContext<'a> {
    pub database: &'a DatabaseConnection,
    pub settings: &'a StorageSettings,
    pub storage: &'a DomainStorage,
}

/// Measures one storage domain per repository.
#[async_trait]
pub trait DomainUsage: Send + Sync + 'static {
    /// Domain-specific counters every [`RepositoryUsage::details`] map uses.
    fn details(&self) -> &'static [UsageDetail] {
        &[]
    }

    /// Usage keyed by repository id. Repositories without stored data may be
    /// omitted; soft-deleted repositories are included so totals account for
    /// everything still held.
    async fn by_repository(
        &self,
        context: UsageContext<'_>,
    ) -> Result<HashMap<Uuid, RepositoryUsage>>;
}
