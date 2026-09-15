//! ## Read stream
//!
//! Owned asynchronous reader over the body of an S3 `GetObject` response.

use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

use aws_sdk_s3::primitives::ByteStream;
use bytes::{Buf, Bytes};
use futures_io::AsyncRead;
use remotefs::fs::AsyncRemoteRead;

/// Reader over a `GetObject` body.
///
/// A `GetObject` transfer has no completion step, so `finish` is a no-op and
/// dropping the reader simply closes the HTTP body.
pub(crate) struct S3Reader {
    body: Pin<Box<ByteStream>>,
    pending: Bytes,
}

impl S3Reader {
    /// Wrap a response body.
    pub(crate) fn new(body: ByteStream) -> Self {
        Self {
            body: Box::pin(body),
            pending: Bytes::new(),
        }
    }

    /// Create a reader that is already at end of file.
    pub(crate) fn empty() -> Self {
        Self::new(ByteStream::from_static(b""))
    }
}

impl AsyncRead for S3Reader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        if buf.is_empty() {
            return Poll::Ready(Ok(0));
        }
        loop {
            if !self.pending.is_empty() {
                let n = self.pending.len().min(buf.len());
                buf[..n].copy_from_slice(&self.pending[..n]);
                self.pending.advance(n);
                return Poll::Ready(Ok(n));
            }
            match self.body.as_mut().poll_next(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(None) => return Poll::Ready(Ok(0)),
                Poll::Ready(Some(Ok(chunk))) => self.pending = chunk,
                Poll::Ready(Some(Err(error))) => {
                    return Poll::Ready(Err(io::Error::other(error)));
                }
            }
        }
    }
}

#[remotefs::async_trait]
impl AsyncRemoteRead for S3Reader {}

#[cfg(test)]
mod test {
    use futures::io::AsyncReadExt as _;
    use pretty_assertions::assert_eq;

    use super::*;

    #[tokio::test]
    async fn should_read_body_across_small_buffers() {
        let mut reader = S3Reader::new(ByteStream::from(b"hello world".to_vec()));
        let mut out = Vec::new();
        let mut buf = [0_u8; 4];
        loop {
            let n = reader.read(&mut buf).await.unwrap();
            if n == 0 {
                break;
            }
            out.extend_from_slice(&buf[..n]);
        }
        assert_eq!(out, b"hello world");
    }

    #[tokio::test]
    async fn should_read_empty_body() {
        let mut reader = S3Reader::empty();
        let mut out = Vec::new();
        assert_eq!(reader.read_to_end(&mut out).await.unwrap(), 0);
        assert!(!reader.seekable());
    }
}
