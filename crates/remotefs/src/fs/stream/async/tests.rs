use std::future::poll_fn;
use std::io::{self, ErrorKind, SeekFrom};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll, Waker};

use futures_io::{AsyncRead, AsyncSeek, AsyncWrite};

use super::{AsyncReadStream, AsyncRemoteRead, AsyncWriteStream};
use crate::fs::{RemoteError, RemoteErrorType, RemoteResult};
use crate::mock::async_io::{AsyncCursor, PendingRead, PendingSeek, PendingWrite, run};

#[test]
fn async_copy_preserves_data_across_pending_polls() {
    run(async {
        let mut reader = PendingRead::new(b"abcdef".to_vec());
        let mut writer = PendingWrite::new(2);
        let count = crate::io::copy(&mut reader, &mut writer).await.unwrap();
        crate::io::flush(&mut writer).await.unwrap();
        assert_eq!(count, 6);
        assert_eq!(writer.into_inner(), b"abcdef");
    });
}

#[test]
fn async_cursor_can_be_constructed() {
    let cursor = AsyncCursor::new(Vec::new());
    assert!(format!("{cursor:?}").contains("AsyncCursor"));
    assert_eq!(AsyncCursor::new(b"data".to_vec()).into_inner(), b"data");
}

#[test]
fn async_copy_handles_zero_write() {
    run(async {
        let mut reader = AsyncCursor::new(b"data".to_vec());
        let mut writer = ZeroWriter;
        let error = crate::io::copy(&mut reader, &mut writer).await.unwrap_err();
        assert_eq!(error.kind(), ErrorKind::WriteZero);
    });
}

#[test]
fn async_copy_retries_interrupted_operations_and_handles_eof() {
    run(async {
        let mut reader = InterruptedReader::new(b"data".to_vec());
        let mut writer = InterruptedWriter::default();
        let count = crate::io::copy(&mut reader, &mut writer).await.unwrap();
        assert_eq!(count, 4);
        assert_eq!(writer.into_inner(), b"data");

        let mut reader = AsyncCursor::new(Vec::new());
        let mut writer = PendingWrite::new(1);
        assert_eq!(crate::io::copy(&mut reader, &mut writer).await.unwrap(), 0);
    });
}

#[test]
fn async_flush_reports_failure() {
    run(async {
        let mut writer = FailingFlush;
        let error = crate::io::flush(&mut writer).await.unwrap_err();
        assert_eq!(error.kind(), ErrorKind::BrokenPipe);
    });
}

#[test]
fn async_stream_finish_reports_backend_failure() {
    run(async {
        let stream = AsyncReadStream::new(FailingFinishRead);
        let error = stream.finish().await.unwrap_err();
        assert_eq!(error.kind(), RemoteErrorType::IoError);
    });
}

#[test]
fn async_streams_are_send_and_unpin() {
    fn assert_send<T: Send>(value: T) {
        drop(value);
    }

    fn assert_unpin<T: Unpin>() {}

    assert_unpin::<AsyncReadStream>();
    assert_unpin::<AsyncWriteStream>();
    assert_send(AsyncReadStream::new(AsyncCursor::new(Vec::new())));
    assert_send(AsyncWriteStream::new(AsyncCursor::new(Vec::new())));
    assert_send(AsyncReadStream::new(AsyncCursor::new(Vec::new())).finish());
    assert_send(AsyncWriteStream::new(AsyncCursor::new(Vec::new())).finish());
}

#[test]
fn non_seekable_async_stream_reports_unsupported() {
    run(async {
        let mut stream = AsyncReadStream::new(PendingRead::new(Vec::new()));
        assert!(!stream.seekable());
        let error = poll_fn(|context| Pin::new(&mut stream).poll_seek(context, SeekFrom::Start(0)))
            .await
            .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Unsupported);
    });
}

