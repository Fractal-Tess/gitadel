use std::{
    collections::HashMap, fs::OpenOptions, io::Write as _, os::unix::fs::OpenOptionsExt as _,
    path::Path, process::Stdio, sync::Arc, time::Duration,
};

use anyhow::{Context, Result, anyhow};
use russh::{
    Channel, ChannelId,
    keys::{
        PrivateKey, load_secret_key,
        ssh_key::{Algorithm, HashAlg, LineEnding, PublicKey},
    },
    server::{Auth, ChannelOpenHandle, Msg, Server as _, Session},
};
use tokio::{
    io::AsyncWriteExt as _,
    net::TcpListener,
    process::{Child, Command},
};
use uuid::Uuid;

use super::{LfsPermission, Permission, RepositoryState};
use crate::config::SshSettings;

#[derive(Clone)]
struct SshServer {
    state: RepositoryState,
}

struct SshHandler {
    state: RepositoryState,
    actor_user_id: Option<Uuid>,
    channels: HashMap<ChannelId, SshChannel>,
}

struct SshChannel {
    channel: Channel<Msg>,
    git_protocol: Option<String>,
}

#[derive(Clone, Copy)]
enum GitService {
    UploadPack,
    ReceivePack,
}

enum SshCommand {
    Git {
        service: GitService,
        namespace: String,
        name: String,
    },
    Lfs {
        permission: LfsPermission,
        namespace: String,
        name: String,
    },
}

pub async fn serve(settings: SshSettings, state: RepositoryState) -> Result<()> {
    let host_key = load_or_create_host_key(&settings.host_key)?;
    let config = Arc::new(russh::server::Config {
        inactivity_timeout: Some(Duration::from_secs(60 * 60)),
        auth_rejection_time: Duration::from_secs(1),
        auth_rejection_time_initial: Some(Duration::ZERO),
        keys: vec![host_key],
        ..Default::default()
    });
    let listener = TcpListener::bind(settings.bind)
        .await
        .with_context(|| format!("could not bind SSH listener to {}", settings.bind))?;
    tracing::info!(address = %settings.bind, "Gitadel SSH server listening");
    let mut server = SshServer { state };
    server
        .run_on_socket(config, &listener)
        .await
        .context("SSH server stopped unexpectedly")
}

impl russh::server::Server for SshServer {
    type Handler = SshHandler;

    fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> Self::Handler {
        SshHandler {
            state: self.state.clone(),
            actor_user_id: None,
            channels: HashMap::new(),
        }
    }

    fn handle_session_error(&mut self, error: <Self::Handler as russh::server::Handler>::Error) {
        tracing::debug!(%error, "SSH session ended with an error");
    }
}

impl russh::server::Handler for SshHandler {
    type Error = anyhow::Error;

    async fn auth_publickey(
        &mut self,
        user: &str,
        public_key: &PublicKey,
    ) -> Result<Auth, Self::Error> {
        if user != "git" {
            return Ok(Auth::reject());
        }
        let fingerprint = public_key.fingerprint(HashAlg::Sha256).to_string();
        match self
            .state
            .identity()
            .authenticate_ssh_key(&fingerprint)
            .await
        {
            Ok(Some(account)) => {
                self.actor_user_id = Some(account.id);
                Ok(Auth::Accept)
            }
            Ok(None) => Ok(Auth::reject()),
            Err(error) => {
                tracing::error!(%error, "SSH public key lookup failed");
                Ok(Auth::reject())
            }
        }
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        reply: ChannelOpenHandle,
        _: &mut Session,
    ) -> Result<(), Self::Error> {
        let id = channel.id();
        reply.accept().await;
        self.channels.insert(
            id,
            SshChannel {
                channel,
                git_protocol: None,
            },
        );
        Ok(())
    }

    async fn env_request(
        &mut self,
        channel_id: ChannelId,
        variable_name: &str,
        variable_value: &str,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let accepted = variable_name == "GIT_PROTOCOL"
            && variable_value == "version=2"
            && self.channels.get_mut(&channel_id).is_some_and(|channel| {
                channel.git_protocol = Some(variable_value.to_owned());
                true
            });
        if accepted {
            session.channel_success(channel_id)?;
        } else {
            session.channel_failure(channel_id)?;
        }
        Ok(())
    }

