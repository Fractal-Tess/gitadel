//! Storage domain commands: `gtd admin storage domain ...`.
//!
//! One set of commands covers every storage domain the server reports, such
//! as `lfs` and `registry`.

use anyhow::Result;
use clap::{Args, Subcommand, ValueEnum};
use reqwest::Method;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{ApiClient, id, query::with_query, route_path};

const DOMAINS: &str = "admin/storage/domains";

#[derive(Debug, Subcommand)]
pub(crate) enum DomainCommand {
    /// List storage domains with their active target, usage, and migrations.
    List,
    /// Show one domain's active target, usage totals, and migrations.
    Status {
        /// Storage domain, such as `lfs` or `registry`.
        domain: String,
    },
    /// List per-repository usage in one domain.
    Repositories {
        /// Storage domain, such as `lfs` or `registry`.
        domain: String,
        #[command(flatten)]
        filters: RepositoryUsageArgs,
    },
    /// Move a domain to another storage target or back to local storage.
    Migrate {
        /// Storage domain, such as `lfs` or `registry`.
        domain: String,
        #[command(flatten)]
        destination: Destination,
        #[arg(long, default_value_t = 100, value_parser = clap::value_parser!(u64).range(1..))]
        batch_size: u64,
    },
    /// Stream a migration's progress until it finishes.
    Progress {
        /// Storage domain, such as `lfs` or `registry`.
        domain: String,
        operation_id: String,
        /// Print the current progress once instead of streaming.
        #[arg(long)]
        once: bool,
    },
}

/// Where a migration moves the domain's objects.
#[derive(Debug, Args)]
#[group(required = true, multiple = false)]
pub(crate) struct Destination {
    /// Storage target id.
    #[arg(long, value_name = "ID")]
    target: Option<String>,
    /// The domain's local storage.
    #[arg(long)]
    local: bool,
}

#[derive(Debug, Args)]
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

