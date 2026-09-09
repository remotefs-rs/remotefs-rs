//! Owned remotefs streams backed by `suppaftp` transfer streams.
//!
//! [`FtpReadStream`] and [`FtpWriteStream`] wrap a [`TransferStream`] and
//! implement [`RemoteRead`] and [`RemoteWrite`]. Read `finish` and drop drain
//! unread data, including after an earlier read error, before closing the data
//! socket and reading the completion reply through [`TransferStream::finish`].
//! Draining ignores the requested byte budget and can block and download the
//! entire remaining file. Drop logs cleanup errors; only `finish` reports them.
//!
//! Cleanup is best effort: if draining fails or the server aborts the transfer,
//! suppaftp consumes only one completion reply, so a follow-up reply can remain
//! queued. The connection is marked unusable after a cleanup failure and must
//! be reconnected before another managed operation.
//! Suppaftp's abort API requires the originating mutable client, which these
//! owned streams cannot access. Writes retain suppaftp's close-and-finalize
//! cleanup; there is no unread upload tail to drain, and reading an upload
//! socket could deadlock with a server waiting for the client to close it.
//!
//! Each stream owns a [`TransferGuard`] that marks the client as busy for the
//! lifetime of the transfer, so the client can refuse control commands until
//! the stream is finished or dropped.

