//! Owned asynchronous writer that feeds a resumable upload.
//!
//! Known-size writes are queued into a bounded channel that the SDK consumes as
//! a `StreamingSource`. Unknown-size writes are spooled to an unnamed temporary
//! file until `finish` reveals the exact size needed for upload selection. The
//! upload future is owned by the writer and polled from every write, flush,
//! close, and `finish` call, so no upload task is detached. Temporary-file I/O
//! is offloaded to Tokio's blocking pool. The object exists only after
//! `finish` succeeds. A dropped writer abandons the upload: the resumable
//! session expires on the server side and nothing is created.

use std::fs::File as SpoolFile;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::Bytes;
use futures_io::AsyncWrite;
use google_cloud_storage::client::Storage;
use google_cloud_storage::model::Object;
use google_cloud_storage::streaming_source::{SizeHint, StreamingSource};
use remotefs::fs::AsyncRemoteWrite;
use remotefs::{RemoteError, RemoteErrorType, RemoteResult};
use tokio::sync::mpsc::error::SendError;
use tokio::sync::mpsc::{self, OwnedPermit, Receiver, Sender};
use tokio::sync::watch;

use crate::client::CONTENT_TYPE;
use crate::error::map_gcs_error;

/// Number of chunks the writer may queue ahead of the upload.
const CHANNEL_CAPACITY: usize = 4;

type UploadFuture = Pin<Box<dyn Future<Output = google_cloud_storage::Result<Object>> + Send>>;
type ReserveFuture =
    Pin<Box<dyn Future<Output = Result<OwnedPermit<Bytes>, SendError<()>>> + Send>>;
type SpoolWriteFuture =
    Pin<Box<dyn Future<Output = (Option<SpoolFile>, io::Result<usize>)> + Send>>;
type SpoolFlushFuture = Pin<Box<dyn Future<Output = (Option<SpoolFile>, io::Result<()>)> + Send>>;

#[derive(Clone)]
enum ChannelSender {
    Bounded(Sender<Bytes>),
}

enum ChannelReceiver {
    Bounded(Receiver<Bytes>),
    Spool(SpoolSource),
}

struct SpoolSource {
    file: Option<SpoolFile>,
    started: bool,
    complete: bool,
}

impl SpoolSource {
    async fn next(&mut self) -> Option<Result<Bytes, io::Error>> {
        if self.complete {
            return None;
        }
        let Some(file) = self.file.take() else {
            self.complete = true;
            return Some(Err(io::Error::other("temporary upload file is missing")));
        };
        let started = self.started;
        let result = tokio::task::spawn_blocking(move || {
            let mut file = file;
            if !started {
                file.seek(SeekFrom::Start(0))?;
            }
            let mut buffer = vec![0_u8; 64 * 1024];
            let count = file.read(&mut buffer)?;
            if count == 0 {
                Ok((file, true, None))
            } else {
                buffer.truncate(count);
                Ok((file, true, Some(Bytes::from(buffer))))
            }
        })
        .await;
        match result {
            Ok(Ok((file, started, bytes))) => {
                self.file = Some(file);
                self.started = started;
                if let Some(bytes) = bytes {
                    Some(Ok(bytes))
                } else {
                    self.complete = true;
                    None
                }
            }
            Ok(Err(error)) => {
                self.complete = true;
                Some(Err(error))
            }
            Err(error) => {
                self.complete = true;
                Some(Err(io::Error::other(error)))
            }
        }
    }
}

/// Channel-backed single-pass payload for the SDK upload.
pub(crate) struct ChannelSource {
    receiver: ChannelReceiver,
    size: Option<u64>,
    size_receiver: Option<watch::Receiver<Option<u64>>>,
}

impl StreamingSource for ChannelSource {
    type Error = io::Error;

    async fn next(&mut self) -> Option<Result<Bytes, Self::Error>> {
        match &mut self.receiver {
            ChannelReceiver::Bounded(receiver) => receiver.recv().await.map(Ok),
            ChannelReceiver::Spool(source) => source.next().await,
        }
    }

