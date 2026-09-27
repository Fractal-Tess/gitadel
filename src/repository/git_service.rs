use std::{
    io::{self, Read, Write},
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use anyhow::{Context, Result};
use sley_core::ObjectFormat;
use sley_protocol::{
    ReceivePackCommandStatus, ReceivePackCommandStatusV2, ReceivePackPushRequestHeader,
    ReceivePackReportStatus, ReceivePackReportStatusV2, ReceivePackRequest,
    ReceivePackUnpackStatus, read_receive_pack_push_options, read_receive_pack_request,
    write_ref_advertisement_set, write_ref_advertisements,
};

use super::protection::{RefCheck, RefGuard};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    runtime::Handle,
    sync::Notify,
};
#[derive(Clone)]
pub(crate) struct BridgeCancellation {
    cancelled: Arc<AtomicBool>,
    notify: Arc<Notify>,
}

impl Default for BridgeCancellation {
    fn default() -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
            notify: Arc::new(Notify::new()),
        }
    }
}
impl BridgeCancellation {
    pub(crate) fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        self.notify.notify_waiters();
    }

    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

/// Sley's protocol and pack engines are deliberately synchronous. Calls block
/// only on the bounded Tokio duplex supplied by the transport.
pub(crate) struct BlockingReader<R> {
    inner: R,
    handle: Handle,
    cancellation: BridgeCancellation,
}

impl<R> BlockingReader<R> {
    pub(crate) fn new(inner: R, handle: Handle, cancellation: BridgeCancellation) -> Self {
        Self {
            inner,
            handle,
            cancellation,
        }
    }
}
impl<R: AsyncRead + Unpin> Read for BlockingReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if self.cancellation.is_cancelled() {
            return Err(sley_core::cancelled_io_error());
        }
        let notified = self.cancellation.notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if self.cancellation.is_cancelled() {
            return Err(sley_core::cancelled_io_error());
        }
        self.handle.block_on(async {
            tokio::select! {
                result = self.inner.read(buffer) => result,
                _ = &mut notified => Err(sley_core::cancelled_io_error()),
            }
        })
    }
}

/// A blocking `Write` facade over an asynchronous transport half.
pub(crate) struct BlockingWriter<W> {
    inner: W,
    handle: Handle,
    cancellation: BridgeCancellation,
}

impl<W> BlockingWriter<W> {
    pub(crate) fn new(inner: W, handle: Handle, cancellation: BridgeCancellation) -> Self {
        Self {
            inner,
            handle,
            cancellation,
        }
    }
}
impl<W: AsyncWrite + Unpin> Write for BlockingWriter<W> {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        if self.cancellation.is_cancelled() {
            return Err(sley_core::cancelled_io_error());
        }
        let notified = self.cancellation.notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if self.cancellation.is_cancelled() {
            return Err(sley_core::cancelled_io_error());
        }
        self.handle.block_on(async {
            tokio::select! {
                result = self.inner.write(buffer) => result,
                _ = &mut notified => Err(sley_core::cancelled_io_error()),
            }
        })
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.cancellation.is_cancelled() {
            return Err(sley_core::cancelled_io_error());
        }
        let notified = self.cancellation.notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if self.cancellation.is_cancelled() {
            return Err(sley_core::cancelled_io_error());
        }
        self.handle.block_on(async {
            tokio::select! {
                result = self.inner.flush() => result,
                _ = &mut notified => Err(sley_core::cancelled_io_error()),
            }
        })
    }
}

pub(crate) fn object_format(name: &str) -> Result<ObjectFormat> {
    match name {
        "sha1" => Ok(ObjectFormat::Sha1),
        "sha256" => Ok(ObjectFormat::Sha256),
        other => Err(anyhow::anyhow!(
            "unsupported repository object format {other}"
        )),
    }
}

pub(crate) fn protocol_v2(value: Option<&str>) -> bool {
    value.is_some_and(|value| value.split(':').any(|token| token == "version=2"))
}

