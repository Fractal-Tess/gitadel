use std::{net::SocketAddr, path::PathBuf};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use serde::Deserialize;
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
    /// Manage repositories through the Gitadel HTTP API.
    Repo {
        #[command(subcommand)]
        command: RepositoryCommand,
    },
}

#[derive(Debug, Subcommand)]
pub enum RepositoryCommand {
    /// Create an empty repository.
    Create {
        /// Repository path in namespace/name form.
        #[arg(value_name = "NAMESPACE/NAME")]
        repository: String,

        /// Gitadel HTTP origin.
        #[arg(
            long,
            env = "GITADEL_SERVER",
            default_value = "http://127.0.0.1:3000",
            value_name = "URL"
        )]
        server: Url,

        /// API token with write scope.
        #[arg(long, env = "GITADEL_TOKEN", hide_env_values = true)]
        token: String,

        /// Keep the repository visible only to authorized users.
        #[arg(long, conflicts_with = "public")]
        private: bool,

        /// Make the repository publicly readable. This is the default.
        #[arg(long, conflicts_with = "private")]
        public: bool,

        /// Short repository description.
        #[arg(long, value_name = "TEXT")]
        description: Option<String>,

        /// Git object format.
        #[arg(long, default_value = "sha1", value_parser = ["sha1", "sha256"])]
        object_format: String,
    },
}

#[derive(Debug, Clone, Deserialize)]
pub struct Settings {
    pub server: ServerSettings,
    pub database: DatabaseSettings,
    pub auth: AuthSettings,
    pub storage: StorageSettings,
    pub ssh: SshSettings,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServerSettings {
    pub bind: SocketAddr,
    pub public_url: Url,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DatabaseSettings {
    pub url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StorageSettings {
    pub repository_root: PathBuf,
    pub lfs_root: PathBuf,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SshSettings {
    pub bind: SocketAddr,
    pub host_key: PathBuf,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AuthSettings {
    pub session_lifetime_hours: i64,
    pub invitation_lifetime_hours: i64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            server: ServerSettings {
                bind: SocketAddr::from(([127, 0, 0, 1], 3000)),
                public_url: Url::parse("http://localhost:3000")
                    .expect("default public URL is valid"),
            },
            database: DatabaseSettings {
                url: "sqlite://gitadel.db?mode=rwc".to_owned(),
            },
            storage: StorageSettings {
                repository_root: PathBuf::from("repositories"),
                lfs_root: PathBuf::from("lfs"),
            },
            ssh: SshSettings {
                bind: SocketAddr::from(([127, 0, 0, 1], 2222)),
                host_key: PathBuf::from("gitadel-ssh-ed25519"),
            },
            auth: AuthSettings {
                session_lifetime_hours: 24 * 30,
                invitation_lifetime_hours: 72,
            },
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

        Ok(settings)
    }
}

impl Cli {
    pub const fn command(&self) -> Option<&GitadelCommand> {
        self.command.as_ref()
    }

    pub fn bootstrap_admin(&self) -> Option<&str> {
        self.bootstrap_admin.as_deref()
    }

    pub const fn password_stdin(&self) -> bool {
        self.password_stdin
    }
}