    async fn size_hint(&self) -> Result<SizeHint, Self::Error> {
        if let Some(size) = self.size {
            return Ok(SizeHint::with_exact(size));
        }
        let Some(mut receiver) = self.size_receiver.clone() else {
            return Err(io::Error::other("missing size completion signal"));
        };
        loop {
            if let Some(size) = *receiver.borrow() {
                return Ok(SizeHint::with_exact(size));
            }
            receiver.changed().await.map_err(|error| {
                io::Error::other(format!("size completion signal closed: {error}"))
            })?;
        }
    }
}

pub(crate) struct GcsWriter {
    object: String,
    sender: Option<ChannelSender>,
    spool: Option<SpoolFile>,
    spool_write: Option<SpoolWriteFuture>,
    spool_flush: Option<SpoolFlushFuture>,
    reserve: Option<ReserveFuture>,
    upload: Option<UploadFuture>,
    outcome: Option<RemoteResult<Object>>,
    size_completion: Option<watch::Sender<Option<u64>>>,
    expected_size: Option<u64>,
    written: u64,
    write_failure: Option<String>,
    finished: bool,
}

impl GcsWriter {
    pub(crate) fn new(
        storage: Storage,
        bucket_resource: String,
        object: String,
        size_hint: Option<u64>,
    ) -> RemoteResult<Self> {
        let (sender, receiver, spool) = if size_hint.is_some() {
            let (sender, receiver) = mpsc::channel(CHANNEL_CAPACITY);
            (
                Some(ChannelSender::Bounded(sender)),
                ChannelReceiver::Bounded(receiver),
                None,
            )
        } else {
            let spool = tempfile::tempfile().map_err(|error| {
                RemoteError::with_source(RemoteErrorType::FileCreateDenied, error)
            })?;
            let reader = spool.try_clone().map_err(|error| {
                RemoteError::with_source(RemoteErrorType::FileCreateDenied, error)
            })?;
            (
                None,
                ChannelReceiver::Spool(SpoolSource {
                    file: Some(reader),
                    started: false,
                    complete: false,
                }),
                Some(spool),
            )
        };
        let (size_sender, size_receiver) = watch::channel(None);
        let source = ChannelSource {
            receiver,
            size: size_hint,
            size_receiver: size_hint.is_none().then_some(size_receiver),
        };
        let name = object.clone();
        let upload: UploadFuture = Box::pin(async move {
            Box::pin(
                storage
                    .write_object(bucket_resource, name, source)
                    .set_content_type(CONTENT_TYPE)
                    .send_buffered(),
            )
            .await
        });
        Ok(Self {
            object,
            sender,
            spool,
            spool_write: None,
            spool_flush: None,
            reserve: None,
            upload: Some(upload),
            outcome: None,
            size_completion: size_hint.is_none().then_some(size_sender),
            expected_size: size_hint,
            written: 0,
            write_failure: None,
            finished: false,
        })
    }

