use std::{
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand, ValueEnum};
use reqwest::{Client, Method, StatusCode};
use serde_json::{Value, json};
use url::Url;
use uuid::Uuid;

mod auth;
use auth::AuthCommand;

#[derive(Debug, Parser)]
#[command(
    name = "gtd",
    version,
    about = "Command-line client for Gitadel"
)]
struct Cli {
    /// Gitadel HTTP origin (credentials and URL paths are not accepted).
    ///
    /// If omitted, GITADEL_SERVER, the saved login, and localhost are tried
    /// in that order.
    #[arg(long, global = true)]
    server: Option<Url>,

    /// API token. The value is hidden in help and is never included in URLs.
    ///
    /// `GITADEL_TOKEN` is used only when no explicit token source is selected.
    #[arg(long, hide_env_values = true, global = true, conflicts_with_all = ["token_file", "token_stdin"])]
    token: Option<String>,

    /// Read the API token from a file (use `-` for stdin).
    #[arg(long, value_name = "FILE", global = true, conflicts_with_all = ["token", "token_stdin"])]
    token_file: Option<PathBuf>,

    /// Read the API token from stdin (useful in CI secret pipes).
    #[arg(long, global = true, conflicts_with_all = ["token", "token_file"])]
    token_stdin: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Manage repositories, controls, lifecycle, and collaborators.
    Repo {
        #[command(subcommand)]
        command: RepoCommand,
    },
    /// Show the authenticated user's profile and manage SSH keys.
    Me {
        #[command(subcommand)]
        command: MeCommand,
    },
    /// Manage organizations and memberships owned by the authenticated user.
    Org {
        #[command(subcommand)]
        command: OrgCommand,
    },
    /// Manage administrator-only instance, authentication, integrity, storage, and backups.
    Admin {
        #[command(subcommand)]
        command: AdminCommand,
    },
    /// Manage the saved CLI login.
    Auth {
        #[command(subcommand)]
        command: AuthCommand,
    },
    /// Call a remaining JSON API endpoint on the configured Gitadel origin.
    Api(ApiCommand),
}

#[derive(Debug, Subcommand)]
enum RepoCommand {
    /// List repositories visible to the authenticated user.
    List,
    /// View a repository (public repositories may be viewed without a token).
    View { repository: String },
    /// Create an empty repository, preserving the server CLI's create options.
    Create(CreateRepository),
    /// Update repository settings. BODY must match the repository control API schema.
    Control(RepositoryBodyCommand),
    /// Archive a repository.
    Archive { repository: String },
    /// Remove the archived state from a repository.
    Unarchive { repository: String },
    /// Soft-delete a repository.
    Delete { repository: String },
    /// Restore a soft-deleted repository during its recovery period.
    Restore { repository: String },
    /// Permanently purge a soft-deleted repository after confirmation.
    Purge {
        repository: String,
        /// Must be exactly `purge`.
        #[arg(long)]
        confirmation: String,
    },
    /// Add or remove the authenticated user's favorite marker.
    Favorite {
        repository: String,
        #[arg(long, conflicts_with = "remove")]
        remove: bool,
    },
    Collaborators {
        #[command(subcommand)]
        command: CollaboratorCommand,
    },
}

