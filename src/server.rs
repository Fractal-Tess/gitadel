use std::borrow::Cow;

use anyhow::{Context, Result};
use axum::{
    Json, Router,
    body::{Body, Bytes},
    extract::State,
    http::{HeaderValue, StatusCode, Uri, header},
    response::{IntoResponse, Response},
    routing::get,
};
use rust_embed::RustEmbed;
use sea_orm::DatabaseConnection;
use serde::Serialize;
use tokio::net::TcpListener;
use tower_http::trace::TraceLayer;
use tracing::info;

use crate::{
    api,
    config::Settings,
    identity::{self, IdentityState},
    repository::{self, RepositoryState},
};

#[derive(RustEmbed)]
#[folder = "frontend/build/"]
struct FrontendAssets;

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
    database: &'static str,
}

pub async fn serve(settings: Settings, database: DatabaseConnection) -> Result<()> {
    let http_bind = settings.server.bind;
    let listener = TcpListener::bind(http_bind)
        .await
        .with_context(|| format!("could not bind HTTP listener to {http_bind}"))?;

    let identity_state =
        IdentityState::new(database, settings.auth, settings.server.public_url.clone())
            .context("could not initialize authentication")?;
    let ssh_port = settings.ssh.bind.port();
    let repository_state = RepositoryState::new(
        identity_state.clone(),
        settings.storage,
        settings.server.public_url,
        ssh_port,
    )
    .await
    .context("could not initialize repository storage")?;
    let api_router = Router::new()
        .route("/", get(api::version))
        .merge(identity::router().with_state(identity_state.clone()))
        .merge(repository::router().with_state(repository_state.clone()));
    let app = Router::new()
        .merge(
            Router::new()
                .route("/healthz", get(health))
                .with_state(identity_state.clone()),
        )
        .merge(identity::oauth_router().with_state(identity_state))
        .nest("/api/v1", api_router)
        .merge(repository::git_http_router().with_state(repository_state.clone()))
        .fallback(get(frontend))
        .layer(TraceLayer::new_for_http());

    info!(address = %http_bind, "Gitadel HTTP server listening");
    let mut http = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(shutdown_signal())
            .await
            .context("HTTP server stopped unexpectedly")
    });
    let mut ssh = tokio::spawn(repository::serve_ssh(settings.ssh, repository_state));
    tokio::select! {
        result = &mut http => {
            ssh.abort();
            result.context("HTTP server task failed")?
        }
        result = &mut ssh => {
            http.abort();
            result.context("SSH server task failed")?
        }
    }
}

async fn health(State(state): State<IdentityState>) -> Result<Json<HealthResponse>, StatusCode> {
    state.database().ping().await.map_err(|error| {
        tracing::error!(%error, "database health check failed");
        StatusCode::SERVICE_UNAVAILABLE
    })?;

    Ok(Json(HealthResponse {
        status: "ok",
        database: "ok",
    }))
}

async fn frontend(uri: Uri) -> Response {
    let requested = uri.path().trim_start_matches('/');
    let asset_name = if requested.is_empty() {
        "index.html"
    } else {
        requested
    };

    if let Some(asset) = FrontendAssets::get(asset_name) {
        return asset_response(asset_name, asset.data);
    }

    if !requested.contains('.')
        && let Some(index) = FrontendAssets::get("index.html")
    {
        return asset_response("index.html", index.data);
    }

    if FrontendAssets::get("index.html").is_none() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "Frontend assets are not built. Run `bun run --cwd frontend build`.",
        )
            .into_response();
    }

    StatusCode::NOT_FOUND.into_response()
}

fn asset_response(path: &str, data: Cow<'static, [u8]>) -> Response {
    let bytes = match data {
        Cow::Borrowed(bytes) => Bytes::from_static(bytes),
        Cow::Owned(bytes) => Bytes::from(bytes),
    };
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    let mut response = Response::new(Body::from(bytes));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(mime.as_ref())
            .unwrap_or(HeaderValue::from_static("application/octet-stream")),
    );
    response
}

async fn shutdown_signal() {
    let interrupt = async {
        if let Err(error) = tokio::signal::ctrl_c().await {
            tracing::warn!(%error, "could not install Ctrl-C handler");
            std::future::pending::<()>().await;
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(error) => {
                tracing::warn!(%error, "could not install SIGTERM handler");
                std::future::pending::<()>().await;
            }
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = interrupt => {},
        () = terminate => {},
    }
}
