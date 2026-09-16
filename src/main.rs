use std::io::Read;

mod actions;
mod api;
mod archive;
mod backup_provider;
mod blob_store;
mod config;
mod database;
mod entity;
mod identity;
mod integrations;
mod migration;
mod network;
mod repository;
mod schedule;
mod server;
mod storage;

use anyhow::{Context, Result, bail};
use clap::Parser;
use config::{Cli, Settings};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "gitadel=info,tower_http=info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let cli = Cli::parse();
    if let Some(command) = cli.command() {
        return match command {
            config::GitadelCommand::Backup { command } => {
                let settings = Settings::load(&cli)?;
                archive::run(command, &settings, cli.config_path()).await
            }
            config::GitadelCommand::Lfs { command } => {
                let settings = Settings::load(&cli)?;
                let _storage_lock = archive::acquire_storage_lock(&settings.database)?;
                storage::run(command, &settings).await
            }
        };
    }
    let settings = Settings::load(&cli)?;
    let _storage_lock = archive::acquire_storage_lock(&settings.database)?;
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
        let account = identity::bootstrap_admin(&database, username, password)
            .await
            .context("could not create the first administrator")?;
        database.close().await?;
        println!("Created administrator {}.", account.username);
        return Ok(());
    }

    loop {
        let database = database::connect_and_migrate(&settings.database).await?;
        let exit = server::serve(settings.clone(), database.clone()).await?;
        database
            .close()
            .await
            .context("could not close database for maintenance")?;
        match exit {
            server::ServerExit::Shutdown => return Ok(()),
            server::ServerExit::Maintenance(action) => {
                if let Err(error) = server::perform_maintenance(&settings, *action).await {
                    tracing::error!(%error, "maintenance operation failed");
                }
            }
        }
    }
}