#[derive(Debug, Args)]
struct CreateRepository {
    /// Repository path in namespace/name form.
    repository: String,
    /// Keep the repository private (the default is public).
    #[arg(long, conflicts_with = "public")]
    private: bool,
    /// Explicitly make the repository public.
    #[arg(long, conflicts_with = "private")]
    public: bool,
    /// Short repository description.
    #[arg(long)]
    description: Option<String>,
    /// Git object format.
    #[arg(long, default_value = "sha1", value_parser = ["sha1", "sha256"])]
    object_format: String,
    /// Optional complete create request JSON (supports mirror creation fields).
    #[arg(long, value_name = "FILE")]
    body_file: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct RepositoryBodyCommand {
    repository: String,
    #[command(flatten)]
    body: BodyCommand,
}

#[derive(Debug, Subcommand)]
enum CollaboratorCommand {
    List {
        repository: String,
    },
    Add {
        repository: String,
        username: String,
        #[arg(long, value_enum)]
        role: CollaboratorRole,
    },
    Remove {
        repository: String,
        username: String,
    },
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum CollaboratorRole {
    Read,
    Write,
}

#[derive(Debug, Subcommand)]
enum MeCommand {
    /// Show the authenticated user through the existing `/user` API route.
    Profile,
    SshKeys {
        #[command(subcommand)]
        command: SshKeyCommand,
    },
}

#[derive(Debug, Subcommand)]
enum SshKeyCommand {
    List,
    Add {
        name: String,
        /// OpenSSH public key text, or `@FILE` to read it from a file.
        public_key: String,
    },
    Remove {
        id: String,
    },
}

#[derive(Debug, Subcommand)]
enum OrgCommand {
    List,
    Create {
        slug: String,
        display_name: String,
    },
    Update {
        slug: String,
        next_slug: String,
        display_name: String,
    },
    Members {
        #[command(subcommand)]
        command: OrganizationMemberCommand,
    },
    Suggestions {
        slug: String,
        #[arg(long, default_value = "")]
        query: String,
    },
}

#[derive(Debug, Subcommand)]
enum OrganizationMemberCommand {
    List {
        slug: String,
    },
    Add {
        slug: String,
        username: String,
        #[arg(long, value_enum)]
        role: OrganizationRole,
    },
    Remove {
        slug: String,
        username: String,
    },
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum OrganizationRole {
    Owner,
    Member,
}

#[derive(Debug, Subcommand)]
enum AdminCommand {
    /// Read or update public/admin instance settings.
    Instance {
        #[command(subcommand)]
        command: BodyOrGetCommand,
    },
    /// Read or update repository integrity-check settings.
    Integrity {
        #[command(subcommand)]
        command: BodyOrGetCommand,
    },
    /// Read authentication configuration or manage OIDC providers.
    Authentication {
        #[command(subcommand)]
        command: AuthenticationCommand,
    },
    /// Manage configured storage targets and migrations.
    Storage {
        #[command(subcommand)]
        command: StorageCommand,
    },
    /// Manage backup providers, schedules, and backup actions.
    Backup {
        #[command(subcommand)]
        command: BackupCommand,
    },
    /// Read administrator audit history.
    Audit {
        #[arg(long, default_value_t = 100)]
        limit: u64,
    },
}

#[derive(Debug, Subcommand)]
enum AuthenticationCommand {
    Get,
    Update(BodyCommand),
    Providers {
        #[command(subcommand)]
        command: AuthenticationProviderCommand,
    },
}

#[derive(Debug, Subcommand)]
enum AuthenticationProviderCommand {
    List,
    Create(BodyCommand),
    Update {
        id: String,
        #[command(flatten)]
        body: BodyCommand,
    },
    Delete {
        id: String,
    },
}

#[derive(Debug, Subcommand)]
enum StorageCommand {
    Targets {
        #[command(subcommand)]
        command: StorageTargetCommand,
    },
    LfsStatus,
    LfsRepositories,
    Migrate {
        target_id: String,
        #[arg(long, default_value_t = 100)]
        batch_size: usize,
    },
    Progress {
        operation_id: String,
    },
}

#[derive(Debug, Subcommand)]
enum StorageTargetCommand {
    List,
    Create(BodyCommand),
    Test(BodyCommand),
    TestSaved { id: String },
    Usage { id: String },
    Delete { id: String },
}

#[derive(Debug, Subcommand)]
enum BackupCommand {
    Providers {
        #[command(subcommand)]
        command: BackupProviderCommand,
    },
    Progress {
        operation_id: String,
    },
    Create {
        provider_id: String,
        #[command(flatten)]
        body: BodyCommand,
    },
    Preflight {
        provider_id: String,
        #[command(flatten)]
        body: BodyCommand,
    },
    List {
        provider_id: String,
    },
    Download {
        provider_id: String,
        key: String,
        #[arg(long, value_name = "FILE")]
        output: PathBuf,
    },

    Delete {
        provider_id: String,
        key: String,
    },
}

#[derive(Debug, Subcommand)]
enum BackupProviderCommand {
    List,
    Test(BodyCommand),
    Create(BodyCommand),
    Update {
        id: String,
        #[command(flatten)]
        body: BodyCommand,
    },
    Delete {
        id: String,
    },
    Schedule {
        id: String,
        #[command(flatten)]
        body: BodyCommand,
    },
}

#[derive(Debug, Args)]
struct BodyCommand {
    /// JSON object/array/value to send.
    #[arg(long, conflicts_with = "body_file")]
    body: Option<String>,
    /// Read and validate JSON from this file, or `-` for stdin.
    #[arg(long, value_name = "FILE", conflicts_with = "body")]
    body_file: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct OptionalBodyCommand {
    #[arg(long, conflicts_with = "body_file")]
    body: Option<String>,
    #[arg(long, value_name = "FILE", conflicts_with = "body")]
    body_file: Option<PathBuf>,
}

#[derive(Debug, Subcommand)]
enum BodyOrGetCommand {
    Get,
    Update(BodyCommand),
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum ApiMethod {
    Get,
    Post,
    Put,
    Patch,
    Delete,
}

#[derive(Debug, Args)]
struct ApiCommand {
    /// API path below `/api/v1`, for example `/repositories`.
    path: String,
    #[arg(short, long, value_enum, default_value = "get")]
    method: ApiMethod,
    #[command(flatten)]
    body: OptionalBodyCommand,
}

struct ApiClient {
    client: Client,
    origin: Url,
    token: Option<String>,
}

impl ApiClient {
    fn new(server: Url, token: Option<String>) -> Result<Self> {
        let origin = validate_origin(server)?;
        let client = Client::builder()
            // Do not forward Authorization across an untrusted redirect.
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .context("could not construct HTTP client")?;
        Ok(Self {
            client,
            origin,
            token,
        })
    }

