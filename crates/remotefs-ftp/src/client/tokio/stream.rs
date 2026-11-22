//! Owned remotefs async streams backed by suppaftp tokio transfer streams.
//!
//! [`TokioReadStream`] and [`TokioWriteStream`] wrap a [`TransferStream`] and
//! implement [`AsyncRemoteRead`] and [`AsyncRemoteWrite`]. `finish` drains any
//! unread download tail, closes the data socket and reads the completion reply
//! through [`TransferStream::finish`]. Draining ignores the requested byte
//! budget and can download the rest of the file.
//!
//! Drop cannot await. A dropped download that has not reached EOF marks the
//! control connection unusable, because closing the socket early can produce
//! `426` followed by `226` while suppaftp drains a single deferred reply.
//! Dropped uploads and downloads at EOF rely on that deferred drain instead.

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::{fmt, io};

use futures_io::{AsyncRead, AsyncWrite};
use log::{error, warn};
use remotefs::fs::{AsyncRemoteRead, AsyncRemoteWrite};
use remotefs::{RemoteError, RemoteResult, async_trait};
use suppaftp::FtpResult;
use suppaftp::tokio::{TokioTlsStream, TransferStream};
use tokio::io::{
    AsyncRead as TokioAsyncRead, AsyncReadExt as _, AsyncWrite as TokioAsyncWrite,
    AsyncWriteExt as _, ReadBuf,
};

use super::super::error::ftp_error;
use super::super::guard::TransferGuard;

/// Drains unread download data, closes the socket and reads the reply.
///
/// Draining is skipped when the transfer already reached EOF. An earlier
/// read error can be transient, so cleanup is still attempted; `finalize`
/// always runs so the guard is released only after the reply was consumed.
async fn finish_read<R, F, Fut>(mut inner: R, at_eof: bool, finalize: F) -> RemoteResult<()>
where
    R: tokio::io::AsyncRead + Unpin,
    F: FnOnce(R) -> Fut,
    Fut: Future<Output = FtpResult<()>>,
{
    let drained = if at_eof {
        Ok(())
    } else {
        tokio::io::copy(&mut inner, &mut tokio::io::sink())
            .await
            .map(|_| ())
            .map_err(RemoteError::from)
    };
    let finished = finalize(inner).await.map_err(|err| {
        error!("Failed to finalize read stream: {err}");
        ftp_error(err)
    });
    match (drained, finished) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Err(drain), Err(finish)) => {
            let kind = drain.kind();
            Err(RemoteError::with_source(
                kind,
                CleanupFailure { drain, finish },
            ))
        }
    }
}

/// Combines a local prefix-skip failure with the finalization outcome.
fn aggregate_skip_failure(skip: RemoteError, finished: RemoteResult<()>) -> RemoteError {
    match finished {
        Ok(()) => skip,
        Err(finish) => {
            let kind = skip.kind();
            RemoteError::with_source(kind, SkipCleanupFailure { skip, finish })
        }
    }
}

/// Retains both failures that can occur while closing a read transfer.
#[derive(Debug)]
struct CleanupFailure {
    drain: RemoteError,
    finish: RemoteError,
}

impl fmt::Display for CleanupFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "drain failed: {}; finalization failed: {}",
            self.drain, self.finish
        )
    }
}

impl std::error::Error for CleanupFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.finish)
    }
}

/// Retains a local prefix-skip failure and its transfer-completion failure.
#[derive(Debug)]
struct SkipCleanupFailure {
    skip: RemoteError,
    finish: RemoteError,
}

impl fmt::Display for SkipCleanupFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "prefix skip failed: {}; finalization failed: {}",
            self.skip, self.finish
        )
    }
}

impl std::error::Error for SkipCleanupFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.finish)
    }
}

/// Retains an upload flush failure and its completion-reply failure.
#[derive(Debug)]
struct WriteCleanupFailure {
    flush: RemoteError,
    finish: RemoteError,
}

impl fmt::Display for WriteCleanupFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "flush failed: {}; finalization failed: {}",
            self.flush, self.finish
        )
    }
}

impl std::error::Error for WriteCleanupFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.finish)
    }
}

