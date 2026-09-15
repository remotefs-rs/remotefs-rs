//! Asynchronous stream types for the russh backend.

use std::collections::VecDeque;
use std::future::Future;
use std::io::{self, SeekFrom};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use futures_io::{AsyncRead, AsyncWrite};
use remotefs::fs::{AsyncRemoteRead, AsyncRemoteWrite, ReadOptions};
use remotefs::{RemoteError, RemoteErrorType, RemoteResult};
use russh::client::Msg;
use russh::{Channel, ChannelMsg};
use russh_sftp::client::SftpSession;
use russh_sftp::client::fs::File;
use tokio::task::{JoinHandle, JoinSet};

const SFTP_PIPELINE_DEPTH: usize = 4;
const SFTP_CHUNK_SIZE: u64 = 4 * 1024 * 1024;
const MAX_PREFETCH: usize = 2;
const BATCH_SIZE: u64 = SFTP_PIPELINE_DEPTH as u64 * SFTP_CHUNK_SIZE;

type BatchTask = JoinHandle<io::Result<Vec<u8>>>;

fn broken_pipe(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::BrokenPipe, format!("{what} finished"))
}

fn protocol_error(
    context: &str,
    err: impl std::error::Error + Send + Sync + 'static,
) -> RemoteError {
    error!("{context}: {err}");
    RemoteError::with_source(RemoteErrorType::ProtocolError, err)
}

pub(super) struct RusshSftpReader {
    session: Arc<SftpSession>,
    path: String,
    start: u64,
    end: Option<u64>,
    fetch_offset: u64,
    position: u64,
    batches: VecDeque<Vec<u8>>,
    cursor: usize,
    pending: Option<(u64, u64, BatchTask)>,
    ranged: bool,
    known_size: bool,
    eof: bool,
}

impl RusshSftpReader {
    pub(super) async fn open(
        session: Arc<SftpSession>,
        path: String,
        opts: &ReadOptions,
    ) -> Result<Self, russh_sftp::client::error::Error> {
        let size = session.metadata(&path).await?.size;
        let start = opts.offset.unwrap_or(0).min(size.unwrap_or(u64::MAX));
        let end = match (size, opts.length) {
            (Some(size), Some(length)) => Some(start.saturating_add(length).min(size)),
            (Some(size), None) => Some(size),
            (None, Some(length)) => Some(start.saturating_add(length)),
            (None, None) => None,
        };
        let mut reader = Self {
            session,
            path,
            start,
            end,
            fetch_offset: start,
            position: start,
            batches: VecDeque::new(),
            cursor: 0,
            pending: None,
            ranged: opts.offset.is_some() || opts.length.is_some(),
            known_size: size.is_some(),
            eof: false,
        };
        reader.start_prefetch();
        Ok(reader)
    }

    fn start_prefetch(&mut self) {
        if self.pending.is_some() || self.batches.len() > MAX_PREFETCH {
            return;
        }
        let batch_len = self.end.map_or(BATCH_SIZE, |end| {
            end.saturating_sub(self.fetch_offset).min(BATCH_SIZE)
        });
        if batch_len == 0 || self.eof {
            return;
        }
        let offset = self.fetch_offset;
        self.fetch_offset += batch_len;
        let session = Arc::clone(&self.session);
        let path = self.path.clone();
        let exact = self.known_size;
        let handle =
            tokio::spawn(async move { fetch_batch(session, path, offset, batch_len, exact).await });
        self.pending = Some((offset, batch_len, handle));
    }

    async fn wait_pending(&mut self) {
        if let Some((offset, _, handle)) = self.pending.take() {
            let _ = handle.await;
            self.fetch_offset = offset;
        }
    }

    fn poll_pending(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<bool>> {
        let Some((offset, batch_len, handle)) = self.pending.as_mut() else {
            return Poll::Ready(Ok(false));
        };
        let result = match Pin::new(handle).poll(cx) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(result) => result,
        };
        let offset = *offset;
        let batch_len = *batch_len;
        self.pending = None;
        match result {
            Ok(Ok(batch)) => {
                if !self.known_size && batch.len() < batch_len as usize {
                    self.eof = true;
                }
                if !batch.is_empty() {
                    self.batches.push_back(batch);
                }
                Poll::Ready(Ok(true))
            }
            Ok(Err(err)) => {
                self.fetch_offset = offset;
                Poll::Ready(Err(err))
            }
            Err(join) => {
                self.fetch_offset = offset;
                Poll::Ready(Err(io::Error::other(join)))
            }
        }
    }
}