    fn endpoint(&self, path: &str) -> Result<Url> {
        let path = path.strip_prefix('/').unwrap_or(path);
        let path = path.strip_prefix("api/v1/").unwrap_or(path);
        let (path, query) = path.split_once('?').unwrap_or((path, ""));
        if path.is_empty() || path.starts_with('/') || path.contains('\\') {
            bail!("API path must be a relative path below /api/v1");
        }
        if path.contains("://") || path.starts_with("//") || path.contains('#') {
            bail!("API path must stay on the configured server origin");
        }
        for segment in path.split('/') {
            if is_traversal_segment(segment) {
                bail!("API path must not contain traversal segments");
            }
        }
        let mut endpoint = self
            .origin
            .join(&format!("api/v1/{path}"))
            .context("API path is invalid")?;
        if !query.is_empty() {
            if query.contains('#') {
                bail!("API query must not contain a fragment");
            }
            endpoint.set_query(Some(query));
        }
        if endpoint.origin() != self.origin.origin() {
            bail!("API path escaped the configured server origin");
        }
        Ok(endpoint)
    }

    async fn request(&self, method: Method, path: &str, body: Option<Value>) -> Result<Value> {
        let endpoint = self.endpoint(path)?;
        let mut request = self.client.request(method, endpoint);
        if let Some(token) = self.token.as_deref() {
            request = request.bearer_auth(token);
        }
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request
            .send()
            .await
            .context("could not reach the Gitadel server")?;
        self.read_response(response).await
    }

    async fn read_response(&self, response: reqwest::Response) -> Result<Value> {
        let status = response.status();
        if status == StatusCode::NO_CONTENT {
            return Ok(json!({"status": status.as_u16()}));
        }
        let bytes = response
            .bytes()
            .await
            .context("could not read Gitadel response")?;
        let parsed = serde_json::from_slice::<Value>(&bytes);
        if !status.is_success() {
            let parsed = parsed.unwrap_or_else(
                |_| json!({"status": status.as_u16(), "body": String::from_utf8_lossy(&bytes)}),
            );
            let message = parsed
                .get("error")
                .and_then(|error| error.get("message"))
                .and_then(Value::as_str)
                .unwrap_or_else(|| status.canonical_reason().unwrap_or("request failed"));
            bail!(
                "Gitadel returned {status}: {}",
                redact(message, self.token.as_deref())
            );
        }
        let parsed =
            parsed.context("Gitadel returned a successful response that was not valid JSON")?;
        Ok(parsed)
    }

    async fn download(&self, path: &str, output: &Path) -> Result<Value> {
        let endpoint = self.endpoint(path)?;
        let mut request = self.client.get(endpoint);
        if let Some(token) = self.token.as_deref() {
            request = request.bearer_auth(token);
        }
        let mut response = request
            .send()
            .await
            .context("could not reach the Gitadel server")?;
        let status = response.status();
        if !status.is_success() {
            let bytes = response
                .bytes()
                .await
                .context("could not read Gitadel response")?;
            let message = String::from_utf8_lossy(&bytes);
            bail!(
                "Gitadel returned {status}: {}",
                redact(&message, self.token.as_deref())
            );
        }
        let parent = output.parent().unwrap_or_else(|| Path::new("."));
        let name = output
            .file_name()
            .ok_or_else(|| anyhow::anyhow!("output must name a file"))?
            .to_string_lossy();
        let temporary = parent.join(format!(".{name}.{}.part", Uuid::new_v4()));
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let result = async {
            let mut file = options
                .open(&temporary)
                .with_context(|| format!("could not create {}", temporary.display()))?;
            while let Some(chunk) = response
                .chunk()
                .await
                .context("could not read backup download")?
            {
                file.write_all(&chunk)
                    .with_context(|| format!("could not write {}", temporary.display()))?;
            }
            file.sync_all()
                .with_context(|| format!("could not flush {}", temporary.display()))?;
            drop(file);
            fs::rename(&temporary, output)
                .with_context(|| format!("could not publish backup to {}", output.display()))?;
            Ok::<(), anyhow::Error>(())
        }
        .await;
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result?;
        Ok(json!({"status": status.as_u16(), "output": output}))
    }

    async fn progress(&self, path: &str, operation_id: Uuid) -> Result<()> {
        let expected = operation_id.to_string();
        loop {
            let endpoint = self.endpoint(path)?;
            let mut request = self.client.get(endpoint);
            if let Some(token) = self.token.as_deref() {
                request = request.bearer_auth(token);
            }
            let mut response = match request.send().await {
                Ok(response) => response,
                Err(error) if error.is_connect() || error.is_timeout() => {
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    continue;
                }
                Err(error) => return Err(error).context("could not request Gitadel progress"),
            };
            let status = response.status();
            if !status.is_success() {
                if status.is_server_error() {
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    continue;
                }
                let bytes = response
                    .bytes()
                    .await
                    .context("could not read progress response")?;
                let message = String::from_utf8_lossy(&bytes);
                bail!(
                    "Gitadel returned {status}: {}",
                    redact(&message, self.token.as_deref())
                );
            }
            let event_stream = response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.split(';').next())
                .is_some_and(|value| value.trim().eq_ignore_ascii_case("text/event-stream"));
            if !event_stream {
                bail!("Gitadel returned a progress response that was not text/event-stream");
            }
            let mut parser = SseParser::default();
            let mut completed = false;
            loop {
                match response.chunk().await {
                    Ok(Some(chunk)) => {
                        if parser.feed(&chunk, &expected, self.token.as_deref())? {
                            completed = true;
                            break;
                        }
                    }
                    Ok(None) => {
                        completed = parser.finish(&expected, self.token.as_deref())?;
                        break;
                    }
                    Err(_) => break,
                }
            }
            if completed {
                return Ok(());
            }
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
    }
}

const MAX_SSE_LINE: usize = 64 * 1024;
const MAX_SSE_EVENT: usize = 1024 * 1024;

#[derive(Default)]
struct SseParser {
    line: Vec<u8>,
    data: Vec<String>,
    event_size: usize,
}

impl SseParser {
    fn feed(&mut self, bytes: &[u8], operation_id: &str, token: Option<&str>) -> Result<bool> {
        for &byte in bytes {
            if byte == b'\r' {
                continue;
            }
            if byte == b'\n' {
                let line = std::mem::take(&mut self.line);
                if line.is_empty() {
                    if self.dispatch(operation_id, token)? {
                        return Ok(true);
                    }
                } else {
                    self.parse_line(&line)?;
                }
            } else {
                self.line.push(byte);
                if self.line.len() > MAX_SSE_LINE {
                    bail!("progress SSE line is too large");
                }
            }
        }
        Ok(false)
    }

