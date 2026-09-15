//! Owned asynchronous streams over an opened [`smb::File`].
//!
//! Both streams keep a client-side position and issue positional
//! `read_at`/`write_at` requests, so they are seekable. A stream must be
//! consumed with `finish`, which drives the pending request and closes the
//! handle. A dropped stream is an abandoned transfer: the handle is closed in
//! the background on the runtime captured at open time. If that runtime has
//! already shut down, the handle is intentionally leaked so the `smb` crate's
//! runtime-dependent destructor cannot panic.

use std::future::Future;
use std::io::{self, SeekFrom};
use std::pin::Pin;
use std::task::{ready, Context, Poll};
use std::time::SystemTime;

use futures_io::{AsyncRead, AsyncWrite};
use remotefs::fs::{AsyncRemoteRead, AsyncRemoteWrite};
use remotefs::{RemoteError, RemoteErrorType, RemoteResult};
use smb::binrw_util::prelude::FileTime;
use smb::{FileAttributes, FileBasicInformation, GetLen, ReadAt, WriteAt};
use tokio::runtime::Handle;

use super::convert::{smb_error, system_time_to_filetime};

/// Largest single SMB read or write issued by a stream or a copy.
pub(super) const IO_CHUNK_SIZE: usize = 64 * 1024;

type ReadFuture =
    Pin<Box<dyn Future<Output = (RuntimeSafeFile, Vec<u8>, Result<usize, smb::Error>)> + Send>>;
type WriteFuture = Pin<Box<dyn Future<Output = (RuntimeSafeFile, Result<(), smb::Error>)> + Send>>;
type PendingFile = Pin<Box<dyn Future<Output = RuntimeSafeFile> + Send>>;

/// Keeps an `smb::File` from running its runtime-dependent `Drop` outside a
/// Tokio runtime.
struct RuntimeSafeFile {
    file: Option<smb::File>,
}

impl RuntimeSafeFile {
    fn new(file: smb::File) -> Self {
        Self { file: Some(file) }
    }

    fn as_file(&self) -> &smb::File {
        self.file
            .as_ref()
            .expect("runtime-safe file must remain present")
    }

    fn into_inner(mut self) -> smb::File {
        self.file
            .take()
            .expect("runtime-safe file must remain present")
    }
}

impl Drop for RuntimeSafeFile {
    fn drop(&mut self) {
        let Some(file) = self.file.take() else {
            return;
        };
        if Handle::try_current().is_ok() {
            drop(file);
        } else {
            warn!("leaking an abandoned SMB file because its Tokio runtime is unavailable");
            std::mem::forget(file);
        }
    }
}

/// Closes `file` and drops it inside the current async context.
pub(super) async fn close_file(file: smb::File) -> RemoteResult<()> {
    let result = file
        .close()
        .await
        .map_err(|error| smb_error(RemoteErrorType::ProtocolError, error));
    drop(file);
    result
}

async fn close_safe_file(file: RuntimeSafeFile) -> RemoteResult<()> {
    close_file(file.into_inner()).await
}

/// Writes all of `chunk` at `position`, retrying short writes.
pub(super) async fn write_all_at(
    file: &smb::File,
    mut chunk: &[u8],
    mut position: u64,
) -> Result<(), smb::Error> {
    while !chunk.is_empty() {
        let written = file.write_at(chunk, position).await?;
        if written == 0 {
            return Err(smb::Error::InvalidState(
                "server accepted zero bytes".to_string(),
            ));
        }
        chunk = &chunk[written..];
        position += written as u64;
    }
    Ok(())
}

fn closed() -> io::Error {
    io::Error::new(io::ErrorKind::BrokenPipe, "stream is closed")
}

fn invalid_seek() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, "seek position out of range")
}

/// Closes an abandoned handle in the background.
fn abandon(pending: Option<PendingFile>, runtime: &Handle, name: &str) {
    let Some(pending) = pending else {
        return;
    };
    warn!("stream for {name} dropped without finish; closing the handle in the background");
    let name = name.to_string();
    runtime.spawn(async move {
        let file = pending.await;
        if let Err(error) = close_safe_file(file).await {
            error!("failed to close abandoned handle for {name}: {error}");
        }
    });
}

enum ReadState {
    Idle(RuntimeSafeFile),
    Reading(ReadFuture),
    Closed,
}

impl ReadState {
    fn into_pending(self) -> Option<PendingFile> {
        match self {
            Self::Idle(file) => Some(Box::pin(async move { file })),
            Self::Reading(future) => Some(Box::pin(async move { future.await.0 })),
            Self::Closed => None,
        }
    }

    async fn into_idle(self) -> Option<RuntimeSafeFile> {
        match self {
            Self::Idle(file) => Some(file),
            Self::Reading(future) => Some(future.await.0),
            Self::Closed => None,
        }
    }
}

/// Owned reader over positional SMB reads with an optional byte budget.
pub(super) struct SmbReadStream {
    state: ReadState,
    pending: Vec<u8>,
    consumed: usize,
    position: u64,
    remaining: Option<u64>,
    fetch_remaining: Option<u64>,
    runtime: Handle,
    name: String,
}