/// Read side of a `RETR` transfer, optionally limited to a byte budget.
pub(crate) struct TokioReadStream<T>
where
    T: TokioTlsStream + Send + 'static,
{
    /// Taken only by `finish` or drop.
    inner: Option<TransferStream<T>>,
    /// Bytes still allowed to be returned; `None` reads until EOF.
    remaining: Option<u64>,
    /// Whether the data socket reached EOF.
    at_eof: bool,
    /// Dropped after the wrapper's cleanup and the underlying transfer.
    _guard: TransferGuard,
}

impl<T> TokioReadStream<T>
where
    T: TokioTlsStream + Send + 'static,
{
    /// Wraps an open `RETR` stream, returning at most `length` bytes.
    pub(crate) fn new(inner: TransferStream<T>, length: Option<u64>, guard: TransferGuard) -> Self {
        Self {
            inner: Some(inner),
            remaining: length,
            at_eof: false,
            _guard: guard,
        }
    }

    /// Skips a locally handled range prefix without consuming the length budget.
    ///
    /// The caller must release the client mutex before awaiting this method.
    pub(crate) async fn skip_prefix(&mut self, offset: u64) -> RemoteResult<()> {
        let inner = self.inner.as_mut().expect("unfinished read stream");
        let skipped = tokio::io::copy(&mut (&mut *inner).take(offset), &mut tokio::io::sink())
            .await
            .map_err(RemoteError::from)?;
        if skipped < offset {
            self.at_eof = true;
        }
        Ok(())
    }

    /// Finalizes a transfer after prefix skipping failed, retaining both errors.
    pub(crate) async fn finish_after_skip(mut self, skip: RemoteError) -> RemoteError {
        let finished = self.finish_inner().await;
        aggregate_skip_failure(skip, finished)
    }

    /// Finalizes the owned transfer and invalidates the connection on failure.
    async fn finish_inner(&mut self) -> RemoteResult<()> {
        let inner = self.inner.take().expect("unfinished read stream");
        let result = finish_read(inner, self.at_eof, TransferStream::finish).await;
        if result.is_err() {
            self._guard.invalidate();
        }
        result
    }
}

impl<T> AsyncRead for TokioReadStream<T>
where
    T: TokioTlsStream + Send + 'static,
{
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        if buf.is_empty() {
            return Poll::Ready(Ok(0));
        }
        let this = &mut *self;
        let limit = match this.remaining {
            Some(0) => return Poll::Ready(Ok(0)),
            Some(budget) => usize::try_from(budget).unwrap_or(usize::MAX).min(buf.len()),
            None => buf.len(),
        };
        let inner = this.inner.as_mut().expect("unfinished read stream");
        let mut read_buf = ReadBuf::new(&mut buf[..limit]);
        match Pin::new(inner).poll_read(cx, &mut read_buf) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Err(err)) => Poll::Ready(Err(err)),
            Poll::Ready(Ok(())) => {
                let read = read_buf.filled().len();
                if let Some(budget) = this.remaining.as_mut() {
                    *budget -= read as u64;
                }
                if read == 0 {
                    this.at_eof = true;
                }
                Poll::Ready(Ok(read))
            }
        }
    }
}

#[async_trait]
impl<T> AsyncRemoteRead for TokioReadStream<T>
where
    T: TokioTlsStream + Send + 'static,
{
    async fn finish(mut self: Box<Self>) -> RemoteResult<()> {
        self.finish_inner().await
    }
}

impl<T> Drop for TokioReadStream<T>
where
    T: TokioTlsStream + Send + 'static,
{
    fn drop(&mut self) {
        let Some(inner) = self.inner.take() else {
            return;
        };
        if self.at_eof {
            warn!("read stream dropped without finish(); its reply is drained by the next command");
        } else {
            self._guard.invalidate();
            warn!("read stream dropped before EOF; reconnect before the next operation");
        }
        drop(inner);
    }
}

/// Write side of a `STOR` or `APPE` transfer.
pub(crate) struct TokioWriteStream<T>
where
    T: TokioTlsStream + Send + 'static,
{
    inner: Option<TransferStream<T>>,
    guard: TransferGuard,
}

impl<T> TokioWriteStream<T>
where
    T: TokioTlsStream + Send + 'static,
{
    /// Wraps an open `STOR` or `APPE` stream.
    pub(crate) fn new(inner: TransferStream<T>, guard: TransferGuard) -> Self {
        Self {
            inner: Some(inner),
            guard,
        }
    }

    fn inner(&mut self) -> Pin<&mut TransferStream<T>> {
        Pin::new(self.inner.as_mut().expect("unfinished write stream"))
    }
}

