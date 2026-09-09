//! Blocking-to-Tokio stream fixtures.

use std::future::Future;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::pin::Pin;
use std::task::{Context, Poll};

use tokio::runtime::Handle;

use crate::fs::{
    AsyncRemoteRead, AsyncRemoteWrite, ReadStream, RemoteError, RemoteErrorType, RemoteResult,
    WriteStream,
};

struct ReadOutput {
    stream: Option<ReadStream>,
    bytes: Vec<u8>,
    result: Option<io::Result<usize>>,
}

impl ReadOutput {
    fn into_parts(mut self) -> (ReadStream, Vec<u8>, io::Result<usize>) {
        (
            self.stream.take().expect("read output owns its stream"),
            std::mem::take(&mut self.bytes),
            self.result.take().expect("read output owns its result"),
        )
    }
}

impl Drop for ReadOutput {
    fn drop(&mut self) {
        if let Some(stream) = self.stream.take() {
            cleanup_stream(stream);
        }
    }
}

struct WriteOutput {
    stream: Option<WriteStream>,
    count: usize,
    result: Option<io::Result<()>>,
}

impl WriteOutput {
    fn into_parts(mut self) -> (WriteStream, usize, io::Result<()>) {
        (
            self.stream.take().expect("write output owns its stream"),
            self.count,
            self.result.take().expect("write output owns its result"),
        )
    }
}

impl Drop for WriteOutput {
    fn drop(&mut self) {
        if let Some(stream) = self.stream.take() {
            cleanup_stream(stream);
        }
    }
}

struct FlushOutput {
    stream: Option<WriteStream>,
    result: Option<io::Result<()>>,
}

impl FlushOutput {
    fn into_parts(mut self) -> (WriteStream, io::Result<()>) {
        (
            self.stream.take().expect("flush output owns its stream"),
            self.result.take().expect("flush output owns its result"),
        )
    }
}

impl Drop for FlushOutput {
    fn drop(&mut self) {
        if let Some(stream) = self.stream.take() {
            cleanup_stream(stream);
        }
    }
}

enum ReadJob {
    Idle(Option<ReadStream>),
    Reading(tokio::task::JoinHandle<ReadOutput>),
    Buffered {
        stream: Option<ReadStream>,
        bytes: Vec<u8>,
        offset: usize,
        error: Option<io::Error>,
    },
    Failed,
}

pub(super) struct UnblockRead {
    job: ReadJob,
    seekable: bool,
}

impl UnblockRead {
    pub(super) fn new(stream: ReadStream) -> Self {
        let seekable = stream.seekable();
        Self {
            job: ReadJob::Idle(Some(stream)),
            seekable,
        }
    }

    fn start_read(&mut self, buffer_size: usize) {
        let ReadJob::Idle(stream) = &mut self.job else {
            return;
        };
        let Some(mut stream) = stream.take() else {
            self.job = ReadJob::Failed;
            return;
        };
        self.job = ReadJob::Reading(spawn_io_worker(move || {
            let mut bytes = vec![0_u8; buffer_size.min(8192)];
            let result = stream.read(&mut bytes);
            ReadOutput {
                stream: Some(stream),
                bytes,
                result: Some(result),
            }
        }));
    }

