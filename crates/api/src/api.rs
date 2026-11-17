use std::sync::LazyLock;

use axum::{Json, response::IntoResponse};
use serde::Serialize;

use crate::repository::render_markdown;

/// Embedded at build time so a running instance can always document itself,
/// without depending on the source tree being present on the host.
const CHANGELOG: &str = include_str!("../../../CHANGELOG.md");

/// Rendering is identical for every caller, so it happens once per process.
static CHANGELOG_HTML: LazyLock<String> = LazyLock::new(|| render_markdown(CHANGELOG));

#[derive(Serialize, utoipa::ToSchema)]
pub struct VersionResponse {
    pub api_version: &'static str,
    pub application_version: &'static str,
}

#[utoipa::path(
    get, path = "/",
    operation_id = "get",
    summary = "Version",
    tag = "Instance",
    security(),
    responses(
        (status = 200, description = "Success", body = VersionResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn version() -> impl IntoResponse + crate::openapi::TypedResponse {
    Json(VersionResponse {
        api_version: "v1",
        application_version: env!("CARGO_PKG_VERSION"),
    })
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct ForgejoVersionResponse {
    pub version: &'static str,
}

/// Legacy Forgejo clients gate otherwise-compatible API calls on a Gitea-style version.
const FORGEJO_COMPAT_VERSION: &str = "1.22.0";

#[utoipa::path(
    get, path = "/version",
    operation_id = "get_version",
    summary = "Forgejo version",
    tag = "Instance",
    security(),
    responses(
        (status = 200, description = "Success", body = ForgejoVersionResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn forgejo_version() -> impl IntoResponse + crate::openapi::TypedResponse {
    Json(ForgejoVersionResponse {
        version: FORGEJO_COMPAT_VERSION,
    })
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct ChangelogResponse {
    pub application_version: &'static str,
    pub rendered_html: &'static str,
}

#[utoipa::path(
    get, path = "/changelog",
    operation_id = "get_changelog",
    summary = "Changelog",
    tag = "Instance",
    security(),
    responses(
        (status = 200, description = "Success", body = ChangelogResponse, content_type = "application/json"),
        (status = "4XX", description = "Invalid input, authentication, permissions, missing resource or conflict", body = crate::identity::ErrorEnvelope),
        (status = "5XX", description = "Server or upstream failure", body = crate::identity::ErrorEnvelope)
    )
)]
pub async fn changelog() -> impl IntoResponse + crate::openapi::TypedResponse {
    Json(ChangelogResponse {
        application_version: env!("CARGO_PKG_VERSION"),
        rendered_html: CHANGELOG_HTML.as_str(),
    })
}
