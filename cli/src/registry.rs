//! Container registry commands: `gtd repo registry` and the deprecated
//! `gtd admin registry` aliases of `gtd admin storage domain`.

use anyhow::Result;
use clap::Subcommand;
use reqwest::Method;
use serde_json::Value;

use crate::{
    ApiClient, id, route_repo,
    storage_domains::{self, RepositoryUsageArgs},
};

/// Lists a repository's container images, their tags, and digests.
pub(crate) async fn browse(api: &ApiClient, repository: &str) -> Result<Value> {
    api.request(
        Method::GET,
        &(route_repo("repositories", repository)? + "/registry"),
        None,
    )
    .await
}

/// Deprecated aliases of `gtd admin storage domain ... registry`.
#[derive(Debug, Subcommand)]
pub(crate) enum AdminRegistryCommand {
    /// Deprecated: use `gtd admin storage domain status registry`.
    Status,
    /// Deprecated: use `gtd admin storage domain repositories registry`.
    Repositories(RepositoryUsageArgs),
    /// Deprecated: use `gtd admin storage domain migrate registry`.
    ///
    /// The nil target id selects repository-backed local storage.
    Migrate {
        target_id: String,
        #[arg(long, default_value_t = 100, value_parser = clap::value_parser!(u64).range(1..))]
        batch_size: u64,
    },
    /// Deprecated: use `gtd admin storage domain progress registry`.
    Progress { operation_id: String },
}

const DOMAIN: &str = "registry";

/// Runs an administrator registry command. Returns `None` when output was already streamed.
pub(crate) async fn run_admin(
    api: &ApiClient,
    command: AdminRegistryCommand,
) -> Result<Option<Value>> {
    let value = match command {
        AdminRegistryCommand::Status => {
            api.request(Method::GET, &storage_domains::status_path(DOMAIN)?, None)
                .await?
        }
        AdminRegistryCommand::Repositories(args) => {
            api.request(
                Method::GET,
                &storage_domains::repositories_path(DOMAIN, args)?,
                None,
            )
            .await?
        }
        AdminRegistryCommand::Migrate {
            target_id,
            batch_size,
        } => storage_domains::migrate(api, DOMAIN, Some(id(&target_id)?), batch_size).await?,
        AdminRegistryCommand::Progress { operation_id } => {
            storage_domains::stream_progress(api, DOMAIN, id(&operation_id)?).await?;
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
    fn repositories_alias_uses_the_domain_api() {
        let path = storage_domains::repositories_path(
            DOMAIN,
            usage_args(&[
                "--owner-type",
                "organization",
                "--search",
                "web app",
                "--sort",
                "bytes_asc",
            ]),
        )
        .unwrap();
        assert_eq!(
            path,
            "admin/storage/domains/registry/repositories?search=web+app&owner_type=organization&sort=bytes_asc&limit=10&offset=0"
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
