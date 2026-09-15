//! Owned streams over a container exec process.
//!
//! A stream wraps one `AttachedProcess`. `finish` waits for the remote
//! command's exit status; dropping a stream without finishing aborts the
//! remote command (best effort, logged) and never counts as completion.

use std::future::{Future, poll_fn};
use std::io;
use std::path::PathBuf;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::SystemTime;

use futures_io::{AsyncRead, AsyncWrite};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::Status;
use kube::api::AttachedProcess;
use remotefs::fs::{AsyncRemoteRead, AsyncRemoteWrite, UnixPex};
use remotefs::{RemoteError, RemoteErrorType, RemoteResult};
use tokio::io::{AsyncRead as _, AsyncWrite as _, ReadBuf};

use super::exec::KubeExec;
use crate::utils::{fmt as fmt_utils, path as path_utils};

type StatusFuture = Pin<Box<dyn Future<Output = Option<Status>> + Send>>;
type TokioReader = Box<dyn tokio::io::AsyncRead + Send + Unpin>;
type TokioWriter = Box<dyn tokio::io::AsyncWrite + Send + Unpin>;

/// Map the exec `Status` object to an exit code.
pub(crate) fn exit_code(status: Option<Status>) -> RemoteResult<u32> {
    let Some(status) = status else {
        return Err(RemoteError::with_message(
            RemoteErrorType::ProtocolError,
            "exec stream closed without a status",
        ));
    };
    match (status.status.as_deref(), status.reason.as_deref()) {
        (Some("Success"), _) => Ok(0),
        (Some("Failure"), Some("NonZeroExitCode")) => status
            .details
            .and_then(|details| details.causes)
            .unwrap_or_default()
            .into_iter()
            .find(|cause| cause.reason.as_deref() == Some("ExitCode"))
            .and_then(|cause| cause.message)
            .and_then(|message| message.parse::<u32>().ok())
            .ok_or_else(|| {
                RemoteError::with_message(
                    RemoteErrorType::ProtocolError,
                    "exec failed without an exit code",
                )
            }),
        _ => Err(RemoteError::with_message(
            RemoteErrorType::ProtocolError,
            status.message.unwrap_or_else(|| "exec failed".to_string()),
        )),
    }
}

/// The exec process behind a stream.
pub(crate) struct ExecHandle {
    process: Option<AttachedProcess>,
    status: Option<StatusFuture>,
}

impl ExecHandle {
    /// Take ownership of `process` and its status future.
    ///
    /// The status future must be taken before any I/O: `join` drops the
    /// status receiver, and a command with no output would otherwise race
    /// its own exit status.
    pub(crate) fn new(mut process: AttachedProcess) -> RemoteResult<Self> {
        let status = process.take_status().ok_or_else(|| {
            RemoteError::with_message(RemoteErrorType::ProtocolError, "exec status already taken")
        })?;
        Ok(Self {
            process: Some(process),
            status: Some(Box::pin(status)),
        })
    }

    /// Wait for the process to exit and return its exit code.
    ///
    /// The caller must have dropped every pipe it took from the process
    /// first, otherwise the background task can block on a full buffer.
    pub(crate) async fn wait(mut self) -> RemoteResult<u32> {
        let status = match self.status.take() {
            Some(status) => status.await,
            None => None,
        };
        if let Some(process) = self.process.take() {
            process
                .join()
                .await
                .map_err(|err| RemoteError::with_source(RemoteErrorType::ProtocolError, err))?;
        }
        exit_code(status)
    }

    /// Abort the remote command without reporting an error.
    pub(crate) fn abort(mut self) {
        if let Some(process) = self.process.take() {
            debug!("aborting kube exec process");
            process.abort();
        }
        self.status = None;
    }
}

impl Drop for ExecHandle {
    fn drop(&mut self) {
        if let Some(process) = self.process.take() {
            warn!("kube exec stream dropped without finish; aborting remote command");
            process.abort();
        }
    }
}

/// Owned read stream over the stdout of a container command.
pub(crate) struct KubeReadStream {
    reader: Option<TokioReader>,
    handle: Option<ExecHandle>,
    eof: bool,
}

impl KubeReadStream {
    pub(crate) fn new(reader: TokioReader, handle: Option<ExecHandle>) -> Self {
        Self {
            reader: Some(reader),
            handle,
            eof: false,
        }
    }

    /// A stream that is already at end of file.
    pub(crate) fn empty() -> Self {
        Self {
            reader: None,
            handle: None,
            eof: true,
        }
    }
}

