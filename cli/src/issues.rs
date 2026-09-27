//! `gtd issue`: repository issues, comments, and labels.

use anyhow::{Result, bail};
use clap::{Subcommand, ValueEnum};
use reqwest::Method;
use serde_json::{Map, Value, json};
use uuid::Uuid;

use crate::{ApiClient, input::TextArgs, query::with_query, route_path, route_repo};

#[derive(Debug, Subcommand)]
pub(crate) enum IssueCommand {
    /// List issues, most recently updated first.
    List {
        repository: String,
        #[arg(long, value_enum, default_value = "open")]
        state: IssueStateFilter,
        /// Only issues whose title or body contains this text.
        #[arg(long)]
        search: Option<String>,
    },
    /// Show an issue.
    View {
        repository: String,
        number: u64,
        /// Also fetch the issue's comments.
        #[arg(long)]
        comments: bool,
    },
    /// Open an issue.
    Create {
        repository: String,
        #[arg(long)]
        title: String,
        #[command(flatten)]
        text: TextArgs,
        /// Label name or ID to apply (repeatable; requires write access).
        #[arg(long = "label", value_name = "LABEL")]
        labels: Vec<String>,
        /// Username to assign (requires write access).
        #[arg(long)]
        assignee: Option<String>,
    },
    /// Change an issue's title, body, or assignee.
    Edit {
        repository: String,
        number: u64,
        #[arg(long)]
        title: Option<String>,
        #[command(flatten)]
        text: TextArgs,
        /// Username to assign.
        #[arg(long, conflicts_with = "unassign")]
        assignee: Option<String>,
        /// Remove the current assignee.
        #[arg(long)]
        unassign: bool,
    },
    /// Close an issue.
    Close { repository: String, number: u64 },
    /// Reopen a closed issue.
    Reopen { repository: String, number: u64 },
    /// Add a comment to an issue.
    Comment {
        repository: String,
        number: u64,
        #[command(flatten)]
        text: TextArgs,
    },
    /// Add or remove labels on an issue.
    Label {
        repository: String,
        number: u64,
        /// Label name or ID to add (repeatable).
        #[arg(long, value_name = "LABEL")]
        add: Vec<String>,
        /// Label name or ID to remove (repeatable).
        #[arg(long, value_name = "LABEL")]
        remove: Vec<String>,
        /// Remove every label before applying --add.
        #[arg(long)]
        clear: bool,
    },
    /// Manage the repository's issue labels.
    Labels {
        #[command(subcommand)]
        command: LabelCommand,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum LabelCommand {
    /// List issue labels.
    List { repository: String },
    /// Create an issue label.
    Create {
        repository: String,
        name: String,
        /// Six hexadecimal digits, with or without a leading `#`.
        #[arg(long)]
        color: String,
        #[arg(long, default_value = "")]
        description: String,
    },
    /// Rename or restyle an issue label.
    Edit {
        repository: String,
        /// Label name or ID.
        label: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        color: Option<String>,
        #[arg(long)]
        description: Option<String>,
    },
    /// Delete an issue label and remove it from every issue.
    Delete {
        repository: String,
        /// Label name or ID.
        label: String,
    },
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(crate) enum IssueStateFilter {
    Open,
    Closed,
    All,
}

impl IssueStateFilter {
    fn query_value(self) -> Option<String> {
        match self {
            Self::Open => Some("open".to_owned()),
            Self::Closed => Some("closed".to_owned()),
            Self::All => None,
        }
    }
}

pub(crate) fn uses_stdin(command: &IssueCommand) -> bool {
    match command {
        IssueCommand::Create { text, .. }
        | IssueCommand::Edit { text, .. }
        | IssueCommand::Comment { text, .. } => text.uses_stdin(),
        _ => false,
    }
}

fn issues_path(repository: &str) -> Result<String> {
    Ok(route_repo("repositories", repository)? + "/issues")
}

fn issue_path(repository: &str, number: u64) -> Result<String> {
    route_path(&issues_path(repository)?, &[&number.to_string()])
}

fn labels_path(repository: &str) -> Result<String> {
    Ok(route_repo("repositories", repository)? + "/issue-labels")
}

fn create_body(
    title: &str,
    body: Option<String>,
    label_ids: &[Uuid],
    assignee: Option<String>,
) -> Value {
    let mut request = json!({
        "title": title,
        "body": body.unwrap_or_default(),
        "label_ids": label_ids,
    });
    if let Some(assignee) = assignee {
        request["assignee"] = json!(assignee);
    }
    request
}

fn edit_body(
    title: Option<String>,
    body: Option<String>,
    assignee: Option<String>,
    unassign: bool,
) -> Result<Value> {
    let mut request = Map::new();
    if let Some(title) = title {
        request.insert("title".to_owned(), json!(title));
    }
    if let Some(body) = body {
        request.insert("body".to_owned(), json!(body));
    }
    // The server treats an empty assignee as "unassigned".
    if let Some(assignee) = assignee.or_else(|| unassign.then(String::new)) {
        request.insert("assignee".to_owned(), json!(assignee));
    }
    if request.is_empty() {
        bail!("provide at least one of --title, --body, --body-file, --assignee, or --unassign");
    }
    Ok(Value::Object(request))
}

fn label_update_body(
    name: Option<String>,
    color: Option<String>,
    description: Option<String>,
) -> Result<Value> {
    let mut request = Map::new();
    for (key, value) in [
        ("name", name),
        ("color", color),
        ("description", description),
    ] {
        if let Some(value) = value {
            request.insert(key.to_owned(), json!(value));
        }
    }
    if request.is_empty() {
        bail!("provide at least one of --name, --color, or --description");
    }
    Ok(Value::Object(request))
}

/// Resolves label names or IDs against the repository's label list.
fn match_labels(labels: &Value, wanted: &[String]) -> Result<Vec<Uuid>> {
    let labels = labels.as_array().map(Vec::as_slice).unwrap_or_default();
    wanted
        .iter()
        .map(|wanted| {
            let found = labels.iter().find(|label| {
                label.get("id").and_then(Value::as_str) == Some(wanted.as_str())
                    || label.get("name").and_then(Value::as_str) == Some(wanted.as_str())
            });
            let Some(id) = found
                .and_then(|label| label.get("id"))
                .and_then(Value::as_str)
            else {
                bail!("issue label not found: {wanted}");
            };
            crate::id(id)
        })
        .collect()
}

async fn resolve_labels(api: &ApiClient, repository: &str, wanted: &[String]) -> Result<Vec<Uuid>> {
    if wanted.is_empty() {
        return Ok(Vec::new());
    }
    // Skip the lookup when every label is already an ID.
    if let Ok(ids) = wanted
        .iter()
        .map(|value| Uuid::parse_str(value))
        .collect::<Result<Vec<_>, _>>()
    {
        return Ok(ids);
    }
    let labels = api
        .request(Method::GET, &labels_path(repository)?, None)
        .await?;
    match_labels(&labels, wanted)
}

fn next_label_ids(issue: &Value, add: &[Uuid], remove: &[Uuid], clear: bool) -> Result<Vec<Uuid>> {
    let mut ids = Vec::new();
    if !clear {
        for label in issue
            .get("labels")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default()
        {
            if let Some(id) = label.get("id").and_then(Value::as_str) {
                ids.push(crate::id(id)?);
            }
        }
    }
    ids.retain(|id| !remove.contains(id));
    for id in add {
        if !ids.contains(id) && !remove.contains(id) {
            ids.push(*id);
        }
    }
    Ok(ids)
}

async fn set_state(api: &ApiClient, repository: &str, number: u64, state: &str) -> Result<Value> {
    api.request(
        Method::PATCH,
        &issue_path(repository, number)?,
        Some(json!({ "state": state })),
    )
    .await
}

pub(crate) async fn run(api: &ApiClient, command: IssueCommand) -> Result<Value> {
    match command {
        IssueCommand::List {
            repository,
            state,
            search,
        } => {
            let path = with_query(
                issues_path(&repository)?,
                &[("state", state.query_value()), ("q", search)],
            );
            api.request(Method::GET, &path, None).await
        }
        IssueCommand::View {
            repository,
            number,
            comments,
        } => {
            let path = issue_path(&repository, number)?;
            let issue = api.request(Method::GET, &path, None).await?;
            if !comments {
                return Ok(issue);
            }
            let comments = api
                .request(Method::GET, &(path + "/comments"), None)
                .await?;
            Ok(json!({ "issue": issue, "comments": comments }))
        }
        IssueCommand::Create {
            repository,
            title,
            text,
            labels,
            assignee,
        } => {
            let body = text.read()?;
            let label_ids = resolve_labels(api, &repository, &labels).await?;
            api.request(
                Method::POST,
                &issues_path(&repository)?,
                Some(create_body(&title, body, &label_ids, assignee)),
            )
            .await
        }
        IssueCommand::Edit {
            repository,
            number,
            title,
            text,
            assignee,
            unassign,
        } => {
            let body = edit_body(title, text.read()?, assignee, unassign)?;
            api.request(Method::PATCH, &issue_path(&repository, number)?, Some(body))
                .await
        }
        IssueCommand::Close { repository, number } => {
            set_state(api, &repository, number, "closed").await
        }
        IssueCommand::Reopen { repository, number } => {
            set_state(api, &repository, number, "open").await
        }
        IssueCommand::Comment {
            repository,
            number,
            text,
        } => {
            let Some(body) = text.read()? else {
                bail!("provide --body or --body-file");
            };
            api.request(
                Method::POST,
                &(issue_path(&repository, number)? + "/comments"),
                Some(json!({ "body": body })),
            )
            .await
        }
        IssueCommand::Label {
            repository,
            number,
            add,
            remove,
            clear,
        } => {
            if add.is_empty() && remove.is_empty() && !clear {
                bail!("provide --add, --remove, or --clear");
            }
            let add = resolve_labels(api, &repository, &add).await?;
            let remove = resolve_labels(api, &repository, &remove).await?;
            let path = issue_path(&repository, number)?;
            let issue = api.request(Method::GET, &path, None).await?;
            let label_ids = next_label_ids(&issue, &add, &remove, clear)?;
            api.request(
                Method::PATCH,
                &path,
                Some(json!({ "label_ids": label_ids })),
            )
            .await
        }
        IssueCommand::Labels { command } => run_labels(api, command).await,
    }
}

async fn label_path(api: &ApiClient, repository: &str, label: &str) -> Result<String> {
    let [id] = resolve_labels(api, repository, &[label.to_owned()])
        .await?
        .try_into()
        .map_err(|_| anyhow::anyhow!("expected exactly one label"))?;
    route_path(&labels_path(repository)?, &[&id.to_string()])
}

async fn run_labels(api: &ApiClient, command: LabelCommand) -> Result<Value> {
    match command {
        LabelCommand::List { repository } => {
            api.request(Method::GET, &labels_path(&repository)?, None)
                .await
        }
        LabelCommand::Create {
            repository,
            name,
            color,
            description,
        } => {
            api.request(
                Method::POST,
                &labels_path(&repository)?,
                Some(json!({ "name": name, "color": color, "description": description })),
            )
            .await
        }
        LabelCommand::Edit {
            repository,
            label,
            name,
            color,
            description,
        } => {
            let body = label_update_body(name, color, description)?;
            let path = label_path(api, &repository, &label).await?;
            api.request(Method::PATCH, &path, Some(body)).await
        }
        LabelCommand::Delete { repository, label } => {
            let path = label_path(api, &repository, &label).await?;
            api.request(Method::DELETE, &path, None).await
        }
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;
    use crate::{Cli, Command};

    fn parse(args: &[&str]) -> Result<IssueCommand, clap::Error> {
        let mut argv = vec!["gtd", "issue"];
        argv.extend_from_slice(args);
        match Cli::try_parse_from(argv)?.command {
            Command::Issue { command } => Ok(command),
            other => panic!("unexpected command: {other:?}"),
        }
    }

    const BUG: &str = "00000000-0000-0000-0000-000000000001";
    const DOCS: &str = "00000000-0000-0000-0000-000000000002";

    fn labels() -> Value {
        json!([{"id": BUG, "name": "bug"}, {"id": DOCS, "name": "docs"}])
    }

    #[test]
    fn list_defaults_to_open_issues() {
        let command = parse(&["list", "alice/demo"]).unwrap();
        assert!(matches!(
            command,
            IssueCommand::List {
                state: IssueStateFilter::Open,
                ..
            }
        ));
    }

    #[test]
    fn edit_rejects_assignee_with_unassign() {
        assert!(parse(&["edit", "a/b", "1", "--assignee", "bob", "--unassign"]).is_err());
    }

    #[test]
    fn comment_rejects_body_with_body_file() {
        assert!(parse(&["comment", "a/b", "1", "--body", "x", "--body-file", "-"]).is_err());
    }

    #[test]
    fn comment_body_file_dash_uses_stdin() {
        let command = parse(&["comment", "a/b", "1", "--body-file", "-"]).unwrap();
        assert!(uses_stdin(&command));
    }

    #[test]
    fn issue_number_must_be_positive_integer() {
        assert!(parse(&["view", "a/b", "-1"]).is_err());
    }

    #[test]
    fn create_body_omits_missing_assignee() {
        let body = create_body("Crash", None, &[], None);
        assert_eq!(body, json!({"title": "Crash", "body": "", "label_ids": []}));
    }

    #[test]
    fn edit_body_sends_empty_assignee_to_unassign() {
        let body = edit_body(None, None, None, true).unwrap();
        assert_eq!(body, json!({"assignee": ""}));
    }

    #[test]
    fn edit_body_requires_a_change() {
        assert!(edit_body(None, None, None, false).is_err());
    }

    #[test]
    fn label_update_body_requires_a_change() {
        assert!(label_update_body(None, None, None).is_err());
    }

    #[test]
    fn match_labels_accepts_names_and_ids() {
        let ids = match_labels(&labels(), &["docs".to_owned(), BUG.to_owned()]).unwrap();
        assert_eq!(ids, [id(DOCS), id(BUG)]);
    }

    #[test]
    fn match_labels_rejects_unknown_names() {
        assert!(match_labels(&labels(), &["missing".to_owned()]).is_err());
    }

    #[test]
    fn next_label_ids_adds_and_removes_without_duplicates() {
        let issue = json!({"labels": [{"id": BUG}]});
        let ids = next_label_ids(&issue, &[id(BUG), id(DOCS)], &[], false).unwrap();
        assert_eq!(ids, [id(BUG), id(DOCS)]);
        let ids = next_label_ids(&issue, &[], &[id(BUG)], false).unwrap();
        assert!(ids.is_empty());
    }

    #[test]
    fn next_label_ids_clear_replaces_existing_labels() {
        let issue = json!({"labels": [{"id": BUG}]});
        let ids = next_label_ids(&issue, &[id(DOCS)], &[], true).unwrap();
        assert_eq!(ids, [id(DOCS)]);
    }

    fn id(value: &str) -> Uuid {
        Uuid::parse_str(value).unwrap()
    }
}
