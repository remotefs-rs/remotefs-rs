//! Asynchronous I/O fixtures for stream and contract tests.

use std::future::Future;
use std::io::{self, Cursor, Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll, Wake, Waker};
use std::thread;

use futures_io::{AsyncRead, AsyncSeek, AsyncWrite};

use super::{Entry, State};
use crate::fs::stream::r#async::{AsyncRemoteRead, AsyncRemoteWrite};
use crate::fs::{FileType, Metadata, RemoteError, RemoteResult};

struct ThreadWaker(thread::Thread);

impl Wake for ThreadWaker {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.unpark();
    }
}

/// Runs a future to completion on the current thread.
pub(crate) fn run<F>(future: F) -> F::Output
where
    F: Future,
{
    let waker = Waker::from(Arc::new(ThreadWaker(thread::current())));
    let mut context = Context::from_waker(&waker);
    let mut future = Box::pin(future);
    loop {
        match Future::poll(Pin::as_mut(&mut future), &mut context) {
            Poll::Ready(output) => return output,
            Poll::Pending => thread::park(),
        }
    }
}

/// An immediately-ready asynchronous cursor.
#[derive(Debug)]
pub(crate) struct AsyncCursor {
    inner: Cursor<Vec<u8>>,
}

impl AsyncCursor {
    pub(crate) fn new(bytes: Vec<u8>) -> Self {
        Self {
            inner: Cursor::new(bytes),
        }
    }

    pub(crate) fn into_inner(self) -> Vec<u8> {
        self.inner.into_inner()
    }
}

impl AsyncRead for AsyncCursor {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _context: &mut Context<'_>,
        buffer: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        Poll::Ready(self.inner.read(buffer))
    }
}

impl AsyncWrite for AsyncCursor {
    fn poll_write(
        mut self: Pin<&mut Self>,
        _context: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<io::Result<usize>> {
        Poll::Ready(self.inner.write(buffer))
    }

    fn poll_flush(mut self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(self.inner.flush())
    }

    fn poll_close(mut self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(self.inner.flush())
    }
}

impl AsyncSeek for AsyncCursor {
    fn poll_seek(
        mut self: Pin<&mut Self>,
        _context: &mut Context<'_>,
        position: SeekFrom,
    ) -> Poll<io::Result<u64>> {
        Poll::Ready(self.inner.seek(position))
    }
}

#[async_trait::async_trait]
impl AsyncRemoteRead for AsyncCursor {
    fn seekable(&self) -> bool {
        true
    }

    async fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.inner.seek(position)
    }
}

#[async_trait::async_trait]
impl AsyncRemoteWrite for AsyncCursor {
    fn seekable(&self) -> bool {
        true
    }

    async fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.inner.seek(position)
    }
}

/// A reader that yields once before each read operation.
#[derive(Debug)]
pub(crate) struct PendingRead {
    inner: Cursor<Vec<u8>>,
    pending: bool,
}

impl PendingRead {
    pub(crate) fn new(bytes: Vec<u8>) -> Self {
        Self {
            inner: Cursor::new(bytes),
            pending: false,
        }
    }
}

impl AsyncRead for PendingRead {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        if !self.pending {
            self.pending = true;
            context.waker().wake_by_ref();
            return Poll::Pending;
        }
        self.pending = false;
        Poll::Ready(self.inner.read(buffer))
    }
}

impl AsyncRemoteRead for PendingRead {}

/// A reader whose first seek poll is pending and which records seek starts.
#[derive(Debug)]
pub(crate) struct PendingSeek {
    inner: Cursor<Vec<u8>>,
    starts: Arc<AtomicUsize>,
    finishes: Arc<AtomicUsize>,
    pending: bool,
}

impl PendingSeek {
    pub(crate) fn new(starts: Arc<AtomicUsize>, finishes: Arc<AtomicUsize>) -> Self {
        Self {
            inner: Cursor::new(Vec::new()),
            starts,
            finishes,
            pending: false,
        }
    }
}

impl AsyncRead for PendingSeek {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _context: &mut Context<'_>,
        buffer: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        Poll::Ready(self.inner.read(buffer))
    }
}

#[async_trait::async_trait]
impl AsyncRemoteRead for PendingSeek {
    fn seekable(&self) -> bool {
        true
    }

    async fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.starts.fetch_add(1, Ordering::AcqRel);
        if !self.pending {
            self.pending = true;
            let mut pending = true;
            std::future::poll_fn(|context| {
                if pending {
                    pending = false;
                    context.waker().wake_by_ref();
                    Poll::Pending
                } else {
                    Poll::Ready(())
                }
            })
            .await;
            self.pending = false;
        }
        self.inner.seek(position)
    }

    async fn finish(self: Box<Self>) -> RemoteResult<()> {
        self.finishes.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }
}

/// A writer that yields once before each write and limits its write size.
#[derive(Debug)]
pub(crate) struct PendingWrite {
    inner: Vec<u8>,
    max_write: usize,
    pending: bool,
}

