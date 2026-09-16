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
    ReceivePackPushRequestHeader, read_receive_pack_push_options, read_receive_pack_request,
    write_ref_advertisement_set, write_ref_advertisements,
};
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
pub(crate) fn serve_receive_pack(
    path: &Path,
    format: ObjectFormat,
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
            header: &header,
            pack_reader: reader,
            config: &config,
            validation: &validation,
            options: sley_remote::ReceivePackServerOptions {
                quiet: false,
                remote_stderr: Some(&mut remote_stderr),
                run_post_hooks: true,
            },
        },
    )?;
    let sideband = sley_remote::request_uses_sideband(&header);
    let report_status = header.commands.capabilities.iter().any(|capability| {
        matches!(
            capability.name.as_str(),
            "report-status" | "report-status-v2"
        )
    });
    let mut response_error = None;
    if sideband
        && let Err(error) = sley_remote::write_receive_pack_sideband_stderr(writer, &remote_stderr)
    {
        response_error = Some(error.to_string());
    }
    if report_status
        && let Err(error) =
            sley_remote::write_receive_pack_server_report(writer, &outcome.report, sideband, true)
    {
        response_error = Some(error.to_string());
    }
    let landed = outcome
        .command_states
        .iter()
        .any(|state| state.error_string.is_none() && state.command.old_id != state.command.new_id);
    Ok(ReceiveOutcome {
        landed,
        response_error,
    })
}
