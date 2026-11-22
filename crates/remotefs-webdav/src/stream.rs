//! Owned transfer streams over buffered WebDAV bodies.
//!
//! `GET` and `PUT` in `dav-xml-client` move whole bodies, so a read stream is
//! a cursor over the downloaded bytes and a write stream is a buffer that is
//! uploaded when the caller finishes it.

use std::io::{self, Cursor, Read, Seek, SeekFrom};
use std::pin::Pin;
use std::task::{Context, Poll};

use dav_xml_client::AsyncDavClient;
use dav_xml_client::transport::AsyncTransport;
use futures_io::{AsyncRead, AsyncWrite};
use remotefs::RemoteResult;
use remotefs::fs::{AsyncRemoteRead, AsyncRemoteWrite};

use crate::error::map_error;

/// Content type sent with every upload; WebDAV servers store bytes as-is.
pub(crate) const CONTENT_TYPE: &str = "application/octet-stream";

/// Upper bound for the buffer reserved from a size hint, so that a wrong
/// hint cannot make a small upload allocate gigabytes up front (8 MiB).
const MAX_PREALLOCATION: usize = 8 * 1024 * 1024;

/// A seekable reader over a fully downloaded `GET` body.
pub(crate) struct WebDavReader {
    body: Cursor<Vec<u8>>,
}

impl WebDavReader {
    /// Wraps an already downloaded body.
    pub(crate) fn new(body: Vec<u8>) -> Self {
        Self {
            body: Cursor::new(body),
        }
    }
}

impl AsyncRead for WebDavReader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _context: &mut Context<'_>,
        buffer: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        Poll::Ready(self.body.read(buffer))
    }
}

#[remotefs::async_trait]
impl AsyncRemoteRead for WebDavReader {
    fn seekable(&self) -> bool {
        true
    }

    async fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.body.seek(position)
    }
}

/// A writer that buffers bytes and uploads them with one `PUT` on `finish`.
///
/// Dropping the writer abandons the upload: nothing reaches the server and a
/// warning is logged.
pub(crate) struct WebDavWriter<T> {
    client: AsyncDavClient<T>,
    url: String,
    body: Vec<u8>,
    finished: bool,
}

impl<T> WebDavWriter<T> {
    /// Creates a writer targeting `url`, reserving space from `size_hint`.
    pub(crate) fn new(client: AsyncDavClient<T>, url: String, size_hint: Option<u64>) -> Self {
        let capacity = size_hint
            .and_then(|hint| usize::try_from(hint).ok())
            .unwrap_or(0)
            .min(MAX_PREALLOCATION);
        Self {
            client,
            url,
            body: Vec::with_capacity(capacity),
            finished: false,
        }
    }
}

impl<T> AsyncWrite for WebDavWriter<T>
where
    T: Unpin,
{
    fn poll_write(
        mut self: Pin<&mut Self>,
        _context: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.body.extend_from_slice(buffer);
        Poll::Ready(Ok(buffer.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

#[remotefs::async_trait]
impl<T> AsyncRemoteWrite for WebDavWriter<T>
where
    T: AsyncTransport + Send + Sync + Unpin + 'static,
{
    async fn finish(mut self: Box<Self>) -> RemoteResult<()> {
        self.finished = true;
        let body = std::mem::take(&mut self.body);
        debug!(
            "uploading {len} bytes to {url}",
            len = body.len(),
            url = self.url
        );
        self.client
            .put(&self.url, body, CONTENT_TYPE)
            .await
            .map_err(map_error)
    }
}

impl<T> Drop for WebDavWriter<T> {
    fn drop(&mut self) {
        if !self.finished {
            warn!(
                "upload to {url} dropped before finish: {len} buffered bytes discarded",
                url = self.url,
                len = self.body.len()
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::SeekFrom;

    use dav_xml_client::transport::MockTransport;
    use dav_xml_client::{AsyncDavClient, Auth};
    use futures::io::{AsyncReadExt as _, AsyncWriteExt as _};
    use pretty_assertions::assert_eq;
    use remotefs::RemoteErrorType;
    use remotefs::fs::{AsyncReadStream, AsyncRemoteRead as _, AsyncWriteStream};

    use super::{WebDavReader, WebDavWriter};

    fn client() -> (AsyncDavClient<MockTransport>, MockTransport) {
        let mock = MockTransport::new();
        (
            AsyncDavClient::new(mock.clone(), Auth::basic("a", "b")),
            mock,
        )
    }

    fn reply(mock: &MockTransport, status: u16) {
        mock.reply(
            http::Response::builder()
                .status(status)
                .body(Vec::new())
                .unwrap(),
        );
    }

    #[tokio::test]
    async fn reader_reads_and_seeks_over_the_buffered_body() {
        let mut reader = WebDavReader::new(b"hello world".to_vec());
        assert!(reader.seekable());
        let mut output = Vec::new();
        reader.read_to_end(&mut output).await.unwrap();
        assert_eq!(output, b"hello world");
        assert_eq!(reader.seek(SeekFrom::Start(6)).await.unwrap(), 6);
        let mut rest = String::new();
        reader.read_to_string(&mut rest).await.unwrap();
        assert_eq!(rest, "world");
        Box::new(reader).finish().await.unwrap();
    }

    #[tokio::test]
    async fn reader_wraps_into_an_owned_stream() {
        let mut stream = AsyncReadStream::new(WebDavReader::new(b"abc".to_vec()));
        assert!(stream.seekable());
        let mut output = Vec::new();
        stream.read_to_end(&mut output).await.unwrap();
        assert_eq!(output, b"abc");
        stream.finish().await.unwrap();
    }

    #[tokio::test]
    async fn writer_uploads_the_buffer_on_finish() {
        let (client, mock) = client();
        reply(&mock, 204);
        let mut stream =
            AsyncWriteStream::new(WebDavWriter::new(client, "http://h/f".to_string(), Some(5)));
        stream.write_all(b"hel").await.unwrap();
        stream.write_all(b"lo").await.unwrap();
        stream.flush().await.unwrap();
        assert!(mock.requests().is_empty(), "nothing is sent before finish");
        stream.finish().await.unwrap();
        let request = mock.last_request().unwrap();
        assert_eq!(request.method(), http::Method::PUT);
        assert_eq!(request.uri(), "http://h/f");
        assert_eq!(request.body(), b"hello");
        assert_eq!(
            request.headers()["content-type"],
            "application/octet-stream"
        );
    }

    #[tokio::test]
    async fn writer_reports_the_server_failure_from_finish() {
        let (client, mock) = client();
        reply(&mock, 403);
        let mut stream =
            AsyncWriteStream::new(WebDavWriter::new(client, "http://h/f".to_string(), None));
        stream.write_all(b"x").await.unwrap();
        let error = stream.finish().await.unwrap_err();
        assert_eq!(error.kind(), RemoteErrorType::PermissionDenied);
    }

    #[tokio::test]
    async fn dropped_writer_sends_nothing() {
        let (client, mock) = client();
        reply(&mock, 204);
        let mut stream =
            AsyncWriteStream::new(WebDavWriter::new(client, "http://h/f".to_string(), None));
        stream.write_all(b"abandoned").await.unwrap();
        drop(stream);
        assert!(mock.requests().is_empty());
    }
}