pub(crate) fn write_advertisement(
    path: &Path,
    format: ObjectFormat,
    receive: bool,
    v2: bool,
    writer: &mut impl Write,
) -> Result<()> {
    let policy = sley_remote::RemotePolicy::default();
    if v2 && !receive {
        let config = sley_config::read_repo_config(path, None)?;
        let mut input = io::empty();
        sley_remote::serve_upload_pack_v2_with_config(
            &policy, path, format, &config, &mut input, writer,
        )
        .context("write protocol v2 upload-pack advertisement")?;
        return Ok(());
    }
    if receive {
        let config = sley_config::read_repo_config(path, None)?;
        let mut refs = sley_remote::local_receive_pack_advertisements(&policy, path, format)
            .context("read repository refs")?;
        sley_remote::attach_receive_pack_capabilities(
            &mut refs,
            format,
            &sley_remote::receive_pack_features_with_config(format, &config),
        )
        .context("encode receive-pack capabilities")?;
        write_ref_advertisements(writer, &refs).context("write Git ref advertisement")?;
    } else {
        let mut refs = sley_remote::local_fetch_advertisement_set(&policy, path, format)
            .context("read repository refs")?;
        sley_remote::attach_upload_pack_capabilities(
            &mut refs.refs,
            format,
            &sley_remote::upload_pack_features(&policy, path, format)?,
        )
        .context("encode upload-pack capabilities")?;
        write_ref_advertisement_set(writer, &refs).context("write Git ref advertisement")?;
    }
    Ok(())
}

/// Serve one smart-HTTP or SSH upload-pack request. Protocol framing remains
/// owned by Sley; transports only provide the bounded reader and writer.
pub(crate) fn serve_upload_pack(
    path: &Path,
    format: ObjectFormat,
    v2: bool,
    stateless: bool,
    reader: &mut impl Read,
    writer: &mut impl Write,
) -> Result<()> {
    let policy = sley_remote::RemotePolicy::default();
    if v2 {
        let config = sley_config::read_repo_config(path, None)?;
        if stateless {
            sley_remote::serve_upload_pack_v2_stateless_with_config(
                &policy, path, format, &config, reader, writer,
            )?;
        } else {
            sley_remote::serve_upload_pack_v2_with_config(
                &policy, path, format, &config, reader, writer,
            )?;
        }
        return Ok(());
    }
    sley_remote::serve_upload_pack_v0(&policy, path, format, reader, writer, stateless)?;
    Ok(())
}

pub(crate) struct ReceiveOutcome {
    pub landed: bool,
    pub response_error: Option<String>,
}

