//! `gtd admin backup restore`: validate a backup and schedule a restore.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::Args;
use reqwest::Method;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{ApiClient, id, input, route_path};

/// Replace all instance data with a backup; the server stops normal service while it restores.
///
/// Without `--preflight-token` the backup named by `--key` is validated first
/// (the same check as `gtd admin backup preflight`). The server requires the
/// administrator password and, in current releases, an interactive session:
/// API tokens are rejected with 403.
#[derive(Debug, Args)]
pub(crate) struct RestoreArgs {
    provider_id: String,
    /// Backup object key to validate and restore.
    #[arg(long, required_unless_present = "preflight_token")]
    key: Option<String>,
    /// Token returned by an earlier `gtd admin backup preflight`.
    #[arg(long, conflicts_with = "key")]
    preflight_token: Option<String>,
    /// Must be exactly `restore`; restoring replaces all current data.
    #[arg(long)]
    confirmation: String,
    /// Skip the safety backup of the current data taken before restoring.
    #[arg(long)]
    no_safety_backup: bool,
    /// Read the administrator password from FILE (`-` for stdin) instead of prompting.
    #[arg(long, value_name = "FILE")]
    password_file: Option<PathBuf>,
}

impl RestoreArgs {
    pub(crate) fn uses_stdin(&self) -> bool {
        input::is_stdin(self.password_file.as_deref())
    }
}

fn restore_body(token: Uuid, password: &str, create_safety_backup: bool) -> Value {
    json!({
        "token": token,
        "password": password,
        "create_safety_backup": create_safety_backup,
    })
}

fn backup_path(provider_id: Uuid, action: &str) -> Result<String> {
    route_path(
        "admin/backup/providers",
        &[&provider_id.to_string(), "backups", action],
    )
}

pub(crate) async fn run(api: &ApiClient, args: RestoreArgs) -> Result<Value> {
    if args.confirmation != "restore" {
        bail!("--confirmation must be exactly restore");
    }
    let provider_id = id(&args.provider_id)?;
    let preflight_token = args.preflight_token.as_deref().map(id).transpose()?;
    // Read the password before preflight so a missing password does not cost a download.
    let password = match args.password_file.as_deref() {
        Some(path) => input::read_secret_file(path, "administrator password")?,
        None => input::prompt_secret(
            "Administrator password: ",
            "administrator password",
            "--password-file",
        )?,
    };
    let token = match (preflight_token, args.key) {
        (Some(token), _) => token,
        (None, Some(key)) => {
            let preflight = api
                .request(
                    Method::POST,
                    &backup_path(provider_id, "preflight")?,
                    Some(json!({ "key": key })),
                )
                .await?;
            eprintln!(
                "Validated backup: {}",
                serde_json::to_string(&preflight).context("could not encode preflight result")?
            );
            let token = preflight
                .get("token")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("Gitadel preflight returned no restore token"))?;
            id(token)?
        }
        (None, None) => bail!("provide --key or --preflight-token"),
    };
    let scheduled = api
        .request(
            Method::POST,
            &backup_path(provider_id, "restore")?,
            Some(restore_body(token, &password, !args.no_safety_backup)),
        )
        .await?;
    if let Some(operation_id) = scheduled.get("operation_id").and_then(Value::as_str) {
        eprintln!("Follow progress with: gtd admin backup progress {operation_id}");
    }
    Ok(scheduled)
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;
    use crate::{AdminCommand, BackupCommand, Cli, Command};

    const PROVIDER: &str = "6f1c1f0e-8a39-4d0b-9d5f-0c1d2e3f4a5b";

    fn parse(args: &[&str]) -> Result<RestoreArgs, clap::Error> {
        let mut argv = vec!["gtd", "admin", "backup", "restore"];
        argv.extend_from_slice(args);
        match Cli::try_parse_from(argv)?.command {
            Command::Admin {
                command:
                    AdminCommand::Backup {
                        command: BackupCommand::Restore(args),
                    },
            } => Ok(args),
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn restore_requires_confirmation() {
        assert!(parse(&[PROVIDER, "--key", "backup.tar.zst"]).is_err());
    }

    #[test]
    fn restore_requires_key_or_preflight_token() {
        assert!(parse(&[PROVIDER, "--confirmation", "restore"]).is_err());
    }

    #[test]
    fn restore_rejects_key_with_preflight_token() {
        let token = Uuid::new_v4().to_string();
        let args = [
            PROVIDER,
            "--confirmation",
            "restore",
            "--key",
            "k",
            "--preflight-token",
            &token,
        ];
        assert!(parse(&args).is_err());
    }

    #[test]
    fn restore_password_file_dash_uses_stdin() {
        let args = parse(&[
            PROVIDER,
            "--confirmation",
            "restore",
            "--key",
            "k",
            "--password-file",
            "-",
        ])
        .unwrap();
        assert!(args.uses_stdin());
    }

    #[test]
    fn restore_body_matches_server_request() {
        let token = Uuid::nil();
        assert_eq!(
            restore_body(token, "hunter2", false),
            json!({"token": token, "password": "hunter2", "create_safety_backup": false})
        );
    }
}