use std::fmt;
use std::io::{self, Read, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use remotefs::fs::{RemoteRead, RemoteWrite};
use remotefs::{RemoteError, RemoteResult};
use suppaftp::{FtpResult, TlsStream, TransferStream};

use super::error::ftp_error;

/// Marks a client as having an in-flight transfer until dropped.
pub(crate) struct TransferGuard {
    active: Arc<AtomicBool>,
    connection_usable: Arc<AtomicBool>,
}

impl TransferGuard {
    /// Takes ownership of an already-raised `active` flag.
    pub(crate) fn new(active: Arc<AtomicBool>, connection_usable: Arc<AtomicBool>) -> Self {
        Self {
            active,
            connection_usable,
        }
    }

    /// Marks the control connection unusable until the client reconnects.
    pub(crate) fn invalidate(&self) {
        self.connection_usable.store(false, Ordering::SeqCst);
    }
}

impl Drop for TransferGuard {
    fn drop(&mut self) {
        self.active.store(false, Ordering::SeqCst);
    }
}

/// Reads up to `remaining` bytes from `inner`; `None` means no limit.
///
/// Returns `Ok(0)` without touching `inner` once the budget is exhausted.
pub(crate) fn read_limited(
    inner: &mut impl Read,
    remaining: &mut Option<u64>,
    buf: &mut [u8],
) -> io::Result<usize> {
    let Some(budget) = *remaining else {
        return inner.read(buf);
    };
    if budget == 0 {
        return Ok(0);
    }
    let limit = usize::try_from(budget).unwrap_or(usize::MAX).min(buf.len());
    let read = inner.read(&mut buf[..limit])?;
    *remaining = Some(budget - read as u64);
    Ok(read)
}

/// Drains unread data before closing the socket and reading the reply.
fn finish_read<R>(
    mut inner: R,
    at_eof: bool,
    finalize: impl FnOnce(R) -> FtpResult<()>,
) -> RemoteResult<()>
where
    R: Read,
{
    // Closing a healthy data socket early can produce 426 followed by 226;
    // suppaftp consumes only one reply. Drain to EOF to complete RETR normally.
    // An earlier read error can be transient; still attempt cleanup. Stop on
    // another error rather than repeatedly retrying a broken connection.
    let drained = if !at_eof {
        io::copy(&mut inner, &mut io::sink())
            .map(|_| ())
            .map_err(remotefs::RemoteError::from)
    } else {
        Ok(())
    };
    // Always finalize, even after draining fails, before releasing the guard.
    let finished = finalize(inner).map_err(|err| {
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

/// Read side of a `RETR` transfer, optionally limited to a byte budget.
pub(crate) struct FtpReadStream<T>
where
    T: TlsStream + Send + 'static,
{
    /// Taken only by consuming finish or drop, before releasing the guard.
    inner: Option<TransferStream<T>>,
    /// Bytes still allowed to be returned; `None` reads until EOF.
    remaining: Option<u64>,
    /// Whether the data socket reached EOF, i.e. the server sent everything.
    at_eof: bool,
    /// Dropped after the wrapper's cleanup and the underlying transfer.
    _guard: TransferGuard,
}

impl<T> FtpReadStream<T>
where
    T: TlsStream + Send + 'static,
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

    /// Skips a locally handled range prefix without consuming the requested length budget.
    ///
    /// The caller must release the client mutex before invoking this method.
    pub(crate) fn skip_prefix(&mut self, offset: u64) -> RemoteResult<()> {
        let skipped = io::copy(
            &mut Read::by_ref(self.inner.as_mut().expect("unfinished read stream")).take(offset),
            &mut io::sink(),
        )
        .map_err(remotefs::RemoteError::from)?;
        if skipped < offset {
            self.at_eof = true;
        }
        Ok(())
    }

    /// Finalizes a transfer after prefix skipping failed, retaining both errors.
    pub(crate) fn finish_after_skip(mut self, skip: RemoteError) -> RemoteError {
        let finished = self.finish_inner();
        match finished {
            Ok(()) => skip,
            Err(finish) => {
                let kind = skip.kind();
                RemoteError::with_source(kind, SkipCleanupFailure { skip, finish })
            }
        }
    }

    /// Finalizes the owned transfer and invalidates the connection on failure.
    fn finish_inner(&mut self) -> RemoteResult<()> {
        let inner = self.inner.take().expect("unfinished read stream");
        let result = finish_read(inner, self.at_eof, TransferStream::finish);
        if result.is_err() {
            self._guard.invalidate();
        }
        result
    }
}

impl<T> Read for FtpReadStream<T>
where
    T: TlsStream + Send + 'static,
{
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let read = read_limited(
            self.inner.as_mut().expect("unfinished read stream"),
            &mut self.remaining,
            buf,
        )?;
        if read == 0 && self.remaining != Some(0) {
            self.at_eof = true;
        }
        Ok(read)
    }
}

impl<T> RemoteRead for FtpReadStream<T>
where
    T: TlsStream + Send + 'static,
{
    fn finish(mut self: Box<Self>) -> RemoteResult<()> {
        // Drop sees None, so it cannot drain or consume a completion reply twice.
        // The guard is released when self drops, after finalization returns.
        self.finish_inner()
    }
}

impl<T> Drop for FtpReadStream<T>
where
    T: TlsStream + Send + 'static,
{
    fn drop(&mut self) {
        if let Some(inner) = self.inner.take()
            && let Err(err) = finish_read(inner, self.at_eof, TransferStream::finish)
        {
            self._guard.invalidate();
            warn!("Failed to clean up abandoned read stream: {err}");
        }
    }
}

/// Write side of a `STOR` or `APPE` transfer.
pub(crate) struct FtpWriteStream<T>
where
    T: TlsStream + Send + 'static,
{
    inner: Option<TransferStream<T>>,
    guard: TransferGuard,
}

impl<T> FtpWriteStream<T>
where
    T: TlsStream + Send + 'static,
{
    /// Wraps an open `STOR` or `APPE` stream.
    pub(crate) fn new(inner: TransferStream<T>, guard: TransferGuard) -> Self {
        Self {
            inner: Some(inner),
            guard,
        }
    }
}

impl<T> Write for FtpWriteStream<T>
where
    T: TlsStream + Send + 'static,
{
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner
            .as_mut()
            .expect("unfinished write stream")
            .write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner
            .as_mut()
            .expect("unfinished write stream")
            .flush()
    }
}

impl<T> RemoteWrite for FtpWriteStream<T>
where
    T: TlsStream + Send + 'static,
{
    fn finish(self: Box<Self>) -> RemoteResult<()> {
        let mut this = *self;
        let inner = this.inner.take().expect("unfinished write stream");
        let result = finish_write(inner);
        if result.is_err() {
            this.guard.invalidate();
        }
        result
    }
}

impl<T> Drop for FtpWriteStream<T>
where
    T: TlsStream + Send + 'static,
{
    fn drop(&mut self) {
        if let Some(inner) = self.inner.take()
            && let Err(err) = finish_write(inner)
        {
            self.guard.invalidate();
            warn!("Failed to clean up abandoned write stream: {err}");
        }
    }
}

/// Flushes and finalizes an upload, retaining both failures when necessary.
fn finish_write<T>(mut inner: TransferStream<T>) -> RemoteResult<()>
where
    T: TlsStream + Send + 'static,
{
    let flushed = inner.flush().map_err(RemoteError::from);
    let finished = inner.finish().map_err(|err| {
        error!("Failed to finalize write stream: {err}");
        ftp_error(err)
    });
    match (flushed, finished) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(flush), Ok(())) | (Ok(()), Err(flush)) => Err(flush),
        (Err(flush), Err(finish)) => {
            let kind = finish.kind();
            Err(RemoteError::with_source(
                kind,
                WriteCleanupFailure { flush, finish },
            ))
        }
    }
}

/// Retains both an upload flush failure and its completion-reply failure.
#[derive(Debug)]
struct WriteCleanupFailure {
    flush: RemoteError,
    finish: RemoteError,
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

#[cfg(test)]
mod tests {
    use std::error::Error as _;
    use std::io::{BufRead, BufReader, Cursor};
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    use pretty_assertions::assert_eq;
    use remotefs::RemoteErrorType;
    use suppaftp::types::Response;
    use suppaftp::{FtpError, Status};