    fn finish(&mut self, operation_id: &str, token: Option<&str>) -> Result<bool> {
        if !self.line.is_empty() {
            let line = std::mem::take(&mut self.line);
            self.parse_line(&line)?;
        }
        self.dispatch(operation_id, token)
    }

    fn parse_line(&mut self, line: &[u8]) -> Result<()> {
        if let Some(data) = line.strip_prefix(b"data:") {
            let data = data.strip_prefix(b" ").unwrap_or(data);
            self.event_size = self.event_size.saturating_add(data.len());
            if self.event_size > MAX_SSE_EVENT {
                bail!("progress SSE event is too large");
            }
            self.data.push(String::from_utf8_lossy(data).into_owned());
        }
        Ok(())
    }

    fn dispatch(&mut self, operation_id: &str, token: Option<&str>) -> Result<bool> {
        if self.data.is_empty() {
            self.event_size = 0;
            return Ok(false);
        }
        let data = self.data.join("\n");
        self.data.clear();
        self.event_size = 0;
        let value = serde_json::from_str::<Value>(&data)
            .context("Gitadel returned invalid JSON in a progress event")?;
        if value.get("operation_id").and_then(Value::as_str) != Some(operation_id) {
            return Ok(false);
        }
        println!("{}", redact(&serde_json::to_string(&value)?, token));
        io::stdout()
            .flush()
            .context("could not flush progress output")?;
        match value.get("phase").and_then(Value::as_str) {
            Some("completed") => Ok(true),
            Some("failed") => {
                let message = value
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("backup operation failed");
                bail!(
                    "Gitadel reported progress failure: {}",
                    redact(message, token)
                )
            }
            _ => Ok(false),
        }
    }
}

fn is_traversal_segment(segment: &str) -> bool {
    if segment == "." || segment == ".." {
        return true;
    }
    let mut decoded = String::with_capacity(segment.len());
    let bytes = segment.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let high = (bytes[index + 1] as char).to_digit(16);
            let low = (bytes[index + 2] as char).to_digit(16);
            if let (Some(high), Some(low)) = (high, low) {
                decoded.push((high * 16 + low) as u8 as char);
                index += 3;
                continue;
            }
        }
        decoded.push(bytes[index] as char);
        index += 1;
    }
    decoded == "." || decoded == ".."
}

fn validate_origin(mut server: Url) -> Result<Url> {
    if !matches!(server.scheme(), "http" | "https")
        || server.host_str().is_none()
        || !server.username().is_empty()
        || server.password().is_some()
        || server.query().is_some()
        || server.fragment().is_some()
        || (server.path() != "/" && !server.path().is_empty())
    {
        bail!("server must be an http(s) origin without credentials, path, query, or fragment");
    }
    server.set_path("/");
    Ok(server)
}

fn redact<'a>(value: &'a str, token: Option<&str>) -> std::borrow::Cow<'a, str> {
    match token.filter(|token| !token.is_empty() && value.contains(token)) {
        Some(token) => std::borrow::Cow::Owned(value.replace(token, "[REDACTED]")),
        None => std::borrow::Cow::Borrowed(value),
    }
}

fn parse_repo(value: &str) -> Result<(&str, &str)> {
    let Some((namespace, name)) = value.split_once('/') else {
        bail!("repository must use namespace/name form");
    };
    if namespace.is_empty()
        || name.is_empty()
        || name.contains('/')
        || namespace.contains(['?', '#', '\\'])
        || name.contains(['?', '#', '\\'])
    {
        bail!("repository must use namespace/name form");
    }
    Ok((namespace, name))
}

fn id(value: &str) -> Result<Uuid> {
    Uuid::parse_str(value).with_context(|| format!("invalid UUID: {value}"))
}

fn body_value(body: &BodyCommand) -> Result<Value> {
    body_value_parts(body.body.as_deref(), body.body_file.as_deref())
}

fn optional_body_value(body: &OptionalBodyCommand) -> Result<Option<Value>> {
    match (&body.body, &body.body_file) {
        (None, None) => Ok(None),
        _ => body_value_parts(body.body.as_deref(), body.body_file.as_deref()).map(Some),
    }
}

fn body_value_parts(body: Option<&str>, file: Option<&Path>) -> Result<Value> {
    let text = if let Some(body) = body {
        body.to_owned()
    } else if let Some(file) = file {
        if file == Path::new("-") {
            let mut text = String::new();
            io::stdin()
                .read_to_string(&mut text)
                .context("could not read JSON from stdin")?;
            text
        } else {
            fs::read_to_string(file)
                .with_context(|| format!("could not read JSON body {}", file.display()))?
        }
    } else {
        bail!("provide --body or --body-file");
    };
    serde_json::from_str(&text).context("JSON body is invalid")
}

fn body_file_is_stdin(file: Option<&PathBuf>) -> bool {
    file.is_some_and(|file| file == Path::new("-"))
}

fn body_command_uses_stdin(body: &BodyCommand) -> bool {
    body_file_is_stdin(body.body_file.as_ref())
}

fn optional_body_command_uses_stdin(body: &OptionalBodyCommand) -> bool {
    body_file_is_stdin(body.body_file.as_ref())
}

fn body_or_get_uses_stdin(command: &BodyOrGetCommand) -> bool {
    matches!(command, BodyOrGetCommand::Update(body) if body_command_uses_stdin(body))
}

fn command_uses_stdin_body(command: &Command) -> bool {
    match command {
        Command::Repo { command } => match command {
            RepoCommand::Create(args) => body_file_is_stdin(args.body_file.as_ref()),
            RepoCommand::Control(args) => body_command_uses_stdin(&args.body),
            _ => false,
        },
        Command::Admin { command } => match command {
            AdminCommand::Instance { command } | AdminCommand::Integrity { command } => {
                body_or_get_uses_stdin(command)
            }
            AdminCommand::Authentication { command } => match command {
                AuthenticationCommand::Update(body)
                | AuthenticationCommand::Providers {
                    command: AuthenticationProviderCommand::Create(body),
                } => body_command_uses_stdin(body),
                AuthenticationCommand::Providers {
                    command: AuthenticationProviderCommand::Update { body, .. },
                } => body_command_uses_stdin(body),
                _ => false,
            },
            AdminCommand::Storage {
                command:
                    StorageCommand::Targets {
                        command:
                            StorageTargetCommand::Create(body) | StorageTargetCommand::Test(body),
                    },
            } => body_command_uses_stdin(body),
            AdminCommand::Backup { command } => match command {
                BackupCommand::Providers { command } => match command {
                    BackupProviderCommand::Test(body) | BackupProviderCommand::Create(body) => {
                        body_command_uses_stdin(body)
                    }
                    BackupProviderCommand::Update { body, .. }
                    | BackupProviderCommand::Schedule { body, .. } => body_command_uses_stdin(body),
                    _ => false,
                },
                BackupCommand::Create { body, .. } | BackupCommand::Preflight { body, .. } => {
                    body_command_uses_stdin(body)
                }
                _ => false,
            },
            _ => false,
        },
        Command::Api(command) => optional_body_command_uses_stdin(&command.body),
        Command::Auth { .. } | Command::Me { .. } | Command::Org { .. } => false,
    }
}

fn token_source_uses_stdin(cli: &Cli) -> bool {
    cli.token_stdin || body_file_is_stdin(cli.token_file.as_ref())
}

fn encode_path_segment(value: &str) -> Result<String> {
    if value.is_empty() {
        bail!("path segment must not be empty");
    }
    let mut url = Url::parse("http://localhost/").expect("static URL must parse");
    url.path_segments_mut()
        .expect("localhost URL has a path")
        .push(value);
    Ok(url.path().trim_start_matches('/').to_owned())
}

fn route_path(prefix: &str, segments: &[&str]) -> Result<String> {
    let mut path = prefix.to_owned();
    for segment in segments {
        path.push('/');
        path.push_str(&encode_path_segment(segment)?);
    }
    Ok(path)
}

fn print_json(value: Value) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

fn backup_provider_update_body(body: &BodyCommand, provider_id: Uuid) -> Result<Value> {
    let mut value = body_value(body)?;
    let object = value
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("backup provider update body must be a JSON object"))?;
    match object.get("id") {
        None => {
            object.insert("id".to_owned(), json!(provider_id));
        }
        Some(Value::String(value)) => {
            if id(value)? != provider_id {
                bail!("backup provider body id does not match the requested resource");
            }
        }
        Some(_) => {
            bail!("backup provider body id does not match the requested resource");
        }
    }
    Ok(value)
}