async fn fetch_batch(
    session: Arc<SftpSession>,
    path: String,
    batch_offset: u64,
    batch_len: u64,
    exact: bool,
) -> io::Result<Vec<u8>> {
    use tokio::io::{AsyncReadExt as _, AsyncSeekExt as _};

    if !exact {
        let mut file = session.open(&path).await.map_err(io::Error::other)?;
        if let Err(err) = file.seek(SeekFrom::Start(batch_offset)).await {
            let _ = file.close().await;
            return Err(err);
        }
        let mut result = Vec::with_capacity(batch_len as usize);
        let mut buf = vec![0_u8; SFTP_CHUNK_SIZE as usize];
        let mut read_error = None;
        while result.len() < batch_len as usize {
            let limit = (batch_len as usize - result.len()).min(buf.len());
            let read = match file.read(&mut buf[..limit]).await {
                Ok(read) => read,
                Err(err) => {
                    read_error = Some(err);
                    break;
                }
            };
            if read == 0 {
                break;
            }
            result.extend_from_slice(&buf[..read]);
        }
        let close_error = file.close().await.map_err(io::Error::other);
        if let Some(err) = read_error {
            return Err(err);
        }
        close_error?;
        return Ok(result);
    }

    let chunk_count = batch_len.div_ceil(SFTP_CHUNK_SIZE);
    let mut tasks = JoinSet::new();
    for index in 0..chunk_count {
        let chunk_offset = index * SFTP_CHUNK_SIZE;
        let len = SFTP_CHUNK_SIZE.min(batch_len - chunk_offset);
        let absolute = batch_offset + chunk_offset;
        let session = Arc::clone(&session);
        let path = path.clone();
        tasks.spawn(async move {
            let mut file = session.open(&path).await.map_err(io::Error::other)?;
            if let Err(err) = file.seek(SeekFrom::Start(absolute)).await {
                let _ = file.close().await;
                return Err(err);
            }
            let mut buf = vec![0_u8; len as usize];
            let read = file.read_exact(&mut buf).await;
            let closed = file.close().await;
            read?;
            closed?;
            Ok::<(usize, Vec<u8>), io::Error>((chunk_offset as usize, buf))
        });
    }
    let mut result = vec![0_u8; batch_len as usize];
    let mut first_error = None;
    while let Some(task) = tasks.join_next().await {
        match task {
            Ok(Ok((chunk_offset, chunk))) => {
                result[chunk_offset..chunk_offset + chunk.len()].copy_from_slice(&chunk);
            }
            Ok(Err(err)) => {
                if first_error.is_none() {
                    first_error = Some(err);
                }
            }
            Err(err) => {
                if first_error.is_none() {
                    first_error = Some(io::Error::other(err));
                }
            }
        }
    }
    if let Some(err) = first_error {
        return Err(err);
    }
    Ok(result)
}

impl AsyncRead for RusshSftpReader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        loop {
            if let Some(front) = self.batches.front() {
                let available = &front[self.cursor..];
                if !available.is_empty() {
                    let count = available.len().min(buf.len());
                    buf[..count].copy_from_slice(&available[..count]);
                    self.cursor += count;
                    self.position += count as u64;
                    return Poll::Ready(Ok(count));
                }
                self.batches.pop_front();
                self.cursor = 0;
                self.start_prefetch();
                continue;
            }
            match self.poll_pending(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Err(err)) => return Poll::Ready(Err(err)),
                Poll::Ready(Ok(true)) => {
                    self.start_prefetch();
                    continue;
                }
                Poll::Ready(Ok(false)) => {
                    if !self.eof && self.end.is_none_or(|end| self.fetch_offset < end) {
                        self.start_prefetch();
                        continue;
                    }
                    return Poll::Ready(Ok(0));
                }
            }
        }
    }
}

#[remotefs::async_trait]
impl AsyncRemoteRead for RusshSftpReader {
    fn seekable(&self) -> bool {
        !self.ranged && self.end.is_some()
    }