#[test]
fn async_write_stream_seeks_and_forwards_io() {
    run(async {
        let mut stream = AsyncWriteStream::new(AsyncCursor::new(Vec::new()));
        assert!(stream.seekable());
        poll_fn(|context| Pin::new(&mut stream).poll_write(context, b"data"))
            .await
            .unwrap();
        let position =
            poll_fn(|context| Pin::new(&mut stream).poll_seek(context, SeekFrom::Start(1)))
                .await
                .unwrap();
        assert_eq!(position, 1);
        poll_fn(|context| Pin::new(&mut stream).poll_write(context, b"X"))
            .await
            .unwrap();
        stream.finish().await.unwrap();
    });
}

#[test]
fn pending_seek_starts_once_and_finish_recovers_the_backend() {
    let starts = Arc::new(AtomicUsize::new(0));
    let finishes = Arc::new(AtomicUsize::new(0));
    let mut stream = AsyncReadStream::new(PendingSeek::new(starts.clone(), finishes.clone()));
    let waker = Waker::noop();
    let mut context = Context::from_waker(waker);
    let mut seek = Box::pin(poll_fn(|context| {
        Pin::new(&mut stream).poll_seek(context, SeekFrom::Start(2))
    }));
    assert!(matches!(seek.as_mut().poll(&mut context), Poll::Pending));
    assert_eq!(starts.load(Ordering::Acquire), 1);
    drop(seek);
    run(stream.finish()).unwrap();
    assert_eq!(finishes.load(Ordering::Acquire), 1);
}

#[derive(Debug)]
struct ZeroWriter;

impl AsyncWrite for ZeroWriter {
    fn poll_write(
        self: Pin<&mut Self>,
        _context: &mut Context<'_>,
        _buffer: &[u8],
    ) -> Poll<io::Result<usize>> {
        Poll::Ready(Ok(0))
    }

    fn poll_flush(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

#[derive(Debug)]
struct InterruptedReader {
    inner: std::io::Cursor<Vec<u8>>,
    interrupted: bool,
}

impl InterruptedReader {
    fn new(bytes: Vec<u8>) -> Self {
        Self {
            inner: std::io::Cursor::new(bytes),
            interrupted: false,
        }
    }
}

impl AsyncRead for InterruptedReader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _context: &mut Context<'_>,
        buffer: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        if !self.interrupted {
            self.interrupted = true;
            return Poll::Ready(Err(io::ErrorKind::Interrupted.into()));
        }
        Poll::Ready(std::io::Read::read(&mut self.inner, buffer))
    }
}

#[derive(Debug, Default)]
struct InterruptedWriter {
    bytes: Vec<u8>,
    interrupted: bool,
}

impl InterruptedWriter {
    fn into_inner(self) -> Vec<u8> {
        self.bytes
    }
}

impl AsyncWrite for InterruptedWriter {
    fn poll_write(
        mut self: Pin<&mut Self>,
        _context: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<io::Result<usize>> {
        if !self.interrupted {
            self.interrupted = true;
            return Poll::Ready(Err(io::ErrorKind::Interrupted.into()));
        }
        self.bytes.extend_from_slice(buffer);
        Poll::Ready(Ok(buffer.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

#[derive(Debug)]
struct FailingFlush;

impl AsyncWrite for FailingFlush {
    fn poll_write(
        self: Pin<&mut Self>,
        _context: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<io::Result<usize>> {
        Poll::Ready(Ok(buffer.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Err(io::ErrorKind::BrokenPipe.into()))
    }

    fn poll_close(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

#[derive(Debug)]
struct FailingFinishRead;

impl AsyncRead for FailingFinishRead {
    fn poll_read(
        self: Pin<&mut Self>,
        _context: &mut Context<'_>,
        _buffer: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        Poll::Ready(Ok(0))
    }
}

#[async_trait::async_trait]
impl AsyncRemoteRead for FailingFinishRead {
    async fn finish(self: Box<Self>) -> RemoteResult<()> {
        Err(RemoteError::with_message(
            RemoteErrorType::IoError,
            "finish failed",
        ))
    }
}