fn route_repo(prefix: &str, repository: &str) -> Result<String> {
    let (namespace, name) = parse_repo(repository)?;
    route_path(prefix, &[namespace, name])
}

async fn run(cli: Cli) -> Result<()> {
    if token_source_uses_stdin(&cli) && command_uses_stdin_body(&cli.command) {
        bail!("token stdin and body stdin cannot be used together");
    }
    if let Command::Auth { command } = &cli.command {
        let value = auth::run(&cli, command).await?;
        return print_json(value);
    }
    let resolved = auth::resolve(&cli)?;
    let api = ApiClient::new(resolved.server, resolved.token)?;
    let value = match cli.command {
        Command::Repo { command } => run_repo(&api, command).await?,
        Command::Me { command } => run_me(&api, command).await?,
        Command::Org { command } => run_org(&api, command).await?,
        Command::Admin {
            command:
                AdminCommand::Storage {
                    command: StorageCommand::Progress { operation_id },
                },
        } => {
            let operation_id = id(&operation_id)?;
            let path = route_path("admin/storage/progress", &[&operation_id.to_string()])?;
            api.progress(&path, operation_id).await?;
            return Ok(());
        }
        Command::Admin {
            command:
                AdminCommand::Backup {
                    command: BackupCommand::Progress { operation_id },
                },
        } => {
            let operation_id = id(&operation_id)?;
            let path = route_path("admin/backups/progress", &[&operation_id.to_string()])?;
            api.progress(&path, operation_id).await?;
            return Ok(());
        }
        Command::Admin { command } => run_admin(&api, command).await?,
        Command::Api(command) => {
            let body = optional_body_value(&command.body)?;
            let method = match command.method {
                ApiMethod::Get => Method::GET,
                ApiMethod::Post => Method::POST,
                ApiMethod::Put => Method::PUT,
                ApiMethod::Patch => Method::PATCH,
                ApiMethod::Delete => Method::DELETE,
            };
            api.request(method, &command.path, body).await?
        }
        Command::Auth { .. } => unreachable!("auth commands are handled before API setup"),
    };
    print_json(value)
}

