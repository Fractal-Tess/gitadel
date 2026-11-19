//! Process-owned listeners and maintenance execution.

use std::{
    net::{SocketAddr, TcpListener as StdTcpListener},
    sync::Arc,
};

use anyhow::{Context, Result};
use gitadel_api::{ActionsState, IdentityState, RepositoryState};
use gitadel_backups::archive::{self, MaintenanceAction, MaintenanceProgressReporter};
use gitadel_config::Settings;
use sea_orm::DatabaseConnection;
use tokio::{sync::mpsc, time::Duration};
use tokio_util::{sync::CancellationToken, task::AbortOnDropHandle};
use tracing::info;

/// Bound the wait for open HTTP connections and ordinary background tasks.
const SHUTDOWN_DEADLINE: Duration = Duration::from_secs(30);

pub async fn perform_maintenance(settings: &Settings, action: MaintenanceAction) -> Result<()> {
    let reporter = MaintenanceProgressReporter::new(&action);
    let operation_id = action.operation_id();
    let status_server = match StdTcpListener::bind(settings.server.bind) {
        Ok(listener) => {
            listener
                .set_nonblocking(true)
                .context("could not configure maintenance listener as non-blocking")?;
            let tls = if let Some(tls) = &settings.server.tls {
                Some(
                    axum_server::tls_rustls::RustlsConfig::from_pem_file(
                        &tls.certificate,
                        &tls.private_key,
                    )
                    .await
                    .context("could not load TLS configuration for maintenance status")?,
                )
            } else {
                None
            };
            let app = gitadel_api::maintenance_router(operation_id, reporter.subscribe());
            let handle = axum_server::Handle::new();
            let server_handle = handle.clone();
            let task = tokio::spawn(async move {
                if let Some(tls) = tls {
                    axum_server::from_tcp_rustls(listener, tls)
                        .context("could not configure maintenance HTTPS listener")?
                        .handle(server_handle)
                        .serve(app.into_make_service())
                        .await
                        .context("maintenance HTTPS listener stopped")
                } else {
                    axum_server::from_tcp(listener)
                        .context("could not configure maintenance HTTP listener")?
                        .handle(server_handle)
                        .serve(app.into_make_service())
                        .await
                        .context("maintenance HTTP listener stopped")
                }
            });
            Some((handle, task))
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
    if let Some((handle, mut status_server)) = status_server {
        handle.graceful_shutdown(Some(Duration::from_secs(2)));
        if tokio::time::timeout(Duration::from_secs(2), &mut status_server)
            .await
            .is_err()
        {
            status_server.abort();
        }
    }
    result
}

async fn seal_stored_secrets(
    database: &DatabaseConnection,
    cipher: &gitadel_secrets::SecretCipher,
) -> Result<()> {
    let report = gitadel_secrets::seal_stored_secrets(database, cipher)
        .await
        .context("could not seal stored secrets")?;
    if report.total() == 0 {
        tracing::debug!("all stored secrets are sealed");
    } else {
        tracing::info!(
            dokploy_api_keys = report.dokploy_api_keys,
            mirror_identity_secrets = report.mirror_identity_secrets,
            webhook_secrets = report.webhook_secrets,
            oidc_client_secrets = report.oidc_client_secrets,
            totp_secrets = report.totp_secrets,
            "sealed stored secrets that were kept as plain text"
        );
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CompletedService {
    Http,
    Ssh,
    MirrorScheduler,
    BackupScheduler,
    ActionsScheduler,
    IntegrityScheduler,
}

fn completed_service(
    result: std::result::Result<Result<()>, tokio::task::JoinError>,
    service: CompletedService,
) -> (Result<ServerExit>, Option<CompletedService>) {
    let context = match service {
        CompletedService::Http => "HTTP server task failed",
        CompletedService::Ssh => "SSH server task failed",
        CompletedService::MirrorScheduler => "mirror scheduler task failed",
        CompletedService::BackupScheduler => "backup scheduler task failed",
        CompletedService::ActionsScheduler => "Actions scheduler task failed",
        CompletedService::IntegrityScheduler => "integrity scheduler task failed",
    };
    (
        result
            .context(context)
            .and_then(|result| result)
            .map(|()| ServerExit::Shutdown),
        Some(service),
    )
}

fn abort_services(
    completed: Option<CompletedService>,
    workers: &[(CompletedService, tokio::task::AbortHandle)],
    cancel_actions: impl FnOnce(),
) {
    for (service, task) in workers {
        if *service != CompletedService::ActionsScheduler && completed != Some(*service) {
            task.abort();
        }
    }
    if completed != Some(CompletedService::ActionsScheduler) {
        cancel_actions();
        if let Some((_, task)) = workers
            .iter()
            .find(|(service, _)| *service == CompletedService::ActionsScheduler)
        {
            task.abort();
        }
    }
}

pub enum ServerExit {
    Shutdown,
    Maintenance(Box<MaintenanceAction>),
}

pub async fn serve(
    settings: Settings,
    database: DatabaseConnection,
    read_database: DatabaseConnection,
) -> Result<ServerExit> {
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
    let runtime_settings = Arc::new(settings.clone());
    let host_key = gitadel_api::load_ssh_host_key(&settings.ssh.host_key)?;
    let cipher = Arc::new(
        gitadel_secrets::SecretCipher::from_host_key(&host_key)
            .context("could not derive the secret key")?,
    );
    let (mailer, mail_worker) = match &settings.smtp {
        Some(smtp) => {
            let (mailer, worker) = gitadel_mail::Mailer::build(smtp)?;
            (mailer, Some(worker))
        }
        None => (gitadel_mail::Mailer::disabled(), None),
    };
    let auth_runtime = Arc::new(
        gitadel_identity::runtime::AuthRuntime::new(
            database.clone(),
            settings.auth.clone(),
            settings.server.public_url.clone(),
            mailer,
            cipher.clone(),
        )
        .context("could not initialize authentication")?,
    );
    seal_stored_secrets(&database, &cipher).await?;
    let lfs = gitadel_storage::lfs::start(&database, &settings.storage)
        .await
        .context("could not initialize LFS storage")?;
    let registry = gitadel_registry::storage::start(&database, &settings.storage)
        .await
        .context("could not initialize registry storage")?;
    let integrity = gitadel_repositories::integrity::IntegrityService::new(
        database.clone(),
        settings.storage.repository_root.clone(),
        settings.storage.lfs_root.clone(),
        lfs.clone(),
    );
    let backup_runtime = Arc::new(gitadel_backups::service::BackupRuntime::new(
        database.clone(),
        Some(runtime_settings.clone()),
        Some(maintenance_sender),
    ));
    let storage_runtime = Arc::new(gitadel_storage::service::StorageRuntime::new(
        database.clone(),
        Some(runtime_settings.clone()),
        Some(lfs),
        Some(registry.clone()),
        &gitadel_registry::storage::RegistryDomain,
        &gitadel_registry::usage::RegistryUsageProvider,
    ));
    let (integrity_settings_version, integrity_settings_changed) = tokio::sync::watch::channel(0);
    let identity_state = IdentityState::from_services(
        database,
        auth_runtime.clone(),
        backup_runtime,
        storage_runtime,
        integrity_settings_version,
        Some(runtime_settings),
    )
    .with_read_database(read_database);
    if let Some(link) = auth_runtime
        .service(gitadel_core::audit::AuditContext::default())
        .prepare_setup_link()
        .await
        .context("could not prepare the first-run setup link")?
    {
        tracing::warn!(
            "No administrator exists yet. Open this one-time link to create one: {link}"
        );
    }
    let ssh_port = settings.ssh.bind.port();
    let repository_state = RepositoryState::new(
        identity_state.clone(),
        settings.storage.clone(),
        settings.server.public_url.clone(),
        ssh_port,
    )
    .await
    .context("could not initialize repository storage")?;
    gitadel_repositories::imports::recover_interrupted(identity_state.database())
        .await
        .context("could not recover interrupted repository imports")?;
    gitadel_api::purge_soft_deleted_repositories(&repository_state)
        .await
        .context("could not remove previously deleted repositories")?;
    let actions_shutdown = CancellationToken::new();
    let actions_state = ActionsState::new(
        repository_state.clone(),
        settings.actions.clone(),
        cipher.as_ref().clone(),
    )
    .with_shutdown(actions_shutdown.clone());
    actions_state
        .service()
        .bootstrap_system_runner()
        .await
        .context("could not prepare the configured system runner")?;
    let app = gitadel_api::http_router(
        &settings,
        &identity_state,
        &repository_state,
        &actions_state,
    );

    info!(
        address = %http_bind,
        protocol = if tls.is_some() { "https" } else { "http" },
        "Gitadel web server listening"
    );
    let shutdown = CancellationToken::new();
    let http_shutdown = shutdown.clone();
    let http_handle = axum_server::Handle::new();
    let graceful_handle = http_handle.clone();
    let mail_worker = mail_worker.map(|worker| tokio::spawn(worker.run()));
    let warmup_state = repository_state.clone();
    let mut warmup = AbortOnDropHandle::new(tokio::spawn(async move {
        if let Err(error) = warmup_state.warm_analysis().await {
            tracing::warn!(%error, "repository analysis backfill failed");
        }
    }));
    let mut http = tokio::spawn(async move {
        let shutdown_task = AbortOnDropHandle::new(tokio::spawn(async move {
            tokio::select! {
                () = shutdown_signal() => {}
                () = http_shutdown.cancelled() => {}
            }
            graceful_handle.graceful_shutdown(Some(SHUTDOWN_DEADLINE));
        }));
        let result = if let Some(tls) = tls {
            axum_server::from_tcp_rustls(listener, tls)
                .context("could not configure HTTPS listener")?
                .handle(http_handle)
                .serve(app.into_make_service_with_connect_info::<SocketAddr>())
                .await
        } else {
            axum_server::from_tcp(listener)
                .context("could not configure HTTP listener")?
                .handle(http_handle)
                .serve(app.into_make_service_with_connect_info::<SocketAddr>())
                .await
        };
        shutdown_task.abort();
        result.context("web server stopped unexpectedly")
    });
    let mut mirror_scheduler = tokio::spawn(crate::workers::serve_mirror_scheduler(
        repository_state.clone(),
    ));
    let mut integrity_scheduler =
        tokio::spawn(gitadel_repositories::integrity::serve_integrity_scheduler(
            integrity.clone(),
            integrity_settings_changed,
        ));
    // Only logs, never fails: it is stopped with the rest rather than joined.
    let audit_retention = tokio::spawn(gitadel_instance::service::serve_audit_retention(
        identity_state.database().clone(),
        settings.audit.retention_days,
    ));
    let mut backup_scheduler = tokio::spawn(gitadel_backups::scheduler::serve_backup_scheduler(
        identity_state.backup_runtime.clone(),
    ));
    let mut actions_scheduler = tokio::spawn(crate::workers::serve_actions_scheduler(
        actions_state.service().clone(),
        actions_shutdown.clone(),
    ));
    let mut ssh = tokio::spawn(gitadel_api::serve_ssh(
        settings.ssh,
        repository_state.clone(),
        actions_state.clone(),
    ));
    let workers = [
        (CompletedService::Http, http.abort_handle()),
        (CompletedService::Ssh, ssh.abort_handle()),
        (
            CompletedService::MirrorScheduler,
            mirror_scheduler.abort_handle(),
        ),
        (
            CompletedService::BackupScheduler,
            backup_scheduler.abort_handle(),
        ),
        (
            CompletedService::IntegrityScheduler,
            integrity_scheduler.abort_handle(),
        ),
        (
            CompletedService::ActionsScheduler,
            actions_scheduler.abort_handle(),
        ),
    ];
    let (outcome, completed) = tokio::select! {
        result = &mut http => completed_service(result, CompletedService::Http),
        result = &mut ssh => completed_service(result, CompletedService::Ssh),
        result = &mut mirror_scheduler => completed_service(result, CompletedService::MirrorScheduler),
        result = &mut backup_scheduler => completed_service(result, CompletedService::BackupScheduler),
        result = &mut actions_scheduler => completed_service(result, CompletedService::ActionsScheduler),
        result = &mut integrity_scheduler => completed_service(result, CompletedService::IntegrityScheduler),
        Some(action) = maintenance_receiver.recv() => {
            shutdown.cancel();
            abort_services(Some(CompletedService::Http), &workers, || actions_shutdown.cancel());
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
    if !matches!(&outcome, Ok(ServerExit::Maintenance(_))) {
        abort_services(completed, &workers, || actions_shutdown.cancel());
    }
    audit_retention.abort();
    let _ = audit_retention.await;
    for (service, task) in [
        (CompletedService::Http, http),
        (CompletedService::Ssh, ssh),
        (CompletedService::MirrorScheduler, mirror_scheduler),
        (CompletedService::BackupScheduler, backup_scheduler),
        (CompletedService::ActionsScheduler, actions_scheduler),
        (CompletedService::IntegrityScheduler, integrity_scheduler),
    ] {
        if completed != Some(service) {
            let _ = task.await;
        }
    }
    repository_state.close_tasks();
    let drain_repository_tasks = async {
        let _ = (&mut warmup).await;
        repository_state.wait_for_tasks().await;
        integrity.drain().await;
    };
    if matches!(&outcome, Ok(ServerExit::Maintenance(_))) {
        // Maintenance rewrites the data the tasks work on, so it must not
        // start while any of them is still running.
        drain_repository_tasks.await;
    } else if tokio::time::timeout(SHUTDOWN_DEADLINE, drain_repository_tasks)
        .await
        .is_err()
    {
        tracing::warn!(
            deadline_seconds = SHUTDOWN_DEADLINE.as_secs(),
            "background tasks did not finish before the shutdown deadline"
        );
    }
    if matches!(&outcome, Ok(ServerExit::Maintenance(_))) {
        registry.wait_for_migration().await;
    }
    if let Some(mail_worker) = mail_worker {
        mail_worker.abort();
        let _ = mail_worker.await;
    }
    outcome
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

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn maintenance_keeps_http_alive_while_background_workers_stop() {
        let (release, wait) = tokio::sync::oneshot::channel::<()>();
        let http = tokio::spawn(async move {
            wait.await.unwrap();
            Ok(())
        });
        let mut tasks = vec![(CompletedService::Http, http)];
        for service in [
            CompletedService::Ssh,
            CompletedService::MirrorScheduler,
            CompletedService::BackupScheduler,
            CompletedService::IntegrityScheduler,
            CompletedService::ActionsScheduler,
        ] {
            tasks.push((service, tokio::spawn(std::future::pending::<Result<()>>())));
        }
        let workers: Vec<_> = tasks
            .iter()
            .map(|(service, task)| (*service, task.abort_handle()))
            .collect();
        let mut actions_cancelled = false;
        abort_services(Some(CompletedService::Http), &workers, || {
            actions_cancelled = true
        });
        assert!(actions_cancelled);
        release.send(()).unwrap();
        for (service, task) in tasks {
            if service == CompletedService::Http {
                assert!(task.await.unwrap().is_ok());
            } else {
                assert!(task.await.unwrap_err().is_cancelled());
            }
        }
    }
}