    async fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        if !self.seekable() {
            return Err(io::ErrorKind::Unsupported.into());
        }
        let current = self.position;
        let end = self.end.expect("seekable SFTP readers have a known end");
        let target = match position {
            SeekFrom::Start(offset) => self.start.checked_add(offset),
            SeekFrom::End(delta) => end.checked_add_signed(delta),
            SeekFrom::Current(delta) => current.checked_add_signed(delta),
        }
        .filter(|target| *target >= self.start)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "seek before start"))?;
        self.wait_pending().await;
        self.batches.clear();
        self.cursor = 0;
        self.fetch_offset = target.min(end);
        self.position = target;
        self.eof = false;
        self.start_prefetch();
        Ok(target - self.start)
    }

    async fn finish(mut self: Box<Self>) -> RemoteResult<()> {
        self.wait_pending().await;
        Ok(())
    }
}

impl Drop for RusshSftpReader {
    fn drop(&mut self) {
        // Dropping the JoinHandle detaches the task. It is intentionally not
        // aborted: the task must finish its awaited SFTP handle close.
        let _ = self.pending.take();
    }
}

pub(super) struct RusshSftpWriter {
    file: Option<File>,
    closed: bool,
}

impl RusshSftpWriter {
    pub(super) fn new(file: File) -> Self {
        Self {
            file: Some(file),
            closed: false,
        }
    }

    fn file(&mut self) -> io::Result<&mut File> {
        self.file
            .as_mut()
            .ok_or_else(|| broken_pipe("SFTP write stream"))
    }
}

impl AsyncWrite for RusshSftpWriter {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let file = match self.file() {
            Ok(file) => file,
            Err(err) => return Poll::Ready(Err(err)),
        };
        tokio::io::AsyncWrite::poll_write(Pin::new(file), cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let file = match self.file() {
            Ok(file) => file,
            Err(err) => return Poll::Ready(Err(err)),
        };
        tokio::io::AsyncWrite::poll_flush(Pin::new(file), cx)
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if self.closed {
            return Poll::Ready(Ok(()));
        }
        let file = match self.file() {
            Ok(file) => file,
            Err(err) => return Poll::Ready(Err(err)),
        };
        let result = tokio::io::AsyncWrite::poll_shutdown(Pin::new(file), cx);
        if matches!(result, Poll::Ready(Ok(()))) {
            self.closed = true;
        }
        result
    }
}

#[remotefs::async_trait]
impl AsyncRemoteWrite for RusshSftpWriter {
    fn seekable(&self) -> bool {
        true
    }

    async fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        use tokio::io::AsyncSeekExt as _;
        self.file()?.seek(position).await
    }

    async fn finish(mut self: Box<Self>) -> RemoteResult<()> {
        if self.closed {
            self.file.take();
            return Ok(());
        }
        match self.file.take() {
            Some(file) => file.close().await.map_err(RemoteError::from),
            None => Ok(()),
        }
    }
}

impl Drop for RusshSftpWriter {
    fn drop(&mut self) {
        if self.file.is_some() {
            warn!("Dropping unfinished SFTP write stream; the transfer is abandoned");
        }
    }
}

type WaitFuture = Pin<Box<dyn Future<Output = (Channel<Msg>, Option<ChannelMsg>)> + Send>>;

enum ScpReadState {
    Idle(Channel<Msg>),
    Waiting(WaitFuture),
    Finished,
}

pub(super) struct RusshScpReader {
    state: ScpReadState,
    buffer: Vec<u8>,
    cursor: usize,
    remaining: u64,
    completion_status: Option<u8>,
    skip: u64,
    limit: Option<u64>,
}

impl RusshScpReader {
    pub(super) fn new(
        channel: Channel<Msg>,
        leftover: Vec<u8>,
        filesize: u64,
        opts: &ReadOptions,
    ) -> Self {
        let mut leftover = leftover;
        let payload_len = usize::try_from(filesize)
            .unwrap_or(usize::MAX)
            .min(leftover.len());
        let completion_status = leftover.get(payload_len).copied();
        leftover.truncate(payload_len);
        let remaining = filesize - leftover.len() as u64;
        Self {
            state: ScpReadState::Idle(channel),
            buffer: leftover,
            cursor: 0,
            remaining,
            completion_status,
            skip: opts.offset.unwrap_or(0),
            limit: opts.length,
        }
    }

    fn drain(&mut self, buf: &mut [u8]) -> Option<usize> {
        loop {
            let available = self.buffer.len() - self.cursor;
            if available == 0 {
                self.buffer.clear();
                self.cursor = 0;
                return None;
            }
            if self.skip > 0 {
                let skipped = usize::try_from(self.skip)
                    .unwrap_or(usize::MAX)
                    .min(available);
                self.cursor += skipped;
                self.skip -= skipped as u64;
                continue;
            }
            let allowed = match self.limit {
                Some(0) => return Some(0),
                Some(limit) => usize::try_from(limit).unwrap_or(usize::MAX),
                None => usize::MAX,
            };
            let count = available.min(buf.len()).min(allowed);
            buf[..count].copy_from_slice(&self.buffer[self.cursor..self.cursor + count]);
            self.cursor += count;
            if let Some(limit) = self.limit.as_mut() {
                *limit -= count as u64;
            }
            return Some(count);
        }
    }

