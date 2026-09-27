//! `gtd webhook`: repository push webhooks and their delivery history.

use std::path::PathBuf;

use anyhow::Result;
use clap::Subcommand;
use reqwest::Method;
use serde_json::{Value, json};

use crate::{ApiClient, id, input, route_path, route_repo};

#[derive(Debug, Subcommand)]
pub(crate) enum WebhookCommand {
    /// List the repository's webhooks.
    List { repository: String },
    /// Create a push webhook. Gitadel sends a ping delivery immediately.
    Create {
        repository: String,
        /// HTTP(S) endpoint that receives JSON push events.
        #[arg(long)]
        url: String,
        /// Secret used to sign deliveries.
        #[arg(long, conflicts_with = "secret_file")]
        secret: Option<String>,
        /// Read the signing secret from FILE (`-` for stdin).
        #[arg(long, value_name = "FILE")]
        secret_file: Option<PathBuf>,
        /// Create the webhook disabled.
        #[arg(long)]
        inactive: bool,
    },
    /// Delete a webhook.
    Delete { repository: String, id: String },
    /// List a webhook's recent deliveries, or show one delivery.
    Deliveries {
        repository: String,
        id: String,
        /// Show only this delivery, including its payload and response.
        #[arg(long, value_name = "DELIVERY_ID")]
        delivery: Option<String>,
    },
    /// Queue a previous delivery to be sent again.
    Redeliver {
        repository: String,
        id: String,
        delivery_id: String,
    },
}

pub(crate) fn uses_stdin(command: &WebhookCommand) -> bool {
    matches!(command, WebhookCommand::Create { secret_file, .. } if input::is_stdin(secret_file.as_deref()))
}

fn hook_path(repository: &str, hook: Option<&str>) -> Result<String> {
    let base = route_repo("repos", repository)? + "/hooks";
    match hook {
        Some(hook) => route_path(&base, &[&id(hook)?.to_string()]),
        None => Ok(base),
    }
}

fn create_body(url: &str, secret: Option<String>, active: bool) -> Value {
    let mut config = json!({ "url": url, "content_type": "json" });
    if let Some(secret) = secret {
        config["secret"] = json!(secret);
    }
    json!({
        "name": "web",
        "active": active,
        "events": ["push"],
        "config": config,
    })
}

pub(crate) async fn run(api: &ApiClient, command: WebhookCommand) -> Result<Value> {
    match command {
        WebhookCommand::List { repository } => {
            api.request(Method::GET, &hook_path(&repository, None)?, None)
                .await
        }
        WebhookCommand::Create {
            repository,
            url,
            secret,
            secret_file,
            inactive,
        } => {
            let secret = match secret_file {
                Some(path) => Some(input::read_secret_file(&path, "webhook secret")?),
                None => secret,
            };
            api.request(
                Method::POST,
                &hook_path(&repository, None)?,
                Some(create_body(&url, secret, !inactive)),
            )
            .await
        }
        WebhookCommand::Delete { repository, id } => {
            api.request(Method::DELETE, &hook_path(&repository, Some(&id))?, None)
                .await
        }
        WebhookCommand::Deliveries {
            repository,
            id,
            delivery,
        } => {
            let base = hook_path(&repository, Some(&id))? + "/deliveries";
            let path = match delivery {
                Some(delivery) => route_path(&base, &[&crate::id(&delivery)?.to_string()])?,
                None => base,
            };
            api.request(Method::GET, &path, None).await
        }
        WebhookCommand::Redeliver {
            repository,
            id,
            delivery_id,
        } => {
            let path = route_path(
                &(hook_path(&repository, Some(&id))? + "/deliveries"),
                &[&crate::id(&delivery_id)?.to_string(), "attempts"],
            )?;
            api.request(Method::POST, &path, None).await
        }
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;
    use crate::{Cli, Command};

    fn parse(args: &[&str]) -> Result<WebhookCommand, clap::Error> {
        let mut argv = vec!["gtd", "webhook"];
        argv.extend_from_slice(args);
        match Cli::try_parse_from(argv)?.command {
            Command::Webhook { command } => Ok(command),
            other => panic!("unexpected command: {other:?}"),
        }
    }

    const HOOK: &str = "00000000-0000-0000-0000-000000000001";

    #[test]
    fn create_rejects_secret_with_secret_file() {
        let args = [
            "create",
            "a/b",
            "--url",
            "https://ci.example.test/hook",
            "--secret",
            "s",
            "--secret-file",
            "-",
        ];
        assert!(parse(&args).is_err());
    }

    #[test]
    fn create_secret_file_dash_uses_stdin() {
        let args = [
            "create",
            "a/b",
            "--url",
            "https://ci.example.test/hook",
            "--secret-file",
            "-",
        ];
        assert!(uses_stdin(&parse(&args).unwrap()));
    }

    #[test]
    fn create_body_matches_server_request() {
        assert_eq!(
            create_body("https://ci.example.test/hook", Some("s".to_owned()), false),
            json!({
                "name": "web",
                "active": false,
                "events": ["push"],
                "config": {"url": "https://ci.example.test/hook", "content_type": "json", "secret": "s"},
            })
        );
    }

    #[test]
    fn hook_path_uses_repos_prefix() {
        assert_eq!(
            hook_path("alice/demo", Some(HOOK)).unwrap(),
            format!("repos/alice/demo/hooks/{HOOK}")
        );
    }

    #[test]
    fn hook_path_rejects_non_uuid_ids() {
        assert!(hook_path("alice/demo", Some("1")).is_err());
    }
}
