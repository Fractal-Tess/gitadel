//! Container registry usage for the administrator storage API.
//!
//! This adapter is the only place the storage API learns how registry usage is
//! measured, so the measurement can move from scanning repository directories
//! to catalog tables without changing the API.

use std::collections::HashMap;

use anyhow::Result;
use async_trait::async_trait;
use uuid::Uuid;

use super::{storage, store::RegistryUsage};
use crate::blob_store::{DomainUsage, RepositoryUsage, UsageContext, UsageDetail, UsageUnit};

const DETAILS: &[UsageDetail] = &[
    UsageDetail {
        key: "blob_count",
        label: "Blobs",
        unit: UsageUnit::Count,
        per_repository: true,
    },
    UsageDetail {
        key: "manifest_count",
        label: "Manifests",
        unit: UsageUnit::Count,
        per_repository: true,
    },
    UsageDetail {
        key: "tag_count",
        label: "Tags",
        unit: UsageUnit::Count,
        per_repository: true,
    },
    UsageDetail {
        key: "image_count",
        label: "Images",
        unit: UsageUnit::Count,
        per_repository: true,
    },
    UsageDetail {
        key: "upload_count",
        label: "Staged uploads",
        unit: UsageUnit::Count,
        per_repository: false,
    },
    UsageDetail {
        key: "upload_bytes",
        label: "Staged upload bytes",
        unit: UsageUnit::Bytes,
        per_repository: false,
    },
];

/// Registry usage: blob and manifest payloads from the active store, plus
/// tags, images, and staged uploads from registry metadata.
pub(crate) struct RegistryUsageProvider;

#[async_trait]
impl DomainUsage for RegistryUsageProvider {
    fn details(&self) -> &'static [UsageDetail] {
        DETAILS
    }

    async fn by_repository(
        &self,
        context: UsageContext<'_>,
    ) -> Result<HashMap<Uuid, RepositoryUsage>> {
        let rows = storage::usage_by_repository(
            context.database,
            &context.settings.repository_root,
            context.storage,
        )
        .await?;
        Ok(rows
            .into_iter()
            .map(|(repository, usage)| (repository.id, repository_usage(usage)))
            .collect())
    }
}

fn repository_usage(usage: RegistryUsage) -> RepositoryUsage {
    RepositoryUsage {
        object_count: usage.object_count,
        total_bytes: usage.total_bytes,
        details: [
            ("blob_count", usage.blob_count),
            ("manifest_count", usage.manifest_count),
            ("tag_count", usage.tag_count),
            ("image_count", usage.image_count),
            ("upload_count", usage.upload_count),
            ("upload_bytes", usage.upload_bytes),
        ]
        .into_iter()
        .collect(),
    }
}