impl SmbReadStream {
    pub(super) fn new(
        file: smb::File,
        offset: u64,
        length: Option<u64>,
        runtime: Handle,
        name: String,
    ) -> Self {
        Self {
            state: ReadState::Idle(RuntimeSafeFile::new(file)),
            pending: Vec::new(),
            consumed: 0,
            position: offset,
            remaining: length,
            fetch_remaining: length,
            runtime,
            name,
        }
    }

    fn chunk_len(&self) -> usize {
        self.fetch_remaining.map_or(IO_CHUNK_SIZE, |remaining| {
            usize::try_from(remaining).map_or(IO_CHUNK_SIZE, |limit| limit.min(IO_CHUNK_SIZE))
        })
    }

    /// Position as seen by the caller (bytes fetched but not delivered are
    /// still ahead of it).
    fn logical_position(&self) -> u64 {
        self.position - (self.pending.len() - self.consumed) as u64
    }

    fn copy_pending(&mut self, buf: &mut [u8]) -> usize {
        let available = &self.pending[self.consumed..];
        let count = available.len().min(buf.len());
        buf[..count].copy_from_slice(&available[..count]);
        self.consumed += count;
        if let Some(remaining) = self.remaining.as_mut() {
            *remaining = remaining.saturating_sub(count as u64);
        }
        if self.consumed == self.pending.len() {
            self.pending.clear();
            self.consumed = 0;
        }
        count
    }
}

impl AsyncRead for SmbReadStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        if buf.is_empty() {
            return Poll::Ready(Ok(0));
        }
        loop {
            if self.consumed < self.pending.len() {
                let count = self.copy_pending(buf);
                return Poll::Ready(Ok(count));
            }
            if self.remaining == Some(0) || self.fetch_remaining == Some(0) {
                return Poll::Ready(Ok(0));
            }
            match std::mem::replace(&mut self.state, ReadState::Closed) {
                ReadState::Closed => return Poll::Ready(Err(closed())),
                ReadState::Idle(file) => {
                    let len = self.chunk_len();
                    let position = self.position;
                    self.state = ReadState::Reading(Box::pin(async move {
                        let mut chunk = vec![0; len];
                        let result = file.as_file().read_at(&mut chunk, position).await;
                        (file, chunk, result)
                    }));
                }
                ReadState::Reading(mut future) => match future.as_mut().poll(cx) {
                    Poll::Pending => {
                        self.state = ReadState::Reading(future);
                        return Poll::Pending;
                    }
                    Poll::Ready((file, mut chunk, result)) => {
                        self.state = ReadState::Idle(file);
                        let read = match result {
                            Ok(read) => read,
                            Err(error) => return Poll::Ready(Err(io::Error::other(error))),
                        };
                        if read == 0 {
                            return Poll::Ready(Ok(0));
                        }
                        self.position += read as u64;
                        if let Some(remaining) = self.fetch_remaining.as_mut() {
                            *remaining = remaining.saturating_sub(read as u64);
                        }
                        chunk.truncate(read);
                        self.pending = chunk;
                        self.consumed = 0;
                    }
                },
            }
        }
    }
}

#[remotefs::async_trait]
impl AsyncRemoteRead for SmbReadStream {
    fn seekable(&self) -> bool {
        true
    }

    async fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        let state = std::mem::replace(&mut self.state, ReadState::Closed);
        let file = state.into_idle().await.ok_or_else(closed)?;
        let logical = self.logical_position();
        let target = match position {
            SeekFrom::Start(offset) => Ok(offset),
            SeekFrom::Current(delta) => logical.checked_add_signed(delta).ok_or_else(invalid_seek),
            SeekFrom::End(delta) => match file.as_file().get_len().await {
                Ok(len) => len.checked_add_signed(delta).ok_or_else(invalid_seek),
                Err(error) => Err(io::Error::other(error)),
            },
        };
        self.state = ReadState::Idle(file);
        let target = target?;
        self.position = target;
        self.fetch_remaining = self.remaining;
        self.pending.clear();
        self.consumed = 0;
        Ok(target)
    }

    async fn finish(self: Box<Self>) -> RemoteResult<()> {
        let mut this = *self;
        let state = std::mem::replace(&mut this.state, ReadState::Closed);
        match state.into_idle().await {
            Some(file) => close_safe_file(file).await,
            None => Ok(()),
        }
    }
}

impl Drop for SmbReadStream {
    fn drop(&mut self) {
        let state = std::mem::replace(&mut self.state, ReadState::Closed);
        abandon(state.into_pending(), &self.runtime, &self.name);
    }
}

enum WriteState {
    Idle(RuntimeSafeFile),
    Writing(WriteFuture),
    Closed,
}

impl WriteState {
    fn into_pending(self) -> Option<PendingFile> {
        match self {
            Self::Idle(file) => Some(Box::pin(async move { file })),
            Self::Writing(future) => Some(Box::pin(async move { future.await.0 })),
            Self::Closed => None,
        }
    }

