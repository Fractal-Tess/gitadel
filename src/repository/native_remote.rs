use std::{
    collections::HashSet,
    future::Future,
    io::{self, Read, Write},
    path::Path,
    sync::Arc,
    time::Duration,
};

use bytes::{Buf, Bytes};
use futures_util::StreamExt as _;
use hmac_13::{Hmac, KeyInit as _, Mac as _};
use reqwest::header::{HeaderName, HeaderValue};
use russh::{
    Channel, ChannelMsg, Disconnect,
    client::{self, Handler},
    keys::{
        PrivateKeyWithHashAlg,
        ssh_key::{
            Algorithm, PrivateKey, PublicKey,
            known_hosts::{HostPatterns, KnownHosts, Marker},
        },
    },
};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_util::{io::StreamReader, sync::CancellationToken};
use url::Url;
use uuid::Uuid;

use crate::{
    blob_store::{BlobDigest, lfs_object_key},
    identity::ApiError,
};

use super::RepositoryState;
const LFS_MEDIA_TYPE: &str = "application/vnd.git-lfs+json";
const REMOTE_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const MAX_SSH_DIAGNOSTIC: usize = 8192;

struct RemoteOperation {
    flag: sley_core::AtomicCancel,
    cancelled: CancellationToken,
    deadline: tokio::time::Instant,
}

impl RemoteOperation {
    fn new() -> Self {
        Self {
            flag: sley_core::AtomicCancel::new(),
            cancelled: CancellationToken::new(),
            deadline: tokio::time::Instant::now() + REMOTE_TIMEOUT,
        }
    }

    fn cancel(&self) {
        self.flag.cancel();
        self.cancelled.cancel();
    }

    async fn wait<T>(&self, future: impl Future<Output = T>) -> io::Result<T> {
        tokio::select! {
            biased;
            _ = self.cancelled.cancelled() => Err(sley_core::cancelled_io_error()),
            result = tokio::time::timeout_at(self.deadline, future) => {
                result.map_err(|_| {
                    self.cancel();
                    io::Error::new(io::ErrorKind::TimedOut, "remote operation timed out")
                })
            }
        }
    }
}

struct CancelRemoteOnDrop(Arc<RemoteOperation>);

impl Drop for CancelRemoteOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

/// Native mirror/import result. The object format is learned from the remote
/// advertisement before the destination repository is initialized.
pub(crate) struct NativeInitialization {
    pub(crate) object_format: String,
    pub(crate) default_branch: Option<String>,
}

#[derive(Clone, Default)]
pub(crate) struct NativeAuthentication {
    pub(crate) username: Option<String>,
    pub(crate) secret: Option<String>,
    pub(crate) ssh_private_key: Option<String>,
    pub(crate) ssh_known_hosts: Option<String>,
}

#[derive(Clone)]
struct ReqwestHttpClient {
    client: reqwest::Client,
    runtime: tokio::runtime::Handle,
    operation: Arc<RemoteOperation>,
}

struct ResponseBody {
    receiver: mpsc::Receiver<Result<Bytes, String>>,
    runtime: tokio::runtime::Handle,
    operation: Arc<RemoteOperation>,
    current: Option<Bytes>,
    offset: usize,
}

impl ResponseBody {
    fn spawn(
        runtime: &tokio::runtime::Handle,
        operation: Arc<RemoteOperation>,
        stream: impl futures_util::Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static,
    ) -> Self {
        let (sender, receiver) = mpsc::channel(2);
        let producer_operation = Arc::clone(&operation);
        runtime.spawn(async move {
            tokio::pin!(stream);
            loop {
                let chunk = match producer_operation.wait(stream.next()).await {
                    Ok(Some(chunk)) => chunk.map_err(|error| error.to_string()),
                    Ok(None) => break,
                    Err(error) => {
                        let _ = sender.send(Err(error.to_string())).await;
                        break;
                    }
                };
                if sender.send(chunk).await.is_err() {
                    break;
                }
            }
        });
        Self {
            receiver,
            runtime: runtime.clone(),
            operation,
            current: None,
            offset: 0,
        }
    }
}

impl Read for ResponseBody {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }
        loop {
            if let Some(chunk) = self.current.as_ref()
                && self.offset < chunk.len()
            {
                let count = (chunk.len() - self.offset).min(output.len());
                output[..count].copy_from_slice(&chunk[self.offset..self.offset + count]);
                self.offset += count;
                if self.offset == chunk.len() {
                    self.current = None;
                    self.offset = 0;
                }
                return Ok(count);
            }
            match self
                .runtime
                .block_on(self.operation.wait(self.receiver.recv()))?
            {
                Some(Ok(chunk)) => self.current = Some(chunk),
                Some(Err(error)) => return Err(io::Error::other(error)),
                None => return Ok(0),
            }
        }
    }
}

impl ReqwestHttpClient {
    fn new(client: reqwest::Client, operation: Arc<RemoteOperation>) -> Self {
        Self {
            client,
            runtime: tokio::runtime::Handle::current(),
            operation,
        }
    }

    fn response(
        &self,
        response: reqwest::Response,
    ) -> sley_core::Result<sley_transport::HttpResponse> {
        let status = response.status().as_u16();
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let content_length = response.content_length();
        let content_range = response
            .headers()
            .get(reqwest::header::CONTENT_RANGE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let body = ResponseBody::spawn(
            &self.runtime,
            Arc::clone(&self.operation),
            response.bytes_stream(),
        );
        Ok(sley_transport::HttpResponse {
            status,
            content_type,
            content_length,
            content_range,
            body: Box::new(body),
        })
    }

    fn send(
        &self,
        request: reqwest::RequestBuilder,
    ) -> sley_core::Result<sley_transport::HttpResponse> {
        let response = self
            .runtime
            .block_on(self.operation.wait(request.send()))?
            .map_err(|error| sley_core::GitError::Command(error.to_string()))?;
        self.response(response)
    }
}

impl sley_remote::HttpClient for ReqwestHttpClient {
    fn get(
        &self,
        url: &str,
        headers: &[(&str, &str)],
    ) -> sley_core::Result<sley_transport::HttpResponse> {
        let mut request = self.client.get(url);
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        self.send(request)
    }

