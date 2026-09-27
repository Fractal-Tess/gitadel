//! `gtd release`: repository releases and their downloadable assets.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::Subcommand;
use reqwest::{Method, header::CONTENT_TYPE};
use serde_json::{Map, Value, json};

use crate::{ApiClient, id, input::TextArgs, route_path, route_repo};

#[derive(Debug, Subcommand)]
pub(crate) enum ReleaseCommand {
    /// List releases, newest first.
    List { repository: String },
    /// Show a release.
    View {
        repository: String,
        /// Release ID, tag, or title.
        release: String,
    },
    /// Publish a release for a tag or revision.
    Create {
        repository: String,
        /// Tag or revision the release points at.
        #[arg(long)]
        target: String,
        /// Release title (defaults to the target).
        #[arg(long)]
        title: Option<String>,
        #[command(flatten)]
        text: TextArgs,
        /// Mark the release as a prerelease.
        #[arg(long)]
        prerelease: bool,
    },
    /// Change a release's target, title, notes, or prerelease flag.
    Edit {
        repository: String,
        /// Release ID, tag, or title.
        release: String,
        #[arg(long)]
        target: Option<String>,
        #[arg(long)]
        title: Option<String>,
        #[command(flatten)]
        text: TextArgs,
        #[arg(long, conflicts_with = "no_prerelease")]
        prerelease: bool,
        #[arg(long)]
        no_prerelease: bool,
    },
    /// Delete a release and its assets.
    Delete {
        repository: String,
        /// Release ID, tag, or title.
        release: String,
    },
    /// Upload, download, or delete release assets.
    Asset {
        #[command(subcommand)]
        command: AssetCommand,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum AssetCommand {
    /// Upload a file as a release asset (up to 2 GiB).
    Upload {
        repository: String,
        /// Release ID, tag, or title.
        release: String,
        file: PathBuf,
        /// Asset file name (defaults to the uploaded file's name).
        #[arg(long)]
        name: Option<String>,
        #[arg(long, default_value = "application/octet-stream")]
        content_type: String,
    },
    /// Download a release asset.
    Download {
        repository: String,
        /// Release ID, tag, or title.
        release: String,
        /// Asset ID or file name.
        asset: String,
        /// Destination file (defaults to the asset name in the current directory).
        #[arg(long, value_name = "FILE")]
        output: Option<PathBuf>,
    },
    /// Delete a release asset.
    Delete {
        repository: String,
        /// Release ID, tag, or title.
        release: String,
        /// Asset ID or file name.
        asset: String,
    },
}

pub(crate) fn uses_stdin(command: &ReleaseCommand) -> bool {
    match command {
        ReleaseCommand::Create { text, .. } | ReleaseCommand::Edit { text, .. } => {
            text.uses_stdin()
        }
        _ => false,
    }
}

fn releases_path(repository: &str) -> Result<String> {
    Ok(route_repo("repositories", repository)? + "/releases")
}

fn create_body(
    target: &str,
    title: Option<String>,
    body: Option<String>,
    prerelease: bool,
) -> Value {
    json!({
        "target_revision": target,
        "title": title.as_deref().unwrap_or(target),
        "body": body.unwrap_or_default(),
        "prerelease": prerelease,
    })
}

fn edit_body(
    target: Option<String>,
    title: Option<String>,
    body: Option<String>,
    prerelease: Option<bool>,
) -> Result<Value> {
    let mut request = Map::new();
    if let Some(target) = target {
        request.insert("target_revision".to_owned(), json!(target));
    }
    if let Some(title) = title {
        request.insert("title".to_owned(), json!(title));
    }
    if let Some(body) = body {
        request.insert("body".to_owned(), json!(body));
    }
    if let Some(prerelease) = prerelease {
        request.insert("prerelease".to_owned(), json!(prerelease));
    }
    if request.is_empty() {
        bail!(
            "provide at least one of --target, --title, --body, --body-file, --prerelease, or --no-prerelease"
        );
    }
    Ok(Value::Object(request))
}

/// Finds the single entry whose `id` or one of `fields` equals `wanted`.
fn find_unique<'a>(
    items: &'a Value,
    wanted: &str,
    fields: &[&str],
    kind: &str,
) -> Result<&'a Value> {
    let items = items.as_array().map(Vec::as_slice).unwrap_or_default();
    if let Some(item) = items
        .iter()
        .find(|item| item.get("id").and_then(Value::as_str) == Some(wanted))
    {
        return Ok(item);
    }
    let mut matches = items.iter().filter(|item| {
        fields
            .iter()
            .any(|field| item.get(field).and_then(Value::as_str) == Some(wanted))
    });
    match (matches.next(), matches.next()) {
        (Some(item), None) => Ok(item),
        (None, _) => bail!("{kind} not found: {wanted}"),
        (Some(_), Some(_)) => bail!("{kind} {wanted} is ambiguous; use its ID"),
    }
}