    /// Drives the upload once; records its outcome when it completes.
    fn poll_upload(&mut self, context: &mut Context<'_>) -> Poll<()> {
        let Some(upload) = self.upload.as_mut() else {
            return Poll::Ready(());
        };
        match upload.as_mut().poll(context) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(result) => {
                self.upload = None;
                self.outcome = Some(
                    result.map_err(|error| map_gcs_error(error, RemoteErrorType::FileCreateDenied)),
                );
                Poll::Ready(())
            }
        }
    }

    fn upload_error(&self) -> Option<io::Error> {
        match &self.outcome {
            Some(Err(error)) => Some(io::Error::other(error.to_string())),
            Some(Ok(_)) => Some(io::Error::other(
                "upload completed before the stream was closed",
            )),
            None => None,
        }
    }

    fn complete_size_hint(&mut self) {
        if let Some(sender) = self.size_completion.take() {
            let _ = sender.send(Some(self.written));
        }
    }

    fn record_write_failure(&mut self, error: &io::Error) {
        if self.write_failure.is_none() {
            self.write_failure = Some(error.to_string());
        }
    }

    fn poll_spool_write(
        &mut self,
        context: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let mut future = if let Some(future) = self.spool_write.take() {
            future
        } else {
            let Some(file) = self.spool.take() else {
                let error = io::Error::from(io::ErrorKind::BrokenPipe);
                self.record_write_failure(&error);
                return Poll::Ready(Err(error));
            };
            let bytes = Bytes::copy_from_slice(buf);
            let length = bytes.len();
            Box::pin(async move {
                match tokio::task::spawn_blocking(move || {
                    let mut file = file;
                    let result = file.write_all(&bytes).map(|()| length);
                    (file, result)
                })
                .await
                {
                    Ok((file, result)) => (Some(file), result),
                    Err(error) => (None, Err(io::Error::other(error))),
                }
            })
        };
        match future.as_mut().poll(context) {
            Poll::Pending => {
                self.spool_write = Some(future);
                Poll::Pending
            }
            Poll::Ready((file, result)) => {
                self.spool = file;
                match result {
                    Ok(length) => {
                        self.written += length as u64;
                        Poll::Ready(Ok(length))
                    }
                    Err(error) => {
                        self.record_write_failure(&error);
                        Poll::Ready(Err(error))
                    }
                }
            }
        }
    }

    fn poll_spool_flush(&mut self, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        let mut future = if let Some(future) = self.spool_flush.take() {
            future
        } else {
            let Some(file) = self.spool.take() else {
                return Poll::Ready(Ok(()));
            };
            Box::pin(async move {
                match tokio::task::spawn_blocking(move || {
                    let mut file = file;
                    let result = file.flush();
                    (file, result)
                })
                .await
                {
                    Ok((file, result)) => (Some(file), result),
                    Err(error) => (None, Err(io::Error::other(error))),
                }
            })
        };
        match future.as_mut().poll(context) {
            Poll::Pending => {
                self.spool_flush = Some(future);
                Poll::Pending
            }
            Poll::Ready((file, result)) => {
                self.spool = file;
                Poll::Ready(result)
            }
        }
    }

    async fn finish_spool(&mut self) -> io::Result<()> {
        if let Some(future) = self.spool_write.take() {
            let (file, result) = future.await;
            self.spool = file;
            let length = result?;
            self.written += length as u64;
        }
        if let Some(future) = self.spool_flush.take() {
            let (file, result) = future.await;
            self.spool = file;
            result?;
        }
        let Some(file) = self.spool.take() else {
            return Ok(());
        };
        let (file, result) = tokio::task::spawn_blocking(move || {
            let mut file = file;
            let result = file.flush();
            (file, result)
        })
        .await
        .map_err(io::Error::other)?;
        self.spool = Some(file);
        result
    }
}

impl AsyncWrite for GcsWriter {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        if buf.is_empty() {
            return Poll::Ready(Ok(0));
        }
        let _ = self.poll_upload(cx);
        if let Some(error) = self.upload_error() {
            return Poll::Ready(Err(error));
        }
        if self.spool.is_some() || self.spool_write.is_some() {
            return self.poll_spool_write(cx, buf);
        }
        let Some(sender) = self.sender.clone() else {
            let error = io::Error::from(io::ErrorKind::BrokenPipe);
            self.record_write_failure(&error);
            return Poll::Ready(Err(error));
        };
        match sender {
            ChannelSender::Bounded(sender) => {
                let mut reserve = self
                    .reserve
                    .take()
                    .unwrap_or_else(|| Box::pin(sender.reserve_owned()));
                match reserve.as_mut().poll(cx) {
                    Poll::Pending => {
                        self.reserve = Some(reserve);
                        Poll::Pending
                    }
                    Poll::Ready(Err(_)) => {
                        let error = io::Error::from(io::ErrorKind::BrokenPipe);
                        self.record_write_failure(&error);
                        Poll::Ready(Err(error))
                    }
                    Poll::Ready(Ok(permit)) => {
                        permit.send(Bytes::copy_from_slice(buf));
                        self.written += buf.len() as u64;
                        Poll::Ready(Ok(buf.len()))
                    }
                }
            }
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.poll_spool_flush(cx) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(Err(error)) => {
                self.record_write_failure(&error);
                return Poll::Ready(Err(error));
            }
            Poll::Ready(Ok(())) => {}
        }
        let _ = self.poll_upload(cx);
        match self.upload_error() {
            Some(error) => Poll::Ready(Err(error)),
            None => Poll::Ready(Ok(())),
        }
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.poll_spool_flush(cx) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(Err(error)) => {
                self.record_write_failure(&error);
                return Poll::Ready(Err(error));
            }
            Poll::Ready(Ok(())) => {}
        }
        let mismatch = self
            .expected_size
            .filter(|expected| *expected != self.written);
        self.reserve = None;
        self.complete_size_hint();
        self.sender = None;
        self.spool = None;
        if let Some(expected) = mismatch {
            let error = io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "written size {} differs from the size hint {expected}",
                    self.written
                ),
            );
            return Poll::Ready(Err(error));
        }
        match self.upload_error() {
            Some(error) => Poll::Ready(Err(error)),
            None => Poll::Ready(Ok(())),
        }
    }
}