async fn run_repo(api: &ApiClient, command: RepoCommand) -> Result<Value> {
    match command {
        RepoCommand::List => api.request(Method::GET, "repositories", None).await,
        RepoCommand::View { repository } => {
            api.request(Method::GET, &route_repo("repositories", &repository)?, None)
                .await
        }
        RepoCommand::Create(args) => {
            let (namespace, name) = parse_repo(&args.repository)?;
            let body = if args.body_file.is_some() {
                body_value_parts(None, args.body_file.as_deref())?
            } else {
                let visibility = match (args.private, args.public) {
                    (true, _) => "private",
                    (false, true) | (false, false) => "public",
                };
                json!({
                    "namespace": namespace,
                    "name": name,
                    "description": args.description,
                    "visibility": visibility,
                    "object_format": args.object_format,
                })
            };
            let object = body
                .as_object()
                .ok_or_else(|| anyhow::anyhow!("repository create body must be a JSON object"))?;
            for field in ["namespace", "name"] {
                if object.get(field).and_then(Value::as_str).is_none() {
                    bail!("repository create body requires a string {field}");
                }
            }
            api.request(Method::POST, "repositories", Some(body)).await
        }
        RepoCommand::Control(args) => {
            let path = route_repo("repositories", &args.repository)? + "/control";
            api.request(Method::PATCH, &path, Some(body_value(&args.body)?))
                .await
        }
        RepoCommand::Archive { repository } => {
            api.request(
                Method::POST,
                &(route_repo("repositories", &repository)? + "/archive"),
                None,
            )
            .await
        }
        RepoCommand::Unarchive { repository } => {
            api.request(
                Method::DELETE,
                &(route_repo("repositories", &repository)? + "/archive"),
                None,
            )
            .await
        }
        RepoCommand::Delete { repository } => {
            api.request(
                Method::POST,
                &(route_repo("repositories", &repository)? + "/delete"),
                None,
            )
            .await
        }
        RepoCommand::Restore { repository } => {
            api.request(
                Method::POST,
                &(route_repo("repositories", &repository)? + "/restore"),
                None,
            )
            .await
        }
        RepoCommand::Purge {
            repository,
            confirmation,
        } => {
            if confirmation != "purge" {
                bail!("--confirmation must be exactly purge");
            }
            api.request(
                Method::DELETE,
                &(route_repo("repositories", &repository)? + "/purge"),
                Some(json!({"confirmation":"purge"})),
            )
            .await
        }
        RepoCommand::Favorite { repository, remove } => {
            api.request(
                if remove { Method::DELETE } else { Method::PUT },
                &(route_repo("repositories", &repository)? + "/favorite"),
                None,
            )
            .await
        }
        RepoCommand::Collaborators { command } => match command {
            CollaboratorCommand::List { repository } => {
                api.request(
                    Method::GET,
                    &(route_repo("repositories", &repository)? + "/collaborators"),
                    None,
                )
                .await
            }
            CollaboratorCommand::Add {
                repository,
                username,
                role,
            } => {
                api.request(
                    Method::POST,
                    &(route_repo("repositories", &repository)? + "/collaborators"),
                    Some(json!({"username":username,"role":format!("{role:?}").to_lowercase()})),
                )
                .await
            }
            CollaboratorCommand::Remove {
                repository,
                username,
            } => {
                let path = route_path(
                    &(route_repo("repositories", &repository)? + "/collaborators"),
                    &[&username],
                )?;
                api.request(Method::DELETE, &path, None).await
            }
        },
    }
}