fn item_id(item: &Value) -> Result<String> {
    let value = item
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("Gitadel returned an entry without an id"))?;
    Ok(id(value)?.to_string())
}

/// Resolves a release reference to its API path and JSON representation.
async fn release(api: &ApiClient, repository: &str, wanted: &str) -> Result<(String, Value)> {
    let base = releases_path(repository)?;
    if let Ok(release_id) = id(wanted) {
        let path = route_path(&base, &[&release_id.to_string()])?;
        let release = api.request(Method::GET, &path, None).await?;
        return Ok((path, release));
    }
    let releases = api.request(Method::GET, &base, None).await?;
    let release = find_unique(&releases, wanted, &["target_revision", "title"], "release")?;
    let path = route_path(&base, &[&item_id(release)?])?;
    Ok((path, release.clone()))
}

fn asset_path(release_path: &str, release: &Value, wanted: &str) -> Result<(String, String)> {
    let assets = release.get("assets").unwrap_or(&Value::Null);
    let asset = find_unique(assets, wanted, &["name"], "release asset")?;
    let name = asset
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or(wanted)
        .to_owned();
    let path = route_path(&(release_path.to_owned() + "/assets"), &[&item_id(asset)?])?;
    Ok((path, name))
}

/// Returns `name` when it is a plain file name that is safe to create in the current directory.
fn default_output(name: &str) -> Result<PathBuf> {
    let path = Path::new(name);
    if path.file_name().and_then(|file| file.to_str()) != Some(name) {
        bail!("asset name {name:?} is not a plain file name; pass --output");
    }
    Ok(path.to_path_buf())
}

fn upload_name(file: &Path, name: Option<String>) -> Result<String> {
    match name {
        Some(name) => Ok(name),
        None => file
            .file_name()
            .and_then(|name| name.to_str())
            .map(str::to_owned)
            .ok_or_else(|| anyhow::anyhow!("could not derive an asset name; pass --name")),
    }
}

impl ApiClient {
    /// Streams `file` as the request body of a PUT to `path`.
    async fn upload(&self, path: &str, file: &Path, content_type: &str) -> Result<Value> {
        let endpoint = self.endpoint(path)?;
        let handle = tokio::fs::File::open(file)
            .await
            .with_context(|| format!("could not open {}", file.display()))?;
        let mut request = self
            .client
            .put(endpoint)
            .header(CONTENT_TYPE, content_type)
            .body(reqwest::Body::from(handle));
        if let Some(token) = self.token.as_deref() {
            request = request.bearer_auth(token);
        }
        let response = request
            .send()
            .await
            .context("could not reach the Gitadel server")?;
        self.read_response(response).await
    }
}

pub(crate) async fn run(api: &ApiClient, command: ReleaseCommand) -> Result<Value> {
    match command {
        ReleaseCommand::List { repository } => {
            api.request(Method::GET, &releases_path(&repository)?, None)
                .await
        }
        ReleaseCommand::View {
            repository,
            release: wanted,
        } => Ok(release(api, &repository, &wanted).await?.1),
        ReleaseCommand::Create {
            repository,
            target,
            title,
            text,
            prerelease,
        } => {
            let body = create_body(&target, title, text.read()?, prerelease);
            api.request(Method::POST, &releases_path(&repository)?, Some(body))
                .await
        }
        ReleaseCommand::Edit {
            repository,
            release: wanted,
            target,
            title,
            text,
            prerelease,
            no_prerelease,
        } => {
            let prerelease = match (prerelease, no_prerelease) {
                (true, _) => Some(true),
                (false, true) => Some(false),
                (false, false) => None,
            };
            let body = edit_body(target, title, text.read()?, prerelease)?;
            let (path, _) = release(api, &repository, &wanted).await?;
            api.request(Method::PATCH, &path, Some(body)).await
        }
        ReleaseCommand::Delete {
            repository,
            release: wanted,
        } => {
            let (path, _) = release(api, &repository, &wanted).await?;
            api.request(Method::DELETE, &path, None).await
        }
        ReleaseCommand::Asset { command } => run_asset(api, command).await,
    }
}

