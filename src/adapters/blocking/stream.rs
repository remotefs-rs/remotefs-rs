//! Tokio-to-blocking stream fixtures.

use std::future::Future;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::pin::Pin;
use std::sync::mpsc;
use std::task::{Context, Poll, ready};

use futures_io::{AsyncRead, AsyncWrite};
use tokio::runtime::Handle;
use tokio::sync::oneshot;
use tokio_util::compat::Compat;
use tokio_util::io::SyncIoBridge;

use crate::fs::stream::r#async::{AsyncReadStream, AsyncWriteStream};
use crate::fs::{RemoteError, RemoteRead, RemoteResult, RemoteWrite};

type ReadBridge = SyncIoBridge<Compat<AsyncReadStream>>;
type WriteBridge = SyncIoBridge<Compat<AsyncWriteStream>>;

pub(super) struct BlockingRead {
    bridge: ReadBridge,
    handle: Handle,
    seekable: bool,
}

impl BlockingRead {
    pub(super) fn new(stream: AsyncReadStream, handle: &Handle) -> Self {
        let seekable = stream.seekable();
        Self {
            bridge: SyncIoBridge::new_with_handle(stream.into_tokio(), handle.clone()),
            handle: handle.clone(),
            seekable,
        }
    }
}

impl Read for BlockingRead {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.bridge.read(buffer)
    }
}

impl RemoteRead for BlockingRead {
    fn seekable(&self) -> bool {
        self.seekable
    }

    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        if self.seekable {
            self.bridge.seek(position)
        } else {
            Err(io::ErrorKind::Unsupported.into())
        }
    }

    fn finish(self: Box<Self>) -> RemoteResult<()> {
        let Self {
            bridge,
            handle,
            seekable: _,
        } = *self;
        let stream = bridge.into_inner().into_inner();
        handle.block_on(stream.finish())
    }
}

pub(super) struct BlockingWrite {
    bridge: WriteBridge,
    handle: Handle,
    seekable: bool,
}

impl BlockingWrite {
    pub(super) fn new(stream: AsyncWriteStream, handle: &Handle) -> Self {
        let seekable = stream.seekable();
        Self {
            bridge: SyncIoBridge::new_with_handle(stream.into_tokio(), handle.clone()),
            handle: handle.clone(),
            seekable,
        }
    }
}

impl Write for BlockingWrite {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.bridge.write(buffer)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.bridge.flush()
    }

    fn write_vectored(&mut self, buffers: &[io::IoSlice<'_>]) -> io::Result<usize> {
        self.bridge.write_vectored(buffers)
    }
}

impl RemoteWrite for BlockingWrite {
    fn seekable(&self) -> bool {
        self.seekable
    }

    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        if self.seekable {
            self.bridge.seek(position)
        } else {
            Err(io::ErrorKind::Unsupported.into())
        }
    }

    fn finish(self: Box<Self>) -> RemoteResult<()> {
        let Self {
            bridge,
            handle,
            seekable: _,
        } = *self;
        let stream = bridge.into_inner().into_inner();
        handle.block_on(stream.finish())
    }
}

const BORROWED_BUFFER_CAPACITY: usize = 8192;

type ReadRequest = (usize, oneshot::Sender<io::Result<Vec<u8>>>);

pub(super) struct BorrowedRead {
    requests: mpsc::Sender<ReadRequest>,
    pending: Option<oneshot::Receiver<io::Result<Vec<u8>>>>,
    buffered: std::io::Cursor<Vec<u8>>,
}

impl BorrowedRead {
    pub(super) fn with<R>(
        inner: &mut (dyn Read + Send),
        operation: impl FnOnce(&mut Self) -> RemoteResult<R>,
    ) -> RemoteResult<R> {
        std::thread::scope(|scope| {
            let (requests, receiver) = mpsc::channel::<ReadRequest>();
            // One worker owns the borrow for the whole transfer. It runs outside
            // Tokio so a borrowed BlockOn stream can enter its runtime safely.
            std::thread::Builder::new()
                .spawn_scoped(scope, move || {
                    while let Ok((size, response)) = receiver.recv() {
                        let mut buffer = vec![0; size];
                        let result = retry_interrupted(|| inner.read(&mut buffer)).map(|count| {
                            buffer.truncate(count);
                            buffer
                        });
                        if response.send(result).is_err() {
                            break;
                        }
                    }
                })
                .map_err(RemoteError::from)?;
            operation(&mut Self {
                requests,
                pending: None,
                buffered: std::io::Cursor::new(Vec::new()),
            })
        })
    }
}

