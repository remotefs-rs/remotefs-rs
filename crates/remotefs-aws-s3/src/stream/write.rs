//! ## Write stream
//!
//! Owned asynchronous writer that stages an S3 multipart upload.
//!
//! S3 cannot append and requires every part but the last one to be at least
//! 5 MiB, so the writer buffers bytes and uploads one part per full buffer.
//! The upload is completed only by [`AsyncRemoteWrite::finish`]; a dropped
//! writer aborts the multipart upload on a best-effort basis.

use std::future::Future;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

use aws_sdk_s3::Client as S3Client;
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::types::{CompletedMultipartUpload, CompletedPart};
use futures_io::AsyncWrite;
use remotefs::fs::AsyncRemoteWrite;
use remotefs::{RemoteError, RemoteErrorType, RemoteResult};

/// Size of every uploaded part except the last one (5 MiB, the S3 minimum).
pub(crate) const PART_SIZE: usize = 5 * 1024 * 1024;

/// Result of an in-flight part upload: the upload id and the completed part.
type PartOutcome = (Option<String>, RemoteResult<CompletedPart>);
type PartFuture = Pin<Box<dyn Future<Output = PartOutcome> + Send>>;

/// Writer that stages one multipart part at a time.
pub(crate) struct S3Writer {
    client: S3Client,
    bucket: String,
    key: String,
    buffer: Vec<u8>,
    upload_id: Option<String>,
    parts: Vec<CompletedPart>,
    pending: Option<PartFuture>,
    finished: bool,
}

impl S3Writer {
    /// Create a writer for an S3 object.
    pub(crate) fn new(client: S3Client, bucket: String, key: String) -> Self {
        Self {
            client,
            bucket,
            key,
            buffer: Vec::with_capacity(PART_SIZE),
            upload_id: None,
            parts: Vec::new(),
            pending: None,
            finished: false,
        }
    }

    /// Drive the in-flight part upload, if any.
    fn poll_pending(&mut self, cx: &mut Context<'_>) -> Poll<RemoteResult<()>> {
        let Some(future) = self.pending.as_mut() else {
            return Poll::Ready(Ok(()));
        };
        let (upload_id, result) = std::task::ready!(future.as_mut().poll(cx));
        self.pending = None;
        if upload_id.is_some() {
            self.upload_id = upload_id;
        }
        Poll::Ready(result.map(|part| self.parts.push(part)))
    }

    /// Move the full buffer into a new part upload.
    fn start_part(&mut self) {
        let body = std::mem::take(&mut self.buffer);
        self.buffer.reserve(PART_SIZE);
        let part_number = self.next_part_number();
        self.pending = Some(Box::pin(upload_part(
            self.client.clone(),
            self.bucket.clone(),
            self.key.clone(),
            self.upload_id.clone(),
            part_number,
            body,
        )));
    }

    fn next_part_number(&self) -> i32 {
        i32::try_from(self.parts.len() + 1).expect("S3 allows at most 10,000 parts")
    }

    /// Upload what is left and complete or create the object.
    async fn complete(&mut self) -> RemoteResult<()> {
        std::future::poll_fn(|cx| self.poll_pending(cx)).await?;
        let body = std::mem::take(&mut self.buffer);
        let Some(upload_id) = self.upload_id.clone() else {
            debug!(
                "PUT '{}' ({} bytes, below multipart threshold)",
                self.key,
                body.len()
            );
            self.client
                .put_object()
                .bucket(self.bucket.as_str())
                .key(self.key.as_str())
                .body(ByteStream::from(body))
                .send()
                .await
                .map_err(|error| RemoteError::with_source(RemoteErrorType::ProtocolError, error))?;
            return Ok(());
        };
        if !body.is_empty() {
            let part_number = self.next_part_number();
            let (_, part) = upload_part(
                self.client.clone(),
                self.bucket.clone(),
                self.key.clone(),
                Some(upload_id.clone()),
                part_number,
                body,
            )
            .await;
            self.parts.push(part?);
        }
        debug!(
            "completing multipart upload {upload_id} for '{}' with {} parts",
            self.key,
            self.parts.len()
        );
        let completed = CompletedMultipartUpload::builder()
            .set_parts(Some(std::mem::take(&mut self.parts)))
            .build();
        self.client
            .complete_multipart_upload()
            .bucket(self.bucket.as_str())
            .key(self.key.as_str())
            .upload_id(upload_id.as_str())
            .multipart_upload(completed)
            .send()
            .await
            .map(|_| ())
            .map_err(|error| RemoteError::with_source(RemoteErrorType::ProtocolError, error))
    }

    /// Best-effort abort of a started multipart upload.
    async fn abort(&mut self) {
        if let Some(upload_id) = self.upload_id.take() {
            abort_upload(
                self.client.clone(),
                self.bucket.clone(),
                self.key.clone(),
                upload_id,
            )
            .await;
        }
    }
}

