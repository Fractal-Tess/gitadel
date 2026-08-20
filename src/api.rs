use axum::{Json, response::IntoResponse};
use serde::Serialize;

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
