//! `gtd import`: import repositories from other forges or clone URLs.

use anyhow::{Result, bail};
use clap::{ArgGroup, Args, Subcommand, ValueEnum};
use reqwest::Method;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{ApiClient, id, route_path};

#[derive(Debug, Subcommand)]
pub(crate) enum ImportCommand {
    /// List the repositories a mirror identity can import from its forge.
    Discover {
        #[arg(long, value_enum)]
        provider: Provider,
        /// Namespace that owns the mirror identity.
        #[arg(long)]
        namespace: String,
        /// Mirror identity ID holding the forge access token.
        #[arg(long)]
        identity: String,
    },
    /// Show an import and the state of each repository in it.
    View { id: String },
    /// Start an import from a forge (--provider/--repo) or a clone URL (--url/--name).
    Create(CreateImport),
    /// Retry the failed repositories of an import.
    Retry { id: String },
    /// Cancel an import's queued repositories.
    Cancel { id: String },
}

#[derive(Debug, Args)]
#[command(group(ArgGroup::new("source").required(true).args(["provider", "url"])))]
pub(crate) struct CreateImport {
    /// Destination namespace; it must also own the mirror identity.
    #[arg(long)]
    namespace: String,
    /// Mirror identity ID holding the credential.
    #[arg(long)]
    identity: String,
    /// Forge to import from.
    #[arg(long, value_enum, requires = "repositories")]
    provider: Option<Provider>,
    /// Source repository ID, full name, or name, optionally renamed with `=NAME` (repeatable).
    #[arg(long = "repo", value_name = "SOURCE[=NAME]", requires = "provider")]
    repositories: Vec<String>,
    /// HTTPS clone URL of a single repository.
    #[arg(long, requires = "name", conflicts_with = "provider")]
    url: Option<String>,
    /// Destination repository name for --url.
    #[arg(long, requires = "url")]
    name: Option<String>,
    /// Destination visibility for --url.
    #[arg(long, value_enum, default_value = "private")]
    visibility: Visibility,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(crate) enum Provider {
    Github,
    Gitlab,
    Gitea,
    Forgejo,
}

impl Provider {
    fn as_str(self) -> &'static str {
        match self {
            Self::Github => "github",
            Self::Gitlab => "gitlab",
            Self::Gitea => "gitea",
            Self::Forgejo => "forgejo",
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(crate) enum Visibility {
    Public,
    Private,
}

impl Visibility {
    fn as_str(self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::Private => "private",
        }
    }
}

fn import_path(value: &str, action: Option<&str>) -> Result<String> {
    let import_id = id(value)?.to_string();
    match action {
        Some(action) => route_path("repository-imports", &[&import_id, action]),
        None => route_path("repository-imports", &[&import_id]),
    }
}

fn discover_body(provider: Provider, namespace: &str, identity: Uuid) -> Value {
    json!({ "provider": provider.as_str(), "namespace": namespace, "identity_id": identity })
}

/// Maps `SOURCE[=NAME]` selections onto discovered repositories.
fn selections(discovered: &Value, requested: &[String]) -> Result<Vec<Value>> {
    let repositories = discovered
        .get("repositories")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    requested
        .iter()
        .map(|selection| {
            let (source, target) = match selection.split_once('=') {
                Some((source, target)) => (source, Some(target)),
                None => (selection.as_str(), None),
            };
            let field = |repository: &Value, key: &str| {
                repository.get(key).and_then(Value::as_str) == Some(source)
            };
            let found = repositories
                .iter()
                .find(|repository| field(repository, "id"))
                .or_else(|| {
                    repositories
                        .iter()
                        .find(|repository| field(repository, "full_name"))
                })
                .or_else(|| {
                    let mut named = repositories
                        .iter()
                        .filter(|repository| field(repository, "name"));
                    match (named.next(), named.next()) {
                        (Some(repository), None) => Some(repository),
                        _ => None,
                    }
                });
            let Some(repository) = found else {
                bail!("source repository not found or ambiguous: {source}");
            };
            let source_id = repository.get("id").and_then(Value::as_str);
            let name = target.or_else(|| repository.get("name").and_then(Value::as_str));
            let (Some(source_id), Some(name)) = (source_id, name) else {
                bail!("Gitadel returned an incomplete source repository for {source}");
            };
            if name.is_empty() {
                bail!("destination name for {source} is empty");
            }
            Ok(json!({ "source_id": source_id, "target_name": name }))
        })
        .collect()
}

async fn create(api: &ApiClient, args: CreateImport) -> Result<Value> {
    let identity = id(&args.identity)?;
    if let Some(url) = args.url {
        let Some(name) = args.name else {
            bail!("--url requires --name");
        };
        return api
            .request(
                Method::POST,
                "repository-imports/direct",
                Some(json!({
                    "remote_url": url,
                    "identity_id": identity,
                    "target_namespace": args.namespace,
                    "target_name": name,
                    "visibility": args.visibility.as_str(),
                })),
            )
            .await;
    }
    let Some(provider) = args.provider else {
        bail!("provide --provider with --repo, or --url with --name");
    };
    let discovered = api
        .request(
            Method::POST,
            "repository-imports/discover",
            Some(discover_body(provider, &args.namespace, identity)),
        )
        .await?;
    let repositories = selections(&discovered, &args.repositories)?;
    api.request(
        Method::POST,
        "repository-imports",
        Some(json!({
            "provider": provider.as_str(),
            "identity_id": identity,
            "target_namespace": args.namespace,
            "repositories": repositories,
        })),
    )
    .await
}

pub(crate) async fn run(api: &ApiClient, command: ImportCommand) -> Result<Value> {
    match command {
        ImportCommand::Discover {
            provider,
            namespace,
            identity,
        } => {
            api.request(
                Method::POST,
                "repository-imports/discover",
                Some(discover_body(provider, &namespace, id(&identity)?)),
            )
            .await
        }
        ImportCommand::View { id } => {
            api.request(Method::GET, &import_path(&id, None)?, None)
                .await
        }
        ImportCommand::Create(args) => create(api, args).await,
        ImportCommand::Retry { id } => {
            api.request(Method::POST, &import_path(&id, Some("retry"))?, None)
                .await
        }
        ImportCommand::Cancel { id } => {
            api.request(Method::DELETE, &import_path(&id, None)?, None)
                .await
        }
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;
    use crate::{Cli, Command};

    const IDENTITY: &str = "00000000-0000-0000-0000-000000000001";

    fn parse(args: &[&str]) -> Result<ImportCommand, clap::Error> {
        let mut argv = vec!["gtd", "import"];
        argv.extend_from_slice(args);
        match Cli::try_parse_from(argv)?.command {
            Command::Import { command } => Ok(command),
            other => panic!("unexpected command: {other:?}"),
        }
    }

    fn create(extra: &[&str]) -> Result<ImportCommand, clap::Error> {
        let mut args = vec!["create", "--namespace", "alice", "--identity", IDENTITY];
        args.extend_from_slice(extra);
        parse(&args)
    }

    fn discovered() -> Value {
        json!({"repositories": [
            {"id": "11", "name": "hello", "full_name": "octo/hello"},
            {"id": "12", "name": "tools", "full_name": "octo/tools"},
            {"id": "13", "name": "tools", "full_name": "other/tools"}
        ]})
    }

    #[test]
    fn create_requires_a_source() {
        assert!(create(&[]).is_err());
    }

    #[test]
    fn create_rejects_url_with_provider() {
        let extra = [
            "--url",
            "https://example.test/x.git",
            "--name",
            "x",
            "--provider",
            "github",
            "--repo",
            "11",
        ];
        assert!(create(&extra).is_err());
    }

    #[test]
    fn create_url_requires_name() {
        assert!(create(&["--url", "https://example.test/x.git"]).is_err());
    }

    #[test]
    fn create_provider_requires_repositories() {
        assert!(create(&["--provider", "github"]).is_err());
    }

    #[test]
    fn create_accepts_provider_selection() {
        assert!(create(&["--provider", "gitlab", "--repo", "octo/hello=greeting"]).is_ok());
    }

    #[test]
    fn selections_resolve_id_full_name_and_rename() {
        let selected = selections(
            &discovered(),
            &["11".to_owned(), "octo/tools=kit".to_owned()],
        )
        .unwrap();
        assert_eq!(
            selected,
            [
                json!({"source_id": "11", "target_name": "hello"}),
                json!({"source_id": "12", "target_name": "kit"}),
            ]
        );
    }

    #[test]
    fn selections_reject_ambiguous_short_names() {
        assert!(selections(&discovered(), &["tools".to_owned()]).is_err());
    }

    #[test]
    fn discover_body_matches_server_request() {
        assert_eq!(
            discover_body(Provider::Forgejo, "alice", Uuid::nil()),
            json!({"provider": "forgejo", "namespace": "alice", "identity_id": Uuid::nil()})
        );
    }
}