async fn upload_part(
    client: S3Client,
    bucket: String,
    key: String,
    upload_id: Option<String>,
    part_number: i32,
    body: Vec<u8>,
) -> PartOutcome {
    let upload_id = match upload_id {
        Some(id) => id,
        None => {
            let created = client
                .create_multipart_upload()
                .bucket(bucket.as_str())
                .key(key.as_str())
                .send()
                .await
                .map_err(|error| RemoteError::with_source(RemoteErrorType::ProtocolError, error));
            match created.map(|output| output.upload_id) {
                Ok(Some(id)) => id,
                Ok(None) => {
                    return (
                        None,
                        Err(RemoteError::with_message(
                            RemoteErrorType::ProtocolError,
                            "multipart upload id missing",
                        )),
                    );
                }
                Err(error) => return (None, Err(error)),
            }
        }
    };
    debug!(
        "uploading part {part_number} ({} bytes) of {upload_id} for '{key}'",
        body.len()
    );
    let result = client
        .upload_part()
        .bucket(bucket.as_str())
        .key(key.as_str())
        .upload_id(upload_id.as_str())
        .part_number(part_number)
        .body(ByteStream::from(body))
        .send()
        .await
        .map(|output| {
            CompletedPart::builder()
                .set_e_tag(output.e_tag)
                .part_number(part_number)
                .build()
        })
        .map_err(|error| RemoteError::with_source(RemoteErrorType::ProtocolError, error));
    (Some(upload_id), result)
}

async fn abort_upload(client: S3Client, bucket: String, key: String, upload_id: String) {
    warn!("aborting multipart upload {upload_id} for '{key}'");
    if let Err(error) = client
        .abort_multipart_upload()
        .bucket(bucket.as_str())
        .key(key.as_str())
        .upload_id(upload_id.as_str())
        .send()
        .await
    {
        warn!("could not abort multipart upload {upload_id} for '{key}': {error}");
    }
}

impl AsyncWrite for S3Writer {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        std::task::ready!(self.poll_pending(cx)).map_err(io::Error::from)?;
        let room = PART_SIZE - self.buffer.len();
        let n = room.min(data.len());
        self.buffer.extend_from_slice(&data[..n]);
        if self.buffer.len() == PART_SIZE {
            self.start_part();
        }
        Poll::Ready(Ok(n))
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.poll_pending(cx).map_err(io::Error::from)
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.poll_pending(cx).map_err(io::Error::from)
    }
}

#[remotefs::async_trait]
impl AsyncRemoteWrite for S3Writer {
    async fn finish(mut self: Box<Self>) -> RemoteResult<()> {
        let result = self.complete().await;
        if result.is_err() {
            self.abort().await;
        }
        self.finished = true;
        result
    }
}

impl Drop for S3Writer {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        let Some(upload_id) = self.upload_id.take() else {
            return;
        };
        warn!(
            "write stream for '{}' dropped before finish; multipart upload {upload_id} is abandoned",
            self.key
        );
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                handle.spawn(abort_upload(
                    self.client.clone(),
                    self.bucket.clone(),
                    self.key.clone(),
                    upload_id,
                ));
            }
            Err(_) => warn!(
                "no tokio runtime available: multipart upload {upload_id} for '{}' is left incomplete",
                self.key
            ),
        }
    }
}

#[cfg(test)]
mod test {
    use aws_sdk_s3::config::{Builder, Region};
    use futures::io::AsyncWriteExt as _;
    use pretty_assertions::assert_eq;

    use super::*;

    fn offline_client() -> S3Client {
        S3Client::from_conf(
            Builder::new()
                .behavior_version_latest()
                .region(Region::new("us-east-1"))
                .endpoint_url("http://127.0.0.1:9")
                .build(),
        )
    }

    #[tokio::test]
    async fn should_buffer_small_writes_without_starting_an_upload() {
        let mut writer = S3Writer::new(offline_client(), "bucket".into(), "key".into());
        writer.write_all(b"hello").await.unwrap();
        writer.flush().await.unwrap();
        assert_eq!(writer.buffer, b"hello");
        assert!(writer.upload_id.is_none());
        assert!(writer.pending.is_none());
        assert!(writer.parts.is_empty());
        assert!(!writer.seekable());
        drop(writer);
    }

    #[tokio::test]
    async fn should_accept_at_most_one_part_per_write() {
        let mut writer = S3Writer::new(offline_client(), "bucket".into(), "key".into());
        let data = vec![7_u8; PART_SIZE + 1];
        let n = std::future::poll_fn(|cx| Pin::new(&mut writer).poll_write(cx, &data))
            .await
            .unwrap();
        assert_eq!(n, PART_SIZE);
        assert!(writer.pending.is_some());
        assert!(writer.buffer.is_empty());
        writer.pending = None;
        drop(writer);
    }
}
