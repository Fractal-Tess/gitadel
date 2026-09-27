//! Container registry commands: `gtd repo registry` and `gtd admin registry`.

use anyhow::Result;
use clap::{Subcommand, ValueEnum};
use reqwest::Method;
use serde_json::{Value, json};

use crate::{ApiClient, id, query::with_query, route_path, route_repo};

/// Lists a repository's container images, their tags, and digests.
pub(crate) async fn browse(api: &ApiClient, repository: &str) -> Result<Value> {
    api.request(
        Method::GET,
        &(route_repo("repositories", repository)? + "/registry"),
        None,
    )
    .await
}

#[derive(Debug, Subcommand)]
pub(crate) enum AdminRegistryCommand {
    /// Show registry storage usage and the active storage target.
    Status,
    /// List per-repository registry usage.
    Repositories(RepositoryUsageArgs),
    /// Move registry objects to another storage target.
    Migrate {
        target_id: String,
        #[arg(long, default_value_t = 100, value_parser = clap::value_parser!(u64).range(1..))]
        batch_size: u64,
    },
    /// Stream a registry migration's progress until it finishes.
    Progress { operation_id: String },
}

#[derive(Debug, clap::Args)]
pub(crate) struct RepositoryUsageArgs {
    /// Filter by repository name.
    #[arg(long)]
    search: Option<String>,
    /// Filter by owner name.
    #[arg(long)]
    owner: Option<String>,
    #[arg(long, value_enum)]
    owner_type: Option<OwnerType>,
    #[arg(long)]
    min_bytes: Option<u64>,
    #[arg(long)]
    max_bytes: Option<u64>,
    #[arg(long, value_enum, default_value = "bytes-desc")]
    sort: UsageSort,
    #[arg(long, default_value_t = 10, value_parser = clap::value_parser!(u64).range(1..=100))]
    limit: u64,
    #[arg(long, default_value_t = 0)]
    offset: u64,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(crate) enum OwnerType {
    User,
    Organization,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(crate) enum UsageSort {
    #[value(alias = "bytes_desc")]
    BytesDesc,
    #[value(alias = "bytes_asc")]
    BytesAsc,
    Name,
}

fn usage_path(args: RepositoryUsageArgs) -> String {
    let owner_type = args.owner_type.map(|owner_type| match owner_type {
        OwnerType::User => "user",
        OwnerType::Organization => "organization",
    });
    let sort = match args.sort {
        UsageSort::BytesDesc => "bytes_desc",
        UsageSort::BytesAsc => "bytes_asc",
        UsageSort::Name => "name",
    };
    with_query(
        "admin/storage/registry/repositories".to_owned(),
        &[
            ("search", args.search),
            ("owner", args.owner),
            ("owner_type", owner_type.map(str::to_owned)),
            ("min_bytes", args.min_bytes.map(|value| value.to_string())),
            ("max_bytes", args.max_bytes.map(|value| value.to_string())),
            ("sort", Some(sort.to_owned())),
            ("limit", Some(args.limit.to_string())),
            ("offset", Some(args.offset.to_string())),
        ],
    )
}

/// Runs an administrator registry command. Returns `None` when output was already streamed.
pub(crate) async fn run_admin(
    api: &ApiClient,
    command: AdminRegistryCommand,
) -> Result<Option<Value>> {
    let value = match command {
        AdminRegistryCommand::Status => {
            api.request(Method::GET, "admin/storage/registry/status", None)
                .await?
        }
        AdminRegistryCommand::Repositories(args) => {
            api.request(Method::GET, &usage_path(args), None).await?
        }
        AdminRegistryCommand::Migrate {
            target_id,
            batch_size,
        } => {
            api.request(
                Method::POST,
                "admin/storage/registry/migrate",
                Some(json!({ "target_id": id(&target_id)?, "batch_size": batch_size })),
            )
            .await?
        }
        AdminRegistryCommand::Progress { operation_id } => {
            let operation_id = id(&operation_id)?;
            let path = route_path(
                "admin/storage/registry/migrations",
                &[&operation_id.to_string(), "events"],
            )?;
            api.progress(&path, operation_id).await?;
            return Ok(None);
        }
    };
    Ok(Some(value))
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;
    use crate::{AdminCommand, Cli, Command, RepoCommand};

    fn parse_admin(args: &[&str]) -> Result<AdminRegistryCommand, clap::Error> {
        let mut argv = vec!["gtd", "admin", "registry"];
        argv.extend_from_slice(args);
        match Cli::try_parse_from(argv)?.command {
            Command::Admin {
                command: AdminCommand::Registry { command },
            } => Ok(command),
            other => panic!("unexpected command: {other:?}"),
        }
    }

    fn usage_args(args: &[&str]) -> RepositoryUsageArgs {
        let mut argv = vec!["repositories"];
        argv.extend_from_slice(args);
        match parse_admin(&argv).unwrap() {
            AdminRegistryCommand::Repositories(args) => args,
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn repo_registry_parses_repository() {
        let cli = Cli::try_parse_from(["gtd", "repo", "registry", "alice/demo"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Repo {
                command: RepoCommand::Registry { .. }
            }
        ));
    }

    #[test]
    fn usage_path_uses_server_defaults() {
        assert_eq!(
            usage_path(usage_args(&[])),
            "admin/storage/registry/repositories?sort=bytes_desc&limit=10&offset=0"
        );
    }

    #[test]
    fn usage_path_encodes_filters() {
        let path = usage_path(usage_args(&[
            "--owner-type",
            "organization",
            "--search",
            "web app",
            "--sort",
            "bytes_asc",
        ]));
        assert_eq!(
            path,
            "admin/storage/registry/repositories?search=web+app&owner_type=organization&sort=bytes_asc&limit=10&offset=0"
        );
    }

    #[test]
    fn repositories_rejects_oversized_page() {
        assert!(parse_admin(&["repositories", "--limit", "101"]).is_err());
    }

    #[test]
    fn migrate_rejects_zero_batch_size() {
        let target = uuid::Uuid::nil().to_string();
        assert!(parse_admin(&["migrate", &target, "--batch-size", "0"]).is_err());
    }
}