    fn post(
        &self,
        url: &str,
        content_type: &str,
        headers: &[(&str, &str)],
        body: &[u8],
    ) -> sley_core::Result<sley_transport::HttpResponse> {
        let mut request = self.client.post(url).header("content-type", content_type);
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        self.send(request.body(body.to_vec()))
    }
}

struct StaticCredentials {
    username: Option<String>,
    secret: Option<String>,
}

impl sley_remote::CredentialProvider for StaticCredentials {
    fn fill(
        &mut self,
        mut request: sley_transport::GitCredential,
    ) -> sley_core::Result<Option<sley_transport::GitCredential>> {
        let (Some(username), Some(secret)) = (&self.username, &self.secret) else {
            return Ok(None);
        };
        request.username = Some(username.clone());
        request.password = Some(secret.clone());

        Ok(Some(request))
    }
}
#[derive(Clone, Default)]
struct TrustedHostKeys {
    allowed: Vec<PublicKey>,
    revoked: Vec<PublicKey>,
}

struct NativeSshHandler {
    host_keys: TrustedHostKeys,
}

impl Handler for NativeSshHandler {
    type Error = russh::Error;

    async fn check_server_key(&mut self, key: &PublicKey) -> Result<bool, Self::Error> {
        let key = key.key_data();
        Ok(!self
            .host_keys
            .revoked
            .iter()
            .any(|trusted| trusted.key_data() == key)
            && self
                .host_keys
                .allowed
                .iter()
                .any(|trusted| trusted.key_data() == key))
    }
}

struct NativeSshStream {
    runtime: tokio::runtime::Handle,
    operation: Arc<RemoteOperation>,
    session: Option<client::Handle<NativeSshHandler>>,
    channel: Option<Channel<client::Msg>>,
    pending: Bytes,
    stderr: Vec<u8>,
    exit_status: Option<u32>,
    read_eof: bool,
    closed: bool,
}

impl NativeSshStream {
    fn receive(&mut self) -> io::Result<Option<ChannelMsg>> {
        let Some(channel) = self.channel.as_mut() else {
            return Ok(None);
        };
        self.runtime.block_on(self.operation.wait(channel.wait()))
    }

    fn record_stderr(&mut self, data: &[u8]) {
        let count = data.len().min(MAX_SSH_DIAGNOSTIC - self.stderr.len());
        self.stderr.extend_from_slice(&data[..count]);
    }

    fn close(&mut self) {
        let channel = self.channel.take();
        let session = self.session.take();
        if channel.is_none() && session.is_none() {
            return;
        }
        self.runtime.spawn(async move {
            let _ = tokio::time::timeout(Duration::from_secs(2), async move {
                if let Some(channel) = channel {
                    let _ = channel.close().await;
                }
                if let Some(session) = session {
                    let _ = session
                        .disconnect(Disconnect::ByApplication, "", "English")
                        .await;
                }
            })
            .await;
        });
    }
}

impl Drop for NativeSshStream {
    fn drop(&mut self) {
        self.close();
    }
}

impl Read for NativeSshStream {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() || self.read_eof {
            return Ok(0);
        }
        while self.pending.is_empty() {
            match self.receive()? {
                Some(ChannelMsg::Data { data }) => self.pending = data,
                Some(ChannelMsg::ExtendedData { data, .. }) => self.record_stderr(&data),
                Some(ChannelMsg::ExitStatus { exit_status }) => {
                    self.exit_status = Some(exit_status);
                }
                Some(ChannelMsg::Eof) => {
                    self.read_eof = true;
                    return Ok(0);
                }
                Some(ChannelMsg::Close) | None => {
                    self.closed = true;
                    self.read_eof = true;
                    return Ok(0);
                }
                _ => {}
            }
        }
        let count = output.len().min(self.pending.len());
        output[..count].copy_from_slice(&self.pending[..count]);
        self.pending.advance(count);
        Ok(count)
    }
}

