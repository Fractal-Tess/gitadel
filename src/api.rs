use std::sync::LazyLock;

use axum::{Json, response::IntoResponse};
use serde::Serialize;

use crate::repository::render_markdown;

/// Embedded at build time so a running instance can always document itself,
/// without depending on the source tree being present on the host.
const CHANGELOG: &str = include_str!("../CHANGELOG.md");

/// Rendering is identical for every caller, so it happens once per process.
static CHANGELOG_HTML: LazyLock<String> = LazyLock::new(|| render_markdown(CHANGELOG));

#[derive(Serialize)]
pub struct VersionResponse {
    pub api_version: &'static str,
    pub application_version: &'static str,
}

pub async fn version() -> impl IntoResponse {
    Json(VersionResponse {
        api_version: "v1",
        application_version: env!("CARGO_PKG_VERSION"),
    })
}

#[derive(Serialize)]
pub struct ChangelogResponse {
    pub application_version: &'static str,
    pub rendered_html: &'static str,
}

pub async fn changelog() -> impl IntoResponse {
    Json(ChangelogResponse {
        application_version: env!("CARGO_PKG_VERSION"),
        rendered_html: CHANGELOG_HTML.as_str(),
    })
}