impl Default for RepositoryUsageArgs {
    fn default() -> Self {
        Self {
            search: None,
            owner: None,
            owner_type: None,
            min_bytes: None,
            max_bytes: None,
            sort: UsageSort::BytesDesc,
            limit: 10,
            offset: 0,
        }
    }
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

pub(crate) fn status_path(domain: &str) -> Result<String> {
    route_path(DOMAINS, &[domain])
}

pub(crate) fn repositories_path(domain: &str, args: RepositoryUsageArgs) -> Result<String> {
    let owner_type = args.owner_type.map(|owner_type| match owner_type {
        OwnerType::User => "user",
        OwnerType::Organization => "organization",
    });
    let sort = match args.sort {
        UsageSort::BytesDesc => "bytes_desc",
        UsageSort::BytesAsc => "bytes_asc",
        UsageSort::Name => "name",
    };
    Ok(with_query(
        route_path(DOMAINS, &[domain, "repositories"])?,
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
    ))
}

/// The migration request body; `None` selects local storage.
fn migration_body(target: Option<Uuid>, batch_size: u64) -> Value {
    match target {
        Some(target_id) if !target_id.is_nil() => {
            json!({ "target_id": target_id, "batch_size": batch_size })
        }
        _ => json!({ "local": true, "batch_size": batch_size }),
    }
}

/// Starts a migration; `None` or the nil id selects local storage.
pub(crate) async fn migrate(
    api: &ApiClient,
    domain: &str,
    target: Option<Uuid>,
    batch_size: u64,
) -> Result<Value> {
    api.request(
        Method::POST,
        &route_path(DOMAINS, &[domain, "migrate"])?,
        Some(migration_body(target, batch_size)),
    )
    .await
}

/// Streams a migration's progress events until it completes or fails.
pub(crate) async fn stream_progress(
    api: &ApiClient,
    domain: &str,
    operation_id: Uuid,
) -> Result<()> {
    let path = route_path(
        DOMAINS,
        &[domain, "migrations", &operation_id.to_string(), "events"],
    )?;
    api.progress(&path, operation_id).await
}

/// Runs a storage domain command. Returns `None` when output was streamed.
pub(crate) async fn run(api: &ApiClient, command: DomainCommand) -> Result<Option<Value>> {
    let value = match command {
        DomainCommand::List => api.request(Method::GET, DOMAINS, None).await?,
        DomainCommand::Status { domain } => {
            api.request(Method::GET, &status_path(&domain)?, None)
                .await?
        }
        DomainCommand::Repositories { domain, filters } => {
            api.request(Method::GET, &repositories_path(&domain, filters)?, None)
                .await?
        }
        DomainCommand::Migrate {
            domain,
            destination,
            batch_size,
        } => {
            let target = destination.target.as_deref().map(id).transpose()?;
            migrate(api, &domain, target, batch_size).await?
        }
        DomainCommand::Progress {
            domain,
            operation_id,
            once,
        } => {
            let operation_id = id(&operation_id)?;
            if !once {
                stream_progress(api, &domain, operation_id).await?;
                return Ok(None);
            }
            let path = route_path(DOMAINS, &[&domain, "migrations", &operation_id.to_string()])?;
            api.request(Method::GET, &path, None).await?
        }
    };
    Ok(Some(value))
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;
    use crate::{AdminCommand, Cli, Command, StorageCommand};

    fn parse(args: &[&str]) -> Result<DomainCommand, clap::Error> {
        let mut argv = vec!["gtd", "admin", "storage", "domain"];
        argv.extend_from_slice(args);
        match Cli::try_parse_from(argv)?.command {
            Command::Admin {
                command:
                    AdminCommand::Storage {
                        command: StorageCommand::Domain { command },
                    },
            } => Ok(command),
            other => panic!("unexpected command: {other:?}"),
        }
    }

    fn repositories(args: &[&str]) -> String {
        let mut argv = vec!["repositories"];
        argv.extend_from_slice(args);
        match parse(&argv).unwrap() {
            DomainCommand::Repositories { domain, filters } => {
                repositories_path(&domain, filters).unwrap()
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn list_and_status_parse() {
        assert!(matches!(parse(&["list"]).unwrap(), DomainCommand::List));
        assert!(matches!(
            parse(&["status", "registry"]).unwrap(),
            DomainCommand::Status { domain } if domain == "registry"
        ));
    }

    #[test]
    fn status_path_encodes_the_domain() {
        assert_eq!(status_path("lfs").unwrap(), "admin/storage/domains/lfs");
        assert_eq!(status_path("a/b").unwrap(), "admin/storage/domains/a%2Fb");
    }

    #[test]
    fn repositories_path_uses_server_defaults() {
        assert_eq!(
            repositories(&["lfs"]),
            "admin/storage/domains/lfs/repositories?sort=bytes_desc&limit=10&offset=0"
        );
    }

    #[test]
    fn repositories_path_encodes_filters() {
        assert_eq!(
            repositories(&[
                "registry",
                "--search",
                "web app",
                "--owner",
                "team",
                "--owner-type",
                "organization",
                "--min-bytes",
                "1",
                "--max-bytes",
                "2",
                "--sort",
                "name",
                "--limit",
                "5",
                "--offset",
                "10",
            ]),
            "admin/storage/domains/registry/repositories?search=web+app&owner=team&owner_type=organization&min_bytes=1&max_bytes=2&sort=name&limit=5&offset=10"
        );
    }

    #[test]
    fn repositories_rejects_oversized_page() {
        assert!(parse(&["repositories", "lfs", "--limit", "101"]).is_err());
    }

    #[test]
    fn migrate_requires_exactly_one_destination() {
        let target = Uuid::new_v4().to_string();
        assert!(parse(&["migrate", "lfs"]).is_err());
        assert!(parse(&["migrate", "lfs", "--target", &target, "--local"]).is_err());
        assert!(parse(&["migrate", "lfs", "--local"]).is_ok());
        assert!(parse(&["migrate", "lfs", "--target", &target]).is_ok());
    }

    #[test]
    fn migrate_rejects_zero_batch_size() {
        assert!(parse(&["migrate", "lfs", "--local", "--batch-size", "0"]).is_err());
    }

    #[test]
    fn migration_body_selects_local_for_missing_or_nil_targets() {
        let target = Uuid::new_v4();
        assert_eq!(
            migration_body(Some(target), 5),
            json!({ "target_id": target, "batch_size": 5 })
        );
        assert_eq!(
            migration_body(None, 5),
            json!({ "local": true, "batch_size": 5 })
        );
        assert_eq!(
            migration_body(Some(Uuid::nil()), 5),
            json!({ "local": true, "batch_size": 5 })
        );
    }

    #[test]
    fn progress_parses_once_flag() {
        let operation = Uuid::new_v4().to_string();
        assert!(matches!(
            parse(&["progress", "registry", &operation, "--once"]).unwrap(),
            DomainCommand::Progress { once: true, .. }
        ));
    }
}
