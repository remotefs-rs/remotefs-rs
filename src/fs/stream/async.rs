//! Runtime-neutral asynchronous streams for remote transfers.

use std::fmt;
use std::future::Future;
use std::io::{self, SeekFrom};
use std::pin::Pin;
use std::task::{Context, Poll};

use futures_io::{AsyncRead, AsyncSeek, AsyncWrite};

use crate::fs::{RemoteError, RemoteResult};

type Reader = Box<dyn AsyncRemoteRead>;
type Writer = Box<dyn AsyncRemoteWrite>;
type ReadSeekFuture = Pin<Box<dyn Future<Output = (Reader, io::Result<u64>)> + Send>>;
type WriteSeekFuture = Pin<Box<dyn Future<Output = (Writer, io::Result<u64>)> + Send>>;

/// A remote asynchronous reader with optional seeking and finalization.
#[async_trait::async_trait]
pub trait AsyncRemoteRead: AsyncRead + Send + Unpin {
    /// Returns whether this reader supports seeking.
    fn seekable(&self) -> bool {
        false
    }

    /// Seeks to a position in the remote stream.
    async fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        let _ = position;
        Err(io::ErrorKind::Unsupported.into())
    }

    /// Finalizes the remote read operation.
    async fn finish(self: Box<Self>) -> RemoteResult<()> {
        Ok(())
    }
}

/// A remote asynchronous writer with optional seeking and finalization.
#[async_trait::async_trait]
pub trait AsyncRemoteWrite: AsyncWrite + Send + Unpin {
    /// Returns whether this writer supports seeking.
    fn seekable(&self) -> bool {
        false
    }

    /// Seeks to a position in the remote stream.
    async fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        let _ = position;
        Err(io::ErrorKind::Unsupported.into())
    }

    /// Finalizes the remote write operation.
    async fn finish(self: Box<Self>) -> RemoteResult<()> {
        Ok(())
    }
}

enum ReadState {
    Ready(Reader),
    Seeking(ReadSeekFuture),
    Transitioning,
}

/// An owned asynchronous remote reader with an explicit finalizer.
#[non_exhaustive]
#[must_use = "call finish() so the backend can complete the transfer"]
pub struct AsyncReadStream {
    state: ReadState,
    seekable: bool,
}

impl AsyncReadStream {
    /// Wraps a backend reader in an owned asynchronous stream.
    pub fn new(inner: impl AsyncRemoteRead + 'static) -> Self {
        let seekable = inner.seekable();
        Self {
            state: ReadState::Ready(Box::new(inner)),
            seekable,
        }
    }

    /// Returns whether the wrapped reader supports seeking.
    pub fn seekable(&self) -> bool {
        self.seekable
    }

    /// Completes the remote read and consumes the stream.
    pub async fn finish(self) -> RemoteResult<()> {
        let (reader, seek_result) = match self.state {
            ReadState::Ready(reader) => (reader, Ok(())),
            ReadState::Seeking(future) => {
                let (reader, result) = future.await;
                (reader, result.map(|_| ()))
            }
            ReadState::Transitioning => unreachable!("stream transition is never observable"),
        };
        let finish_result = reader.finish().await;
        match (seek_result, finish_result) {
            (Ok(()), result) => result,
            (Err(error), Ok(())) => Err(RemoteError::from(error)),
            (Err(error), Err(finish)) => Err(RemoteError::with_source(
                finish.kind(),
                AsyncFinishFailure {
                    seek: error,
                    finish,
                },
            )),
        }
    }

    fn poll_pending_seek(&mut self, context: &mut Context<'_>) -> Poll<io::Result<u64>> {
        let ReadState::Seeking(future) = &mut self.state else {
            return Poll::Ready(Ok(0));
        };
        match future.as_mut().poll(context) {
            Poll::Pending => Poll::Pending,
            Poll::Ready((reader, result)) => {
                self.state = ReadState::Ready(reader);
                Poll::Ready(result)
            }
        }
    }

    fn start_seek(&mut self, position: SeekFrom) {
        let state = std::mem::replace(&mut self.state, ReadState::Transitioning);
        let ReadState::Ready(mut reader) = state else {
            self.state = state;
            return;
        };
        self.state = ReadState::Seeking(Box::pin(async move {
            let result = AsyncRemoteRead::seek(&mut *reader, position).await;
            (reader, result)
        }));
    }
}

impl Unpin for AsyncReadStream {}

impl fmt::Debug for AsyncReadStream {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AsyncReadStream")
            .field("seekable", &self.seekable)
            .finish()
    }
}