    /// Waits for an in-flight write and reports its outcome.
    async fn into_idle(self) -> Option<(RuntimeSafeFile, Result<(), smb::Error>)> {
        match self {
            Self::Idle(file) => Some((file, Ok(()))),
            Self::Writing(future) => Some(future.await),
            Self::Closed => None,
        }
    }
}

/// Owned writer over positional SMB writes.
///
/// `poll_write` accepts one chunk at a time into an in-flight request;
/// failures surface on the next `poll_write`, `poll_flush`, `seek`, or
/// `finish`.
pub(super) struct SmbWriteStream {
    state: WriteState,
    position: u64,
    end_position: u64,
    pending_end: Option<u64>,
    modified: Option<SystemTime>,
    runtime: Handle,
    name: String,
}

impl SmbWriteStream {
    pub(super) fn new(
        file: smb::File,
        position: u64,
        modified: Option<SystemTime>,
        runtime: Handle,
        name: String,
    ) -> Self {
        Self {
            state: WriteState::Idle(RuntimeSafeFile::new(file)),
            position,
            end_position: position,
            pending_end: None,
            modified,
            runtime,
            name,
        }
    }

    fn poll_pending(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match std::mem::replace(&mut self.state, WriteState::Closed) {
            WriteState::Closed => Poll::Ready(Err(closed())),
            WriteState::Idle(file) => {
                self.state = WriteState::Idle(file);
                Poll::Ready(Ok(()))
            }
            WriteState::Writing(mut future) => match future.as_mut().poll(cx) {
                Poll::Pending => {
                    self.state = WriteState::Writing(future);
                    Poll::Pending
                }
                Poll::Ready((file, result)) => {
                    if result.is_ok() {
                        if let Some(end) = self.pending_end.take() {
                            self.end_position = self.end_position.max(end);
                        }
                    } else {
                        self.pending_end = None;
                    }
                    self.state = WriteState::Idle(file);
                    Poll::Ready(result.map_err(io::Error::other))
                }
            },
        }
    }
}

impl AsyncWrite for SmbWriteStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        ready!(self.poll_pending(cx))?;
        if buf.is_empty() {
            return Poll::Ready(Ok(0));
        }
        let WriteState::Idle(file) = std::mem::replace(&mut self.state, WriteState::Closed) else {
            return Poll::Ready(Err(closed()));
        };
        let chunk = buf[..buf.len().min(IO_CHUNK_SIZE)].to_vec();
        let accepted = chunk.len();
        let position = self.position;
        self.position += accepted as u64;
        self.pending_end = Some(self.position);
        self.state = WriteState::Writing(Box::pin(async move {
            let result = write_all_at(file.as_file(), &chunk, position).await;
            (file, result)
        }));
        Poll::Ready(Ok(accepted))
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.poll_pending(cx)
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.poll_pending(cx)
    }
}

#[remotefs::async_trait]
impl AsyncRemoteWrite for SmbWriteStream {
    fn seekable(&self) -> bool {
        true
    }

    async fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        let state = std::mem::replace(&mut self.state, WriteState::Closed);
        let (file, pending) = state.into_idle().await.ok_or_else(closed)?;
        let target = match pending {
            Err(error) => {
                self.pending_end = None;
                Err(io::Error::other(error))
            }
            Ok(()) => {
                if let Some(end) = self.pending_end.take() {
                    self.end_position = self.end_position.max(end);
                }
                match position {
                    SeekFrom::Start(offset) => Ok(offset),
                    SeekFrom::Current(delta) => self
                        .position
                        .checked_add_signed(delta)
                        .ok_or_else(invalid_seek),
                    SeekFrom::End(delta) => self
                        .end_position
                        .checked_add_signed(delta)
                        .ok_or_else(invalid_seek),
                }
            }
        };
        self.state = WriteState::Idle(file);
        let target = target?;
        self.position = target;
        Ok(target)
    }

    async fn finish(self: Box<Self>) -> RemoteResult<()> {
        let mut this = *self;
        let state = std::mem::replace(&mut this.state, WriteState::Closed);
        let Some((file, pending)) = state.into_idle().await else {
            return Ok(());
        };
        let result = async {
            pending.map_err(|error| smb_error(RemoteErrorType::IoError, error))?;
            file.as_file().flush().await.map_err(RemoteError::from)?;
            if let Some(modified) = this.modified {
                let info = FileBasicInformation {
                    creation_time: FileTime::ZERO,
                    last_access_time: FileTime::ZERO,
                    last_write_time: system_time_to_filetime(modified),
                    change_time: FileTime::ZERO,
                    file_attributes: FileAttributes::new(),
                };
                file.as_file()
                    .set_info(info)
                    .await
                    .map_err(|error| smb_error(RemoteErrorType::PermissionDenied, error))?;
            }
            Ok::<(), RemoteError>(())
        }
        .await;
        let closed = close_safe_file(file).await;
        result.and(closed)
    }
}

impl Drop for SmbWriteStream {
    fn drop(&mut self) {
        let state = std::mem::replace(&mut self.state, WriteState::Closed);
        abandon(state.into_pending(), &self.runtime, &self.name);
    }
}
