use std::{fmt, net::SocketAddr, path::PathBuf};

use anyhow::{Context, Result, ensure};
use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};
use url::Url;

/// Command-line options for the Gitadel server.
#[derive(Debug, Parser)]
#[command(
    name = "gitadel",
    version,
    about = "Run the Gitadel server.",
    after_help = "Configuration precedence: command line > environment > TOML > defaults.\n\
                  TOML defaults to ./gitadel.toml. Environment keys use GITADEL__SECTION__KEY,\n\
                  for example GITADEL__SERVER__BIND=0.0.0.0:3000.\n\
                  CLI environment aliases: GITADEL_CONFIG, GITADEL_BIND, GITADEL_PUBLIC_URL, and GITADEL_DATABASE_URL."
)]
pub struct Cli {
    /// Path to an optional TOML configuration file.
    #[arg(short, long, env = "GITADEL_CONFIG", default_value = "gitadel.toml")]
    config: PathBuf,

    /// Address on which the HTTP server listens.
    #[arg(long, env = "GITADEL_BIND", value_name = "ADDRESS")]
    bind: Option<SocketAddr>,

    /// Public browser origin used for cookies and passkey verification.
    #[arg(long, env = "GITADEL_PUBLIC_URL", value_name = "URL")]
    public_url: Option<Url>,

    /// PEM certificate chain for the embedded HTTPS listener.
    #[arg(
        long,
        env = "GITADEL_TLS_CERTIFICATE",
        value_name = "PATH",
        requires = "tls_private_key"
    )]
    tls_certificate: Option<PathBuf>,

    /// PEM private key for the embedded HTTPS listener.
    #[arg(
        long,
        env = "GITADEL_TLS_PRIVATE_KEY",
        value_name = "PATH",
        requires = "tls_certificate"
    )]
    tls_private_key: Option<PathBuf>,

    /// SeaORM database URL. The default creates ./gitadel.db when SQLite opens it.
    #[arg(long, env = "GITADEL_DATABASE_URL", value_name = "URL")]
    database_url: Option<String>,

    /// Root directory containing bare Git repositories.
    #[arg(long, env = "GITADEL_REPOSITORY_ROOT", value_name = "PATH")]
    repository_root: Option<PathBuf>,

    /// Root directory containing Git LFS objects.
    #[arg(long, env = "GITADEL_LFS_ROOT", value_name = "PATH")]
    lfs_root: Option<PathBuf>,

    /// Address on which the embedded SSH server listens.
    #[arg(long, env = "GITADEL_SSH_BIND", value_name = "ADDRESS")]
    ssh_bind: Option<SocketAddr>,

    /// Persistent OpenSSH private host key used by the embedded SSH server.
    #[arg(long, env = "GITADEL_SSH_HOST_KEY", value_name = "PATH")]
    ssh_host_key: Option<PathBuf>,

    /// Create the initial administrator without starting the HTTP server.
    #[arg(long, env = "GITADEL_BOOTSTRAP_ADMIN", value_name = "USERNAME")]
    bootstrap_admin: Option<String>,

    /// Read the bootstrap administrator password from standard input.
    #[arg(long, requires = "bootstrap_admin")]
    password_stdin: bool,

    #[command(subcommand)]
    command: Option<GitadelCommand>,
}

#[derive(Debug, Subcommand)]
pub enum GitadelCommand {
    #[command(hide = true)]
    ImageRender {
        path: String,
        #[arg(long)]
        thumbnail: bool,
    },
    /// Create or restore an offline integrity-checked backup.
    Backup {
        #[command(subcommand)]
        command: BackupCommand,
    },
    /// Manage Git LFS storage targets and offline migrations.
    Lfs {
        #[command(subcommand)]
        command: LfsCommand,
    },
}