async fn run_asset(api: &ApiClient, command: AssetCommand) -> Result<Value> {
    match command {
        AssetCommand::Upload {
            repository,
            release: wanted,
            file,
            name,
            content_type,
        } => {
            let name = upload_name(&file, name)?;
            let (path, _) = release(api, &repository, &wanted).await?;
            let path = crate::query::with_query(path + "/assets", &[("name", Some(name))]);
            api.upload(&path, &file, &content_type).await
        }
        AssetCommand::Download {
            repository,
            release: wanted,
            asset,
            output,
        } => {
            let (release_path, release) = release(api, &repository, &wanted).await?;
            let (path, name) = asset_path(&release_path, &release, &asset)?;
            let output = match output {
                Some(output) => output,
                None => default_output(&name)?,
            };
            api.download(&path, &output).await
        }
        AssetCommand::Delete {
            repository,
            release: wanted,
            asset,
        } => {
            let (release_path, release) = release(api, &repository, &wanted).await?;
            let (path, _) = asset_path(&release_path, &release, &asset)?;
            api.request(Method::DELETE, &path, None).await
        }
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;
    use crate::{Cli, Command};

    fn parse(args: &[&str]) -> Result<ReleaseCommand, clap::Error> {
        let mut argv = vec!["gtd", "release"];
        argv.extend_from_slice(args);
        match Cli::try_parse_from(argv)?.command {
            Command::Release { command } => Ok(command),
            other => panic!("unexpected command: {other:?}"),
        }
    }

    const FIRST: &str = "00000000-0000-0000-0000-000000000001";
    const SECOND: &str = "00000000-0000-0000-0000-000000000002";

    fn releases() -> Value {
        json!([
            {"id": FIRST, "target_revision": "v1.0.0", "title": "First", "assets": [
                {"id": SECOND, "name": "gtd.tar.gz"}
            ]},
            {"id": SECOND, "target_revision": "v1.1.0", "title": "First"}
        ])
    }

    #[test]
    fn create_requires_target() {
        assert!(parse(&["create", "a/b"]).is_err());
    }

    #[test]
    fn edit_rejects_both_prerelease_flags() {
        assert!(parse(&["edit", "a/b", "v1", "--prerelease", "--no-prerelease"]).is_err());
    }

    #[test]
    fn create_body_defaults_title_to_target() {
        assert_eq!(
            create_body("v1.0.0", None, None, true),
            json!({"target_revision": "v1.0.0", "title": "v1.0.0", "body": "", "prerelease": true})
        );
    }

    #[test]
    fn edit_body_includes_only_supplied_fields() {
        let body = edit_body(None, Some("New".to_owned()), None, Some(false)).unwrap();
        assert_eq!(body, json!({"title": "New", "prerelease": false}));
    }

    #[test]
    fn edit_body_requires_a_change() {
        assert!(edit_body(None, None, None, None).is_err());
    }

    #[test]
    fn find_unique_matches_tag() {
        let releases = releases();
        let found = find_unique(
            &releases,
            "v1.1.0",
            &["target_revision", "title"],
            "release",
        );
        assert_eq!(found.unwrap()["id"], SECOND);
    }

    #[test]
    fn find_unique_rejects_ambiguous_title() {
        assert!(
            find_unique(
                &releases(),
                "First",
                &["target_revision", "title"],
                "release"
            )
            .is_err()
        );
    }

    #[test]
    fn asset_path_resolves_asset_by_name() {
        let release = &releases()[0];
        let (path, name) =
            asset_path("repositories/a/b/releases/x", release, "gtd.tar.gz").unwrap();
        assert_eq!(path, format!("repositories/a/b/releases/x/assets/{SECOND}"));
        assert_eq!(name, "gtd.tar.gz");
    }

    #[test]
    fn default_output_rejects_path_components() {
        assert!(default_output("../escape").is_err());
        assert!(default_output("..").is_err());
        assert!(default_output("gtd.tar.gz").is_ok());
    }

    #[test]
    fn upload_name_defaults_to_file_name() {
        let name = upload_name(Path::new("dist/gtd.tar.gz"), None).unwrap();
        assert_eq!(name, "gtd.tar.gz");
    }
}
