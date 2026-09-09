//! Tokio-to-blocking stream fixtures.

use std::io::{self, Read, Seek, SeekFrom, Write};
use std::pin::Pin;
use std::task::{Context, Poll};

use futures_io::{AsyncRead, AsyncWrite};
use tokio::runtime::Handle;
use tokio_util::compat::Compat;
use tokio_util::io::SyncIoBridge;

use crate::fs::stream::r#async::{AsyncReadStream, AsyncWriteStream};
use crate::fs::{RemoteRead, RemoteResult, RemoteWrite};

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

pub(super) struct BorrowedRead<'a>(&'a mut (dyn Read + Send));

impl<'a> BorrowedRead<'a> {
    pub(super) fn new(inner: &'a mut (dyn Read + Send)) -> Self {
        Self(inner)
    }
}

impl AsyncRead for BorrowedRead<'_> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _context: &mut Context<'_>,
        buffer: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        let source = &mut *self.0;
        let result = std::thread::scope(|scope| {
            scope
                .spawn(|| source.read(buffer))
                .join()
                .expect("borrowed reader worker panicked")
        });
        Poll::Ready(result)
    }
}

pub(super) struct BorrowedWrite<'a>(&'a mut (dyn Write + Send));

impl<'a> BorrowedWrite<'a> {
    pub(super) fn new(inner: &'a mut (dyn Write + Send)) -> Self {
        Self(inner)
    }
}

impl AsyncWrite for BorrowedWrite<'_> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        _context: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<io::Result<usize>> {
        let target = &mut *self.0;
        let result = std::thread::scope(|scope| {
            scope
                .spawn(|| target.write(buffer))
                .join()
                .expect("borrowed writer worker panicked")
        });
        Poll::Ready(result)
    }

    fn poll_flush(mut self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<io::Result<()>> {
        let target = &mut *self.0;
        let result = std::thread::scope(|scope| {
            scope
                .spawn(|| target.flush())
                .join()
                .expect("borrowed writer worker panicked")
        });
        Poll::Ready(result)
    }

    fn poll_close(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.poll_flush(_context)
    }
}
