use std::io::Read;

mod api;
mod client;
mod config;
mod database;
mod entity;
mod identity;
mod migration;
mod repository;
mod server;

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
        return client::run(command).await;
    }
    let settings = Settings::load(&cli)?;
    let database = database::connect_and_migrate(&settings.database).await?;
    if let Some(username) = cli.bootstrap_admin() {
        if !cli.password_stdin() {
            bail!("--bootstrap-admin requires --password-stdin");
        }
        let mut password = String::new();
        std::io::stdin()
            .read_to_string(&mut password)
            .context("could not read the administrator password from standard input")?;
        let password = password.trim_end_matches(['\r', '\n']).to_owned();
        let account = identity::bootstrap_admin(&database, username, password)
            .await
            .context("could not create the first administrator")?;
        println!("Created administrator {}.", account.username);
        return Ok(());
    }
    server::serve(settings, database).await
}
