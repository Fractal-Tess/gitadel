//! File browsing and stable code sharing.
use crate::{ApiClient, query::with_query, route_repo};
use anyhow::{Result, bail};
use clap::{Args, Subcommand};
use reqwest::Method;
use serde_json::{Value, json};

#[derive(Debug, Args)]
pub(crate) struct FileArgs {
    pub repository: String,
    pub path: String,
    #[arg(long)]
    pub rev: Option<String>,
    #[arg(long)]
    pub start: Option<usize>,
    #[arg(long)]
    pub end: Option<usize>,
}
#[derive(Debug, Subcommand)]
pub(crate) enum FileCommand {
    /// Read a file, including its total line count.
    View(FileArgs),
    /// Read selected lines and a commit-pinned permanent link.
    Snippet(FileArgs),
    /// Get the permanent link to a file or selected lines.
    Link(FileArgs),
    /// Attribute selected file lines to their original commits.
    Blame(FileArgs),
    /// List function, class, and type declarations.
    Symbols(FileArgs),
    /// List commits affecting this path.
    History {
        #[command(flatten)]
        file: FileArgs,
        #[arg(long, default_value_t = 1)]
        page: usize,
    },
}

pub(crate) async fn run(api: &ApiClient, command: FileCommand) -> Result<Value> {
    let (file, mut endpoint, page, link) = match command {
        FileCommand::View(file) => (file, "blob", None, false),
        FileCommand::Snippet(file) => (file, "snippet", None, false),
        FileCommand::Link(file) => (file, "snippet", None, true),
        FileCommand::Blame(file) => (file, "blame", None, false),
        FileCommand::Symbols(file) => (file, "symbols", None, false),
        FileCommand::History { file, page } => (file, "history", Some(page), false),
    };
    if file.start.is_some() || file.end.is_some() {
        if matches!(endpoint, "symbols" | "history") {
            bail!("Line ranges are supported by view, snippet, link, and blame.");
        }
        if endpoint == "blob" {
            endpoint = "snippet";
        }
    }
    let path = with_query(
        format!(
            "{}/{endpoint}",
            route_repo("repositories", &file.repository)?
        ),
        &[
            ("path", Some(file.path)),
            ("rev", file.rev),
            ("start", file.start.map(|value| value.to_string())),
            ("end", file.end.map(|value| value.to_string())),
            ("page", page.map(|value| value.to_string())),
        ],
    );
    let response = api.request(Method::GET, &path, None).await?;
    Ok(if link {
        json!({"url":response["permalink"],"commit_oid":response["commit_oid"]})
    } else {
        response
    })
}

#[derive(Debug, Subcommand)]
pub(crate) enum WebsiteCommand {
    /// List the repository's hosted websites.
    List { repository: String },
    /// Replace websites in display order. Each entry is a URL or LABEL=URL.
    Set {
        repository: String,
        #[arg(required = true)]
        websites: Vec<String>,
    },
    /// Remove all hosted website links.
    Clear { repository: String },
}
pub(crate) async fn websites(api: &ApiClient, command: WebsiteCommand) -> Result<Value> {
    let (repository, links) = match command {
        WebsiteCommand::List { repository } => (repository, None),
        WebsiteCommand::Clear { repository } => (repository, Some(vec![])),
        WebsiteCommand::Set {
            repository,
            websites,
        } => (
            repository,
            Some(
                websites
                    .into_iter()
                    .map(|value| {
                        let (label, url) =
                            if value.starts_with("http://") || value.starts_with("https://") {
                                ("", value.as_str())
                            } else {
                                value.split_once('=').unwrap_or(("", value.as_str()))
                            };
                        json!({"label":label,"url":url})
                    })
                    .collect(),
            ),
        ),
    };
    let path = format!("{}/websites", route_repo("repositories", &repository)?);
    match links {
        None => api.request(Method::GET, &path, None).await,
        Some(websites) => {
            api.request(Method::PUT, &path, Some(json!({"websites":websites})))
                .await
        }
    }
}