async fn run_me(api: &ApiClient, command: MeCommand) -> Result<Value> {
    match command {
        MeCommand::Profile => api.request(Method::GET, "user", None).await,
        MeCommand::SshKeys { command } => match command {
            SshKeyCommand::List => api.request(Method::GET, "me/ssh-keys", None).await,
            SshKeyCommand::Add { name, public_key } => {
                let public_key = if let Some(path) = public_key.strip_prefix('@') {
                    fs::read_to_string(path)
                        .with_context(|| format!("could not read SSH key {path}"))?
                } else {
                    public_key
                };
                if public_key.trim().is_empty() {
                    bail!("SSH public key is empty");
                }
                api.request(
                    Method::POST,
                    "me/ssh-keys",
                    Some(json!({"name":name,"public_key":public_key})),
                )
                .await
            }
            SshKeyCommand::Remove { id: value } => {
                api.request(
                    Method::DELETE,
                    &format!("me/ssh-keys/{}", id(&value)?),
                    None,
                )
                .await
            }
        },
    }
}

async fn run_org(api: &ApiClient, command: OrgCommand) -> Result<Value> {
    match command {
        OrgCommand::List => api.request(Method::GET, "organizations", None).await,
        OrgCommand::Create { slug, display_name } => {
            api.request(
                Method::POST,
                "organizations",
                Some(json!({"slug":slug,"display_name":display_name})),
            )
            .await
        }
        OrgCommand::Update {
            slug,
            next_slug,
            display_name,
        } => {
            api.request(
                Method::PUT,
                &route_path("organizations", &[&slug])?,
                Some(json!({"slug":next_slug,"display_name":display_name})),
            )
            .await
        }
        OrgCommand::Suggestions { slug, query } => {
            let path = format!(
                "{}?q={}",
                route_path("organizations", &[&slug])? + "/member-suggestions",
                url::form_urlencoded::byte_serialize(query.as_bytes()).collect::<String>()
            );
            api.request(Method::GET, &path, None).await
        }
        OrgCommand::Members { command } => match command {
            OrganizationMemberCommand::List { slug } => {
                api.request(
                    Method::GET,
                    &(route_path("organizations", &[&slug])? + "/members"),
                    None,
                )
                .await
            }
            OrganizationMemberCommand::Add {
                slug,
                username,
                role,
            } => {
                api.request(
                    Method::POST,
                    &(route_path("organizations", &[&slug])? + "/members"),
                    Some(json!({"username":username,"role":format!("{role:?}").to_lowercase()})),
                )
                .await
            }
            OrganizationMemberCommand::Remove { slug, username } => {
                let path = route_path(
                    &(route_path("organizations", &[&slug])? + "/members"),
                    &[&username],
                )?;
                api.request(Method::DELETE, &path, None).await
            }
        },
    }
}

async fn run_admin(api: &ApiClient, command: AdminCommand) -> Result<Value> {
    match command {
        AdminCommand::Instance { command } => run_get_or_body(api, command, "admin/instance").await,
        AdminCommand::Integrity { command } => {
            run_get_or_body(api, command, "admin/integrity").await
        }
        AdminCommand::Authentication { command } => match command {
            AuthenticationCommand::Get => {
                api.request(Method::GET, "admin/authentication", None).await
            }
            AuthenticationCommand::Update(body) => {
                api.request(
                    Method::PUT,
                    "admin/authentication",
                    Some(body_value(&body)?),
                )
                .await
            }
            AuthenticationCommand::Providers { command } => match command {
                AuthenticationProviderCommand::List => {
                    api.request(Method::GET, "admin/authentication/providers", None)
                        .await
                }
                AuthenticationProviderCommand::Create(body) => {
                    api.request(
                        Method::POST,
                        "admin/authentication/providers",
                        Some(body_value(&body)?),
                    )
                    .await
                }
                AuthenticationProviderCommand::Update { id: value, body } => {
                    api.request(
                        Method::PUT,
                        &format!("admin/authentication/providers/{}", id(&value)?),
                        Some(body_value(&body)?),
                    )
                    .await
                }
                AuthenticationProviderCommand::Delete { id: value } => {
                    api.request(
                        Method::DELETE,
                        &format!("admin/authentication/providers/{}", id(&value)?),
                        None,
                    )
                    .await
                }
            },
        },
        AdminCommand::Storage { command } => run_storage(api, command).await,
        AdminCommand::Backup { command } => run_backup(api, command).await,
        AdminCommand::Audit { limit } => {
            api.request(
                Method::GET,
                &format!("audit?limit={}", limit.clamp(1, 500)),
                None,
            )
            .await
        }
    }
}

async fn run_get_or_body(api: &ApiClient, command: BodyOrGetCommand, path: &str) -> Result<Value> {
    match command {
        BodyOrGetCommand::Get => api.request(Method::GET, path, None).await,
        BodyOrGetCommand::Update(body) => {
            api.request(Method::PUT, path, Some(body_value(&body)?))
                .await
        }
    }
}