/// Serve one receive-pack request and emit its negotiated status report. The
/// outcome distinguishes ref updates from a merely successful protocol process,
/// allowing callers to record audit/webhook events only for landed commands.
///
/// Every command is checked against `guard` before Sley may write a ref.
/// Rejected commands are withheld from Sley and reported as `ng` with the
/// rule's message, in the client's original command order.
pub(crate) fn serve_receive_pack(
    path: &Path,
    format: ObjectFormat,
    guard: &RefGuard,
    reader: &mut impl Read,
    writer: &mut impl Write,
) -> Result<ReceiveOutcome> {
    let commands = read_receive_pack_request(format, reader)?;
    let has_push_options = commands
        .capabilities
        .iter()
        .any(|capability| capability.name == "push-options");
    let push_options = if has_push_options {
        Some(read_receive_pack_push_options(reader)?)
    } else {
        None
    };
    let header = ReceivePackPushRequestHeader {
        commands,
        push_options,
    };
    let mut checks = header
        .commands
        .commands
        .iter()
        .map(|command| {
            guard.check(
                &command.name,
                command.old_id.is_null(),
                command.new_id.is_null(),
            )
        })
        .collect::<Vec<_>>();
    if checks.iter().all(|check| *check == RefCheck::Allowed) {
        let (outcome, remote_stderr) = run_receive_pack(path, format, &header, reader)?;
        let response_error = write_receive_report(writer, &header, &outcome.report, &remote_stderr);
        return Ok(ReceiveOutcome {
            landed: landed(&outcome),
            response_error,
        });
    }

    // Some command is protected. Receive the pack into a private quarantine
    // first so ancestry can be checked against the incoming objects, and so
    // the client is never left blocked while its pack goes unread.
    let sends_pack = header
        .commands
        .commands
        .iter()
        .any(|command| !command.new_id.is_null());
    let probe = if sends_pack {
        match receive_probe_pack(path, format, reader) {
            Ok(probe) => probe,
            Err(error) => {
                let message = format!("unpacker error: {error}");
                let rejections = vec![Some(message.clone()); header.commands.commands.len()];
                let report = rejection_report(&header, &rejections, Some(message));
                let response_error = write_receive_report(writer, &header, &report, &[]);
                return Ok(ReceiveOutcome {
                    landed: false,
                    response_error,
                });
            }
        }
    } else {
        None
    };
    for (check, command) in checks.iter_mut().zip(&header.commands.commands) {
        let RefCheck::RequiresFastForward(message) = check else {
            continue;
        };
        let database = probe.as_ref().map_or_else(
            || sley_odb::FileObjectDatabase::from_git_dir(path, format),
            |(quarantine, _)| quarantine.database(),
        );
        *check = match sley_remote::is_fast_forward(
            path,
            &database,
            format,
            &command.old_id,
            &command.new_id,
        ) {
            Ok(true) => RefCheck::Allowed,
            Ok(false) => RefCheck::Rejected(std::mem::take(message)),
            Err(error) => RefCheck::Rejected(format!("cannot verify fast-forward update: {error}")),
        };
    }
    let mut rejections = checks
        .into_iter()
        .map(|check| match check {
            RefCheck::Rejected(message) => Some(message),
            RefCheck::Allowed | RefCheck::RequiresFastForward(_) => None,
        })
        .collect::<Vec<_>>();
    let atomic = header
        .commands
        .capabilities
        .iter()
        .any(|capability| capability.name == "atomic");
    if atomic && rejections.iter().any(Option::is_some) {
        for rejection in rejections
            .iter_mut()
            .filter(|rejection| rejection.is_none())
        {
            *rejection = Some("atomic push failure".to_owned());
        }
    }
    let accepted = header
        .commands
        .commands
        .iter()
        .zip(&rejections)
        .filter(|(_, rejection)| rejection.is_none())
        .map(|(command, _)| command.clone())
        .collect::<Vec<_>>();
    if accepted.is_empty() {
        let report = rejection_report(&header, &rejections, None);
        let response_error = write_receive_report(writer, &header, &report, &[]);
        return Ok(ReceiveOutcome {
            landed: false,
            response_error,
        });
    }
    let needs_pack = accepted.iter().any(|command| !command.new_id.is_null());
    let accepted_header = ReceivePackPushRequestHeader {
        commands: ReceivePackRequest {
            shallow: header.commands.shallow.clone(),
            commands: accepted,
            capabilities: header.commands.capabilities.clone(),
        },
        push_options: header.push_options.clone(),
    };
    // Sley receives the already-read pack again from the probe's pack file.
    // An empty pack (all objects already present) may leave no file behind.
    let mut pack: Box<dyn Read> = match probe.as_ref() {
        Some((_, pack_path)) if needs_pack => match std::fs::File::open(pack_path) {
            Ok(file) => Box::new(io::BufReader::new(file)),
            Err(_) => Box::new(io::empty()),
        },
        _ => Box::new(io::empty()),
    };
    let (outcome, remote_stderr) = run_receive_pack(path, format, &accepted_header, &mut pack)?;
    drop(probe);
    let report = merge_reports(&header, &rejections, &outcome.report);
    let response_error = write_receive_report(writer, &header, &report, &remote_stderr);
    Ok(ReceiveOutcome {
        landed: landed(&outcome),
        response_error,
    })
}

type ProbePack = (sley_odb::IncomingPackQuarantine, std::path::PathBuf);

/// Receive the client's pack into a disposable quarantine. Dropping the
/// quarantine removes every object; nothing is promoted from here.
fn receive_probe_pack(
    path: &Path,
    format: ObjectFormat,
    reader: &mut impl Read,
) -> Result<Option<ProbePack>> {
    let mut prefix = [0u8; 4];
    match reader.read_exact(&mut prefix) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error.into()),
    }
    if &prefix != b"PACK" {
        anyhow::bail!("receive-pack packfile must start with PACK");
    }
    let quarantine = sley_odb::IncomingPackQuarantine::new(path, format)?;
    let mut stream = Read::chain(io::Cursor::new(prefix), reader);
    let installed = quarantine
        .database()
        .install_raw_pack_from_reader(&mut stream)?;
    Ok(Some((quarantine, installed.pack_path)))
}