impl AsyncRead for KubeReadStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        if self.eof || buf.is_empty() {
            return Poll::Ready(Ok(0));
        }
        let Some(reader) = self.reader.as_mut() else {
            return Poll::Ready(Ok(0));
        };
        let mut read_buf = ReadBuf::new(buf);
        match Pin::new(reader).poll_read(cx, &mut read_buf) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Err(err)) => Poll::Ready(Err(err)),
            Poll::Ready(Ok(())) => {
                let count = read_buf.filled().len();
                if count == 0 {
                    self.eof = true;
                }
                Poll::Ready(Ok(count))
            }
        }
    }
}

#[remotefs::async_trait]
impl AsyncRemoteRead for KubeReadStream {
    async fn finish(mut self: Box<Self>) -> RemoteResult<()> {
        self.reader = None;
        let Some(handle) = self.handle.take() else {
            return Ok(());
        };
        if !self.eof {
            debug!("read stream finished before EOF; aborting remote command");
            handle.abort();
            return Ok(());
        }
        match handle.wait().await? {
            0 => Ok(()),
            rc => Err(RemoteError::with_message(
                RemoteErrorType::ProtocolError,
                format!("remote read exited with status {rc}"),
            )),
        }
    }
}

/// Metadata applied after a successful write.
pub(crate) struct Finalize {
    pub(crate) exec: KubeExec,
    pub(crate) path: PathBuf,
    pub(crate) mode: Option<UnixPex>,
    pub(crate) modified: Option<SystemTime>,
}

/// Owned write stream over the stdin of a container command.
pub(crate) struct KubeWriteStream {
    writer: Option<TokioWriter>,
    handle: Option<ExecHandle>,
    size_hint: Option<u64>,
    written: u64,
    finalize: Option<Finalize>,
}

impl KubeWriteStream {
    pub(crate) fn new(
        writer: TokioWriter,
        handle: Option<ExecHandle>,
        size_hint: Option<u64>,
        finalize: Option<Finalize>,
    ) -> Self {
        Self {
            writer: Some(writer),
            handle,
            size_hint,
            written: 0,
            finalize,
        }
    }

    fn writer(&mut self) -> io::Result<&mut TokioWriter> {
        self.writer
            .as_mut()
            .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "stream already closed"))
    }
}

impl AsyncWrite for KubeWriteStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let writer = match self.writer() {
            Ok(writer) => writer,
            Err(err) => return Poll::Ready(Err(err)),
        };
        match Pin::new(writer).poll_write(cx, buf) {
            Poll::Ready(Ok(count)) => {
                self.written += count as u64;
                Poll::Ready(Ok(count))
            }
            other => other,
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.writer() {
            Ok(writer) => Pin::new(writer).poll_flush(cx),
            Err(err) => Poll::Ready(Err(err)),
        }
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.writer() {
            Ok(writer) => Pin::new(writer).poll_shutdown(cx),
            Err(err) => Poll::Ready(Err(err)),
        }
    }
}

#[remotefs::async_trait]
impl AsyncRemoteWrite for KubeWriteStream {
    async fn finish(mut self: Box<Self>) -> RemoteResult<()> {
        // Signal EOF explicitly so `cat` stops reading, then release the
        // pipe before waiting for the exit status.
        if let Some(mut writer) = self.writer.take() {
            poll_fn(|cx| Pin::new(&mut writer).poll_shutdown(cx))
                .await
                .map_err(RemoteError::from)?;
        }
        if let Some(handle) = self.handle.take() {
            match handle.wait().await? {
                0 => {}
                rc => {
                    return Err(RemoteError::with_message(
                        RemoteErrorType::FileCreateDenied,
                        format!("remote write exited with status {rc}"),
                    ));
                }
            }
        }
        if let Some(expected) = self.size_hint
            && expected != self.written
        {
            return Err(RemoteError::with_message(
                RemoteErrorType::ProtocolError,
                format!(
                    "written size {} differs from the size hint {expected}",
                    self.written
                ),
            ));
        }
        let Some(finalize) = self.finalize.take() else {
            return Ok(());
        };
        let quoted = path_utils::shell_quote(&finalize.path);
        if let Some(mode) = finalize.mode {
            finalize
                .exec
                .shell_ok(
                    &format!("chmod {:o} {quoted}", u32::from(mode)),
                    RemoteErrorType::StatFailed,
                )
                .await?;
        }
        if let Some(modified) = finalize.modified {
            finalize
                .exec
                .shell_ok(
                    &format!(
                        "touch -m -t {} {quoted}",
                        fmt_utils::fmt_time_utc(modified, "%Y%m%d%H%M.%S")
                    ),
                    RemoteErrorType::StatFailed,
                )
                .await?;
        }
        Ok(())
    }
}