impl AsyncRead for BorrowedRead {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        if buffer.is_empty() {
            return Poll::Ready(Ok(0));
        }
        let buffered = self.buffered.read(buffer)?;
        if buffered != 0 {
            return Poll::Ready(Ok(buffered));
        }
        if self.pending.is_none() {
            let (response, pending) = oneshot::channel();
            self.requests
                .send((buffer.len().min(BORROWED_BUFFER_CAPACITY), response))
                .map_err(|_| worker_stopped())?;
            self.pending = Some(pending);
        }
        let result =
            ready!(Pin::new(self.pending.as_mut().expect("read response exists")).poll(context));
        self.pending = None;
        let bytes = result.map_err(|_| worker_stopped())??;
        self.buffered = std::io::Cursor::new(bytes);
        Poll::Ready(self.buffered.read(buffer))
    }
}

enum WriteRequest {
    Write(Vec<u8>, oneshot::Sender<io::Result<()>>),
    Flush(oneshot::Sender<io::Result<()>>),
}

pub(super) struct BorrowedWrite {
    requests: mpsc::Sender<WriteRequest>,
    write: Option<oneshot::Receiver<io::Result<()>>>,
    flush: Option<oneshot::Receiver<io::Result<()>>>,
    needs_flush: bool,
}

impl BorrowedWrite {
    pub(super) fn with(
        inner: &mut (dyn Write + Send),
        operation: impl FnOnce(&mut Self) -> RemoteResult<u64>,
    ) -> RemoteResult<u64> {
        std::thread::scope(|scope| {
            let (requests, receiver) = mpsc::channel();
            std::thread::Builder::new()
                .spawn_scoped(scope, move || {
                    while let Ok(request) = receiver.recv() {
                        let sent = match request {
                            WriteRequest::Write(bytes, response) => {
                                response.send(inner.write_all(&bytes)).is_ok()
                            }
                            WriteRequest::Flush(response) => {
                                response.send(retry_interrupted(|| inner.flush())).is_ok()
                            }
                        };
                        if !sent {
                            break;
                        }
                    }
                })
                .map_err(RemoteError::from)?;
            let mut writer = Self {
                requests,
                write: None,
                flush: None,
                needs_flush: true,
            };
            let result = operation(&mut writer);
            let drained = writer.drain().map_err(RemoteError::from);
            crate::fs::stream::complete_transfer(result, drained)
        })
    }

    fn drain(&mut self) -> io::Result<()> {
        // The async backend may return after accepting locally buffered bytes.
        // Check their worker result outside block_on before releasing the borrow.
        if let Some(pending) = self.write.take() {
            pending.blocking_recv().map_err(|_| worker_stopped())??;
        }
        if self.needs_flush && self.flush.is_none() {
            let (response, pending) = oneshot::channel();
            self.requests
                .send(WriteRequest::Flush(response))
                .map_err(|_| worker_stopped())?;
            self.flush = Some(pending);
        }
        if let Some(pending) = self.flush.take() {
            pending.blocking_recv().map_err(|_| worker_stopped())??;
            self.needs_flush = false;
        }
        Ok(())
    }

    fn poll_pending_write(&mut self, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        let result =
            ready!(Pin::new(self.write.as_mut().expect("write response exists")).poll(context));
        self.write = None;
        Poll::Ready(result.map_err(|_| worker_stopped())?)
    }

    fn poll_pending_flush(&mut self, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        let result =
            ready!(Pin::new(self.flush.as_mut().expect("flush response exists")).poll(context));
        self.flush = None;
        let result = result.map_err(|_| worker_stopped())?;
        if result.is_ok() {
            self.needs_flush = false;
        }
        Poll::Ready(result)
    }
}

impl AsyncWrite for BorrowedWrite {
    fn poll_write(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<io::Result<usize>> {
        if self.flush.is_some() {
            ready!(self.poll_pending_flush(context))?;
        }
        if self.write.is_some() {
            ready!(self.poll_pending_write(context))?;
        }
        if buffer.is_empty() {
            return Poll::Ready(Ok(0));
        }
        let (response, pending) = oneshot::channel();
        let count = buffer.len().min(BORROWED_BUFFER_CAPACITY);
        let bytes = buffer[..count].to_vec();
        self.requests
            .send(WriteRequest::Write(bytes, response))
            .map_err(|_| worker_stopped())?;
        self.write = Some(pending);
        self.needs_flush = true;
        // Ownership transfers before acknowledging these bytes. A later Pending
        // write accepts no bytes, so cancelling it cannot replay an earlier write.
        Poll::Ready(Ok(count))
    }

    fn poll_flush(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        if self.write.is_some() {
            ready!(self.poll_pending_write(context))?;
        }
        if self.flush.is_none() {
            let (response, pending) = oneshot::channel();
            self.requests
                .send(WriteRequest::Flush(response))
                .map_err(|_| worker_stopped())?;
            self.flush = Some(pending);
        }
        self.poll_pending_flush(context)
    }

    fn poll_close(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.poll_flush(context)
    }
}

fn worker_stopped() -> io::Error {
    io::Error::other("borrowed I/O worker stopped before responding")
}

fn retry_interrupted<T>(mut operation: impl FnMut() -> io::Result<T>) -> io::Result<T> {
    loop {
        match operation() {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            result => return result,
        }
    }
}
