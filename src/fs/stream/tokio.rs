//! Tokio compatibility helpers for remote transfer streams.

use std::pin::Pin;
use std::task::{Context, Poll};

use futures_io::{AsyncRead, AsyncWrite};
use tokio_util::compat::{Compat, TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

use super::r#async::{AsyncRemoteRead, AsyncRemoteWrite};

pub(super) struct TokioReader<R> {
    inner: Compat<R>,
}

impl<R> TokioReader<R> {
    pub(super) fn new(inner: R) -> Self
    where
        R: tokio::io::AsyncRead,
    {
        Self {
            inner: inner.compat(),
        }
    }
}

impl<R> AsyncRead for TokioReader<R>
where
    R: tokio::io::AsyncRead + Unpin,
{
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut [u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.inner).poll_read(context, buffer)
    }
}

#[async_trait::async_trait]
impl<R> AsyncRemoteRead for TokioReader<R> where R: tokio::io::AsyncRead + Send + Unpin + 'static {}

pub(super) struct TokioWriter<R> {
    inner: Compat<R>,
}

impl<R> TokioWriter<R> {
    pub(super) fn new(inner: R) -> Self
    where
        R: tokio::io::AsyncWrite,
    {
        Self {
            inner: inner.compat_write(),
        }
    }
}

impl<R> AsyncWrite for TokioWriter<R>
where
    R: tokio::io::AsyncWrite + Unpin,
{
    fn poll_write(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(context, buffer)
    }

    fn poll_flush(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(context)
    }

    fn poll_close(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_close(context)
    }
}

#[async_trait::async_trait]
impl<R> AsyncRemoteWrite for TokioWriter<R> where R: tokio::io::AsyncWrite + Send + Unpin + 'static {}