impl Write for NativeSshStream {
    fn write(&mut self, input: &[u8]) -> io::Result<usize> {
        if input.is_empty() {
            return Ok(0);
        }
        let channel = self
            .channel
            .as_ref()
            .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "SSH channel is closed"))?;
        self.runtime
            .block_on(
                self.operation
                    .wait(channel.data_bytes(Bytes::copy_from_slice(input))),
            )?
            .map_err(io::Error::other)?;
        Ok(input.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl sley_remote::SshStream for NativeSshStream {
    fn finish(&mut self) -> sley_core::Result<()> {
        if self.exit_status.is_none()
            && !self.closed
            && let Some(channel) = self.channel.as_ref()
        {
            self.runtime
                .block_on(self.operation.wait(channel.eof()))?
                .map_err(|error| sley_core::GitError::Command(error.to_string()))?;
        }
        while self.exit_status.is_none() && !self.closed {
            match self.receive()? {
                Some(ChannelMsg::ExitStatus { exit_status }) => {
                    self.exit_status = Some(exit_status);
                }
                Some(ChannelMsg::ExtendedData { data, .. }) => self.record_stderr(&data),
                Some(ChannelMsg::Close) | None => self.closed = true,
                _ => {}
            }
        }
        let status = self.exit_status;
        self.close();
        match status {
            Some(0) => Ok(()),
            Some(status) => Err(sley_core::GitError::Command(format!(
                "SSH service exited with status {status}: {}",
                String::from_utf8_lossy(&self.stderr).trim()
            ))),
            None => Err(sley_core::GitError::Command(
                "SSH service closed without an exit status".into(),
            )),
        }
    }
}

struct NativeSshTransport {
    private_key: String,
    host_keys: TrustedHostKeys,
    operation: Arc<RemoteOperation>,
}

impl NativeSshTransport {
    fn new(
        private_key: String,
        host_keys: TrustedHostKeys,
        operation: Arc<RemoteOperation>,
    ) -> Self {
        Self {
            private_key,
            host_keys,
            operation,
        }
    }

    fn open_command(
        &self,
        remote: &sley_transport::RemoteUrl,
        command: String,
        protocol: Option<sley_protocol::ProtocolVersion>,
    ) -> sley_core::Result<NativeSshStream> {
        let host = remote
            .host
            .as_deref()
            .ok_or_else(|| sley_core::GitError::InvalidFormat("SSH remote has no host".into()))?;
        let user = remote.user.as_deref().unwrap_or("git");
        let port = remote.port.unwrap_or(22);
        let key = PrivateKey::from_openssh(&self.private_key)
            .map_err(|error| sley_core::GitError::Command(error.to_string()))?;
        let runtime = tokio::runtime::Handle::try_current()
            .map_err(|error| sley_core::GitError::Command(error.to_string()))?;
        runtime.block_on(self.operation.wait(async {
            let session = client::connect(
                Arc::new(client::Config {
                    inactivity_timeout: Some(REMOTE_TIMEOUT),
                    ..Default::default()
                }),
                (host, port),
                NativeSshHandler {
                    host_keys: self.host_keys.clone(),
                },
            )
            .await
            .map_err(|error| sley_core::GitError::Command(error.to_string()))?;
            let mut stream = NativeSshStream {
                runtime: runtime.clone(),
                operation: Arc::clone(&self.operation),
                session: Some(session),
                channel: None,
                pending: Bytes::new(),
                stderr: Vec::new(),
                exit_status: None,
                read_eof: false,
                closed: false,
            };
            let session = stream.session.as_mut().expect("new SSH session");
            let hash = if matches!(key.algorithm(), Algorithm::Rsa { .. }) {
                session
                    .best_supported_rsa_hash()
                    .await
                    .map_err(|error| sley_core::GitError::Command(error.to_string()))?
                    .flatten()
            } else {
                None
            };
            let authenticated = session
                .authenticate_publickey(user, PrivateKeyWithHashAlg::new(Arc::new(key), hash))
                .await
                .map_err(|error| sley_core::GitError::Command(error.to_string()))?;
            if !authenticated.success() {
                return Err(sley_core::GitError::Command(
                    "SSH public-key authentication failed".into(),
                ));
            }
            stream.channel = Some(
                session
                    .channel_open_session()
                    .await
                    .map_err(|error| sley_core::GitError::Command(error.to_string()))?,
            );
            let channel = stream.channel.as_ref().expect("new SSH channel");
            if let Some(protocol) = protocol {
                let value = match protocol {
                    sley_protocol::ProtocolVersion::V0 => "version=0",
                    sley_protocol::ProtocolVersion::V1 => "version=1",
                    sley_protocol::ProtocolVersion::V2 => "version=2",
                };
                channel
                    .set_env(false, "GIT_PROTOCOL", value)
                    .await
                    .map_err(|error| sley_core::GitError::Command(error.to_string()))?;
            }
            channel
                .exec(true, command)
                .await
                .map_err(|error| sley_core::GitError::Command(error.to_string()))?;
            Ok(stream)
        }))?
    }
}

impl sley_remote::SshTransport for NativeSshTransport {
    fn open(
        &self,
        remote: &sley_transport::RemoteUrl,
        service: sley_protocol::GitService,
        options: sley_remote::SshTransportOptions,
        service_command: Option<&str>,
    ) -> sley_core::Result<Box<dyn sley_remote::SshStream>> {
        let program = service_command.unwrap_or(match service {
            sley_protocol::GitService::UploadPack => "git-upload-pack",
            sley_protocol::GitService::ReceivePack => "git-receive-pack",
            sley_protocol::GitService::UploadArchive => "git-upload-archive",
        });
        let command = format!("{program} '{}'", shell_quote(&remote.path));
        Ok(Box::new(self.open_command(
            remote,
            command,
            options.protocol,
        )?))
    }
}

fn shell_quote(value: &str) -> String {
    value.replace('\'', "'\\''")
}

#[derive(Default)]
struct SilentProgress;
impl sley_remote::ProgressSink for SilentProgress {}

fn remote_policy() -> sley_remote::RemotePolicy {
    sley_remote::RemotePolicy {
        namespace: sley_core::Namespace::new("gitadel"),
        transport: sley_remote::TransportPolicy::default(),
    }
}

fn fetch_options() -> sley_remote::FetchOptions {
    sley_remote::FetchOptions {
        policy: remote_policy(),
        quiet: true,
        progress: Some(false),
        auto_follow_tags: false,
        fetch_all_tags: false,
        include_remote_head: true,
        prune: true,
        prune_tags: true,
        dry_run: false,
        force: false,
        append: false,
        write_fetch_head: false,
        tag_option_explicit: true,
        prune_option_explicit: true,
        prune_tags_option_explicit: true,
        refmap: None,
        depth: None,
        merge_srcs: Vec::new(),
        filter: None,
        filter_auto: false,
        refetch: false,
        cloning: false,
        record_promisor_refs: false,
        update_shallow: false,
        reject_shallow: false,
        deepen_relative: false,
        update_head_ok: false,
        deepen_since: None,
        deepen_not: Vec::new(),
        ssh_options: None,
        upload_pack_command: None,
        atomic: false,
        negotiation_restrict: None,
        negotiation_include: None,
        negotiate_only: false,
    }
}

fn map_remote_error(error: impl std::fmt::Display) -> ApiError {
    ApiError::bad_request(error.to_string())
}

fn parse_remote(value: &str) -> Result<sley_transport::RemoteUrl, ApiError> {
    sley_transport::parse_remote_url(value).map_err(map_remote_error)
}

fn discover_remote(
    remote: &sley_transport::RemoteUrl,
    client: &ReqwestHttpClient,
    credentials: &mut StaticCredentials,
    ssh_transport: Option<&dyn sley_remote::SshTransport>,
) -> Result<(sley_core::ObjectFormat, Option<String>), ApiError> {
    let (format, features) = match remote.transport {
        sley_transport::RemoteTransport::Http | sley_transport::RemoteTransport::Https => {
            let discovery =
                sley_remote::http_discover_upload_pack(client, remote, credentials, None)
                    .map_err(map_remote_error)?;
            (discovery.object_format, discovery.features)
        }
        sley_transport::RemoteTransport::Ssh => {
            let transport = ssh_transport
                .ok_or_else(|| ApiError::bad_request("SSH transport is not configured"))?;
            let discovery = sley_remote::discover_ssh_upload_pack_advertisements_with_transport(
                remote,
                sley_remote::SshTransportOptions {
                    protocol: Some(sley_protocol::ProtocolVersion::V2),
                    ..Default::default()
                },
                None,
                transport,
            )
            .map_err(map_remote_error)?;
            (discovery.object_format, discovery.features)
        }
        _ => return Err(ApiError::bad_request("Mirror URL must use HTTPS or SSH.")),
    };
    let mut head = features
        .symrefs
        .into_iter()
        .find(|symref| symref.starts_with("HEAD:"));
    if let Some(head) = &mut head {
        head.drain(..5);
    }
    Ok((format, head))
}

fn known_host_keys(
    known_hosts: Option<&str>,
    host: &str,
    port: u16,
) -> sley_core::Result<TrustedHostKeys> {
    let hostname = host.to_ascii_lowercase();
    let hostname = if port == 22 {
        hostname
    } else {
        format!("[{hostname}]:{port}")
    };
    let mut keys = TrustedHostKeys::default();
    let mut append = |text: &str| -> sley_core::Result<()> {
        for entry in KnownHosts::new(text) {
            let entry = entry.map_err(|error| sley_core::GitError::Command(error.to_string()))?;
            if !known_host_matches(entry.host_patterns(), &hostname) {
                continue;
            }
            match entry.marker().copied() {
                None => keys.allowed.push(entry.into()),
                Some(Marker::Revoked) => keys.revoked.push(entry.into()),
                Some(Marker::CertAuthority) => {}
            }
        }
        Ok(())
    };
    if let Some(text) = known_hosts {
        append(text)?;
    }
    let mut read_file = |path: &Path| -> sley_core::Result<()> {
        match std::fs::read_to_string(path) {
            Ok(text) => append(&text),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    };
    for path in ["/etc/ssh/ssh_known_hosts", "/etc/ssh/ssh_known_hosts2"] {
        read_file(Path::new(path))?;
    }
    if known_hosts.is_none()
        && let Some(home) = std::env::var_os("HOME")
    {
        for name in ["known_hosts", "known_hosts2"] {
            read_file(&Path::new(&home).join(".ssh").join(name))?;
        }
    }
    if keys.allowed.is_empty() {
        return Err(sley_core::GitError::Command(format!(
            "no trusted SSH host key is configured for {hostname}"
        )));
    }
    Ok(keys)
}

fn known_host_matches(patterns: &HostPatterns, host: &str) -> bool {
    match patterns {
        HostPatterns::HashedName { salt, hash } => Hmac::<sha1::Sha1>::new_from_slice(salt)
            .is_ok_and(|mac| mac.chain_update(host.as_bytes()).verify_slice(hash).is_ok()),
        HostPatterns::Patterns(patterns) => {
            let mut matched = false;
            for pattern in patterns {
                let (negated, pattern) = match pattern.strip_prefix('!') {
                    Some(pattern) => (true, pattern),
                    None => (false, pattern.as_str()),
                };
                if ssh_host_pattern_matches(pattern.as_bytes(), host.as_bytes()) {
                    if negated {
                        return false;
                    }
                    matched = true;
                }
            }
            matched
        }
    }
}

fn ssh_host_pattern_matches(pattern: &[u8], host: &[u8]) -> bool {
    let (mut p, mut h, mut star, mut retry) = (0, 0, None, 0);
    while h < host.len() {
        if p < pattern.len() && (pattern[p] == b'?' || pattern[p].eq_ignore_ascii_case(&host[h])) {
            p += 1;
            h += 1;
        } else if p < pattern.len() && pattern[p] == b'*' {
            star = Some(p);
            p += 1;
            retry = h;
        } else if let Some(star) = star {
            retry += 1;
            h = retry;
            p = star + 1;
        } else {
            return false;
        }
    }
    while p < pattern.len() && pattern[p] == b'*' {
        p += 1;
    }
    p == pattern.len()
}

fn initialize_repository(
    path: &Path,
    remote_url: &str,
    format: sley_core::ObjectFormat,
) -> sley_core::Result<sley::Repository> {
    let repository = sley::Repository::init_with_format(path, format, true)?;
    let mut config = repository.load_repo_config()?;
    config.sections.push(sley_config::ConfigSection::new(
        "remote",
        Some("origin".to_owned()),
        vec![
            sley_config::ConfigEntry::new("url", Some(remote_url.to_owned())),
            sley_config::ConfigEntry::new("fetch", Some("+refs/*:refs/*".to_owned())),
            sley_config::ConfigEntry::new("mirror", Some("true".to_owned())),
        ],
    ));
    repository.save_repo_config(&config)?;
    Ok(repository)
}

fn branch_from_head(head: Option<&str>) -> Option<&str> {
    head.and_then(|head| head.strip_prefix("refs/heads/"))
        .filter(|branch| !branch.is_empty())
}

async fn run_blocking<T: Send + 'static>(
    work: impl FnOnce(Arc<RemoteOperation>) -> Result<T, ApiError> + Send + 'static,
) -> Result<T, ApiError> {
    let operation = Arc::new(RemoteOperation::new());
    let _cancel = CancelRemoteOnDrop(Arc::clone(&operation));
    let worker_operation = Arc::clone(&operation);
    let mut task = tokio::task::spawn_blocking(move || work(worker_operation));
    match tokio::time::timeout_at(operation.deadline, &mut task).await {
        Ok(joined) => joined.map_err(|error| ApiError::internal(error.to_string()))?,
        Err(_) => {
            operation.cancel();
            let _ = task.await;
            Err(ApiError::bad_request(
                "Remote operation timed out after 10 minutes.",
            ))
        }
    }
}

async fn remote_http_client(
    state: &RepositoryState,
    remote_url: &str,
) -> Result<reqwest::Client, ApiError> {
    if parse_remote(remote_url)?.transport != sley_transport::RemoteTransport::Https {
        return Ok(state.http_client().clone());
    }
    let url = Url::parse(remote_url).map_err(ApiError::internal)?;
    let origin = url.origin();
    let redirects = reqwest::redirect::Policy::custom(move |attempt| {
        if attempt.previous().len() >= 10 {
            attempt.error("too many Git HTTP redirects")
        } else if attempt.url().origin() != origin {
            attempt.error("Git HTTPS redirects must stay on the original origin")
        } else {
            attempt.follow()
        }
    });
    crate::network::pinned_public_https_client(&url, REMOTE_TIMEOUT, redirects)
        .await
        .map_err(ApiError::bad_request)
}

pub(super) async fn initialize(
    state: &RepositoryState,
    path: &Path,
    remote_url: &str,
    authentication: NativeAuthentication,
) -> Result<NativeInitialization, ApiError> {
    let path = path.to_owned();
    let remote_url = remote_url.to_owned();
    let client = remote_http_client(state, remote_url.as_str()).await?;
    run_blocking(move |operation| {
        let client = ReqwestHttpClient::new(client, Arc::clone(&operation));
        let remote = parse_remote(&remote_url)?;
        let ssh_transport = if let (Some(private_key), Some(host)) = (
            authentication.ssh_private_key.clone(),
            remote.host.as_deref(),
        ) {
            Some(NativeSshTransport::new(
                private_key,
                known_host_keys(
                    authentication.ssh_known_hosts.as_deref(),
                    host,
                    remote.port.unwrap_or(22),
                )
                .map_err(map_remote_error)?,
                Arc::clone(&operation),
            ))
        } else {
            None
        };
        let mut credentials = StaticCredentials {
            username: authentication.username,
            secret: authentication.secret,
        };
        let transport = ssh_transport
            .as_ref()
            .map(|transport| transport as &dyn sley_remote::SshTransport);
        let (format, discovered_head) =
            discover_remote(&remote, &client, &mut credentials, transport)?;
        let repository =
            initialize_repository(&path, &remote_url, format).map_err(map_remote_error)?;
        let mut progress = SilentProgress;
        let mut outcome = repository
            .fetch_with_clients_and_cancel(
                "origin",
                &["+refs/*:refs/*".to_owned()],
                fetch_options(),
                &mut credentials,
                &mut progress,
                Some(&client),
                transport,
                sley_core::CancelFlag::new(&operation.flag),
            )
            .map_err(map_remote_error)?;
        if outcome.head_symref.is_none()
            && !outcome
                .ref_updates
                .iter()
                .any(|update| update.src == "HEAD")
        {
            outcome.head_symref = discovered_head;
        }
        let branch = crate::repository::resources::select_advertised_or_default_branch(
            &repository,
            branch_from_head(outcome.head_symref.as_deref()),
        )
        .map_err(map_remote_error)?;
        if let Some(branch) = branch.as_deref() {
            let reference = format!("refs/heads/{branch}");
            repository
                .set_head_symref(reference, sley::HeadUpdateOptions::new())
                .map_err(map_remote_error)?;
        }
        Ok(NativeInitialization {
            object_format: format.name().to_owned(),
            default_branch: branch,
        })
    })
    .await
}

pub(super) async fn synchronize(
    state: &RepositoryState,
    path: &Path,
    remote_url: &str,
    authentication: NativeAuthentication,
) -> Result<(), ApiError> {
    let path = path.to_owned();
    let remote_url = remote_url.to_owned();
    let client = remote_http_client(state, remote_url.as_str()).await?;
    run_blocking(move |operation| {
        let client = ReqwestHttpClient::new(client, Arc::clone(&operation));
        let remote = parse_remote(&remote_url)?;
        let ssh_transport = if let (Some(private_key), Some(host)) = (
            authentication.ssh_private_key.clone(),
            remote.host.as_deref(),
        ) {
            Some(NativeSshTransport::new(
                private_key,
                known_host_keys(
                    authentication.ssh_known_hosts.as_deref(),
                    host,
                    remote.port.unwrap_or(22),
                )
                .map_err(map_remote_error)?,
                Arc::clone(&operation),
            ))
        } else {
            None
        };
        let transport = ssh_transport
            .as_ref()
            .map(|transport| transport as &dyn sley_remote::SshTransport);
        let mut credentials = StaticCredentials {
            username: authentication.username,
            secret: authentication.secret,
        };
        let repository = sley::Repository::open(&path).map_err(map_remote_error)?;
        let mut progress = SilentProgress;
        let outcome = repository
            .fetch_with_clients_and_cancel(
                "origin",
                &["+refs/*:refs/*".to_owned()],
                fetch_options(),
                &mut credentials,
                &mut progress,
                Some(&client),
                transport,
                sley_core::CancelFlag::new(&operation.flag),
            )
            .map_err(map_remote_error)?;
        // The persisted default, when present, is authoritative. The caller
        // repairs HEAD after fetching instead of adopting a remote change.
        let _ = outcome;
        Ok(())
    })
    .await
}

#[derive(Serialize)]
struct LfsBatchRequest<'a> {
    operation: &'static str,
    objects: &'a [LfsObject],
}

