//! Owned asynchronous reader over a `ReadObject` response.
//!
//! The server already limits the byte range, so `finish` has no protocol work
//! to do; dropping the reader closes the response body. A client-side limit is
//! applied as well so a permissive emulator cannot return more than requested.

use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::{Buf, Bytes};
use futures_io::AsyncRead;
use google_cloud_storage::read_object::ReadObjectResponse;
use remotefs::fs::AsyncRemoteRead;

type NextChunk = Option<google_cloud_storage::Result<Bytes>>;
type NextFuture = Pin<Box<dyn Future<Output = (ReadObjectResponse, NextChunk)> + Send>>;

enum State {
    Idle(ReadObjectResponse),
    Fetching(NextFuture),
    Done,
}

pub(crate) struct GcsReader {
    state: State,
    pending: Bytes,
    remaining: Option<u64>,
}

impl GcsReader {
    /// Wraps a response, returning at most `limit` bytes when a limit is set.
    pub(crate) fn new(response: ReadObjectResponse, limit: Option<u64>) -> Self {
        Self {
            state: State::Idle(response),
            pending: Bytes::new(),
            remaining: limit,
        }
    }

    /// Creates a reader that is already at end of file.
    pub(crate) fn empty() -> Self {
        Self {
            state: State::Done,
            pending: Bytes::new(),
            remaining: Some(0),
        }
    }

    fn copy_pending(&mut self, buffer: &mut [u8]) -> usize {
        let allowed = self.remaining.map_or(buffer.len(), |remaining| {
            usize::try_from(remaining).map_or(buffer.len(), |limit| limit.min(buffer.len()))
        });
        let count = self.pending.len().min(allowed);
        buffer[..count].copy_from_slice(&self.pending[..count]);
        self.pending.advance(count);
        if let Some(remaining) = self.remaining.as_mut() {
            *remaining -= count as u64;
        }
        count
    }
}

impl AsyncRead for GcsReader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        if buf.is_empty() || self.remaining == Some(0) {
            return Poll::Ready(Ok(0));
        }
        loop {
            if !self.pending.is_empty() {
                return Poll::Ready(Ok(self.copy_pending(buf)));
            }
            match std::mem::replace(&mut self.state, State::Done) {
                State::Done => return Poll::Ready(Ok(0)),
                State::Idle(mut response) => {
                    self.state = State::Fetching(Box::pin(async move {
                        let chunk = response.next().await;
                        (response, chunk)
                    }));
                }
                State::Fetching(mut future) => match future.as_mut().poll(cx) {
                    Poll::Pending => {
                        self.state = State::Fetching(future);
                        return Poll::Pending;
                    }
                    Poll::Ready((_, None)) => return Poll::Ready(Ok(0)),
                    Poll::Ready((_, Some(Err(error)))) => {
                        return Poll::Ready(Err(io::Error::other(error)));
                    }
                    Poll::Ready((response, Some(Ok(chunk)))) => {
                        self.state = State::Idle(response);
                        self.pending = chunk;
                    }
                },
            }
        }
    }
}

#[remotefs::async_trait]
impl AsyncRemoteRead for GcsReader {}

#[cfg(test)]
mod tests {
    use futures::io::AsyncReadExt as _;
    use google_cloud_storage::model_ext::ObjectHighlights;
    use pretty_assertions::assert_eq;

    use super::*;

    fn response(payload: &'static str) -> ReadObjectResponse {
        ReadObjectResponse::from_source(ObjectHighlights::default(), payload)
    }

    #[tokio::test]
    async fn reads_across_small_buffers() {
        let mut reader = GcsReader::new(response("hello world"), None);
        let mut output = Vec::new();
        let mut buffer = [0_u8; 4];
        loop {
            let count = reader.read(&mut buffer).await.unwrap();
            if count == 0 {
                break;
            }
            output.extend_from_slice(&buffer[..count]);
        }
        assert_eq!(output, b"hello world");
        assert!(!reader.seekable());
    }

    #[tokio::test]
    async fn client_side_limit_caps_the_output() {
        let mut reader = GcsReader::new(response("hello world"), Some(5));
        let mut output = Vec::new();
        reader.read_to_end(&mut output).await.unwrap();
        assert_eq!(output, b"hello");
    }

    #[tokio::test]
    async fn empty_reader_is_at_eof() {
        let mut reader = GcsReader::empty();
        let mut output = Vec::new();
        assert_eq!(reader.read_to_end(&mut output).await.unwrap(), 0);
        Box::new(reader).finish().await.unwrap();
    }
}