async fn run_storage(api: &ApiClient, command: StorageCommand) -> Result<Value> {
    match command {
        StorageCommand::Targets { command } => match command {
            StorageTargetCommand::List => {
                api.request(Method::GET, "admin/storage/targets", None)
                    .await
            }
            StorageTargetCommand::Create(body) => {
                api.request(
                    Method::POST,
                    "admin/storage/targets",
                    Some(body_value(&body)?),
                )
                .await
            }
            StorageTargetCommand::Test(body) => {
                api.request(
                    Method::POST,
                    "admin/storage/targets/test",
                    Some(body_value(&body)?),
                )
                .await
            }
            StorageTargetCommand::TestSaved { id: value } => {
                api.request(
                    Method::POST,
                    &format!("admin/storage/targets/{}/test", id(&value)?),
                    None,
                )
                .await
            }
            StorageTargetCommand::Usage { id: value } => {
                api.request(
                    Method::POST,
                    &format!("admin/storage/targets/{}/usage", id(&value)?),
                    None,
                )
                .await
            }
            StorageTargetCommand::Delete { id: value } => {
                api.request(
                    Method::DELETE,
                    &format!("admin/storage/targets/{}", id(&value)?),
                    None,
                )
                .await
            }
        },
        StorageCommand::LfsStatus => {
            api.request(Method::GET, "admin/storage/lfs/status", None)
                .await
        }
        StorageCommand::LfsRepositories => {
            api.request(Method::GET, "admin/storage/lfs/repositories", None)
                .await
        }
        StorageCommand::Migrate {
            target_id,
            batch_size,
        } => {
            api.request(
                Method::POST,
                "admin/storage/migrations",
                Some(json!({"target_id":id(&target_id)?,"batch_size":batch_size})),
            )
            .await
        }
        StorageCommand::Progress { .. } => {
            unreachable!("storage progress is handled by the top-level dispatcher")
        }
    }
}

fn backup_object_path(provider_id: Uuid, key: &str) -> Result<String> {
    let path = route_path(
        "admin/backup/providers",
        &[&provider_id.to_string(), "backups", "object"],
    )?;
    Ok(format!(
        "{path}?key={}",
        url::form_urlencoded::byte_serialize(key.as_bytes()).collect::<String>()
    ))
}

async fn run_backup(api: &ApiClient, command: BackupCommand) -> Result<Value> {
    match command {
        BackupCommand::Providers { command } => match command {
            BackupProviderCommand::List => {
                api.request(Method::GET, "admin/backup/providers", None)
                    .await
            }
            BackupProviderCommand::Test(body) => {
                api.request(
                    Method::POST,
                    "admin/backup/providers/test",
                    Some(body_value(&body)?),
                )
                .await
            }
            BackupProviderCommand::Create(body) => {
                api.request(
                    Method::POST,
                    "admin/backup/providers",
                    Some(body_value(&body)?),
                )
                .await
            }
            BackupProviderCommand::Update { id: value, body } => {
                let provider_id = id(&value)?;
                api.request(
                    Method::PUT,
                    &route_path("admin/backup/providers", &[&provider_id.to_string()])?,
                    Some(backup_provider_update_body(&body, provider_id)?),
                )
                .await
            }
            BackupProviderCommand::Delete { id: value } => {
                let provider_id = id(&value)?;
                api.request(
                    Method::DELETE,
                    &route_path("admin/backup/providers", &[&provider_id.to_string()])?,
                    None,
                )
                .await
            }
            BackupProviderCommand::Schedule { id: value, body } => {
                let provider_id = id(&value)?;
                api.request(
                    Method::PUT,
                    &route_path(
                        "admin/backup/providers",
                        &[&provider_id.to_string(), "schedule"],
                    )?,
                    Some(body_value(&body)?),
                )
                .await
            }
        },
        BackupCommand::Progress { .. } => {
            unreachable!("backup progress is handled by the top-level dispatcher")
        }
        BackupCommand::Create { provider_id, body } => {
            let provider_id = id(&provider_id)?;
            api.request(
                Method::POST,
                &route_path(
                    "admin/backup/providers",
                    &[&provider_id.to_string(), "backups"],
                )?,
                Some(body_value(&body)?),
            )
            .await
        }
        BackupCommand::Preflight { provider_id, body } => {
            let provider_id = id(&provider_id)?;
            api.request(
                Method::POST,
                &route_path(
                    "admin/backup/providers",
                    &[&provider_id.to_string(), "backups", "preflight"],
                )?,
                Some(body_value(&body)?),
            )
            .await
        }
        BackupCommand::List { provider_id } => {
            let provider_id = id(&provider_id)?;
            api.request(
                Method::GET,
                &route_path(
                    "admin/backup/providers",
                    &[&provider_id.to_string(), "backups"],
                )?,
                None,
            )
            .await
        }
        BackupCommand::Download {
            provider_id,
            key,
            output,
        } => {
            let provider_id = id(&provider_id)?;
            api.download(&backup_object_path(provider_id, &key)?, &output)
                .await
        }
        BackupCommand::Delete { provider_id, key } => {
            let provider_id = id(&provider_id)?;
            api.request(
                Method::DELETE,
                &backup_object_path(provider_id, &key)?,
                None,
            )
            .await
        }
    }
}

#[tokio::main]
async fn main() {
    let result = run(Cli::parse()).await;
    if let Err(error) = result {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}