    use super::*;

    // Exercise the actual wrapper and suppaftp finalizer against a scripted peer.
    // An unread tail larger than the socket buffers exposes premature closure.
    fn check_read_cleanup(
        length: Option<u64>,
        fail_read: bool,
        skip_prefix: bool,
        finish: bool,
        finish_after_skip: bool,
        fail_completion: bool,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let active = Arc::new(AtomicBool::new(true));
        let server_active = Arc::clone(&active);
        let (send_tail, receive_tail) = mpsc::channel();
        let server = thread::spawn(move || {
            let (control, _) = listener.accept().unwrap();
            control
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            control
                .set_write_timeout(Some(Duration::from_secs(5)))
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
                "227 passive (127,0,0,1,{high},{low})\r\n",
                high = port / 256,
                low = port % 256
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
            receive_tail.recv_timeout(Duration::from_secs(5)).unwrap();
            assert!(server_active.load(Ordering::SeqCst));
            let sent = data.write_all(&vec![b'x'; 8 * 1024 * 1024]);
            drop(data);
            // The wrapper must remain busy through data cleanup and the reply.
            assert!(server_active.load(Ordering::SeqCst));
            let reply: &[u8] = if fail_completion || sent.is_err() {
                b"426 aborted\r\n226 abort complete\r\n"
            } else {
                b"226 complete\r\n"
            };
            control.get_mut().write_all(reply).unwrap();
            command.clear();
            control.read_line(&mut command).unwrap();
            assert_eq!(command, "NOOP\r\n");
            control.get_mut().write_all(b"200 ready\r\n").unwrap();
            sent
        });

        let mut ftp = suppaftp::FtpStream::connect(address).unwrap();
        ftp.get_ref()
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let transfer = ftp.retr_as_stream("/file").unwrap();
        let socket = transfer.get_ref().get_ref().try_clone().unwrap();
        let usable = Arc::new(AtomicBool::new(true));
        let mut reader = FtpReadStream::new(
            transfer,
            length,
            TransferGuard::new(Arc::clone(&active), Arc::clone(&usable)),
        );
        let mut skip_error = None;
        if fail_read {
            socket.set_nonblocking(true).unwrap();
            if skip_prefix {
                let err = reader.skip_prefix(1).unwrap_err();
                assert!(err.source().unwrap().is::<io::Error>());
                if finish_after_skip {
                    skip_error = Some(err);
                }
            } else {
                assert_eq!(
                    reader.read(&mut [0; 1]).unwrap_err().kind(),
                    io::ErrorKind::WouldBlock
                );
            }
            socket.set_nonblocking(false).unwrap();
        }
        // Do not keep a cloned data socket alive during finalization.
        drop(socket);
        send_tail.send(()).unwrap();
        if !fail_read && length == Some(0) {
            assert_eq!(reader.read(&mut [0; 2]).unwrap(), 0);
        } else if !fail_read {
            reader.read_exact(&mut [0; 2]).unwrap();
        }
        let result = if finish_after_skip {
            Err(reader.finish_after_skip(skip_error.expect("prefix failure is required")))
        } else if finish {
            Box::new(reader).finish()
        } else {
            drop(reader);
            Ok(())
        };
        assert!(!active.load(Ordering::SeqCst));
        let reused = ftp.noop();
        let sent = server.join().unwrap();
        if finish_after_skip {
            let error = result.unwrap_err();
            assert!(error.to_string().contains("prefix skip failed"));
            assert!(error.to_string().contains("finalization failed"));
            assert!(error.source().unwrap().is::<SkipCleanupFailure>());
            assert!(reused.is_err());
        } else {
            result.unwrap();
            reused.unwrap();
        }
        sent.unwrap();
    }

    #[test]
    fn should_drain_abandoned_and_length_limited_readers_on_drop() {
        for length in [None, Some(0), Some(2)] {
            check_read_cleanup(length, false, false, false, false, false);
        }
    }

    #[test]
    fn should_drain_after_read_or_prefix_failure_on_drop() {
        for skip_prefix in [false, true] {
            check_read_cleanup(None, true, skip_prefix, false, false, false);
        }
    }

    #[test]
    fn should_drain_after_read_failure_on_finish() {
        check_read_cleanup(None, true, false, true, false, false);
    }

    #[test]
    fn should_finish_limited_reads_without_finalizing_twice() {
        check_read_cleanup(Some(2), false, false, true, false, false);
    }

    #[test]
    fn should_aggregate_prefix_skip_and_completion_failures() {
        check_read_cleanup(None, true, true, false, true, true);
    }