    fn poll_reading(&mut self, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        let ReadJob::Reading(job) = &mut self.job else {
            return Poll::Ready(Ok(()));
        };
        let result = Pin::new(job).poll(context);
        match result {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Err(error)) => {
                self.job = ReadJob::Failed;
                Poll::Ready(Err(io::Error::other(error)))
            }
            Poll::Ready(Ok(output)) => {
                let (stream, mut bytes, result) = output.into_parts();
                let error = match result {
                    Ok(count) => {
                        bytes.truncate(count);
                        None
                    }
                    Err(error) => {
                        bytes.clear();
                        Some(error)
                    }
                };
                self.job = ReadJob::Buffered {
                    stream: Some(stream),
                    bytes,
                    offset: 0,
                    error,
                };
                Poll::Ready(Ok(()))
            }
        }
    }

    async fn recover_for_seek(&mut self) -> io::Result<(ReadStream, usize)> {
        let job = std::mem::replace(&mut self.job, ReadJob::Failed);
        match job {
            ReadJob::Idle(Some(stream)) => Ok((stream, 0)),
            ReadJob::Idle(None) | ReadJob::Failed => {
                Err(io::Error::other("remote read stream is unavailable"))
            }
            ReadJob::Buffered {
                stream,
                bytes,
                offset,
                error,
            } => {
                let stream = stream.expect("buffered read always owns its stream");
                self.job = ReadJob::Idle(Some(stream));
                if let Some(error) = error {
                    return Err(error);
                }
                Ok((
                    match std::mem::replace(&mut self.job, ReadJob::Failed) {
                        ReadJob::Idle(Some(stream)) => stream,
                        _ => unreachable!("read stream was restored above"),
                    },
                    bytes.len().saturating_sub(offset),
                ))
            }
            ReadJob::Reading(job) => match job.await {
                Err(error) => Err(io::Error::other(error)),
                Ok(output) => {
                    let (stream, mut bytes, result) = output.into_parts();
                    let count = match result {
                        Ok(count) => count,
                        Err(error) => {
                            self.job = ReadJob::Idle(Some(stream));
                            return Err(error);
                        }
                    };
                    bytes.truncate(count);
                    let unread = bytes.len();
                    bytes.clear();
                    self.job = ReadJob::Idle(Some(stream));
                    Ok((
                        match std::mem::replace(&mut self.job, ReadJob::Failed) {
                            ReadJob::Idle(Some(stream)) => stream,
                            _ => unreachable!("read stream was restored above"),
                        },
                        unread,
                    ))
                }
            },
        }
    }

    async fn recover_for_finish(&mut self) -> Result<(ReadStream, Option<io::Error>), RemoteError> {
        let job = std::mem::replace(&mut self.job, ReadJob::Failed);
        match job {
            ReadJob::Idle(Some(stream)) => Ok((stream, None)),
            ReadJob::Idle(None) | ReadJob::Failed => Err(RemoteError::with_message(
                RemoteErrorType::ProtocolError,
                "remote read stream is unavailable",
            )),
            ReadJob::Buffered { stream, error, .. } => {
                Ok((stream.expect("buffered read always owns its stream"), error))
            }
            ReadJob::Reading(job) => match job.await {
                Err(error) => Err(RemoteError::with_source(
                    RemoteErrorType::ProtocolError,
                    error,
                )),
                Ok(output) => {
                    let (stream, _bytes, result) = output.into_parts();
                    Ok((stream, result.err()))
                }
            },
        }
    }
}

impl futures_io::AsyncRead for UnblockRead {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.as_mut().get_mut();
        if buffer.is_empty() {
            return Poll::Ready(Ok(0));
        }
        loop {
            match this.poll_reading(context) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                Poll::Ready(Ok(())) => {}
            }
            if let ReadJob::Buffered {
                stream,
                bytes,
                offset,
                error,
            } = &mut this.job
            {
                if let Some(error) = error.take() {
                    return Poll::Ready(Err(error));
                }
                if *offset < bytes.len() {
                    let count = buffer.len().min(bytes.len() - *offset);
                    buffer[..count].copy_from_slice(&bytes[*offset..*offset + count]);
                    *offset += count;
                    return Poll::Ready(Ok(count));
                }
                let stream = stream.take().expect("buffered read always owns its stream");
                let eof = bytes.is_empty();
                this.job = ReadJob::Idle(Some(stream));
                if eof {
                    return Poll::Ready(Ok(0));
                }
                continue;
            }
            if matches!(this.job, ReadJob::Idle(Some(_))) {
                this.start_read(buffer.len());
                continue;
            }
            return Poll::Ready(Err(io::Error::other("remote read stream is unavailable")));
        }
    }
}

#[async_trait::async_trait]
impl AsyncRemoteRead for UnblockRead {
    fn seekable(&self) -> bool {
        self.seekable
    }

    async fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        if !self.seekable {
            return Err(io::ErrorKind::Unsupported.into());
        }
        let (mut stream, unread) = self.recover_for_seek().await?;
        let position = match adjust_current(position, unread) {
            Ok(position) => position,
            Err(error) => {
                self.job = ReadJob::Idle(Some(stream));
                return Err(error);
            }
        };
        let result = spawn_io_worker(move || {
            let result = stream.seek(position);
            (stream, result)
        })
        .await
        .map_err(io::Error::other)?;
        self.job = ReadJob::Idle(Some(result.0));
        result.1
    }

    async fn finish(self: Box<Self>) -> RemoteResult<()> {
        let mut this = *self;
        let (stream, pending) = this.recover_for_finish().await?;
        let finished = spawn_io_worker(move || stream.finish())
            .await
            .map_err(|error| RemoteError::with_source(RemoteErrorType::ProtocolError, error))?;
        combine_stream_errors(pending, finished)
    }
}

impl Drop for UnblockRead {
    fn drop(&mut self) {
        let job = std::mem::replace(&mut self.job, ReadJob::Failed);
        match job {
            ReadJob::Idle(Some(stream))
            | ReadJob::Buffered {
                stream: Some(stream),
                ..
            } => cleanup_stream(stream),
            ReadJob::Idle(None) | ReadJob::Buffered { stream: None, .. } | ReadJob::Failed => {}
            ReadJob::Reading(job) => drop(job),
        }
    }
}