impl<T> AsyncWrite for TokioWriteStream<T>
where
    T: TokioTlsStream + Send + 'static,
{
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.inner().poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.inner().poll_flush(cx)
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.inner().poll_shutdown(cx)
    }
}

#[async_trait]
impl<T> AsyncRemoteWrite for TokioWriteStream<T>
where
    T: TokioTlsStream + Send + 'static,
{
    async fn finish(self: Box<Self>) -> RemoteResult<()> {
        let mut this = *self;
        let inner = this.inner.take().expect("unfinished write stream");
        let result = finish_write(inner).await;
        if result.is_err() {
            this.guard.invalidate();
        }
        result
    }
}

impl<T> Drop for TokioWriteStream<T>
where
    T: TokioTlsStream + Send + 'static,
{
    fn drop(&mut self) {
        if let Some(inner) = self.inner.take() {
            warn!(
                "write stream dropped without finish(); its reply is drained by the next command"
            );
            drop(inner);
        }
    }
}

/// Flushes and finalizes an upload, retaining both failures when necessary.
async fn finish_write<T>(mut inner: TransferStream<T>) -> RemoteResult<()>
where
    T: TokioTlsStream + Send + 'static,
{
    let flushed = inner.flush().await.map_err(RemoteError::from);
    let finished = inner.finish().await.map_err(|err| {
        error!("Failed to finalize write stream: {err}");
        ftp_error(err)
    });
    match (flushed, finished) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Err(flush), Err(finish)) => {
            let kind = finish.kind();
            Err(RemoteError::with_source(
                kind,
                WriteCleanupFailure { flush, finish },
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as _;
    use std::io::{BufRead, BufReader, Cursor, Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, mpsc};
    use std::thread;
    use std::time::Duration;

    use futures::AsyncReadExt as _;
    use pretty_assertions::assert_eq;
    use remotefs::RemoteErrorType;
    use suppaftp::tokio::AsyncFtpStream;
    use suppaftp::types::Response;
    use suppaftp::{FtpError, Status};

    use super::*;

    #[tokio::test]
    async fn should_drain_early_and_length_limited_reads_before_finalizing() {
        for at_eof_after in [0usize, 2, 6] {
            let mut inner = Cursor::new(b"abcdef".to_vec());
            let mut scratch = vec![0; at_eof_after];
            tokio::io::AsyncReadExt::read_exact(&mut inner, &mut scratch)
                .await
                .unwrap();
            finish_read(inner, false, |inner| async move {
                assert_eq!(inner.position(), 6);
                Ok(())
            })
            .await
            .unwrap();
        }
    }

    struct Unreadable;

    impl tokio::io::AsyncRead for Unreadable {
        fn poll_read(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            _buf: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            panic!("must not read after EOF");
        }
    }

    #[tokio::test]
    async fn should_not_drain_after_eof() {
        finish_read(Unreadable, true, |_| async { Ok(()) })
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn should_preserve_typed_finalization_errors_including_426() {
        for at_eof in [false, true] {
            let err = finish_read(Cursor::new(b"data".to_vec()), at_eof, |_| async {
                Err(FtpError::UnexpectedResponse(Response {
                    status: Status::TransferAborted,
                    body: b"aborted".to_vec(),
                }))
            })
            .await
            .unwrap_err();
            assert_eq!(err.kind(), RemoteErrorType::ConnectionError);
            assert!(err.source().unwrap().is::<FtpError>());
        }
    }

    struct BrokenReader;

    impl tokio::io::AsyncRead for BrokenReader {
        fn poll_read(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            _buf: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            Poll::Ready(Err(io::Error::new(
                io::ErrorKind::ConnectionReset,
                "drain failed",
            )))
        }
    }

    #[tokio::test]
    async fn should_finalize_after_drain_failure_and_preserve_the_read_error() {
        let finalized = Arc::new(AtomicBool::new(false));
        let seen = Arc::clone(&finalized);
        let err = finish_read(BrokenReader, false, move |_| async move {
            seen.store(true, Ordering::SeqCst);
            Ok(())
        })
        .await
        .unwrap_err();
        assert!(finalized.load(Ordering::SeqCst));
        assert_eq!(err.kind(), RemoteErrorType::ConnectionError);
        assert!(err.to_string().contains("drain failed"));
    }

    #[tokio::test]
    async fn should_preserve_drain_and_finalization_errors_together() {
        let err = finish_read(BrokenReader, false, |_| async {
            Err(FtpError::UnexpectedResponse(Response {
                status: Status::TransferAborted,
                body: b"finalization failed".to_vec(),
            }))
        })
        .await
        .unwrap_err();
        assert_eq!(err.kind(), RemoteErrorType::ConnectionError);
        assert!(err.to_string().contains("drain failed"));
        assert!(err.to_string().contains("finalization failed"));
        assert!(err.source().unwrap().is::<CleanupFailure>());
    }

    #[test]
    fn should_aggregate_skip_and_finalization_failures() {
        let skip =
            RemoteError::with_message(RemoteErrorType::ConnectionError, "prefix skip failed");
        let same = aggregate_skip_failure(
            RemoteError::with_message(RemoteErrorType::ConnectionError, "prefix skip failed"),
            Ok(()),
        );
        assert_eq!(same.to_string(), skip.to_string());
        let both = aggregate_skip_failure(
            skip,
            Err(RemoteError::with_message(
                RemoteErrorType::ProtocolError,
                "finalization failed",
            )),
        );
        assert_eq!(both.kind(), RemoteErrorType::ConnectionError);
        assert!(both.to_string().contains("prefix skip failed"));
        assert!(both.to_string().contains("finalization failed"));
        assert!(both.source().unwrap().is::<SkipCleanupFailure>());
    }

    /// Scripted RETR peer. Returns the server thread; `release` unblocks the
    /// 8 MiB tail so the wrapper must drain it. `expect_noop` makes the server
    /// wait for a follow-up `NOOP`, proving the control channel is reusable.
    fn scripted_retr_server(
        active: Arc<AtomicBool>,
        expect_noop: bool,
    ) -> (
        std::net::SocketAddr,
        mpsc::Sender<()>,
        thread::JoinHandle<()>,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (release, wait) = mpsc::channel();
        let server = thread::spawn(move || {
            let (control, _) = listener.accept().unwrap();
            control
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut control = BufReader::new(control);
            control.get_mut().write_all(b"220 ready\r\n").unwrap();
            let mut command = String::new();
            control.read_line(&mut command).unwrap();
            assert_eq!(command, "PASV\r\n");
            let data_listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let port = data_listener.local_addr().unwrap().port();
            write!(
                control.get_mut(),
                "227 passive (127,0,0,1,{},{})\r\n",
                port / 256,
                port % 256
            )
            .unwrap();
            command.clear();
            control.read_line(&mut command).unwrap();
            assert_eq!(command, "RETR /file\r\n");
            let (mut data, _) = data_listener.accept().unwrap();
            data.set_write_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            control
                .get_mut()
                .write_all(b"150 opening data\r\n")
                .unwrap();
            wait.recv_timeout(Duration::from_secs(5)).unwrap();
            assert!(active.load(Ordering::SeqCst));
            data.write_all(b"ab").unwrap();
            let sent = data.write_all(&vec![b'x'; 8 * 1024 * 1024]);
            drop(data);
            let reply: &[u8] = if sent.is_err() {
                b"426 aborted\r\n226 abort complete\r\n"
            } else {
                b"226 complete\r\n"
            };
            let _ = control.get_mut().write_all(reply);
            if expect_noop {
                command.clear();
                control.read_line(&mut command).unwrap();
                assert_eq!(command, "NOOP\r\n");
                control.get_mut().write_all(b"200 ready\r\n").unwrap();
            }
        });
        (address, release, server)
    }

    async fn open_retr(
        address: std::net::SocketAddr,
        length: Option<u64>,
        active: &Arc<AtomicBool>,
        usable: &Arc<AtomicBool>,
    ) -> (
        AsyncFtpStream,
        TokioReadStream<suppaftp::tokio::AsyncNoTlsStream>,
    ) {
        let mut ftp = AsyncFtpStream::connect(address).await.unwrap();
        let transfer = ftp.retr_as_stream("/file").await.unwrap();
        active.store(true, Ordering::SeqCst);
        let reader = TokioReadStream::new(
            transfer,
            length,
            TransferGuard::new(Arc::clone(active), Arc::clone(usable)),
        );
        (ftp, reader)
    }

    #[tokio::test]
    async fn should_drain_limited_reader_on_finish_and_keep_control_usable() {
        let active = Arc::new(AtomicBool::new(false));
        let usable = Arc::new(AtomicBool::new(true));
        let (address, release, server) = scripted_retr_server(Arc::clone(&active), true);
        let (mut ftp, mut reader) = open_retr(address, Some(2), &active, &usable).await;
        release.send(()).unwrap();
        let mut buf = [0; 4];
        assert_eq!(reader.read(&mut buf).await.unwrap(), 2);
        assert_eq!(reader.read(&mut buf).await.unwrap(), 0);
        Box::new(reader).finish().await.unwrap();
        assert!(!active.load(Ordering::SeqCst));
        assert!(usable.load(Ordering::SeqCst));
        ftp.noop().await.unwrap();
        tokio::task::spawn_blocking(move || server.join().unwrap())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn should_mark_connection_unusable_when_dropped_before_eof() {
        let active = Arc::new(AtomicBool::new(false));
        let usable = Arc::new(AtomicBool::new(true));
        let (address, release, server) = scripted_retr_server(Arc::clone(&active), false);
        let (ftp, mut reader) = open_retr(address, None, &active, &usable).await;
        release.send(()).unwrap();
        let mut buf = [0; 2];
        futures::AsyncReadExt::read_exact(&mut reader, &mut buf)
            .await
            .unwrap();
        drop(reader);
        assert!(!active.load(Ordering::SeqCst));
        assert!(!usable.load(Ordering::SeqCst));
        tokio::task::spawn_blocking(move || server.join().unwrap())
            .await
            .unwrap();
        drop(ftp);
    }

    #[tokio::test]
    async fn should_keep_connection_usable_when_dropped_at_eof() {
        let active = Arc::new(AtomicBool::new(false));
        let usable = Arc::new(AtomicBool::new(true));
        let (address, release, server) = scripted_retr_server(Arc::clone(&active), true);
        let (mut ftp, mut reader) = open_retr(address, None, &active, &usable).await;
        release.send(()).unwrap();
        let mut sink = Vec::new();
        reader.read_to_end(&mut sink).await.unwrap();
        assert_eq!(sink.len(), 2 + 8 * 1024 * 1024);
        drop(reader);
        assert!(!active.load(Ordering::SeqCst));
        assert!(usable.load(Ordering::SeqCst));
        ftp.noop().await.unwrap();
        tokio::task::spawn_blocking(move || server.join().unwrap())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn should_finish_uploads_after_flush() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (control, _) = listener.accept().unwrap();
            let mut control = BufReader::new(control);
            control.get_mut().write_all(b"220 ready\r\n").unwrap();
            let mut command = String::new();
            control.read_line(&mut command).unwrap();
            assert_eq!(command, "PASV\r\n");
            let data_listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let port = data_listener.local_addr().unwrap().port();
            write!(
                control.get_mut(),
                "227 passive (127,0,0,1,{},{})\r\n",
                port / 256,
                port % 256
            )
            .unwrap();
            command.clear();
            control.read_line(&mut command).unwrap();
            assert_eq!(command, "STOR /file\r\n");
            let (mut data, _) = data_listener.accept().unwrap();
            control
                .get_mut()
                .write_all(b"150 opening data\r\n")
                .unwrap();
            let mut received = Vec::new();
            data.read_to_end(&mut received).unwrap();
            assert_eq!(received, b"hello");
            control.get_mut().write_all(b"226 stored\r\n").unwrap();
            command.clear();
            control.read_line(&mut command).unwrap();
            assert_eq!(command, "NOOP\r\n");
            control.get_mut().write_all(b"200 ready\r\n").unwrap();
        });
        let active = Arc::new(AtomicBool::new(true));
        let usable = Arc::new(AtomicBool::new(true));
        let mut ftp = AsyncFtpStream::connect(address).await.unwrap();
        let transfer = ftp.put_with_stream("/file").await.unwrap();
        let mut writer = TokioWriteStream::new(
            transfer,
            TransferGuard::new(Arc::clone(&active), Arc::clone(&usable)),
        );
        futures::AsyncWriteExt::write_all(&mut writer, b"hello")
            .await
            .unwrap();
        Box::new(writer).finish().await.unwrap();
        assert!(!active.load(Ordering::SeqCst));
        assert!(usable.load(Ordering::SeqCst));
        ftp.noop().await.unwrap();
        tokio::task::spawn_blocking(move || server.join().unwrap())
            .await
            .unwrap();
    }
}