impl AsyncRead for AsyncReadStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        match self.poll_pending_seek(context) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
            Poll::Ready(Ok(_)) => {}
        }
        let ReadState::Ready(reader) = &mut self.state else {
            return Poll::Pending;
        };
        Pin::new(&mut **reader).poll_read(context, buffer)
    }
}

impl AsyncSeek for AsyncReadStream {
    fn poll_seek(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        position: SeekFrom,
    ) -> Poll<io::Result<u64>> {
        if matches!(self.state, ReadState::Ready(_)) {
            self.start_seek(position);
        }
        self.poll_pending_seek(context)
    }
}

enum WriteState {
    Ready(Writer),
    Seeking(WriteSeekFuture),
    Transitioning,
}

/// An owned asynchronous remote writer with an explicit finalizer.
#[non_exhaustive]
#[must_use = "call finish() so the backend can complete the transfer"]
pub struct AsyncWriteStream {
    state: WriteState,
    seekable: bool,
}

impl AsyncWriteStream {
    /// Wraps a backend writer in an owned asynchronous stream.
    pub fn new(inner: impl AsyncRemoteWrite + 'static) -> Self {
        let seekable = inner.seekable();
        Self {
            state: WriteState::Ready(Box::new(inner)),
            seekable,
        }
    }

    /// Returns whether the wrapped writer supports seeking.
    pub fn seekable(&self) -> bool {
        self.seekable
    }

    /// Completes the remote write and consumes the stream.
    pub async fn finish(self) -> RemoteResult<()> {
        let (writer, seek_result) = match self.state {
            WriteState::Ready(writer) => (writer, Ok(())),
            WriteState::Seeking(future) => {
                let (writer, result) = future.await;
                (writer, result.map(|_| ()))
            }
            WriteState::Transitioning => unreachable!("stream transition is never observable"),
        };
        let finish_result = writer.finish().await;
        match (seek_result, finish_result) {
            (Ok(()), result) => result,
            (Err(error), Ok(())) => Err(RemoteError::from(error)),
            (Err(error), Err(finish)) => Err(RemoteError::with_source(
                finish.kind(),
                AsyncFinishFailure {
                    seek: error,
                    finish,
                },
            )),
        }
    }

    fn poll_pending_seek(&mut self, context: &mut Context<'_>) -> Poll<io::Result<u64>> {
        let WriteState::Seeking(future) = &mut self.state else {
            return Poll::Ready(Ok(0));
        };
        match future.as_mut().poll(context) {
            Poll::Pending => Poll::Pending,
            Poll::Ready((writer, result)) => {
                self.state = WriteState::Ready(writer);
                Poll::Ready(result)
            }
        }
    }

    fn start_seek(&mut self, position: SeekFrom) {
        let state = std::mem::replace(&mut self.state, WriteState::Transitioning);
        let WriteState::Ready(mut writer) = state else {
            self.state = state;
            return;
        };
        self.state = WriteState::Seeking(Box::pin(async move {
            let result = AsyncRemoteWrite::seek(&mut *writer, position).await;
            (writer, result)
        }));
    }
}

impl Unpin for AsyncWriteStream {}

impl fmt::Debug for AsyncWriteStream {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AsyncWriteStream")
            .field("seekable", &self.seekable)
            .finish()
    }
}

impl AsyncWrite for AsyncWriteStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.poll_pending_seek(context) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
            Poll::Ready(Ok(_)) => {}
        }
        let WriteState::Ready(writer) = &mut self.state else {
            return Poll::Pending;
        };
        Pin::new(&mut **writer).poll_write(context, buffer)
    }

    fn poll_flush(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.poll_pending_seek(context) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
            Poll::Ready(Ok(_)) => {}
        }
        let WriteState::Ready(writer) = &mut self.state else {
            return Poll::Pending;
        };
        Pin::new(&mut **writer).poll_flush(context)
    }

    fn poll_close(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.poll_pending_seek(context) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
            Poll::Ready(Ok(_)) => {}
        }
        let WriteState::Ready(writer) = &mut self.state else {
            return Poll::Pending;
        };
        Pin::new(&mut **writer).poll_close(context)
    }
}

impl AsyncSeek for AsyncWriteStream {
    fn poll_seek(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        position: SeekFrom,
    ) -> Poll<io::Result<u64>> {
        if matches!(self.state, WriteState::Ready(_)) {
            self.start_seek(position);
        }
        self.poll_pending_seek(context)
    }
}

#[derive(Debug, thiserror::Error)]
#[error("seek failed: {seek}; finalization also failed: {finish}")]
struct AsyncFinishFailure {
    seek: io::Error,
    #[source]
    finish: RemoteError,
}

#[cfg(test)]
mod tests;