    #[test]
    fn should_read_everything_without_limit() {
        let mut inner = Cursor::new(b"abcdef".to_vec());
        let mut remaining = None;
        let mut buf = [0u8; 16];
        assert_eq!(
            read_limited(&mut inner, &mut remaining, &mut buf).unwrap(),
            6
        );
        assert_eq!(&buf[..6], b"abcdef");
        assert_eq!(
            read_limited(&mut inner, &mut remaining, &mut buf).unwrap(),
            0
        );
    }

    #[test]
    fn should_stop_at_the_budget() {
        let mut inner = Cursor::new(b"abcdef".to_vec());
        let mut remaining = Some(4);
        let mut buf = [0u8; 3];
        assert_eq!(
            read_limited(&mut inner, &mut remaining, &mut buf).unwrap(),
            3
        );
        assert_eq!(&buf, b"abc");
        assert_eq!(remaining, Some(1));
        assert_eq!(
            read_limited(&mut inner, &mut remaining, &mut buf).unwrap(),
            1
        );
        assert_eq!(&buf[..1], b"d");
        assert_eq!(remaining, Some(0));
        assert_eq!(
            read_limited(&mut inner, &mut remaining, &mut buf).unwrap(),
            0
        );
        // the inner reader was not consumed past the budget
        assert_eq!(inner.position(), 4);
    }

    #[test]
    fn should_return_nothing_for_zero_budget() {
        let mut inner = Cursor::new(b"abcdef".to_vec());
        let mut remaining = Some(0);
        let mut buf = [0u8; 3];
        assert_eq!(
            read_limited(&mut inner, &mut remaining, &mut buf).unwrap(),
            0
        );
        assert_eq!(inner.position(), 0);
    }

    #[test]
    fn should_release_the_transfer_flag_on_drop() {
        let active = Arc::new(AtomicBool::new(true));
        let guard = TransferGuard::new(Arc::clone(&active), Arc::new(AtomicBool::new(true)));
        assert!(active.load(Ordering::SeqCst));
        drop(guard);
        assert!(!active.load(Ordering::SeqCst));
    }

    #[test]
    fn should_mark_the_connection_unusable_when_cleanup_fails() {
        let active = Arc::new(AtomicBool::new(true));
        let usable = Arc::new(AtomicBool::new(true));
        let guard = TransferGuard::new(Arc::clone(&active), Arc::clone(&usable));

        guard.invalidate();

        assert!(!usable.load(Ordering::SeqCst));
        drop(guard);
        assert!(!active.load(Ordering::SeqCst));
    }

    #[test]
    fn should_drain_early_and_length_limited_reads_before_finalizing() {
        for length in [None, Some(0), Some(2), Some(6)] {
            let mut inner = Cursor::new(b"abcdef".to_vec());
            let mut remaining = length;
            read_limited(&mut inner, &mut remaining, &mut [0; 2]).unwrap();
            finish_read(inner, false, |inner| {
                assert_eq!(inner.position(), 6);
                Ok(())
            })
            .unwrap();
        }
    }

    struct Unreadable;

    impl Read for Unreadable {
        fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
            panic!("must not read after EOF");
        }
    }

    #[test]
    fn should_not_drain_after_eof() {
        finish_read(Unreadable, true, |_| Ok(())).unwrap();
    }

    #[test]
    fn should_preserve_typed_finalization_errors_including_426() {
        for at_eof in [false, true] {
            let err = finish_read(Cursor::new(b"data"), at_eof, |_| {
                Err(FtpError::UnexpectedResponse(Response {
                    status: Status::TransferAborted,
                    body: b"aborted".to_vec(),
                }))
            })
            .unwrap_err();
            assert_eq!(err.kind(), RemoteErrorType::ConnectionError);
            assert!(err.source().unwrap().is::<FtpError>());
        }
    }

    struct BrokenReader;

    impl Read for BrokenReader {
        fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::new(
                io::ErrorKind::ConnectionReset,
                "drain failed",
            ))
        }
    }

    #[test]
    fn should_finalize_after_drain_failure_and_preserve_the_read_error() {
        let mut finalized = false;
        let err = finish_read(BrokenReader, false, |_| {
            finalized = true;
            Ok(())
        })
        .unwrap_err();
        assert!(finalized);
        assert_eq!(err.kind(), RemoteErrorType::ConnectionError);
        assert!(err.source().unwrap().is::<io::Error>());
        assert!(err.to_string().contains("drain failed"));
    }

    #[test]
    fn should_preserve_drain_and_finalization_errors_together() {
        let err = finish_read(BrokenReader, false, |_| {
            Err(FtpError::UnexpectedResponse(Response {
                status: Status::TransferAborted,
                body: b"finalization failed".to_vec(),
            }))
        })
        .unwrap_err();

        assert_eq!(err.kind(), RemoteErrorType::ConnectionError);
        assert!(err.to_string().contains("drain failed"));
        assert!(err.to_string().contains("finalization failed"));
    }
}
