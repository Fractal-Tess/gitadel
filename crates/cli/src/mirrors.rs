//! `gtd mirror`: pull-mirror settings, synchronization, and namespace mirror identities.

use std::path::PathBuf;

use anyhow::{Result, bail};
use clap::Subcommand;
use reqwest::Method;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{ApiClient, id, input, route_path, route_repo};

#[derive(Debug, Subcommand)]
pub(crate) enum MirrorCommand {
    /// Show a mirror's upstream, schedule, and last synchronization.
    View { repository: String },
    /// Fetch from the upstream now.
    Sync { repository: String },
    /// Change the mirror's credential or schedule; unspecified settings are kept.
    Update {
        repository: String,
        /// Mirror identity ID used to authenticate to the upstream.
        #[arg(long, conflicts_with = "no_identity")]
        identity: Option<String>,
        /// Fetch anonymously.
        #[arg(long)]
        no_identity: bool,
        /// Cron schedule with five to seven fields, for example `0 * * * *`.
        #[arg(long, conflicts_with = "manual")]
        schedule: Option<String>,
        /// Synchronize only on request.
        #[arg(long)]
        manual: bool,
    },
    /// Turn the mirror into a standard writable repository. This cannot be undone.
    Convert {
        repository: String,
        /// Must be exactly `convert`.
        #[arg(long)]
        confirmation: String,
    },
    /// Manage a namespace's mirror credentials.
    Identity {
        #[command(subcommand)]
        command: MirrorIdentityCommand,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum MirrorIdentityCommand {
    /// List a namespace's mirror identities.
    List { namespace: String },
    /// Store an access token for mirroring from a forge.
    Create {
        namespace: String,
        name: String,
        /// Forge origin the token belongs to, for example `https://github.com`.
        #[arg(long)]
        server_url: String,
        /// Read the forge access token from FILE (`-` for stdin) instead of prompting.
        #[arg(long, value_name = "FILE")]
        access_token_file: Option<PathBuf>,
    },
    /// Delete a mirror identity.
    Delete { namespace: String, id: String },
}

pub(crate) fn uses_stdin(command: &MirrorCommand) -> bool {
    matches!(
        command,
        MirrorCommand::Identity {
            command: MirrorIdentityCommand::Create { access_token_file, .. },
        } if input::is_stdin(access_token_file.as_deref())
    )
}

fn mirror_path(repository: &str) -> Result<String> {
    Ok(route_repo("repositories", repository)? + "/mirror")
}

fn identities_path(namespace: &str) -> Result<String> {
    route_path("namespaces", &[namespace, "mirror-identities"])
}

/// Setting change requested on the command line: keep, clear, or replace.
#[derive(Debug, PartialEq)]
enum Change<T> {
    Keep,
    Clear,
    Set(T),
}

impl<T> Change<T> {
    fn from_flags(value: Option<T>, clear: bool) -> Self {
        match (value, clear) {
            (Some(value), _) => Self::Set(value),
            (None, true) => Self::Clear,
            (None, false) => Self::Keep,
        }
    }
}

/// Builds the full replacement body the server expects from the current mirror state.
fn update_body(current: &Value, identity: Change<Uuid>, schedule: Change<String>) -> Result<Value> {
    if identity == Change::Keep && schedule == Change::Keep {
        bail!("provide --identity, --no-identity, --schedule, or --manual");
    }
    let identity = match identity {
        Change::Keep => current.get("identity_id").cloned().unwrap_or(Value::Null),
        Change::Clear => Value::Null,
        Change::Set(id) => json!(id),
    };
    let schedule = match schedule {
        Change::Keep => current.get("schedule").cloned().unwrap_or(Value::Null),
        Change::Clear => Value::Null,
        Change::Set(schedule) => json!(schedule),
    };
    Ok(json!({ "identity_id": identity, "schedule": schedule }))
}

pub(crate) async fn run(api: &ApiClient, command: MirrorCommand) -> Result<Value> {
    match command {
        MirrorCommand::View { repository } => {
            api.request(Method::GET, &mirror_path(&repository)?, None)
                .await
        }
        MirrorCommand::Sync { repository } => {
            api.request(Method::POST, &(mirror_path(&repository)? + "/sync"), None)
                .await
        }
        MirrorCommand::Update {
            repository,
            identity,
            no_identity,
            schedule,
            manual,
        } => {
            let identity =
                Change::from_flags(identity.as_deref().map(id).transpose()?, no_identity);
            let schedule = Change::from_flags(schedule, manual);
            let path = mirror_path(&repository)?;
            let current = api.request(Method::GET, &path, None).await?;
            let body = update_body(&current, identity, schedule)?;
            api.request(Method::PATCH, &path, Some(body)).await
        }
        MirrorCommand::Convert {
            repository,
            confirmation,
        } => {
            if confirmation != "convert" {
                bail!("--confirmation must be exactly convert");
            }
            api.request(Method::DELETE, &mirror_path(&repository)?, None)
                .await
        }
        MirrorCommand::Identity { command } => run_identity(api, command).await,
    }
}

async fn run_identity(api: &ApiClient, command: MirrorIdentityCommand) -> Result<Value> {
    match command {
        MirrorIdentityCommand::List { namespace } => {
            api.request(Method::GET, &identities_path(&namespace)?, None)
                .await
        }
        MirrorIdentityCommand::Create {
            namespace,
            name,
            server_url,
            access_token_file,
        } => {
            let token = match access_token_file {
                Some(path) => input::read_secret_file(&path, "forge access token")?,
                None => input::prompt_secret(
                    "Forge access token: ",
                    "forge access token",
                    "--access-token-file",
                )?,
            };
            api.request(
                Method::POST,
                &identities_path(&namespace)?,
                Some(json!({ "name": name, "token": token, "server_url": server_url })),
            )
            .await
        }
        MirrorIdentityCommand::Delete {
            namespace,
            id: value,
        } => {
            let path = route_path(&identities_path(&namespace)?, &[&id(&value)?.to_string()])?;
            api.request(Method::DELETE, &path, None).await
        }
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;
    use crate::{Cli, Command};

    fn parse(args: &[&str]) -> Result<MirrorCommand, clap::Error> {
        let mut argv = vec!["gtd", "mirror"];
        argv.extend_from_slice(args);
        match Cli::try_parse_from(argv)?.command {
            Command::Mirror { command } => Ok(command),
            other => panic!("unexpected command: {other:?}"),
        }
    }

    const IDENTITY: &str = "00000000-0000-0000-0000-000000000001";

    fn current() -> Value {
        json!({"identity_id": IDENTITY, "schedule": "0 0 * * *", "remote_url": "https://example.test/x.git"})
    }

    #[test]
    fn update_rejects_schedule_with_manual() {
        assert!(parse(&["update", "a/b", "--schedule", "0 * * * *", "--manual"]).is_err());
    }

    #[test]
    fn convert_requires_confirmation() {
        assert!(parse(&["convert", "a/b"]).is_err());
    }

    #[test]
    fn identity_create_token_file_dash_uses_stdin() {
        let command = parse(&[
            "identity",
            "create",
            "alice",
            "github",
            "--server-url",
            "https://github.com",
            "--access-token-file",
            "-",
        ])
        .unwrap();
        assert!(uses_stdin(&command));
    }

    #[test]
    fn update_body_keeps_unspecified_settings() {
        let body = update_body(
            &current(),
            Change::Keep,
            Change::Set("0 * * * *".to_owned()),
        )
        .unwrap();
        assert_eq!(
            body,
            json!({"identity_id": IDENTITY, "schedule": "0 * * * *"})
        );
    }

    #[test]
    fn update_body_clears_requested_settings() {
        let body = update_body(&current(), Change::Clear, Change::Clear).unwrap();
        assert_eq!(body, json!({"identity_id": null, "schedule": null}));
    }

    #[test]
    fn update_body_requires_a_change() {
        assert!(update_body(&current(), Change::Keep, Change::Keep).is_err());
    }

    #[test]
    fn identities_path_encodes_namespace() {
        assert_eq!(
            identities_path("alice").unwrap(),
            "namespaces/alice/mirror-identities"
        );
    }
}