#[derive(Debug, Subcommand)]
pub enum LfsCommand {
    /// Manage configured LFS storage targets.
    Target {
        #[command(subcommand)]
        command: Box<LfsTargetCommand>,
    },
    /// Copy, verify, and select an LFS storage target. Gitadel must be stopped.
    Migrate {
        /// Destination target UUID.
        #[arg(value_name = "TARGET_ID")]
        target_id: uuid::Uuid,
        /// Maximum objects copied before durable progress is updated.
        #[arg(long, default_value_t = 100)]
        batch_size: usize,
    },
}

#[derive(Debug, Subcommand)]
pub enum LfsTargetCommand {
    /// List configured targets and the active selection.
    List,
    /// Add and test a local filesystem target.
    AddFilesystem {
        #[arg(long)]
        name: String,
        #[arg(long, value_name = "PATH")]
        path: PathBuf,
    },
    /// Add and test an S3-compatible target.
    AddS3 {
        #[arg(long)]
        name: String,
        #[arg(long)]
        endpoint: Url,
        #[arg(long)]
        bucket: String,
        #[arg(long, env = "GITADEL_LFS_S3_ACCESS_KEY", hide_env_values = true)]
        access_key: String,
        #[arg(long, env = "GITADEL_LFS_S3_SECRET_KEY", hide_env_values = true)]
        secret_key: String,
        #[arg(long, default_value = "us-east-1")]
        region: String,
        #[arg(long, default_value = "gitadel-lfs")]
        prefix: String,
    },
    /// Exercise write, stat, read, and delete against a target.
    Test {
        #[arg(value_name = "TARGET_ID")]
        target_id: uuid::Uuid,
    },
}