    async fn take_channel(&mut self) -> Option<Channel<Msg>> {
        match std::mem::replace(&mut self.state, ScpReadState::Finished) {
            ScpReadState::Idle(channel) => Some(channel),
            ScpReadState::Waiting(future) => Some(future.await.0),
            ScpReadState::Finished => None,
        }
    }
}

impl AsyncRead for RusshScpReader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        loop {
            if let Some(count) = self.drain(buf) {
                return Poll::Ready(Ok(count));
            }
            if self.remaining == 0 || self.limit == Some(0) {
                return Poll::Ready(Ok(0));
            }
            match std::mem::replace(&mut self.state, ScpReadState::Finished) {
                ScpReadState::Finished => return Poll::Ready(Err(broken_pipe("SCP channel"))),
                ScpReadState::Idle(mut channel) => {
                    self.state = ScpReadState::Waiting(Box::pin(async move {
                        let msg = channel.wait().await;
                        (channel, msg)
                    }));
                }
                ScpReadState::Waiting(mut future) => match future.as_mut().poll(cx) {
                    Poll::Pending => {
                        self.state = ScpReadState::Waiting(future);
                        return Poll::Pending;
                    }
                    Poll::Ready((channel, msg)) => {
                        self.state = ScpReadState::Idle(channel);
                        match msg {
                            Some(ChannelMsg::Data { data }) => {
                                let take = usize::try_from(self.remaining)
                                    .unwrap_or(usize::MAX)
                                    .min(data.len());
                                self.buffer.extend_from_slice(&data[..take]);
                                self.remaining -= take as u64;
                                if take < data.len() {
                                    self.completion_status.get_or_insert(data[take]);
                                }
                            }
                            Some(ChannelMsg::Eof | ChannelMsg::Close) | None => {
                                self.state = ScpReadState::Finished;
                                return Poll::Ready(Err(io::Error::new(
                                    io::ErrorKind::UnexpectedEof,
                                    format!(
                                        "SCP channel closed with {} bytes remaining",
                                        self.remaining
                                    ),
                                )));
                            }
                            Some(_) => {}
                        }
                    }
                },
            }
        }
    }
}

#[remotefs::async_trait]
impl AsyncRemoteRead for RusshScpReader {
    fn seekable(&self) -> bool {
        false
    }

    async fn finish(mut self: Box<Self>) -> RemoteResult<()> {
        self.limit = None;
        self.skip = 0;
        let mut discarded = [0_u8; 64 * 1024];
        while self.remaining > 0 {
            let count = std::future::poll_fn(|context| {
                Pin::new(&mut *self).poll_read(context, &mut discarded)
            })
            .await
            .map_err(RemoteError::from)?;
            if count == 0 {
                break;
            }
        }
        if self.remaining != 0 {
            return Err(RemoteError::with_message(
                RemoteErrorType::ProtocolError,
                format!("SCP channel closed with {} bytes remaining", self.remaining),
            ));
        }
        let Some(channel) = self.take_channel().await else {
            return Err(RemoteError::with_message(
                RemoteErrorType::ProtocolError,
                "SCP receive stream was already closed before completion",
            ));
        };
        let mut channel = channel;
        let status = if let Some(status) = self.completion_status.take() {
            status
        } else {
            loop {
                match channel.wait().await {
                    Some(ChannelMsg::Data { data }) => {
                        let Some(status) = data.first() else {
                            continue;
                        };
                        break *status;
                    }
                    Some(ChannelMsg::Eof | ChannelMsg::Close) | None => {
                        return Err(RemoteError::with_message(
                            RemoteErrorType::ProtocolError,
                            "SCP channel closed before the completion status",
                        ));
                    }
                    Some(other) => {
                        trace!("Skipping non-data SCP completion message: {other:?}");
                    }
                }
            }
        };
        if status != 0 {
            return Err(RemoteError::with_message(
                RemoteErrorType::ProtocolError,
                format!("Unexpected SCP completion status: {status}"),
            ));
        }
        channel
            .data(&[0_u8][..])
            .await
            .map_err(|err| protocol_error("Failed to acknowledge SCP completion", err))?;
        channel
            .eof()
            .await
            .map_err(|err| protocol_error("Failed to send EOF on SCP channel", err))?;
        channel
            .close()
            .await
            .map_err(|err| protocol_error("Failed to close SCP channel", err))
    }
}