enum WriteJob {
    Idle(Option<WriteStream>),
    Writing(tokio::task::JoinHandle<WriteOutput>),
    Flushing(tokio::task::JoinHandle<FlushOutput>),
    Failed,
}

pub(super) struct UnblockWrite {
    job: WriteJob,
    seekable: bool,
}

impl UnblockWrite {
    pub(super) fn new(stream: WriteStream) -> Self {
        let seekable = stream.seekable();
        Self {
            job: WriteJob::Idle(Some(stream)),
            seekable,
        }
    }

    fn poll_writing(&mut self, context: &mut Context<'_>) -> Poll<io::Result<Option<usize>>> {
        let WriteJob::Writing(job) = &mut self.job else {
            return Poll::Ready(Ok(None));
        };
        let result = Pin::new(job).poll(context);
        match result {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Err(error)) => {
                self.job = WriteJob::Failed;
                Poll::Ready(Err(io::Error::other(error)))
            }
            Poll::Ready(Ok(output)) => {
                let (stream, count, result) = output.into_parts();
                self.job = WriteJob::Idle(Some(stream));
                Poll::Ready(result.map(|()| Some(count)))
            }
        }
    }

    fn poll_flushing(&mut self, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        let WriteJob::Flushing(job) = &mut self.job else {
            return Poll::Ready(Ok(()));
        };
        let result = Pin::new(job).poll(context);
        match result {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Err(error)) => {
                self.job = WriteJob::Failed;
                Poll::Ready(Err(io::Error::other(error)))
            }
            Poll::Ready(Ok(output)) => {
                let (stream, result) = output.into_parts();
                self.job = WriteJob::Idle(Some(stream));
                Poll::Ready(result)
            }
        }
    }

    fn start_write(&mut self, buffer: &[u8]) {
        let WriteJob::Idle(stream) = &mut self.job else {
            return;
        };
        let Some(mut stream) = stream.take() else {
            self.job = WriteJob::Failed;
            return;
        };
        let bytes = buffer[..buffer.len().min(8192)].to_vec();
        self.job = WriteJob::Writing(spawn_io_worker(move || {
            let result = stream.write(&bytes);
            let count = result.as_ref().copied().unwrap_or(0);
            WriteOutput {
                stream: Some(stream),
                count,
                result: Some(result.map(|_| ())),
            }
        }));
    }

    fn start_flush(&mut self) {
        let WriteJob::Idle(stream) = &mut self.job else {
            return;
        };
        let Some(mut stream) = stream.take() else {
            self.job = WriteJob::Failed;
            return;
        };
        self.job = WriteJob::Flushing(spawn_io_worker(move || {
            let result = stream.flush();
            FlushOutput {
                stream: Some(stream),
                result: Some(result),
            }
        }));
    }

    async fn recover_for_seek(&mut self) -> io::Result<WriteStream> {
        let job = std::mem::replace(&mut self.job, WriteJob::Failed);
        match job {
            WriteJob::Idle(Some(stream)) => Ok(stream),
            WriteJob::Idle(None) | WriteJob::Failed => {
                Err(io::Error::other("remote write stream is unavailable"))
            }
            WriteJob::Writing(job) => match job.await {
                Err(error) => Err(io::Error::other(error)),
                Ok(output) => {
                    let (stream, _count, result) = output.into_parts();
                    self.job = WriteJob::Idle(Some(stream));
                    result.map(
                        |()| match std::mem::replace(&mut self.job, WriteJob::Failed) {
                            WriteJob::Idle(Some(stream)) => stream,
                            _ => unreachable!("write stream was restored above"),
                        },
                    )
                }
            },
            WriteJob::Flushing(job) => match job.await {
                Err(error) => Err(io::Error::other(error)),
                Ok(output) => {
                    let (stream, result) = output.into_parts();
                    self.job = WriteJob::Idle(Some(stream));
                    result.and_then(
                        |()| match std::mem::replace(&mut self.job, WriteJob::Failed) {
                            WriteJob::Idle(Some(stream)) => Ok(stream),
                            _ => Err(io::Error::other("remote write stream is unavailable")),
                        },
                    )
                }
            },
        }
    }

    async fn recover_for_finish(
        &mut self,
    ) -> Result<(WriteStream, Option<io::Error>), RemoteError> {
        let job = std::mem::replace(&mut self.job, WriteJob::Failed);
        match job {
            WriteJob::Idle(Some(stream)) => Ok((stream, None)),
            WriteJob::Idle(None) | WriteJob::Failed => Err(RemoteError::with_message(
                RemoteErrorType::ProtocolError,
                "remote write stream is unavailable",
            )),
            WriteJob::Writing(job) => match job.await {
                Err(error) => Err(RemoteError::with_source(
                    RemoteErrorType::ProtocolError,
                    error,
                )),
                Ok(output) => {
                    let (stream, _count, result) = output.into_parts();
                    Ok((stream, result.err()))
                }
            },
            WriteJob::Flushing(job) => match job.await {
                Err(error) => Err(RemoteError::with_source(
                    RemoteErrorType::ProtocolError,
                    error,
                )),
                Ok(output) => {
                    let (stream, result) = output.into_parts();
                    Ok((stream, result.err()))
                }
            },
        }
    }
}