#[derive(Debug, Subcommand)]
pub enum BackupCommand {
    /// Create a compressed backup. Gitadel must not be running against this database.
    Create {
        /// Destination `.tar.zst` archive.
        #[arg(value_name = "OUTPUT")]
        output: PathBuf,
    },
    /// Create a compressed backup and upload it to configured S3 storage.
    CreateS3 {
        /// Object key. Defaults to a unique key under the configured prefix.
        #[arg(long, value_name = "KEY")]
        key: Option<String>,
    },
    /// Restore a backup into empty configured storage paths.
    Restore {
        /// Source `.tar.zst` archive.
        #[arg(value_name = "INPUT")]
        input: PathBuf,
    },
    /// Download an S3 backup and restore it into empty configured storage paths.
    RestoreS3 {
        /// Object key printed by `backup create-s3`.
        #[arg(value_name = "KEY")]
        key: String,
    },
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Settings {
    pub server: ServerSettings,
    pub database: DatabaseSettings,
    pub auth: AuthSettings,
    pub storage: StorageSettings,
    pub ssh: SshSettings,
    #[serde(default)]
    pub backup: BackupSettings,
    #[serde(default)]
    pub actions: ActionsSettings,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ServerSettings {
    pub bind: SocketAddr,
    pub public_url: Url,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tls: Option<TlsSettings>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TlsSettings {
    pub certificate: PathBuf,
    pub private_key: PathBuf,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DatabaseSettings {
    pub url: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct StorageSettings {
    pub repository_root: PathBuf,
    pub lfs_root: PathBuf,
    pub actions_artifact_root: PathBuf,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SshSettings {
    pub bind: SocketAddr,
    pub host_key: PathBuf,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AuthSettings {
    pub session_lifetime_hours: i64,
    pub invitation_lifetime_hours: i64,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct BackupSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub s3: Option<S3Settings>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SystemRunnerBootstrapSettings {
    pub name: String,
    pub labels: Vec<String>,
    pub registration_token_file: PathBuf,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct ActionsSettings {
    pub allowed_action_origins: Vec<String>,
    pub default_actions_origin: String,
    pub runner_loss_seconds: i64,
    pub fetch_timeout_seconds: u64,
    pub max_log_request_bytes: usize,
    pub max_log_row_bytes: usize,
    pub max_log_response_bytes: usize,
    pub max_job_log_bytes: i64,
    pub retention_days: i64,
    pub max_artifact_bytes: i64,
    pub max_artifact_upload_request_bytes: usize,
    pub max_artifact_block_list_bytes: usize,
    pub max_artifact_blocks: usize,
    pub max_artifact_name_bytes: usize,
    pub artifact_grant_lifetime_seconds: i64,
    pub lfs_read: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_runner: Option<SystemRunnerBootstrapSettings>,
}

impl Default for ActionsSettings {
    fn default() -> Self {
        Self {
            allowed_action_origins: Vec::new(),
            default_actions_origin: "https://code.forgejo.org".to_owned(),
            runner_loss_seconds: 300,
            fetch_timeout_seconds: 20,
            max_log_request_bytes: 1_048_576,
            max_log_row_bytes: 65_536,
            max_log_response_bytes: 256 * 1_024,
            max_job_log_bytes: 16 * 1_048_576,
            retention_days: 90,
            max_artifact_bytes: 2 * 1024 * 1024 * 1024,
            max_artifact_upload_request_bytes: 16 * 1024 * 1024,
            max_artifact_block_list_bytes: 1024 * 1024,
            max_artifact_blocks: 50_000,
            max_artifact_name_bytes: 255,
            artifact_grant_lifetime_seconds: 3_600,
            lfs_read: true,
            system_runner: None,
        }
    }
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
pub struct S3Settings {
    pub endpoint: Url,
    pub bucket: String,
    pub access_key: String,
    pub secret_key: String,
    #[serde(default = "default_s3_region")]
    pub region: String,
    #[serde(default = "default_s3_prefix")]
    pub prefix: String,
}

impl fmt::Debug for S3Settings {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("S3Settings")
            .field("endpoint", &self.endpoint)
            .field("bucket", &self.bucket)
            .field("access_key", &self.access_key)
            .field("secret_key", &"<redacted>")
            .field("region", &self.region)
            .field("prefix", &self.prefix)
            .finish()
    }
}

fn default_s3_region() -> String {
    "us-east-1".to_owned()
}

fn default_s3_prefix() -> String {
    "backups".to_owned()
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            server: ServerSettings {
                bind: SocketAddr::from(([127, 0, 0, 1], 3000)),
                public_url: Url::parse("http://localhost:3000")
                    .expect("default public URL is valid"),
                tls: None,
            },
            database: DatabaseSettings {
                url: "sqlite://gitadel.db?mode=rwc".to_owned(),
            },
            storage: StorageSettings {
                repository_root: PathBuf::from("repositories"),
                lfs_root: PathBuf::from("lfs"),
                actions_artifact_root: PathBuf::from("actions-artifacts"),
            },
            ssh: SshSettings {
                bind: SocketAddr::from(([127, 0, 0, 1], 2222)),
                host_key: PathBuf::from("gitadel-ssh-ed25519"),
            },
            auth: AuthSettings {
                session_lifetime_hours: 24 * 30,
                invitation_lifetime_hours: 72,
            },
            backup: BackupSettings::default(),
            actions: ActionsSettings::default(),
        }
    }
}

impl Settings {
    pub fn load(cli: &Cli) -> Result<Self> {
        let defaults = Self::default();
        let configured = config::Config::builder()
            .set_default("server.bind", defaults.server.bind.to_string())?
            .set_default("server.public_url", defaults.server.public_url.to_string())?
            .set_default("database.url", defaults.database.url)?
            .set_default(
                "storage.repository_root",
                defaults
                    .storage
                    .repository_root
                    .to_string_lossy()
                    .into_owned(),
            )?
            .set_default(
                "storage.lfs_root",
                defaults.storage.lfs_root.to_string_lossy().into_owned(),
            )?
            .set_default(
                "storage.actions_artifact_root",
                defaults
                    .storage
                    .actions_artifact_root
                    .to_string_lossy()
                    .into_owned(),
            )?
            .set_default("ssh.bind", defaults.ssh.bind.to_string())?
            .set_default(
                "ssh.host_key",
                defaults.ssh.host_key.to_string_lossy().into_owned(),
            )?
            .set_default(
                "auth.session_lifetime_hours",
                defaults.auth.session_lifetime_hours,
            )?
            .set_default(
                "auth.invitation_lifetime_hours",
                defaults.auth.invitation_lifetime_hours,
            )?
            .add_source(config::File::from(cli.config.as_path()).required(false))
            .add_source(config::Environment::with_prefix("GITADEL").separator("__"))
            .build()
            .context("could not load Gitadel configuration")?;

        let mut settings: Self = configured
            .try_deserialize()
            .context("Gitadel configuration has invalid values")?;

        if let Some(bind) = cli.bind {
            settings.server.bind = bind;
        }
        if let Some(public_url) = &cli.public_url {
            settings.server.public_url.clone_from(public_url);
        }
        if let (Some(certificate), Some(private_key)) = (&cli.tls_certificate, &cli.tls_private_key)
        {
            settings.server.tls = Some(TlsSettings {
                certificate: certificate.clone(),
                private_key: private_key.clone(),
            });
        }
        if let Some(database_url) = &cli.database_url {
            settings.database.url.clone_from(database_url);
        }
        if let Some(repository_root) = &cli.repository_root {
            settings.storage.repository_root.clone_from(repository_root);
        }
        if let Some(lfs_root) = &cli.lfs_root {
            settings.storage.lfs_root.clone_from(lfs_root);
        }
        if let Some(ssh_bind) = cli.ssh_bind {
            settings.ssh.bind = ssh_bind;
        }
        if let Some(ssh_host_key) = &cli.ssh_host_key {
            settings.ssh.host_key.clone_from(ssh_host_key);
        }

        if let Some(s3) = &settings.backup.s3 {
            validate_s3_settings(s3)?;
        }
        validate_server_settings(&settings.server)?;
        validate_actions_settings(&settings.actions)?;

        Ok(settings)
    }
}
fn validate_server_settings(settings: &ServerSettings) -> Result<()> {
    if settings.tls.is_some() {
        ensure!(
            settings.public_url.scheme() == "https",
            "server.public_url must use https when server.tls is configured"
        );
    }
    Ok(())
}

fn validate_actions_settings(settings: &ActionsSettings) -> Result<()> {
    ensure!(
        (30..=3_600).contains(&settings.runner_loss_seconds),
        "Actions runner loss timeout must be between 30 and 3600 seconds"
    );
    ensure!(
        settings.fetch_timeout_seconds <= 60,
        "Actions fetch timeout must not exceed 60 seconds"
    );
    ensure!(
        settings.max_log_request_bytes > 0
            && settings.max_log_row_bytes > 0
            && settings.max_log_response_bytes > 0
            && settings.max_job_log_bytes > 0,
        "Actions log byte limits must be positive"
    );
    ensure!(
        settings.max_log_row_bytes <= settings.max_log_request_bytes
            && settings.max_log_row_bytes <= settings.max_log_response_bytes
            && settings.max_log_response_bytes <= settings.max_job_log_bytes as usize,
        "Actions log row/response limits must fit within their enclosing limits"
    );
    ensure!(
        (1..=3_650).contains(&settings.retention_days),
        "Actions artifact retention must be between 1 and 3650 days"
    );
    ensure!(
        (1..=2 * 1024 * 1024 * 1024).contains(&settings.max_artifact_bytes),
        "Actions artifact size limit must be between 1 byte and 2 GiB"
    );
    ensure!(
        (1..=64 * 1024 * 1024).contains(&settings.max_artifact_upload_request_bytes)
            && settings.max_artifact_upload_request_bytes <= settings.max_artifact_bytes as usize,
        "Actions artifact upload request limit is invalid"
    );
    ensure!(
        (1..=16 * 1024 * 1024).contains(&settings.max_artifact_block_list_bytes)
            && settings.max_artifact_block_list_bytes <= settings.max_artifact_upload_request_bytes,
        "Actions artifact block-list limit is invalid"
    );
    ensure!(
        (1..=100_000).contains(&settings.max_artifact_blocks),
        "Actions artifact block count must be between 1 and 100000"
    );
    ensure!(
        (1..=255).contains(&settings.max_artifact_name_bytes),
        "Actions artifact name limit must be between 1 and 255 bytes"
    );
    ensure!(
        (60..=86_400).contains(&settings.artifact_grant_lifetime_seconds),
        "Actions artifact grant lifetime must be between 60 and 86400 seconds"
    );
    validate_http_origin(&settings.default_actions_origin, "default Actions origin")?;
    for origin in &settings.allowed_action_origins {
        validate_http_origin(origin, "allowed Actions origin")?;
    }
    Ok(())
}

fn validate_http_origin(value: &str, name: &str) -> Result<()> {
    let url = Url::parse(value).with_context(|| format!("{name} is not a valid URL"))?;
    ensure!(
        matches!(url.scheme(), "http" | "https")
            && url.path() == "/"
            && url.query().is_none()
            && url.fragment().is_none()
            && url.username().is_empty()
            && url.password().is_none(),
        "{name} must be an HTTP(S) origin without a path, query, credentials, or fragment"
    );
    Ok(())
}

pub fn validate_s3_settings(settings: &S3Settings) -> Result<()> {
    ensure!(
        matches!(settings.endpoint.scheme(), "http" | "https")
            && settings.endpoint.path() == "/"
            && settings.endpoint.query().is_none()
            && settings.endpoint.fragment().is_none()
            && settings.endpoint.username().is_empty()
            && settings.endpoint.password().is_none(),
        "backup S3 endpoint must be an HTTP(S) origin without a path, query, credentials, or fragment"
    );
    ensure!(
        !settings.bucket.trim().is_empty() && !settings.bucket.contains('/'),
        "backup S3 bucket must be a non-empty bucket name"
    );
    ensure!(
        !settings.access_key.is_empty() && !settings.secret_key.is_empty(),
        "backup S3 access key and secret key must not be empty"
    );
    ensure!(
        !settings.region.trim().is_empty(),
        "backup S3 region must not be empty"
    );
    ensure!(
        settings.prefix == settings.prefix.trim_matches('/'),
        "backup S3 prefix must not start or end with '/'"
    );
    Ok(())
}

impl Cli {
    pub const fn command(&self) -> Option<&GitadelCommand> {
        self.command.as_ref()
    }

    pub fn config_path(&self) -> &PathBuf {
        &self.config
    }

    pub fn bootstrap_admin(&self) -> Option<&str> {
        self.bootstrap_admin.as_deref()
    }

    pub const fn password_stdin(&self) -> bool {
        self.password_stdin
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tls_settings(public_url: &str) -> ServerSettings {
        ServerSettings {
            bind: SocketAddr::from(([127, 0, 0, 1], 3000)),
            public_url: Url::parse(public_url).expect("test public URL is valid"),
            tls: Some(TlsSettings {
                certificate: PathBuf::from("cert.pem"),
                private_key: PathBuf::from("key.pem"),
            }),
        }
    }

    #[test]
    fn local_tls_requires_an_https_public_url() {
        let error = validate_server_settings(&tls_settings("http://localhost:3000"))
            .expect_err("HTTP must be rejected when local TLS is enabled");

        assert!(
            error
                .to_string()
                .contains("server.public_url must use https")
        );
    }

    #[test]
    fn local_tls_accepts_an_https_public_url() {
        validate_server_settings(&tls_settings("https://localhost:3000"))
            .expect("HTTPS is valid with local TLS");
    }
}
