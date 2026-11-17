//! `gtd repo deploy-key`: SSH keys bound to one repository.

use std::fs;

use anyhow::{Context, Result, bail};
use clap::Subcommand;
use reqwest::Method;
use serde_json::{Value, json};

use crate::{ApiClient, id, route_path, route_repo};

#[derive(Debug, Subcommand)]
pub(crate) enum DeployKeyCommand {
    /// List the repository's deploy keys.
    List { repository: String },
    /// Add a deploy key. Keys are read-only unless --read-write is given.
    Add {
        repository: String,
        title: String,
        /// OpenSSH public key text, or `@FILE` to read it from a file.
        public_key: String,
        /// Allow pushes with this key.
        #[arg(long)]
        read_write: bool,
    },
    /// Remove a deploy key by ID.
    Remove { repository: String, id: String },
}

pub(crate) async fn run(api: &ApiClient, command: DeployKeyCommand) -> Result<Value> {
    match command {
        DeployKeyCommand::List { repository } => {
            api.request(Method::GET, &keys_path(&repository)?, None)
                .await
        }
        DeployKeyCommand::Add {
            repository,
            title,
            public_key,
            read_write,
        } => {
            let public_key = match public_key.strip_prefix('@') {
                Some(path) => fs::read_to_string(path)
                    .with_context(|| format!("could not read SSH key {path}"))?,
                None => public_key,
            };
            if public_key.trim().is_empty() {
                bail!("SSH public key is empty");
            }
            api.request(
                Method::POST,
                &keys_path(&repository)?,
                Some(json!({"title": title, "key": public_key, "read_only": !read_write})),
            )
            .await
        }
        DeployKeyCommand::Remove {
            repository,
            id: value,
        } => {
            let path = route_path(&keys_path(&repository)?, &[&id(&value)?.to_string()])?;
            api.request(Method::DELETE, &path, None).await
        }
    }
}

fn keys_path(repository: &str) -> Result<String> {
    Ok(route_repo("repositories", repository)? + "/deploy-keys")
}
