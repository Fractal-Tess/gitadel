//! `gtd actions`: workflow runs, logs, cancellation, secrets, and variables.

use std::{
    fs,
    io::{self, IsTerminal as _, Read as _},
    path::PathBuf,
};

use anyhow::{Context as _, Result, bail};
use clap::{ArgGroup, Args, Subcommand};
use reqwest::Method;
use serde_json::{Value, json};

use super::{ApiClient, parse_repo, route_path};

#[derive(Debug, Subcommand)]
pub(crate) enum ActionsCommand {
    /// List workflow runs of a repository, newest first.
    Runs {
        #[command(flatten)]
        repository: RepositoryArg,
        /// Page number, starting at 1.
        #[arg(long, default_value_t = 1)]
        page: usize,
        /// Only list runs for this commit.
        #[arg(long)]
        commit: Option<String>,
    },
    /// Show a workflow run and its jobs.
    View {
        #[command(flatten)]
        repository: RepositoryArg,
        run_id: String,
    },
    /// Print job logs as plain text. Without --job, every job of the run is printed.
    Logs {
        #[command(flatten)]
        repository: RepositoryArg,
        run_id: String,
        /// Only print this job's log.
        #[arg(long)]
        job: Option<i64>,
    },
    /// Cancel a queued or running workflow run.
    Cancel {
        #[command(flatten)]
        repository: RepositoryArg,
        run_id: String,
    },
    /// Manage write-only Actions secrets of a repository or namespace.
    Secret {
        #[command(subcommand)]
        command: ValueCommand,
    },
    /// Manage Actions variables of a repository or namespace.
    Variable {
        #[command(subcommand)]
        command: ValueCommand,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum ValueCommand {
    /// Create or replace a value. Without --value or --value-file, stdin is read.
    Set {
        name: String,
        #[command(flatten)]
        scope: ScopeArg,
        /// The value. Prefer --value-file or stdin for secrets to keep them
        /// out of shell history.
        #[arg(long, conflicts_with = "value_file")]
        value: Option<String>,
        /// Read the value from this file (`-` for stdin).
        #[arg(long, value_name = "FILE", conflicts_with = "value")]
        value_file: Option<PathBuf>,
    },
    /// List names (and, for variables, values).
    List {
        #[command(flatten)]
        scope: ScopeArg,
    },
    /// Delete a value.
    Delete {
        name: String,
        #[command(flatten)]
        scope: ScopeArg,
    },
}

#[derive(Debug, Args)]
#[command(group(ArgGroup::new("scope").required(true).args(["repo", "namespace"])))]
pub(crate) struct ScopeArg {
    /// Repository in namespace/name form.
    #[arg(long, value_name = "NAMESPACE/NAME")]
    repo: Option<String>,
    /// User or organization namespace; every repository in it inherits the value.
    #[arg(long)]
    namespace: Option<String>,
}

impl ScopeArg {
    fn route(&self, segments: &[&str]) -> Result<String> {
        match (&self.repo, &self.namespace) {
            (Some(repo), None) => RepositoryArg { repo: repo.clone() }.route(segments),
            (None, Some(namespace)) => {
                let mut all = vec![namespace.as_str(), "actions"];
                all.extend_from_slice(segments);
                route_path("namespaces", &all)
            }
            _ => bail!("provide exactly one of --repo or --namespace"),
        }
    }
}

#[derive(Debug, Args)]
pub(crate) struct RepositoryArg {
    /// Repository in namespace/name form.
    #[arg(long = "repo", value_name = "NAMESPACE/NAME")]
    repo: String,
}

impl RepositoryArg {
    fn route(&self, segments: &[&str]) -> Result<String> {
        let (namespace, name) = parse_repo(&self.repo)?;
        let mut all = vec![namespace, name, "actions"];
        all.extend_from_slice(segments);
        route_path("repositories", &all)
    }
}

/// Runs an Actions command. `None` means the command already wrote its output.
pub(crate) async fn run(api: &ApiClient, command: ActionsCommand) -> Result<Option<Value>> {
    match command {
        ActionsCommand::Runs {
            repository,
            page,
            commit,
        } => {
            let mut path = format!("{}?page={page}", repository.route(&["runs"])?);
            if let Some(commit) = commit {
                if !commit.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                    bail!("--commit must be a hexadecimal commit ID");
                }
                path.push_str(&format!("&commit={commit}"));
            }
            api.request(Method::GET, &path, None).await.map(Some)
        }
        ActionsCommand::View { repository, run_id } => api
            .request(Method::GET, &repository.route(&["runs", &run_id])?, None)
            .await
            .map(Some),
        ActionsCommand::Logs {
            repository,
            run_id,
            job,
        } => {
            print_logs(api, &repository, &run_id, job).await?;
            Ok(None)
        }
        ActionsCommand::Secret { command } => run_value(api, "secrets", command).await.map(Some),
        ActionsCommand::Variable { command } => {
            run_value(api, "variables", command).await.map(Some)
        }
        ActionsCommand::Cancel { repository, run_id } => api
            .request(
                Method::POST,
                &repository.route(&["runs", &run_id, "cancel"])?,
                None,
            )
            .await
            .map(Some),
    }
}

async fn print_logs(
    api: &ApiClient,
    repository: &RepositoryArg,
    run_id: &str,
    job: Option<i64>,
) -> Result<()> {
    let jobs: Vec<(i64, String)> = match job {
        Some(job) => vec![(job, String::new())],
        None => {
            let detail = api
                .request(Method::GET, &repository.route(&["runs", run_id])?, None)
                .await?;
            detail
                .get("jobs")
                .and_then(Value::as_array)
                .map(|jobs| {
                    jobs.iter()
                        .filter_map(|job| {
                            Some((
                                job.get("id")?.as_i64()?,
                                job.get("name")?.as_str()?.to_owned(),
                            ))
                        })
                        .collect()
                })
                .unwrap_or_default()
        }
    };
    let headed = jobs.len() > 1 || job.is_none();
    for (job_id, name) in jobs {
        if headed {
            println!("=== {name} (job {job_id}) ===");
        }
        let base = repository.route(&["runs", run_id, "jobs", &job_id.to_string(), "logs"])?;
        let mut cursor: Option<String> = None;
        loop {
            let path = match &cursor {
                Some(cursor) => format!("{base}?cursor={cursor}"),
                None => base.clone(),
            };
            let page = api.request(Method::GET, &path, None).await?;
            let text = page.get("text").and_then(Value::as_str).unwrap_or_default();
            if !text.is_empty() {
                println!("{text}");
            }
            match page.get("next_cursor").and_then(Value::as_str) {
                // Cursors are URL-safe base64 produced by the server.
                Some(next)
                    if next.bytes().all(|byte| {
                        byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')
                    }) =>
                {
                    cursor = Some(next.to_owned());
                }
                _ => break,
            }
        }
    }
    Ok(())
}

async fn run_value(api: &ApiClient, kind: &str, command: ValueCommand) -> Result<Value> {
    match command {
        ValueCommand::List { scope } => {
            api.request(Method::GET, &scope.route(&[kind])?, None).await
        }
        ValueCommand::Delete { name, scope } => {
            api.request(Method::DELETE, &scope.route(&[kind, &name])?, None)
                .await
        }
        ValueCommand::Set {
            name,
            scope,
            value,
            value_file,
        } => {
            let value = match (value, value_file) {
                (Some(value), _) => value,
                (None, Some(file)) if file.as_os_str() != "-" => fs::read_to_string(&file)
                    .with_context(|| format!("could not read {}", file.display()))?,
                (None, _) => read_stdin_value()?,
            };
            api.request(
                Method::PUT,
                &scope.route(&[kind, &name])?,
                Some(json!({ "value": value })),
            )
            .await
        }
    }
}

fn read_stdin_value() -> Result<String> {
    if io::stdin().is_terminal() {
        eprintln!("Reading the value from stdin; finish with Ctrl-D.");
    }
    let mut value = String::new();
    io::stdin()
        .read_to_string(&mut value)
        .context("could not read the value from stdin")?;
    // A trailing newline from `echo` or a heredoc is almost never intended.
    if value.ends_with('\n') {
        value.pop();
        if value.ends_with('\r') {
            value.pop();
        }
    }
    Ok(value)
}

/// Whether the command reads a value from stdin (which conflicts with `--token-stdin`).
pub(crate) fn uses_stdin(command: &ActionsCommand) -> bool {
    match command {
        ActionsCommand::Secret { command } | ActionsCommand::Variable { command } => matches!(
            command,
            ValueCommand::Set { value: None, value_file, .. }
                if value_file.as_ref().is_none_or(|file| file.as_os_str() == "-")
        ),
        _ => false,
    }
}
