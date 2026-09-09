//! Owned streams for remote transfers and their completion contract.
//!
//! A backend implements [`RemoteRead`] or [`RemoteWrite`] for its stream type,
//! then wraps it with [`ReadStream::new`] or [`WriteStream::new`]. The wrapper
//! delegates standard I/O and consumes the backend stream exactly once when
//! [`ReadStream::finish`] or [`WriteStream::finish`] is called.
//!
//! Seeking is always available as a trait method, but returns
//! [`std::io::ErrorKind::Unsupported`] for a stream whose backend does not
//! support it. The `seekable` query lets callers choose a compatible strategy.
//! Dropping a stream does not call its finalizer, because a dropped transfer
//! may intentionally be abandoned.

use std::fmt;
use std::io::{self, Read, Seek, SeekFrom, Write};

use super::{RemoteError, RemoteResult};

#[cfg(feature = "async")]
pub mod r#async;

/// A remote reader that can optionally seek and finalize its transfer.
pub trait RemoteRead: Read + Send {
    /// Returns whether this reader supports seeking.
    fn seekable(&self) -> bool {
        false
    }

    /// Seeks to a position in the remote stream.
    ///
    /// The default implementation returns [`io::ErrorKind::Unsupported`].
    fn seek(&mut self, _position: SeekFrom) -> io::Result<u64> {
        Err(io::ErrorKind::Unsupported.into())
    }

    /// Finalizes the remote read operation.
    ///
    /// The default implementation has no protocol-specific work to perform.
    fn finish(self: Box<Self>) -> RemoteResult<()> {
        Ok(())
    }
}

/// A remote writer that can optionally seek and finalize its transfer.
pub trait RemoteWrite: Write + Send {
    /// Returns whether this writer supports seeking.
    fn seekable(&self) -> bool {
        false
    }

    /// Seeks to a position in the remote stream.
    ///
    /// The default implementation returns [`io::ErrorKind::Unsupported`].
    fn seek(&mut self, _position: SeekFrom) -> io::Result<u64> {
        Err(io::ErrorKind::Unsupported.into())
    }

    /// Finalizes the remote write operation.
    ///
    /// The default implementation has no protocol-specific work to perform.
    fn finish(self: Box<Self>) -> RemoteResult<()> {
        Ok(())
    }
}

/// An owned remote reader with an explicit transfer finalizer.
#[non_exhaustive]
#[must_use = "call finish() so the backend can complete the transfer"]
pub struct ReadStream(Box<dyn RemoteRead>);

impl ReadStream {
    /// Wraps a backend reader in an owned remote stream.
    pub fn new(inner: impl RemoteRead + 'static) -> Self {
        Self(Box::new(inner))
    }

    /// Returns whether the wrapped reader supports seeking.
    pub fn seekable(&self) -> bool {
        self.0.seekable()
    }

    /// Completes the remote read and consumes the stream.
    ///
    /// # Errors
    ///
    /// Returns a backend finalization error when the remote transfer did not
    /// complete successfully.
    pub fn finish(self) -> RemoteResult<()> {
        self.0.finish()
    }
}

impl fmt::Debug for ReadStream {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReadStream")
            .field("seekable", &self.seekable())
            .finish()
    }
}

impl Read for ReadStream {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.0.read(buffer)
    }
}

impl Seek for ReadStream {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.0.seek(position)
    }
}

/// An owned remote writer with an explicit transfer finalizer.
#[non_exhaustive]
#[must_use = "call finish() so the backend can complete the transfer"]
pub struct WriteStream(Box<dyn RemoteWrite>);

/// Combines transfer and finalization failures without discarding either.
pub(crate) fn complete_transfer(
    copied: RemoteResult<u64>,
    finished: RemoteResult<()>,
) -> RemoteResult<u64> {
    match (copied, finished) {
        (Ok(count), Ok(())) => Ok(count),
        (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
        (Err(copy), Err(finish)) => {
            let kind = copy.kind();
            Err(RemoteError::with_source(
                kind,
                TransferFailure { copy, finish },
            ))
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("{copy}; transfer finalization also failed: {finish}")]
struct TransferFailure {
    #[source]
    copy: RemoteError,
    finish: RemoteError,
}

impl WriteStream {
    /// Wraps a backend writer in an owned remote stream.
    pub fn new(inner: impl RemoteWrite + 'static) -> Self {
        Self(Box::new(inner))
    }

    /// Returns whether the wrapped writer supports seeking.
    pub fn seekable(&self) -> bool {
        self.0.seekable()
    }

    /// Completes the remote write and consumes the stream.
    ///
    /// # Errors
    ///
    /// Returns a backend finalization error when the remote transfer did not
    /// complete successfully.
    pub fn finish(self) -> RemoteResult<()> {
        self.0.finish()
    }
}

impl fmt::Debug for WriteStream {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WriteStream")
            .field("seekable", &self.seekable())
            .finish()
    }
}

impl Write for WriteStream {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.0.write(buffer)
    }

    fn write_vectored(&mut self, buffers: &[io::IoSlice<'_>]) -> io::Result<usize> {
        self.0.write_vectored(buffers)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

impl Seek for WriteStream {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.0.seek(position)
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    impl RemoteRead for Cursor<Vec<u8>> {}
    impl RemoteWrite for Cursor<Vec<u8>> {}

    #[test]
    fn streams_delegate_io_and_default_to_nonseekable() {
        let mut reader = ReadStream::new(Cursor::new(b"hello".to_vec()));
        let mut buffer = Vec::new();
        reader.read_to_end(&mut buffer).unwrap();
        assert_eq!(buffer, b"hello");
        assert!(!reader.seekable());
        assert_eq!(
            reader.seek(SeekFrom::Start(0)).unwrap_err().kind(),
            io::ErrorKind::Unsupported
        );

        let mut writer = WriteStream::new(Cursor::new(Vec::new()));
        writer.write_all(b"hello").unwrap();
        writer.flush().unwrap();
        assert!(!writer.seekable());
        assert_eq!(
            writer.seek(SeekFrom::Start(0)).unwrap_err().kind(),
            io::ErrorKind::Unsupported
        );
    }
}
