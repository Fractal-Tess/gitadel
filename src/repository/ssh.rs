use std::{
    collections::HashMap, fs::OpenOptions, io::Write as _, os::unix::fs::OpenOptionsExt as _,
    path::Path, sync::Arc, time::Duration,
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
use tokio::net::TcpListener;
use uuid::Uuid;

use super::{
    LfsPermission, Permission, RepositoryState,
    git_service::{self, BlockingReader, BlockingWriter, BridgeCancellation},
    resources::{CreateRepositoryOptions, create_owned_repository, record_push},
    webhooks::{dispatch_push, snapshot_refs},
};
use crate::{actions::ActionsState, config::SshSettings};

struct SshServer {
    state: RepositoryState,
    actions: ActionsState,
}

struct SshHandler {
    state: RepositoryState,
    actions: ActionsState,
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

pub async fn serve(
    settings: SshSettings,
    state: RepositoryState,
    actions: ActionsState,
) -> Result<()> {
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
    let mut server = SshServer { state, actions };
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
            actions: self.actions.clone(),
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
        let repository = match repository_for_git_service(
            &self.state,
            actor_user_id,
            service,
            &namespace,
            &name,
        )
        .await
        {
            Ok(repository) => repository,
            Err(error) => {
                tracing::debug!(
                    %error,
                    %namespace,
                    %name,
                    "could not resolve repository for Git SSH service"
                );
                return reject(channel_id, session, "Repository not found.\n");
            }
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
        let format = match git_service::object_format(&repository.object_format) {
            Ok(format) => format,
            Err(error) => {
                tracing::error!(%error, "unsupported repository object format");
                return reject(channel_id, session, "Could not start Git service.\n");
            }
        };
        let protocol_v2 = git_service::protocol_v2(channel_state.git_protocol.as_deref());
        let receive = matches!(service, GitService::ReceivePack);
        let refs_before = if receive {
            match snapshot_refs(&path).await {
                Ok(refs) => Some(refs),
                Err(error) => {
                    tracing::warn!(%error, %namespace, %name, "could not snapshot refs before push");
                    None
                }
            }
        } else {
            None
        };
        session.channel_success(channel_id)?;
        let channel_id_for_status = channel_id;
        let session_handle = session.handle();
        // into_stream closes on drop, before the supervisor can send exit status.
        let (mut channel_reader, channel_writer) = channel_state.channel.split();
        let cancellation = BridgeCancellation::default();
        let worker_cancellation = cancellation.clone();
        let handle = tokio::runtime::Handle::current();
        let worker_path = path.clone();
        let mut worker = tokio::task::spawn_blocking(move || {
            let mut reader = BlockingReader::new(
                channel_reader.make_reader(),
                handle.clone(),
                worker_cancellation.clone(),
            );
            let mut writer =
                BlockingWriter::new(channel_writer.make_writer(), handle, worker_cancellation);
            if receive {
                git_service::write_advertisement(&worker_path, format, true, false, &mut writer)?;
                git_service::serve_receive_pack(&worker_path, format, &mut reader, &mut writer)
                    .map(|outcome| (true, outcome.landed, outcome.response_error))
            } else if protocol_v2 {
                git_service::serve_upload_pack(
                    &worker_path,
                    format,
                    true,
                    false,
                    &mut reader,
                    &mut writer,
                )
                .map(|()| (true, false, None))
            } else {
                git_service::write_advertisement(&worker_path, format, false, false, &mut writer)?;
                git_service::serve_upload_pack(
                    &worker_path,
                    format,
                    false,
                    false,
                    &mut reader,
                    &mut writer,
                )
                .map(|()| (true, false, None))
            }
        });
        let repository_name = format!("{namespace}/{name}");
        let scheduler = self.state.clone();
        let task_state = self.state.clone();
        let actions = self.actions.clone();
        let audit_repository = repository.clone();
        let maintenance_path = path.clone();
        scheduler.spawn_task(async move {
            let result = tokio::time::timeout(Duration::from_secs(30 * 60), &mut worker).await;
            let result = match result {
                Err(_) => {
                    cancellation.cancel();
                    tracing::warn!(repository = %repository_name, "native Git SSH service exceeded time limit");
                    worker.await
                }
                Ok(result) => result,
            };
            let (service_ok, landed, response_error) = match result {
                Ok(Ok((service_ok, landed, response_error))) => {
                    (service_ok, landed, response_error)
                }
                Ok(Err(error)) => {
                    tracing::warn!(%error, repository = %repository_name, "native Git SSH service failed");
                    (false, false, None)
                }
                Err(error) => {
                    tracing::warn!(%error, repository = %repository_name, "native Git SSH task failed");
                    (false, false, None)
                }
            };
            if let Some(error) = response_error {
                tracing::debug!(%error, repository = %repository_name, "Git SSH response delivery failed");
            }
            if receive && landed {
                if let Err(error) =
                    record_push(&task_state, audit_repository.id, actor_user_id, repository_name.clone()).await
                {
                    tracing::warn!(%error, repository = %repository_name, "could not record repository push");
                }
                if let Some(refs_before) = refs_before
                    && let Err(error) = dispatch_push(
                        &task_state,
                        &actions,
                        &audit_repository,
                        actor_user_id,
                        refs_before,
                    )
                    .await
                {
                    tracing::warn!(%error, repository = %repository_name, "could not queue repository webhooks");
                }
                super::maintenance::run(&maintenance_path, &repository_name).await;
            }
            let exit_status = if service_ok { 0 } else { 1 };
            let _ = session_handle
                .exit_status_request(channel_id_for_status, exit_status)
                .await;
            let _ = session_handle.eof(channel_id_for_status).await;
            let _ = session_handle.close(channel_id_for_status).await;
        });
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

async fn repository_for_git_service(
    state: &RepositoryState,
    actor_user_id: Uuid,
    service: GitService,
    namespace: &str,
    name: &str,
) -> Result<crate::entity::repository::Model, crate::identity::ApiError> {
    match state.find(namespace, name).await {
        Ok(repository) => Ok(repository),
        Err(error) if matches!(service, GitService::UploadPack) => Err(error),
        Err(_) => {
            let options = CreateRepositoryOptions {
                namespace: namespace.to_owned(),
                name: name.to_owned(),
                description: None,
                visibility: None,
                object_format: None,
                mirror: None,
            };
            match create_owned_repository(state, actor_user_id, options).await {
                Ok(repository) => Ok(repository),
                Err(create_error) => state.find(namespace, name).await.map_err(|_| create_error),
            }
        }
    }
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