    async fn exec_request(
        &mut self,
        channel_id: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let Some(actor_user_id) = self.actor_user_id else {
            return reject(channel_id, session, "Authentication required.\n");
        };
        let Some(channel_state) = self.channels.remove(&channel_id) else {
            return reject(channel_id, session, "Invalid SSH channel.\n");
        };
        let Ok(command) = std::str::from_utf8(data) else {
            return reject(channel_id, session, "Invalid command.\n");
        };
        let Some(command) = parse_command(command) else {
            return reject(
                channel_id,
                session,
                "Only Git transport and LFS authentication commands are supported.\n",
            );
        };
        let (service, namespace, name) = match command {
            SshCommand::Git {
                service,
                namespace,
                name,
            } => (service, namespace, name),
            SshCommand::Lfs {
                permission,
                namespace,
                name,
            } => {
                let repository = match self.state.find(&namespace, &name).await {
                    Ok(repository) => repository,
                    Err(_) => return reject(channel_id, session, "Repository not found.\n"),
                };
                let repository_permission = match permission {
                    LfsPermission::Read => Permission::Read,
                    LfsPermission::Write => Permission::Write,
                };
                if self
                    .state
                    .authorize(&repository, Some(actor_user_id), repository_permission)
                    .await
                    .is_err()
                {
                    return reject(channel_id, session, "Repository not found.\n");
                }
                let token = self
                    .state
                    .issue_lfs_token(repository.id, actor_user_id, permission)
                    .await;
                let response = serde_json::json!({
                    "href": self.state.lfs_endpoint(&repository),
                    "header": {
                        "Authorization": format!("Bearer {token}"),
                    },
                    "expires_in": 15 * 60,
                });
                let mut output = serde_json::to_vec(&response)?;
                output.push(b'\n');
                session.channel_success(channel_id)?;
                session.data(channel_id, output)?;
                session.exit_status_request(channel_id, 0)?;
                session.eof(channel_id)?;
                session.close(channel_id)?;
                return Ok(());
            }
        };
        let repository = match self.state.find(&namespace, &name).await {
            Ok(repository) => repository,
            Err(_) => return reject(channel_id, session, "Repository not found.\n"),
        };
        let permission = match service {
            GitService::UploadPack => Permission::Read,
            GitService::ReceivePack => Permission::Write,
        };
        if self
            .state
            .authorize(&repository, Some(actor_user_id), permission)
            .await
            .is_err()
        {
            return reject(channel_id, session, "Repository not found.\n");
        }
        if matches!(service, GitService::ReceivePack) && repository.archived_at.is_some() {
            return reject(
                channel_id,
                session,
                "Archived repositories are read-only.\n",
            );
        }

        let path = self.state.repository_path(&repository);
        let program = match service {
            GitService::UploadPack => "git-upload-pack",
            GitService::ReceivePack => "git-receive-pack",
        };
        let mut command = Command::new(program);
        command
            .arg(&path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(git_protocol) = channel_state.git_protocol {
            command.env("GIT_PROTOCOL", git_protocol);
        }
        let child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                tracing::error!(%error, %program, "could not start Git SSH service");
                return reject(channel_id, session, "Could not start Git service.\n");
            }
        };

        session.channel_success(channel_id)?;
        let maintenance_path = matches!(service, GitService::ReceivePack).then_some(path);
        bridge_process(
            child,
            channel_state.channel,
            format!("{namespace}/{name}"),
            maintenance_path,
            matches!(service, GitService::ReceivePack).then(|| (self.state.clone(), actor_user_id)),
        );
        Ok(())
    }

    async fn channel_close(
        &mut self,
        channel: ChannelId,
        _: &mut Session,
    ) -> Result<(), Self::Error> {
        self.channels.remove(&channel);
        Ok(())
    }
}