#[derive(Serialize)]
struct LfsObject {
    oid: String,
    size: u64,
}

#[derive(Deserialize)]
struct LfsBatchResponse {
    objects: Vec<LfsBatchObject>,
}

#[derive(Deserialize)]
struct LfsBatchObject {
    oid: String,
    #[serde(default)]
    actions: std::collections::BTreeMap<String, LfsAction>,
    error: Option<LfsError>,
}

#[derive(Deserialize)]
struct LfsAction {
    href: String,
    #[serde(default)]
    header: std::collections::BTreeMap<String, String>,
}

#[derive(Deserialize)]
struct LfsError {
    code: Option<u16>,
    message: Option<String>,
}

const LFS_POINTER_MAX_BYTES: usize = 1024;

fn lfs_pointer(content: &[u8]) -> Option<(String, u64)> {
    fn object_id(value: &str) -> Option<&str> {
        value.strip_prefix("sha256:").filter(|oid| {
            oid.len() == 64
                && oid
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
    }

    if content.len() >= LFS_POINTER_MAX_BYTES {
        return None;
    }
    let text = std::str::from_utf8(content).ok()?.trim();
    let mut values = [""; 3];
    let mut next = 0;
    let mut extensions: [Option<(&str, &str)>; 10] = [None; 10];
    for line in text.lines().filter(|line| !line.is_empty()) {
        if next == values.len() {
            return None;
        }
        let (key, value) = line.split_once(' ')?;
        if key == ["version", "oid", "size"][next] {
            values[next] = value;
            next += 1;
            continue;
        }
        let (priority, name) = key.strip_prefix("ext-")?.split_once('-')?;
        if priority.len() != 1
            || !priority.as_bytes()[0].is_ascii_digit()
            || !name
                .as_bytes()
                .first()
                .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
        {
            return None;
        }
        let slot = &mut extensions[usize::from(priority.as_bytes()[0] - b'0')];
        if slot.is_some_and(|(previous, _)| previous != key) {
            return None;
        }
        *slot = Some((key, value));
    }
    if next != values.len()
        || !matches!(
            values[0],
            "https://git-lfs.github.com/spec/v1"
                | "https://hawser.github.com/spec/v1"
                | "http://git-media.io/v/2"
        )
    {
        return None;
    }
    for (_, value) in extensions.into_iter().flatten() {
        object_id(value)?;
    }
    let oid = object_id(values[1])?;
    let size = u64::try_from(values[2].parse::<i64>().ok()?).ok()?;
    Some((oid.to_owned(), size))
}

fn collect_lfs_pointers(
    path: &Path,
    cancel: sley_core::CancelFlag<'_>,
) -> sley_core::Result<Vec<LfsObject>> {
    let repository = sley::Repository::open(path)?;
    let format = repository.object_format();
    let shallow = sley_remote::read_shallow(repository.git_dir(), format)?;
    let refs = repository.references();
    let mut pending = refs
        .list_refs()?
        .into_iter()
        .filter_map(|reference| match reference.target {
            sley_refs::RefTarget::Direct(oid) => Some(oid),
            sley_refs::RefTarget::Symbolic(_) => None,
        })
        .collect::<Vec<_>>();
    if let Some(sley_refs::RefTarget::Direct(oid)) = refs.read_ref("HEAD")? {
        pending.push(oid);
    }
    let mut seen = HashSet::new();
    let mut pointers = std::collections::BTreeMap::<String, u64>::new();
    while let Some(oid) = pending.pop() {
        cancel.check()?;
        if !seen.insert(oid) {
            continue;
        }
        if matches!(
            repository.read_object_header(&oid)?,
            Some((sley::GitObjectType::Blob, size)) if size >= LFS_POINTER_MAX_BYTES as u64
        ) {
            continue;
        }
        let object = repository.read_object(&oid)?;
        match object.object_type {
            sley::GitObjectType::Commit => {
                let commit = sley::CommitObject::parse(format, &object.body)?;
                pending.push(commit.tree);
                if !shallow.contains(&oid) {
                    pending.extend(commit.parents);
                }
            }
            sley::GitObjectType::Tree => {
                pending.extend(
                    sley::TreeObject::parse(format, &object.body)?
                        .entries
                        .into_iter()
                        .filter(|entry| entry.mode != 0o160000)
                        .map(|entry| entry.oid),
                );
            }
            sley::GitObjectType::Tag => {
                pending.push(sley::TagObject::parse(format, &object.body)?.object);
            }
            sley::GitObjectType::Blob => {
                if let Some((oid, size)) = lfs_pointer(&object.body) {
                    pointers.entry(oid).or_insert(size);
                }
            }
        }
    }
    Ok(pointers
        .into_iter()
        .map(|(oid, size)| LfsObject { oid, size })
        .collect())
}

fn lfs_endpoint(remote_url: &str) -> Result<Url, ApiError> {
    let mut endpoint = Url::parse(remote_url)
        .map_err(|_| ApiError::internal("import remote URL is not a valid URL"))?;
    if endpoint.scheme() == "ssh" {
        return Err(ApiError::bad_request(
            "SSH LFS endpoint requires git-lfs-authenticate",
        ));
    }
    let path = endpoint.path().trim_end_matches('/');
    endpoint.set_path(&format!("{path}/info/lfs/objects/batch"));
    endpoint.set_query(None);
    endpoint.set_fragment(None);
    Ok(endpoint)
}

#[derive(Deserialize)]
struct LfsSshAuthenticate {
    href: String,
    #[serde(default)]
    header: std::collections::BTreeMap<String, String>,
}

fn ssh_lfs_endpoint(
    remote_url: &str,
    authentication: &NativeAuthentication,
    operation: Arc<RemoteOperation>,
) -> Result<(Url, std::collections::BTreeMap<String, String>), ApiError> {
    let remote = parse_remote(remote_url)?;
    let (Some(private_key), Some(host)) = (
        authentication.ssh_private_key.clone(),
        remote.host.as_deref(),
    ) else {
        return Err(ApiError::bad_request("SSH LFS credentials are missing"));
    };
    let transport = NativeSshTransport::new(
        private_key,
        known_host_keys(
            authentication.ssh_known_hosts.as_deref(),
            host,
            remote.port.unwrap_or(22),
        )
        .map_err(map_remote_error)?,
        operation,
    );
    let command = format!(
        "git-lfs-authenticate '{}' download",
        shell_quote(&remote.path)
    );
    let mut stream = transport
        .open_command(&remote, command, None)
        .map_err(map_remote_error)?;
    let mut body = Vec::new();
    (&mut stream)
        .take(1024 * 1024 + 1)
        .read_to_end(&mut body)
        .map_err(|error| ApiError::bad_request(error.to_string()))?;
    if body.len() > 1024 * 1024 {
        return Err(ApiError::bad_request(
            "Git LFS SSH authentication response is too large.",
        ));
    }
    sley_remote::SshStream::finish(&mut stream).map_err(map_remote_error)?;
    let auth: LfsSshAuthenticate = serde_json::from_slice(&body).map_err(|error| {
        ApiError::bad_request(format!(
            "Could not parse git-lfs-authenticate response: {error}"
        ))
    })?;
    let mut endpoint = Url::parse(&auth.href)
        .map_err(|error| ApiError::bad_request(format!("Invalid Git LFS endpoint: {error}")))?;
    if !matches!(endpoint.scheme(), "http" | "https") {
        return Err(ApiError::bad_request(
            "Git LFS endpoint must use HTTP or HTTPS.",
        ));
    }
    let path = endpoint.path().trim_end_matches('/');
    endpoint.set_path(&format!("{path}/objects/batch"));
    Ok((endpoint, auth.header))
}

pub(super) async fn fetch_lfs(
    state: &RepositoryState,
    storage_key: Uuid,
    repository_path: &Path,
    remote_url: &str,
    authentication: NativeAuthentication,
) -> Result<(), ApiError> {
    tokio::time::timeout(REMOTE_TIMEOUT, async {
    let pointers = run_blocking({
        let path = repository_path.to_owned();
        move |operation| collect_lfs_pointers(&path, sley_core::CancelFlag::new(&operation.flag))
            .map_err(map_remote_error)
    })
    .await?;
    if pointers.is_empty() {
        return Ok(());
    }
    let transport = parse_remote(remote_url)?.transport;
    let is_ssh = transport == sley_transport::RemoteTransport::Ssh;
    let (endpoint, endpoint_headers) = if is_ssh {
        let remote_url = remote_url.to_owned();
        let credentials = authentication.clone();
        run_blocking(move |operation| ssh_lfs_endpoint(&remote_url, &credentials, operation)).await?
    } else {
        (lfs_endpoint(remote_url)?, std::collections::BTreeMap::new())
    };
    let client = remote_http_client(state, remote_url).await?;
    let mut download_clients = std::collections::HashMap::new();
    if transport == sley_transport::RemoteTransport::Https {
        download_clients.insert(endpoint.origin(), client.clone());
    }
    let _guard = state.lfs_operation_guard().await;
    for pointers in pointers.chunks(100) {
    let request = LfsBatchRequest {
        operation: "download",
        objects: pointers,
    };
    let mut builder = client
        .post(endpoint.clone())
        .header(reqwest::header::ACCEPT, LFS_MEDIA_TYPE)
        .header(reqwest::header::CONTENT_TYPE, LFS_MEDIA_TYPE)
        .json(&request);
    for (name, value) in &endpoint_headers {
        builder = builder.header(name.as_str(), value.as_str());
    }
    if !is_ssh
        && let (Some(username), Some(secret)) =
            (authentication.username.as_deref(), authentication.secret.as_deref())
    {
        builder = builder.basic_auth(username, Some(secret));
    }
    let response = builder
        .send()
        .await
        .map_err(|error| ApiError::bad_request(format!("Could not import Git LFS objects: {error}")))?;
    if !response.status().is_success() {
        return Err(ApiError::bad_request(format!(
            "Could not import Git LFS objects: HTTP {}",
            response.status()
        )));
    }
    let batch: LfsBatchResponse = response
        .json()
        .await
        .map_err(|error| ApiError::bad_request(format!("Could not parse Git LFS response: {error}")))?;
    let mut results = batch
        .objects
        .into_iter()
        .map(|object| (object.oid.clone(), object))
        .collect::<std::collections::BTreeMap<_, _>>();
    for object in pointers {
        let Some(object_result) = results.remove(&object.oid) else {
            return Err(ApiError::bad_request(format!(
                "Could not import Git LFS object {}: response omitted object",
                object.oid
            )));
        };
        let Some(action) = object_result.actions.get("download") else {
            let detail = object_result
                .error
                .map(|error| match error.code {
                    Some(code) => format!("HTTP {code}: {}", error.message.unwrap_or_default()),
                    None => error.message.unwrap_or_else(|| "download action missing".to_owned()),
                })
                .unwrap_or_else(|| "download action missing".to_owned());
            return Err(ApiError::bad_request(format!(
                "Could not import Git LFS object {}: {detail}",
                object.oid
            )));
        };
        let action_url = Url::parse(&action.href)
            .map_err(|error| ApiError::bad_request(format!("Invalid Git LFS download URL: {error}")))?;
        let download_client = if transport == sley_transport::RemoteTransport::Https {
            if action_url.scheme() != "https" {
                return Err(ApiError::bad_request(
                    "HTTPS Git remotes must use HTTPS Git LFS downloads.",
                ));
            }
            let origin = action_url.origin();
            if let Some(client) = download_clients.get(&origin) {
                client.clone()
            } else {
                let client = remote_http_client(state, action_url.as_str()).await?;
                download_clients.insert(origin, client.clone());
                client
            }
        } else {
            client.clone()
        };
        let mut download = download_client.get(action_url);
        for (name, value) in &action.header {
            let name = HeaderName::try_from(name.as_str()).map_err(ApiError::internal)?;
            let value = HeaderValue::try_from(value.as_str()).map_err(ApiError::internal)?;
            download = download.header(name, value);
        }
        let response = download.send().await.map_err(|error| {
            ApiError::bad_request(format!("Could not download Git LFS object {}: {error}", object.oid))
        })?;
        if !response.status().is_success() {
            return Err(ApiError::bad_request(format!(
                "Could not download Git LFS object {}: HTTP {}",
                object.oid,
                response.status()
            )));
        }
        let expected = object.oid.parse::<BlobDigest>().map_err(ApiError::internal)?;
        let key = lfs_object_key(storage_key, &object.oid).map_err(ApiError::internal)?;
        let reader =
            StreamReader::new(response.bytes_stream().map(|chunk| chunk.map_err(io::Error::other)));
        let reader = tokio::io::AsyncReadExt::take(reader, object.size.saturating_add(1));
        let metadata = state
            .lfs_store()
            .put_verified(&key, expected, Box::pin(reader))
            .await
            .map_err(|error| {
                if error
                    .chain()
                    .any(|cause| cause.downcast_ref::<crate::blob_store::DigestMismatch>().is_some())
                {
                    ApiError::bad_request(format!(
                        "Downloaded Git LFS object {} does not match its SHA-256 identifier.",
                        object.oid
                    ))
                } else {
                    ApiError::internal(error)
                }
            })?;
        if metadata.size != object.size {
            return Err(ApiError::bad_request(format!(
                "Git LFS object {} has size {}, expected {}",
                object.oid, metadata.size, object.size
            )));
        }
    }
    }
    Ok(())
    }).await.map_err(|_| ApiError::bad_request("Git LFS transfer timed out after 10 minutes."))?
}

#[cfg(test)]
mod tests {
    use super::*;

    const OID: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    const HOST_KEY: &str =
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIBnLEmF9TiNhBQKjiE7P4GVjMCmZaXKT3tSvXxDEJ3iQ";

    #[test]
    fn lfs_pointer_matches_git_blob_scanner_boundaries() {
        let pointer =
            format!("version https://git-lfs.github.com/spec/v1\noid sha256:{OID}\nsize 7\n");
        let expected = Some((OID.to_owned(), 7));
        let mut padded = pointer.as_bytes().to_vec();
        padded.resize(1023, b' ');
        assert_eq!(lfs_pointer(&padded), expected);
        padded.push(b' ');
        assert_eq!(lfs_pointer(&padded), None);

        let legacy =
            format!("version https://hawser.github.com/spec/v1\r\noid sha256:{OID}\r\nsize 7");
        assert_eq!(lfs_pointer(legacy.as_bytes()), expected);
    }

    #[test]
    fn lfs_pointer_rejects_invalid_content_instead_of_importing_it() {
        let uppercase = format!(
            "version https://git-lfs.github.com/spec/v1\noid sha256:{}\nsize 7\n",
            OID.to_ascii_uppercase()
        );
        assert_eq!(lfs_pointer(uppercase.as_bytes()), None);
        let unknown = format!(
            "version https://git-lfs.github.com/spec/v1\noid sha256:{OID}\nsize 7\nunknown value\n"
        );
        assert_eq!(lfs_pointer(unknown.as_bytes()), None);
        let overflowing = format!(
            "version https://git-lfs.github.com/spec/v1\noid sha256:{OID}\nsize 9223372036854775808\n"
        );
        assert_eq!(lfs_pointer(overflowing.as_bytes()), None);
    }

    #[test]
    fn known_host_patterns_enforce_negation_and_hashed_ports() {
        let line = format!("*.example.invalid,!blocked.example.invalid {HOST_KEY}\n");
        let entry = KnownHosts::new(&line).next().unwrap().unwrap();
        assert!(known_host_matches(
            entry.host_patterns(),
            "git.example.invalid"
        ));
        assert!(!known_host_matches(
            entry.host_patterns(),
            "BLOCKED.example.invalid"
        ));
        assert!(!known_host_matches(
            entry.host_patterns(),
            "git.other.invalid"
        ));

        let hashed =
            format!("|1|AAECAwQFBgcICQoLDA0ODxAREhM=|eZ3g3Ll+99WZJ+JU+I0m0Qgw+co= {HOST_KEY}\n");
        let entry = KnownHosts::new(&hashed).next().unwrap().unwrap();
        assert!(known_host_matches(
            entry.host_patterns(),
            "[git.example.invalid]:2222"
        ));
        assert!(!known_host_matches(
            entry.host_patterns(),
            "[git.example.invalid]:2223"
        ));
        assert!(!known_host_matches(
            entry.host_patterns(),
            "git.example.invalid"
        ));
    }

    #[tokio::test]
    async fn host_key_comments_do_not_change_trust_or_revocation() {
        let key = PublicKey::from_openssh(HOST_KEY).unwrap();
        let other = PublicKey::from_openssh(
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIPkqT0c97RcrMYzZSG9xih+OINZzxiWhvJSRNM9/1Xsl",
        )
        .unwrap();
        let allowed = PublicKey::from_openssh(&format!("{HOST_KEY} administrator note")).unwrap();
        let mut handler = NativeSshHandler {
            host_keys: TrustedHostKeys {
                allowed: vec![allowed],
                revoked: Vec::new(),
            },
        };
        assert!(handler.check_server_key(&key).await.unwrap());
        assert!(!handler.check_server_key(&other).await.unwrap());
        handler
            .host_keys
            .revoked
            .push(PublicKey::from_openssh(&format!("{HOST_KEY} revoked key")).unwrap());
        assert!(!handler.check_server_key(&key).await.unwrap());
    }
}