impl Drop for RusshScpReader {
    fn drop(&mut self) {
        if !matches!(self.state, ScpReadState::Finished) {
            warn!("Dropping unfinished SCP recv channel; the transfer is abandoned");
        }
    }
}

pub(super) struct RusshScpWriter {
    channel: Option<Channel<Msg>>,
    writer: Pin<Box<dyn tokio::io::AsyncWrite + Send>>,
    remaining: u64,
    closed: bool,
}

impl RusshScpWriter {
    pub(super) fn new(channel: Channel<Msg>, size: u64) -> Self {
        let writer = Box::pin(channel.make_writer());
        Self {
            channel: Some(channel),
            writer,
            remaining: size,
            closed: false,
        }
    }
}

impl AsyncWrite for RusshScpWriter {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        if self.channel.is_none() {
            return Poll::Ready(Err(broken_pipe("SCP channel")));
        }
        if self.closed {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "SCP channel is closed",
            )));
        }
        if self.remaining == 0 {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "SCP source exceeds the declared size",
            )));
        }
        let count = self.remaining.min(buf.len() as u64) as usize;
        match self.writer.as_mut().poll_write(cx, &buf[..count]) {
            Poll::Ready(Ok(written)) => {
                self.remaining -= written as u64;
                Poll::Ready(Ok(written))
            }
            result => result,
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.writer.as_mut().poll_flush(cx)
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if self.closed {
            return Poll::Ready(Ok(()));
        }
        let result = self.writer.as_mut().poll_flush(cx);
        if matches!(result, Poll::Ready(Ok(()))) {
            self.closed = true;
        }
        result
    }
}

#[remotefs::async_trait]
impl AsyncRemoteWrite for RusshScpWriter {
    async fn finish(mut self: Box<Self>) -> RemoteResult<()> {
        use tokio::io::AsyncWriteExt as _;
        let Some(mut channel) = self.channel.take() else {
            return Ok(());
        };
        if self.remaining != 0 {
            let _ = channel.eof().await;
            let _ = channel.close().await;
            return Err(RemoteError::with_message(
                RemoteErrorType::ProtocolError,
                format!(
                    "SCP source ended before the declared size ({remaining} bytes missing)",
                    remaining = self.remaining
                ),
            ));
        }
        self.writer.flush().await.map_err(RemoteError::from)?;
        channel
            .data(&[0_u8][..])
            .await
            .map_err(|err| protocol_error("Failed to send SCP completion", err))?;
        super::scp::wait_for_ack(&mut channel).await?;
        channel
            .eof()
            .await
            .map_err(|err| protocol_error("Failed to send EOF on SCP channel", err))?;
        channel
            .close()
            .await
            .map_err(|err| protocol_error("Failed to close SCP channel", err))
    }
}

impl Drop for RusshScpWriter {
    fn drop(&mut self) {
        if self.channel.is_some() {
            warn!("Dropping unfinished SCP send channel; the transfer is abandoned");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scp_reader_drain_honors_skip_and_limit() {
        fn reader(
            leftover: &[u8],
            filesize: u64,
            opts: ReadOptions,
        ) -> (Vec<u8>, u64, Option<u64>) {
            let mut this = RusshScpReader {
                state: ScpReadState::Finished,
                buffer: leftover.to_vec(),
                cursor: 0,
                remaining: filesize - leftover.len() as u64,
                completion_status: None,
                skip: opts.offset.unwrap_or(0),
                limit: opts.length,
            };
            let mut out = vec![0_u8; 16];
            let count = this.drain(&mut out).unwrap_or(0);
            out.truncate(count);
            (out, this.skip, this.limit)
        }
        assert_eq!(
            reader(b"abcdef", 6, ReadOptions::default().offset(2).length(2)),
            (b"cd".to_vec(), 0, Some(0))
        );
        assert_eq!(
            reader(b"abc", 6, ReadOptions::default().offset(5)),
            (Vec::new(), 2, None)
        );
        assert_eq!(
            reader(b"abc", 3, ReadOptions::default().length(0)),
            (Vec::new(), 0, Some(0))
        );
    }
}