fn run_receive_pack(
    path: &Path,
    format: ObjectFormat,
    header: &ReceivePackPushRequestHeader,
    pack_reader: &mut dyn Read,
) -> Result<(sley_remote::ReceivePackServerOutcome, Vec<u8>)> {
    let config = sley_config::read_repo_config(path, None)?;
    let validation = sley_fsck::FsckPolicy::from_config(
        &config,
        sley_fsck::FsckConfigKind::Receive,
        format,
        path,
        false,
    )?;
    let mut remote_stderr = Vec::new();
    let outcome = sley_remote::serve_receive_pack(
        None,
        sley_remote::ReceivePackServerRequest {
            policy: &sley_remote::RemotePolicy::default(),
            git_dir: path,
            format,
            header,
            pack_reader,
            config: &config,
            validation: &validation,
            options: sley_remote::ReceivePackServerOptions {
                quiet: false,
                remote_stderr: Some(&mut remote_stderr),
                run_post_hooks: true,
            },
        },
    )?;
    Ok((outcome, remote_stderr))
}

fn landed(outcome: &sley_remote::ReceivePackServerOutcome) -> bool {
    outcome
        .command_states
        .iter()
        .any(|state| state.error_string.is_none() && state.command.old_id != state.command.new_id)
}

fn uses_report_status_v2(header: &ReceivePackPushRequestHeader) -> bool {
    header
        .commands
        .capabilities
        .iter()
        .any(|capability| capability.name == "report-status-v2")
}

/// A report in which every command was rejected before reaching Sley.
fn rejection_report(
    header: &ReceivePackPushRequestHeader,
    rejections: &[Option<String>],
    unpack_error: Option<String>,
) -> sley_remote::ReceivePackServerReport {
    let unpack = match unpack_error {
        Some(message) => ReceivePackUnpackStatus::Error(message),
        None => ReceivePackUnpackStatus::Ok,
    };
    let rejected = header
        .commands
        .commands
        .iter()
        .zip(rejections)
        .map(|(command, rejection)| {
            (
                command.name.clone(),
                rejection
                    .clone()
                    .unwrap_or_else(|| "atomic push failure".to_owned()),
            )
        });
    if uses_report_status_v2(header) {
        sley_remote::ReceivePackServerReport::V2(ReceivePackReportStatusV2 {
            unpack,
            commands: rejected
                .map(|(name, message)| ReceivePackCommandStatusV2::Ng { name, message })
                .collect(),
        })
    } else {
        sley_remote::ReceivePackServerReport::V1(ReceivePackReportStatus {
            unpack,
            commands: rejected
                .map(|(name, message)| ReceivePackCommandStatus::Ng { name, message })
                .collect(),
        })
    }
}

/// Interleave protection rejections with Sley's report for the accepted
/// commands, preserving the order in which the client sent them.
fn merge_reports(
    header: &ReceivePackPushRequestHeader,
    rejections: &[Option<String>],
    report: &sley_remote::ReceivePackServerReport,
) -> sley_remote::ReceivePackServerReport {
    match report {
        sley_remote::ReceivePackServerReport::V1(status) => {
            sley_remote::ReceivePackServerReport::V1(ReceivePackReportStatus {
                unpack: status.unpack.clone(),
                commands: interleave(header, rejections, &status.commands, |name, message| {
                    ReceivePackCommandStatus::Ng { name, message }
                }),
            })
        }
        sley_remote::ReceivePackServerReport::V2(status) => {
            sley_remote::ReceivePackServerReport::V2(ReceivePackReportStatusV2 {
                unpack: status.unpack.clone(),
                commands: interleave(header, rejections, &status.commands, |name, message| {
                    ReceivePackCommandStatusV2::Ng { name, message }
                }),
            })
        }
    }
}

fn interleave<T: Clone>(
    header: &ReceivePackPushRequestHeader,
    rejections: &[Option<String>],
    accepted: &[T],
    rejected: impl Fn(String, String) -> T,
) -> Vec<T> {
    let mut accepted = accepted.iter().cloned();
    let mut merged = Vec::with_capacity(rejections.len());
    for (command, rejection) in header.commands.commands.iter().zip(rejections) {
        match rejection {
            Some(message) => merged.push(rejected(command.name.clone(), message.clone())),
            None => merged.extend(accepted.next()),
        }
    }
    merged.extend(accepted);
    merged
}

