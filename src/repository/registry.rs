use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
};
use axum_extra::extract::cookie::CookieJar;
use serde::Serialize;

use super::{RepositoryState, browser::readable_repository};
use crate::{
    identity::ApiError,
    registry::store::{ImageMetadata, ImageReferenceMetadata, RegistryStore},
};

#[derive(Debug, Serialize)]
pub(crate) struct RegistryResponse {
    pub registry_host: String,
    pub image_prefix: String,
    pub images: Vec<RegistryImageResponse>,
}

#[derive(Debug, Serialize)]
pub(crate) struct RegistryImageResponse {
    pub name: String,
    pub size_bytes: u64,
    pub updated_at: Option<String>,
    pub references: Vec<RegistryReferenceResponse>,
}

#[derive(Debug, Serialize)]
pub(crate) struct RegistryReferenceResponse {
    pub tag: Option<String>,
    pub digest: String,
    pub updated_at: Option<String>,
}

pub(crate) async fn browse(
    State(state): State<RepositoryState>,
    Path((namespace, name)): Path<(String, String)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<RegistryResponse>, ApiError> {
    let repository = readable_repository(&state, &headers, &jar, &namespace, &name).await?;
    let _operation = state.registry_storage().lock_operation().await;
    let registry_host = registry_host(&state)?;
    let image_prefix = format!(
        "{registry_host}/{}/{}",
        repository.namespace, repository.name
    );
    let images = RegistryStore::new()
        .browse_images(
            &state.repository_path(&repository),
            repository.storage_key,
            state.registry_storage().store(),
        )
        .await
        .map_err(ApiError::internal)?
        .into_iter()
        .map(|image| image_response(&image_prefix, image))
        .collect();
    Ok(Json(RegistryResponse {
        registry_host,
        image_prefix,
        images,
    }))
}

fn image_response(prefix: &str, image: ImageMetadata) -> RegistryImageResponse {
    let name = if image.suffix.is_empty() {
        prefix.to_owned()
    } else {
        format!("{prefix}/{}", image.suffix)
    };
    RegistryImageResponse {
        name,
        size_bytes: image.size_bytes,
        updated_at: image.updated_at,
        references: image
            .references
            .into_iter()
            .map(reference_response)
            .collect(),
    }
}

fn reference_response(reference: ImageReferenceMetadata) -> RegistryReferenceResponse {
    RegistryReferenceResponse {
        tag: reference.tag,
        digest: reference.digest,
        updated_at: reference.updated_at,
    }
}

fn registry_host(state: &RepositoryState) -> Result<String, ApiError> {
    let url = state.public_url();
    let host = url
        .host_str()
        .ok_or_else(|| ApiError::internal("Public URL has no host."))?;
    let host = if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]")
    } else {
        host.to_owned()
    };
    Ok(match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host,
    })
}
