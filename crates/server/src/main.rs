#![cfg_attr(feature = "api-contract-export", allow(dead_code, unused_imports))]

use std::io::Read;

mod runtime;
mod workers;

use gitadel_config as config;
use gitadel_db as database;
use gitadel_storage::lfs as storage;

use anyhow::{Context, Result, bail};
use clap::Parser;
use config::{Cli, Settings};

/// Initialize process logging before running the server or offline commands.
fn init_tracing() {
    use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "gitadel=info,tower_http=info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();
}

#[cfg(feature = "api-contract-export")]
fn main() -> Result<()> {
    print!("{}", gitadel_api::export_openapi()?);
    Ok(())
}

#[cfg(not(feature = "api-contract-export"))]
fn main() -> Result<()> {
    if gitadel_api::CONTRACT_EXPORT_ONLY {
        print!("{}", gitadel_api::export_openapi()?);
        return Ok(());
    }
    let cli = Cli::parse();
    if matches!(cli.command(), Some(config::GitadelCommand::Openapi)) {
        print!("{}", gitadel_api::export_openapi()?);
        return Ok(());
    }
    if let Some(config::GitadelCommand::ImageRender { path, thumbnail }) = cli.command() {
        return gitadel_api::image_worker(path, *thumbnail);
    }
    run(cli)
}

#[tokio::main]
#[cfg(not(feature = "api-contract-export"))]
async fn run(cli: Cli) -> Result<()> {
    init_tracing();

    if let Some(command) = cli.command() {
        return match command {
            config::GitadelCommand::Openapi => unreachable!("contract export runs offline"),
            config::GitadelCommand::ImageRender { .. } => {
                unreachable!("Image workers run before starting Tokio")
            }
            config::GitadelCommand::Backup { command } => {
                let settings = Settings::load(&cli)?;
                gitadel_backups::archive::run(command, &settings, cli.config_path()).await
            }
            config::GitadelCommand::Lfs { command } => {
                let settings = Settings::load(&cli)?;
                let _storage_lock =
                    gitadel_backups::archive::acquire_storage_lock(&settings.database)?;
                storage::run(
                    command,
                    &settings,
                    &gitadel_registry::storage::RegistryDomain,
                )
                .await
            }
        };
    }
    let settings = Settings::load(&cli)?;
    let _storage_lock = gitadel_backups::archive::acquire_storage_lock(&settings.database)?;
    if let Some(username) = cli.bootstrap_admin() {
        if !cli.password_stdin() {
            bail!("--bootstrap-admin requires --password-stdin");
        }
        let mut password = String::new();
        std::io::stdin()
            .read_to_string(&mut password)
            .context("could not read the administrator password from standard input")?;
        let password = password.trim_end_matches(['\r', '\n']).to_owned();
        let database = database::connect_and_migrate(&settings.database).await?;
        let account = gitadel_identity::bootstrap_admin(&database, username, password)
            .await
            .context("could not create the first administrator")?;
        database.close().await?;
        println!("Created administrator {}.", account.username);
        return Ok(());
    }

    loop {
        let database = database::connect_and_migrate(&settings.database).await?;
        let read_database = database::connect_reader(&settings.database, &database).await?;
        let exit =
            runtime::serve(settings.clone(), database.clone(), read_database.clone()).await?;
        read_database
            .close()
            .await
            .context("could not close the read-only database pool for maintenance")?;
        database
            .close()
            .await
            .context("could not close database for maintenance")?;
        match exit {
            runtime::ServerExit::Shutdown => return Ok(()),
            runtime::ServerExit::Maintenance(action) => {
                if let Err(error) = runtime::perform_maintenance(&settings, *action).await {
                    tracing::error!(%error, "maintenance operation failed");
                }
            }
        }
    }
}