#[remotefs::async_trait]
impl AsyncRemoteWrite for GcsWriter {
    async fn finish(mut self: Box<Self>) -> RemoteResult<()> {
        if let Err(error) = self.finish_spool().await {
            self.finished = true;
            return Err(RemoteError::from(error));
        }
        self.reserve = None;
        self.sender = None;
        self.spool = None;
        self.complete_size_hint();
        self.finished = true;
        if let Some(error) = self.write_failure.take() {
            return Err(RemoteError::with_message(RemoteErrorType::IoError, error));
        }
        if let Some(expected) = self.expected_size
            && expected != self.written
        {
            return Err(RemoteError::with_message(
                RemoteErrorType::ProtocolError,
                format!(
                    "written size {} differs from the size hint {expected}",
                    self.written
                ),
            ));
        }
        if let Some(upload) = self.upload.take() {
            self.outcome = Some(
                upload
                    .await
                    .map_err(|error| map_gcs_error(error, RemoteErrorType::FileCreateDenied)),
            );
        }
        let object = match self.outcome.take() {
            Some(Ok(object)) => object,
            Some(Err(error)) => return Err(error),
            None => {
                return Err(RemoteError::with_message(
                    RemoteErrorType::ProtocolError,
                    "upload finished without a result",
                ));
            }
        };
        if object.size < 0 {
            return Err(RemoteError::with_message(
                RemoteErrorType::ProtocolError,
                "storage returned a negative object size",
            ));
        }
        let size = object.size.cast_unsigned();
        if size != self.written {
            return Err(RemoteError::with_message(
                RemoteErrorType::ProtocolError,
                format!(
                    "uploaded size {size} differs from the {written} bytes written",
                    written = self.written
                ),
            ));
        }
        if let Some(expected) = self.expected_size
            && expected != size
        {
            return Err(RemoteError::with_message(
                RemoteErrorType::ProtocolError,
                format!("uploaded size {size} differs from the size hint {expected}"),
            ));
        }
        Ok(())
    }
}

impl Drop for GcsWriter {
    fn drop(&mut self) {
        if !self.finished {
            debug!(
                "abandoned upload of '{object}' after {written} bytes",
                object = self.object,
                written = self.written
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[tokio::test]
    async fn channel_source_yields_chunks_then_eof_and_reports_its_hint() {
        let (sender, receiver) = mpsc::channel(2);
        let mut source = ChannelSource {
            receiver: ChannelReceiver::Bounded(receiver),
            size: Some(5),
            size_receiver: None,
        };
        sender.send(Bytes::from_static(b"hel")).await.unwrap();
        sender.send(Bytes::from_static(b"lo")).await.unwrap();
        drop(sender);
        assert_eq!(source.next().await.unwrap().unwrap(), b"hel".as_slice());
        assert_eq!(source.next().await.unwrap().unwrap(), b"lo".as_slice());
        assert!(source.next().await.is_none());
        let hint = source.size_hint().await.unwrap();
        assert_eq!(hint.upper(), Some(5));
    }
}