fn write_receive_report(
    writer: &mut impl Write,
    header: &ReceivePackPushRequestHeader,
    report: &sley_remote::ReceivePackServerReport,
    remote_stderr: &[u8],
) -> Option<String> {
    let sideband = sley_remote::request_uses_sideband(header);
    let report_status = header.commands.capabilities.iter().any(|capability| {
        matches!(
            capability.name.as_str(),
            "report-status" | "report-status-v2"
        )
    });
    let mut response_error = None;
    if sideband
        && let Err(error) = sley_remote::write_receive_pack_sideband_stderr(writer, remote_stderr)
    {
        response_error = Some(error.to_string());
    }
    if report_status
        && let Err(error) =
            sley_remote::write_receive_pack_server_report(writer, report, sideband, true)
    {
        response_error = Some(error.to_string());
    }
    response_error
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use sley::{ObjectId, RefPrecondition, ReferenceTarget, Repository as GitRepository};

    use super::*;
    use crate::repository::{
        files::commit_file,
        protection::{RefGuard, test_rule},
    };

    const ZERO: &str = "0000000000000000000000000000000000000000";

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new(label: &str) -> Self {
            Self(
                std::env::temp_dir()
                    .join(format!("gitadel-receive-{label}-{}", uuid::Uuid::new_v4())),
            )
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn commit(path: &Path, branch: &str, parent: Option<ObjectId>, file: &str) -> ObjectId {
        let parent = parent.map(|oid| oid.to_hex());
        commit_file(
            path,
            branch,
            parent.as_deref(),
            file,
            file.as_bytes().to_vec(),
            file,
            "test",
        )
        .unwrap()
    }

    fn pack(path: &Path, root: ObjectId) -> Vec<u8> {
        let repository = GitRepository::open_exact_bare(path).unwrap();
        repository
            .reachable_pack_plan()
            .root(root)
            .build()
            .unwrap()
            .unwrap()
            .prepare_to_memory()
            .unwrap()
            .pack
    }

    fn pkt(line: &str) -> Vec<u8> {
        format!("{:04x}{line}", line.len() + 4).into_bytes()
    }

    /// Build a receive-pack request whose first command carries capabilities.
    fn request(commands: &[(String, String, &str)], pack: Option<Vec<u8>>) -> Vec<u8> {
        let mut body = Vec::new();
        for (index, (old, new, name)) in commands.iter().enumerate() {
            let capabilities = if index == 0 { "\0report-status" } else { "" };
            body.extend(pkt(&format!("{old} {new} {name}{capabilities}\n")));
        }
        body.extend_from_slice(b"0000");
        body.extend(pack.unwrap_or_default());
        body
    }

    fn push(path: &Path, guard: &RefGuard, body: Vec<u8>) -> (ReceiveOutcome, String) {
        let mut output = Vec::new();
        let outcome = serve_receive_pack(
            path,
            ObjectFormat::Sha1,
            guard,
            &mut io::Cursor::new(body),
            &mut output,
        )
        .unwrap();
        (outcome, String::from_utf8_lossy(&output).into_owned())
    }

    fn tip(path: &Path, branch: &str) -> Option<ObjectId> {
        let repository = GitRepository::open_exact_bare(path).unwrap();
        match repository
            .references()
            .read_ref(&format!("refs/heads/{branch}"))
            .unwrap()
        {
            Some(ReferenceTarget::Direct(oid)) => Some(oid),
            _ => None,
        }
    }

    fn main_guard() -> RefGuard {
        RefGuard::for_tests(vec![test_rule("branch", "main")], None, false, &[])
    }

    #[test]
    fn protected_branch_rejects_force_push_but_accepts_other_refs_in_the_same_push() {
        let server = TestDirectory::new("server");
        let client = TestDirectory::new("client");
        GitRepository::init_bare(&server.0).unwrap();
        GitRepository::init_bare(&client.0).unwrap();
        let base = commit(&server.0, "main", None, "base.txt");
        let unrelated = commit(&client.0, "main", None, "unrelated.txt");

        let body = request(
            &[
                (base.to_hex(), unrelated.to_hex(), "refs/heads/main"),
                (ZERO.to_owned(), unrelated.to_hex(), "refs/heads/scratch"),
            ],
            Some(pack(&client.0, unrelated)),
        );
        let (outcome, report) = push(&server.0, &main_guard(), body);

        assert!(
            report
                .contains("ng refs/heads/main protected branch `main`: force-push is not allowed"),
            "{report}"
        );
        assert!(report.contains("ok refs/heads/scratch"), "{report}");
        assert!(outcome.landed);
        assert_eq!(tip(&server.0, "main"), Some(base));
        assert_eq!(tip(&server.0, "scratch"), Some(unrelated));
        let leftovers = std::fs::read_dir(server.0.join("objects"))
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .path()
                    .extension()
                    .is_some_and(|ext| ext == "incoming")
            })
            .count();
        assert_eq!(leftovers, 0, "probe quarantine was not removed");
    }

    #[test]
    fn protected_branch_accepts_fast_forward_pushes() {
        let server = TestDirectory::new("server");
        GitRepository::init_bare(&server.0).unwrap();
        let base = commit(&server.0, "main", None, "base.txt");
        let next = commit(&server.0, "main", Some(base), "next.txt");
        let repository = GitRepository::open_exact_bare(&server.0).unwrap();
        let references = repository.references();
        let mut transaction = references.transaction();
        transaction.update_to(
            "refs/heads/main".to_owned(),
            ReferenceTarget::Direct(base),
            RefPrecondition::MustExistAndMatch(ReferenceTarget::Direct(next)),
            None,
        );
        transaction.commit().unwrap();

        let body = request(
            &[(base.to_hex(), next.to_hex(), "refs/heads/main")],
            Some(pack(&server.0, next)),
        );
        let (outcome, report) = push(&server.0, &main_guard(), body);

        assert!(report.contains("ok refs/heads/main"), "{report}");
        assert!(outcome.landed);
        assert_eq!(tip(&server.0, "main"), Some(next));
    }

    #[test]
    fn protected_branch_rejects_deletion() {
        let server = TestDirectory::new("server");
        GitRepository::init_bare(&server.0).unwrap();
        let base = commit(&server.0, "main", None, "base.txt");

        let body = request(&[(base.to_hex(), ZERO.to_owned(), "refs/heads/main")], None);
        let (outcome, report) = push(&server.0, &main_guard(), body);

        assert!(
            report.contains("ng refs/heads/main protected branch `main`: deletion is not allowed"),
            "{report}"
        );
        assert!(!outcome.landed);
        assert_eq!(tip(&server.0, "main"), Some(base));
    }

    #[test]
    fn unprotected_force_push_still_lands() {
        let server = TestDirectory::new("server");
        let client = TestDirectory::new("client");
        GitRepository::init_bare(&server.0).unwrap();
        GitRepository::init_bare(&client.0).unwrap();
        let base = commit(&server.0, "main", None, "base.txt");
        let unrelated = commit(&client.0, "main", None, "unrelated.txt");

        let body = request(
            &[(base.to_hex(), unrelated.to_hex(), "refs/heads/main")],
            Some(pack(&client.0, unrelated)),
        );
        let (outcome, report) = push(&server.0, &RefGuard::default(), body);

        assert!(report.contains("ok refs/heads/main"), "{report}");
        assert!(outcome.landed);
        assert_eq!(tip(&server.0, "main"), Some(unrelated));
    }

    #[test]
    fn protected_tags_reject_moves_and_allow_new_tags() {
        let server = TestDirectory::new("server");
        GitRepository::init_bare(&server.0).unwrap();
        let base = commit(&server.0, "main", None, "base.txt");
        let next = commit(&server.0, "main", Some(base), "next.txt");
        let guard = RefGuard::for_tests(vec![test_rule("tag", "v*")], None, false, &[]);

        let body = request(
            &[(ZERO.to_owned(), base.to_hex(), "refs/tags/v1")],
            Some(pack(&server.0, base)),
        );
        let (_, report) = push(&server.0, &guard, body);
        assert!(report.contains("ok refs/tags/v1"), "{report}");

        let body = request(
            &[(base.to_hex(), next.to_hex(), "refs/tags/v1")],
            Some(pack(&server.0, next)),
        );
        let (outcome, report) = push(&server.0, &guard, body);
        assert!(
            report.contains(
                "ng refs/tags/v1 protected tag `v1`: existing tags cannot be moved or deleted"
            ),
            "{report}"
        );
        assert!(!outcome.landed);
    }
}