fn bridge_process(
    mut child: Child,
    mut channel: Channel<Msg>,
    repository: String,
    maintenance_path: Option<std::path::PathBuf>,
    push_audit: Option<(RepositoryState, Uuid)>,
) {
    let Some(mut stdin) = child.stdin.take() else {
        return;
    };
    let Some(mut stdout) = child.stdout.take() else {
        return;
    };
    let Some(mut stderr) = child.stderr.take() else {
        return;
    };

    tokio::spawn(async move {
        let mut channel_writer = channel.make_writer();
        let mut stderr_writer = channel.make_writer_ext(Some(1));
        let mut channel_reader = channel.make_reader();
        let output = tokio::spawn(async move {
            let _ = tokio::io::copy(&mut stdout, &mut channel_writer).await;
        });
        let errors = tokio::spawn(async move {
            let _ = tokio::io::copy(&mut stderr, &mut stderr_writer).await;
        });
        let mut input = Box::pin(async {
            let _ = tokio::io::copy(&mut channel_reader, &mut stdin).await;
            let _ = stdin.shutdown().await;
        });
        let mut wait = Box::pin(tokio::time::timeout(
            Duration::from_secs(30 * 60),
            child.wait(),
        ));
        let first_result = tokio::select! {
            result = &mut wait => {
                tracing::debug!(%repository, "Git SSH process exited");
                Some(result)
            },
            () = &mut input => {
                tracing::debug!(%repository, "Git SSH client input closed");
                None
            },
        };
        drop(input);
        drop(stdin);
        drop(channel_reader);
        let result = match first_result {
            Some(result) => result,
            None => (&mut wait).await,
        };
        drop(wait);
        if result.is_err() {
            let _ = child.kill().await;
        }
        let _ = output.await;
        tracing::debug!(%repository, "Git SSH process output closed");
        let successful = matches!(&result, Ok(Ok(status)) if status.success());
        let _ = errors.await;
        tracing::debug!(%repository, "Git SSH process error output closed");
        if successful
            && let Some((state, actor_user_id)) = push_audit
            && let Err(error) = state
                .identity()
                .audit(
                    Some(actor_user_id),
                    "repository.push",
                    Some(repository.clone()),
                )
                .await
        {
            tracing::warn!(%error, %repository, "could not record repository push");
        }
        let exit_status = match result {
            Ok(Ok(status)) => status
                .code()
                .and_then(|code| u32::try_from(code).ok())
                .unwrap_or(1),
            Ok(Err(error)) => {
                tracing::error!(%error, %repository, "Git SSH process wait failed");
                1
            }
            Err(_) => {
                tracing::warn!(%repository, "Git SSH process exceeded time limit");
                1
            }
        };
        if successful && let Some(path) = maintenance_path {
            match Command::new("git")
                .arg("--git-dir")
                .arg(path)
                .args(["gc", "--auto", "--quiet"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .output()
                .await
            {
                Ok(output) if output.status.success() => {}
                Ok(output) => tracing::warn!(
                    %repository,
                    stderr = %String::from_utf8_lossy(&output.stderr),
                    "automatic Git maintenance failed"
                ),
                Err(error) => tracing::warn!(
                    %error,
                    %repository,
                    "could not start automatic Git maintenance"
                ),
            }
        }
        let _ = channel.exit_status(exit_status).await;
        let _ = channel.eof().await;
        let _ = channel.close().await;
    });
}

fn parse_command(command: &str) -> Option<SshCommand> {
    let arguments = shlex::split(command)?;
    match arguments.as_slice() {
        [program, repository]
            if matches!(program.as_str(), "git-upload-pack" | "git-receive-pack") =>
        {
            let (namespace, name) = parse_repository_path(repository)?;
            Some(SshCommand::Git {
                service: if program == "git-upload-pack" {
                    GitService::UploadPack
                } else {
                    GitService::ReceivePack
                },
                namespace,
                name,
            })
        }
        [program, repository, operation] if program == "git-lfs-authenticate" => {
            let (namespace, name) = parse_repository_path(repository)?;
            let permission = match operation.as_str() {
                "download" => LfsPermission::Read,
                "upload" => LfsPermission::Write,
                _ => return None,
            };
            Some(SshCommand::Lfs {
                permission,
                namespace,
                name,
            })
        }
        _ => None,
    }
}

fn parse_repository_path(repository: &str) -> Option<(String, String)> {
    let repository = repository.trim_start_matches('/');
    let (namespace, name) = repository.split_once('/')?;
    if namespace.is_empty() || name.contains('/') {
        return None;
    }
    let name = name.strip_suffix(".git")?;
    Some((namespace.to_owned(), name.to_owned()))
}

fn reject(channel: ChannelId, session: &mut Session, message: &str) -> Result<(), anyhow::Error> {
    session.channel_failure(channel)?;
    session.extended_data(channel, 1, message.as_bytes().to_vec())?;
    session.exit_status_request(channel, 1)?;
    session.eof(channel)?;
    session.close(channel)?;
    Ok(())
}

fn load_or_create_host_key(path: &Path) -> Result<PrivateKey> {
    if path.exists() {
        return load_secret_key(path, None)
            .with_context(|| format!("could not load SSH host key {}", path.display()));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("could not create {}", parent.display()))?;
    }
    let key = PrivateKey::random(&mut rand_10::rng(), Algorithm::Ed25519)
        .map_err(|error| anyhow!("could not generate SSH host key: {error}"))?;
    let encoded = key
        .to_openssh(LineEnding::LF)
        .context("could not encode SSH host key")?;
    match OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
    {
        Ok(mut file) => {
            file.write_all(encoded.as_bytes())
                .with_context(|| format!("could not write SSH host key {}", path.display()))?;
            file.sync_all()
                .with_context(|| format!("could not sync SSH host key {}", path.display()))?;
            Ok(key)
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            load_secret_key(path, None)
                .with_context(|| format!("could not load SSH host key {}", path.display()))
        }
        Err(error) => {
            Err(error).with_context(|| format!("could not create SSH host key {}", path.display()))
        }
    }
}
