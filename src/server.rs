use std::{borrow::Cow, convert::Infallible, net::TcpListener as StdTcpListener};

use anyhow::{Context, Result};
use axum::{
    Json, Router,
    body::{Body, Bytes},
    extract::{Path, State},
    http::{HeaderValue, StatusCode, Uri, header},
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
    routing::get,
};
use futures_util::stream;
use rust_embed::RustEmbed;
use sea_orm::DatabaseConnection;
use serde::Serialize;
use tokio::{
    net::TcpListener,
    sync::{mpsc, watch},
    time::Duration,
};
use tokio_util::sync::CancellationToken;
use tower_http::{compression::CompressionLayer, trace::TraceLayer};
use tracing::info;

use crate::{
    actions::{self, ActionsState},
    api,
    archive::{self, MaintenanceAction, MaintenanceProgress, MaintenanceProgressReporter},
    config::Settings,
    identity::{self, IdentityState},
    repository::{self, GitHttpState, RepositoryState},
};

#[derive(RustEmbed)]
#[folder = "frontend/build/"]
struct FrontendAssets;

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
    database: &'static str,
}

pub enum ServerExit {
    Shutdown,
    Maintenance(Box<MaintenanceAction>),
}

pub async fn serve(settings: Settings, database: DatabaseConnection) -> Result<ServerExit> {
    let http_bind = settings.server.bind;
    let listener = StdTcpListener::bind(http_bind)
        .with_context(|| format!("could not bind web listener to {http_bind}"))?;
    listener
        .set_nonblocking(true)
        .context("could not configure web listener as non-blocking")?;
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let tls = if let Some(tls) = &settings.server.tls {
        Some(
            axum_server::tls_rustls::RustlsConfig::from_pem_file(
                &tls.certificate,
                &tls.private_key,
            )
            .await
            .with_context(|| {
                format!(
                    "could not load TLS certificate {} and private key {}",
                    tls.certificate.display(),
                    tls.private_key.display()
                )
            })?,
        )
    } else {
        None
    };

    let (maintenance_sender, mut maintenance_receiver) = mpsc::channel(1);
    let identity_state =
        IdentityState::new_with_runtime(database, settings.clone(), maintenance_sender)
            .context("could not initialize authentication")?;
    identity_state
        .initialize_lfs_storage(settings.storage.lfs_root.clone())
        .await
        .context("could not initialize LFS storage")?;
    let ssh_port = settings.ssh.bind.port();
    let repository_state = RepositoryState::new(
        identity_state.clone(),
        settings.storage.clone(),
        settings.server.public_url.clone(),
        ssh_port,
    )
    .await
    .context("could not initialize repository storage")?;
    repository::recover_imports(&repository_state)
        .await
        .context("could not recover interrupted repository imports")?;
    let actions_state = ActionsState::new(repository_state.clone(), settings.actions.clone());
    actions::bootstrap_system_runner(&actions_state)
        .await
        .context("could not prepare the configured system runner")?;
    let git_http_state = GitHttpState::new(repository_state.clone(), actions_state.clone());
    let api_router = Router::new()
        .route("/", get(api::version))
        .route("/version", get(api::forgejo_version))
        .route("/changelog", get(api::changelog))
        .merge(identity::router().with_state(identity_state.clone()))
        .merge(repository::router().with_state(repository_state.clone()))
        .merge(actions_state.api_router());
    let app = Router::new()
        .merge(
            Router::new()
                .route("/healthz", get(health))
                .with_state(identity_state.clone()),
        )
        .merge(identity::oauth_router().with_state(identity_state.clone()))
        .nest("/api/v1", api_router)
        .merge(actions_state.artifact_router())
        .nest("/api/actions", actions_state.protocol_router())
        .merge(repository::git_http_router().with_state(git_http_state))
        .fallback(get(frontend))
        .layer(CompressionLayer::new())
        .layer(TraceLayer::new_for_http());

    info!(
        address = %http_bind,
        protocol = if tls.is_some() { "https" } else { "http" },
        "Gitadel web server listening"
    );
    let shutdown = CancellationToken::new();
    let http_shutdown = shutdown.clone();
    let http_handle = axum_server::Handle::new();
    let graceful_handle = http_handle.clone();
    let mut http = tokio::spawn(async move {
        let shutdown_task = tokio::spawn(async move {
            tokio::select! {
                () = shutdown_signal() => {}
                () = http_shutdown.cancelled() => {}
            }
            graceful_handle.graceful_shutdown(None);
        });
        let result = if let Some(tls) = tls {
            axum_server::from_tcp_rustls(listener, tls)
                .context("could not configure HTTPS listener")?
                .handle(http_handle)
                .serve(app.into_make_service())
                .await
        } else {
            axum_server::from_tcp(listener)
                .context("could not configure HTTP listener")?
                .handle(http_handle)
                .serve(app.into_make_service())
                .await
        };
        shutdown_task.abort();
        result.context("web server stopped unexpectedly")
    });
    let mut mirror_scheduler =
        tokio::spawn(repository::serve_mirror_scheduler(repository_state.clone()));
    let mut integrity_scheduler = tokio::spawn(repository::serve_integrity_scheduler(
        repository_state.clone(),
    ));
    let mut backup_scheduler = tokio::spawn(archive::serve_backup_scheduler(identity_state));
    let mut actions_scheduler =
        tokio::spawn(actions::serve_actions_scheduler(actions_state.clone()));
    let mut ssh = tokio::spawn(repository::serve_ssh(
        settings.ssh,
        repository_state.clone(),
        actions_state.clone(),
    ));
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum CompletedService {
        Http,
        Ssh,
        MirrorScheduler,
        BackupScheduler,
        ActionsScheduler,
        IntegrityScheduler,
    }

    let (outcome, completed) = tokio::select! {
        result = &mut http => {
            ssh.abort();
            mirror_scheduler.abort();
            backup_scheduler.abort();
            integrity_scheduler.abort();
            actions_state.cancel();
            actions_scheduler.abort();
            (
                result
                    .context("HTTP server task failed")
                    .and_then(|result| result)
                    .map(|()| ServerExit::Shutdown),
                Some(CompletedService::Http),
            )
        }
        result = &mut ssh => {
            http.abort();
            mirror_scheduler.abort();
            backup_scheduler.abort();
            integrity_scheduler.abort();
            actions_state.cancel();
            actions_scheduler.abort();
            (
                result
                    .context("SSH server task failed")
                    .and_then(|result| result)
                    .map(|()| ServerExit::Shutdown),
                Some(CompletedService::Ssh),
            )
        }
        result = &mut mirror_scheduler => {
            http.abort();
            ssh.abort();
            backup_scheduler.abort();
            integrity_scheduler.abort();
            actions_state.cancel();
            actions_scheduler.abort();
            (
                result
                    .context("mirror scheduler task failed")
                    .and_then(|result| result)
                    .map(|()| ServerExit::Shutdown),
                Some(CompletedService::MirrorScheduler),
            )
        }
        result = &mut backup_scheduler => {
            http.abort();
            ssh.abort();
            mirror_scheduler.abort();
            integrity_scheduler.abort();
            actions_state.cancel();
            actions_scheduler.abort();
            (
                result
                    .context("backup scheduler task failed")
                    .and_then(|result| result)
                    .map(|()| ServerExit::Shutdown),
                Some(CompletedService::BackupScheduler),
            )
        }
        result = &mut actions_scheduler => {
            http.abort();
            ssh.abort();
            mirror_scheduler.abort();
            backup_scheduler.abort();
            integrity_scheduler.abort();
            (
                result
                    .context("Actions scheduler task failed")
                    .and_then(|result| result)
                    .map(|()| ServerExit::Shutdown),
                Some(CompletedService::ActionsScheduler),
            )
        }
        result = &mut integrity_scheduler => {
            http.abort();
            ssh.abort();
            mirror_scheduler.abort();
            backup_scheduler.abort();
            actions_state.cancel();
            actions_scheduler.abort();
            (
                result
                    .context("integrity scheduler task failed")
                    .and_then(|result| result)
                    .map(|()| ServerExit::Shutdown),
                Some(CompletedService::IntegrityScheduler),
            )
        }
        Some(action) = maintenance_receiver.recv() => {
            shutdown.cancel();
            ssh.abort();
            mirror_scheduler.abort();
            backup_scheduler.abort();
            integrity_scheduler.abort();
            actions_state.cancel();
            actions_scheduler.abort();
            let http_completed =
                tokio::time::timeout(Duration::from_secs(5), &mut http).await.is_ok();
            if !http_completed {
                http.abort();
            }
            (
                Ok(ServerExit::Maintenance(Box::new(action))),
                http_completed.then_some(CompletedService::Http),
            )
        }
    };
    if completed != Some(CompletedService::Http) {
        let _ = http.await;
    }
    if completed != Some(CompletedService::Ssh) {
        let _ = ssh.await;
    }
    if completed != Some(CompletedService::MirrorScheduler) {
        let _ = mirror_scheduler.await;
    }
    if completed != Some(CompletedService::BackupScheduler) {
        let _ = backup_scheduler.await;
    }
    if completed != Some(CompletedService::ActionsScheduler) {
        let _ = actions_scheduler.await;
    }
    if completed != Some(CompletedService::IntegrityScheduler) {
        let _ = integrity_scheduler.await;
    }
    repository_state.close_tasks();
    repository_state.wait_for_tasks().await;
    outcome
}