/// Copy at most `limit` bytes (all bytes when `None`) from `src` to `dst`.
pub(crate) async fn copy_limited(
    src: &mut (dyn AsyncRead + Send + Unpin),
    dst: &mut (dyn AsyncWrite + Send + Unpin),
    limit: Option<u64>,
) -> io::Result<u64> {
    let mut buffer = [0_u8; 8192];
    let mut total = 0_u64;
    loop {
        let want = match limit {
            Some(limit) => {
                let remaining = limit - total;
                if remaining == 0 {
                    return Ok(total);
                }
                usize::try_from(remaining).map_or(buffer.len(), |r| r.min(buffer.len()))
            }
            None => buffer.len(),
        };
        let read = poll_fn(|cx| Pin::new(&mut *src).poll_read(cx, &mut buffer[..want])).await;
        let count = match read {
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            result => result?,
        };
        if count == 0 {
            return Ok(total);
        }
        let mut written = 0;
        while written < count {
            let result =
                poll_fn(|cx| Pin::new(&mut *dst).poll_write(cx, &buffer[written..count])).await;
            match result {
                Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
                Ok(size) => written += size,
                Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
                Err(err) => return Err(err),
            }
        }
        total += count as u64;
    }
}

#[cfg(test)]
mod test {

    use std::io::Cursor;

    use futures::io::{AsyncReadExt as _, AsyncWriteExt as _};
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::{StatusCause, StatusDetails};
    use pretty_assertions::assert_eq;

    use super::*;

    fn failure(code: &str) -> Status {
        Status {
            status: Some("Failure".to_string()),
            reason: Some("NonZeroExitCode".to_string()),
            details: Some(StatusDetails {
                causes: Some(vec![StatusCause {
                    reason: Some("ExitCode".to_string()),
                    message: Some(code.to_string()),
                    ..Default::default()
                }]),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn should_map_exit_status() {
        assert_eq!(
            exit_code(Some(Status {
                status: Some("Success".to_string()),
                ..Default::default()
            }))
            .unwrap(),
            0
        );
        assert_eq!(exit_code(Some(failure("3"))).unwrap(), 3);
        assert_eq!(
            exit_code(None).unwrap_err().kind(),
            RemoteErrorType::ProtocolError
        );
        assert_eq!(
            exit_code(Some(Status {
                status: Some("Failure".to_string()),
                message: Some("boom".to_string()),
                ..Default::default()
            }))
            .unwrap_err()
            .to_string(),
            "protocol error (boom)"
        );
    }

    #[tokio::test]
    async fn should_read_until_eof_and_finish() {
        let mut stream = KubeReadStream::new(Box::new(Cursor::new(b"hello".to_vec())), None);
        let mut out = Vec::new();
        stream.read_to_end(&mut out).await.unwrap();
        assert_eq!(out, b"hello");
        assert!(!stream.seekable());
        Box::new(stream).finish().await.unwrap();
    }

    #[tokio::test]
    async fn empty_stream_is_at_eof() {
        let mut stream = KubeReadStream::empty();
        let mut out = Vec::new();
        assert_eq!(stream.read_to_end(&mut out).await.unwrap(), 0);
        Box::new(stream).finish().await.unwrap();
    }

    #[tokio::test]
    async fn should_count_written_bytes_and_verify_size_hint() {
        let mut stream = KubeWriteStream::new(Box::new(Vec::new()), None, Some(5), None);
        stream.write_all(b"hello").await.unwrap();
        stream.flush().await.unwrap();
        assert_eq!(stream.written, 5);
        Box::new(stream).finish().await.unwrap();

        let mut stream = KubeWriteStream::new(Box::new(Vec::new()), None, Some(3), None);
        stream.write_all(b"hello").await.unwrap();
        assert_eq!(
            Box::new(stream).finish().await.unwrap_err().kind(),
            RemoteErrorType::ProtocolError
        );
    }

    #[tokio::test]
    async fn should_copy_limited() {
        let mut src = futures::io::Cursor::new(b"hello world".to_vec());
        let mut dst = futures::io::Cursor::new(Vec::new());
        assert_eq!(copy_limited(&mut src, &mut dst, Some(5)).await.unwrap(), 5);
        assert_eq!(dst.into_inner(), b"hello");

        let mut src = futures::io::Cursor::new(b"hello".to_vec());
        let mut dst = futures::io::Cursor::new(Vec::new());
        assert_eq!(copy_limited(&mut src, &mut dst, None).await.unwrap(), 5);
        assert_eq!(copy_limited(&mut src, &mut dst, Some(0)).await.unwrap(), 0);
    }
}