impl futures_io::AsyncWrite for UnblockWrite {
    fn poll_write(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.as_mut().get_mut();
        if buffer.is_empty() {
            return Poll::Ready(Ok(0));
        }
        loop {
            match this.poll_flushing(context) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                Poll::Ready(Ok(())) => {}
            }
            if let WriteJob::Writing(_) = this.job {
                return match this.poll_writing(context) {
                    Poll::Pending => Poll::Pending,
                    Poll::Ready(Ok(Some(count))) => Poll::Ready(Ok(count)),
                    Poll::Ready(Ok(None)) => continue,
                    Poll::Ready(Err(error)) => Poll::Ready(Err(error)),
                };
            }
            if matches!(this.job, WriteJob::Idle(Some(_))) {
                this.start_write(buffer);
                continue;
            }
            return Poll::Ready(Err(io::Error::other("remote write stream is unavailable")));
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.as_mut().get_mut();
        match this.poll_writing(context) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
            Poll::Ready(Ok(_)) => {}
        }
        if !matches!(this.job, WriteJob::Flushing(_)) {
            if matches!(this.job, WriteJob::Idle(Some(_))) {
                this.start_flush();
            } else {
                return Poll::Ready(Err(io::Error::other("remote write stream is unavailable")));
            }
        }
        this.poll_flushing(context)
    }

    fn poll_close(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.poll_flush(context)
    }
}

#[async_trait::async_trait]
impl AsyncRemoteWrite for UnblockWrite {
    fn seekable(&self) -> bool {
        self.seekable
    }

    async fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        if !self.seekable {
            return Err(io::ErrorKind::Unsupported.into());
        }
        let mut stream = self.recover_for_seek().await?;
        let result = spawn_io_worker(move || {
            let result = stream.seek(position);
            (stream, result)
        })
        .await
        .map_err(io::Error::other)?;
        self.job = WriteJob::Idle(Some(result.0));
        result.1
    }

    async fn finish(self: Box<Self>) -> RemoteResult<()> {
        let mut this = *self;
        let (stream, pending) = this.recover_for_finish().await?;
        let finished = spawn_io_worker(move || stream.finish())
            .await
            .map_err(|error| RemoteError::with_source(RemoteErrorType::ProtocolError, error))?;
        combine_stream_errors(pending, finished)
    }
}

impl Drop for UnblockWrite {
    fn drop(&mut self) {
        let job = std::mem::replace(&mut self.job, WriteJob::Failed);
        match job {
            WriteJob::Idle(Some(stream)) => cleanup_stream(stream),
            WriteJob::Idle(None) | WriteJob::Failed => {}
            WriteJob::Writing(job) => drop(job),
            WriteJob::Flushing(job) => drop(job),
        }
    }
}

fn adjust_current(position: SeekFrom, unread: usize) -> io::Result<SeekFrom> {
    let SeekFrom::Current(offset) = position else {
        return Ok(position);
    };
    let unread = i64::try_from(unread).map_err(|_| io::ErrorKind::InvalidInput)?;
    Ok(SeekFrom::Current(
        offset
            .checked_sub(unread)
            .ok_or(io::ErrorKind::InvalidInput)?,
    ))
}

fn cleanup_stream<S>(stream: S)
where
    S: Send + 'static,
{
    log::warn!("remote stream dropped without explicit finalization");
    if let Ok(handle) = Handle::try_current() {
        handle.spawn_blocking(move || drop(stream));
    } else {
        drop(stream);
    }
}

fn spawn_io_worker<F, R>(operation: F) -> tokio::task::JoinHandle<R>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    tokio::task::spawn_blocking(move || {
        std::thread::scope(|scope| {
            scope
                .spawn(operation)
                .join()
                .expect("remote stream worker panicked")
        })
    })
}

#[derive(Debug, thiserror::Error)]
#[error("{primary}; stream finalization also failed: {finish}")]
struct StreamFinishFailure {
    primary: RemoteError,
    #[source]
    finish: RemoteError,
}

fn combine_stream_errors(
    pending: Option<io::Error>,
    finished: RemoteResult<()>,
) -> RemoteResult<()> {
    let Some(pending) = pending else {
        return finished;
    };
    let primary = RemoteError::from(pending);
    match finished {
        Ok(()) => Err(primary),
        Err(finish) => Err(RemoteError::with_source(
            primary.kind(),
            StreamFinishFailure { primary, finish },
        )),
    }
}