#[derive(Clone)]
struct MaintenanceStatusState {
    operation_id: uuid::Uuid,
    receiver: watch::Receiver<MaintenanceProgress>,
}

pub async fn perform_maintenance(settings: &Settings, action: MaintenanceAction) -> Result<()> {
    let reporter = MaintenanceProgressReporter::new(&action);
    let operation_id = action.operation_id();
    let shutdown = CancellationToken::new();
    let http_shutdown = shutdown.clone();
    let status_server = match TcpListener::bind(settings.server.bind).await {
        Ok(listener) => {
            let state = MaintenanceStatusState {
                operation_id,
                receiver: reporter.subscribe(),
            };
            let app = Router::new()
                .route(
                    "/api/v1/admin/backups/progress/{operation_id}",
                    get(maintenance_progress),
                )
                .route(
                    "/api/v1/admin/storage/progress/{operation_id}",
                    get(maintenance_progress),
                )
                .with_state(state);
            Some(tokio::spawn(async move {
                axum::serve(listener, app)
                    .with_graceful_shutdown(http_shutdown.cancelled_owned())
                    .await
            }))
        }
        Err(error) => {
            tracing::warn!(%error, "could not start maintenance progress server");
            None
        }
    };

    let result = archive::perform_maintenance(action, settings, &reporter).await;
    match &result {
        Ok(()) => reporter.complete(),
        Err(error) => reporter.fail(error),
    }
    tokio::time::sleep(Duration::from_secs(2)).await;
    shutdown.cancel();
    if let Some(mut status_server) = status_server
        && tokio::time::timeout(Duration::from_secs(2), &mut status_server)
            .await
            .is_err()
    {
        status_server.abort();
    }
    result
}

async fn maintenance_progress(
    Path(operation_id): Path<uuid::Uuid>,
    State(state): State<MaintenanceStatusState>,
) -> Result<Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>>, StatusCode> {
    if operation_id != state.operation_id {
        return Err(StatusCode::NOT_FOUND);
    }
    let events = stream::unfold(
        (state.receiver, true),
        |(mut receiver, initial)| async move {
            if !initial && receiver.changed().await.is_err() {
                return None;
            }
            let progress = receiver.borrow_and_update().clone();
            let data = serde_json::to_string(&progress).ok()?;
            Some((
                Ok(Event::default()
                    .data(data)
                    .retry(Duration::from_millis(500))),
                (receiver, false),
            ))
        },
    );
    Ok(Sse::new(events).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(1))
            .text("maintenance"),
    ))
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
    let cache_control = if path.starts_with("_app/immutable/") {
        HeaderValue::from_static("public, max-age=31536000, immutable")
    } else if path == "index.html" {
        HeaderValue::from_static("no-cache, max-age=0, must-revalidate")
    } else {
        HeaderValue::from_static("public, max-age=3600")
    };
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, cache_control);
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