impl PendingWrite {
    pub(crate) fn new(max_write: usize) -> Self {
        assert!(max_write > 0, "max_write must be positive");
        Self {
            inner: Vec::new(),
            max_write,
            pending: false,
        }
    }

    pub(crate) fn into_inner(self) -> Vec<u8> {
        self.inner
    }
}

impl AsyncWrite for PendingWrite {
    fn poll_write(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<io::Result<usize>> {
        if !self.pending {
            self.pending = true;
            context.waker().wake_by_ref();
            return Poll::Pending;
        }
        self.pending = false;
        let count = buffer.len().min(self.max_write);
        self.inner.extend_from_slice(&buffer[..count]);
        Poll::Ready(Ok(count))
    }

    fn poll_flush(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

impl AsyncRemoteWrite for PendingWrite {}

/// An asynchronous reader backed by staged fixture bytes.
pub(crate) struct AsyncMockReader {
    state: Arc<std::sync::Mutex<State>>,
    cursor: Cursor<Vec<u8>>,
    seekable: bool,
    finished: bool,
}

impl AsyncMockReader {
    pub(crate) fn new(state: Arc<std::sync::Mutex<State>>, bytes: Vec<u8>, seekable: bool) -> Self {
        Self {
            state,
            cursor: Cursor::new(bytes),
            seekable,
            finished: false,
        }
    }
}

impl AsyncRead for AsyncMockReader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _context: &mut Context<'_>,
        buffer: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        Poll::Ready(self.cursor.read(buffer))
    }
}

#[async_trait::async_trait]
impl AsyncRemoteRead for AsyncMockReader {
    fn seekable(&self) -> bool {
        self.seekable
    }

    async fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        if self.seekable {
            Seek::seek(&mut self.cursor, position)
        } else {
            Err(io::ErrorKind::Unsupported.into())
        }
    }

    async fn finish(mut self: Box<Self>) -> RemoteResult<()> {
        self.finished = true;
        let mut state = self.state.lock().expect("mock state lock poisoned");
        state.finish_count += 1;
        match state.fail_finish {
            Some(kind) => Err(RemoteError::new(kind)),
            None => Ok(()),
        }
    }
}

impl Drop for AsyncMockReader {
    fn drop(&mut self) {
        if !self.finished
            && let Ok(mut state) = self.state.lock()
        {
            state.unfinished_count += 1;
        }
    }
}

/// An asynchronous writer that commits staged bytes when finished.
pub(crate) struct AsyncMockWriter {
    state: Arc<std::sync::Mutex<State>>,
    path: PathBuf,
    cursor: Cursor<Vec<u8>>,
    metadata: Metadata,
    seekable: bool,
    finished: bool,
}

impl AsyncMockWriter {
    pub(crate) fn new(
        state: Arc<std::sync::Mutex<State>>,
        path: PathBuf,
        bytes: Vec<u8>,
        metadata: Metadata,
        seekable: bool,
    ) -> Self {
        Self {
            state,
            path,
            cursor: Cursor::new(bytes),
            metadata,
            seekable,
            finished: false,
        }
    }

    pub(crate) fn seek_to_end(&mut self) {
        Seek::seek(&mut self.cursor, SeekFrom::End(0)).expect("cursor end is always seekable");
    }
}

impl AsyncWrite for AsyncMockWriter {
    fn poll_write(
        mut self: Pin<&mut Self>,
        _context: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<io::Result<usize>> {
        Poll::Ready(self.cursor.write(buffer))
    }

    fn poll_flush(mut self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(self.cursor.flush())
    }

    fn poll_close(mut self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(self.cursor.flush())
    }
}

#[async_trait::async_trait]
impl AsyncRemoteWrite for AsyncMockWriter {
    fn seekable(&self) -> bool {
        self.seekable
    }

    async fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        if self.seekable {
            Seek::seek(&mut self.cursor, position)
        } else {
            Err(io::ErrorKind::Unsupported.into())
        }
    }

    async fn finish(mut self: Box<Self>) -> RemoteResult<()> {
        self.finished = true;
        let mut state = self.state.lock().expect("mock state lock poisoned");
        state.finish_count += 1;
        if let Some(kind) = state.fail_finish {
            return Err(RemoteError::new(kind));
        }
        let mut metadata = std::mem::take(&mut self.metadata);
        metadata.file_type = FileType::File;
        metadata.size = Some(self.cursor.get_ref().len() as u64);
        metadata.symlink = None;
        state.entries.insert(
            self.path.clone(),
            Entry {
                metadata,
                bytes: std::mem::take(self.cursor.get_mut()),
            },
        );
        Ok(())
    }
}

impl Drop for AsyncMockWriter {
    fn drop(&mut self) {
        if !self.finished
            && let Ok(mut state) = self.state.lock()
        {
            state.unfinished_count += 1;
        }
    }
}
