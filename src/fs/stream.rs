//! The byte streams returned when a remote file is opened for read or write.
//!
//! [`crate::RemoteFs::open`] hands back a [`ReadStream`] and
//! [`crate::RemoteFs::create`]/[`crate::RemoteFs::append`] hand back a
//! [`WriteStream`]. Both wrap whatever the client actually opened, so a caller
//! sees one concrete type per direction instead of a different boxed trait object
//! per protocol.
//!
//! # Seeking
//!
//! Protocols disagree on random access: SFTP can seek, SCP streams a file once
//! from start to end. Rather than expose two types, each stream carries either a
//! plain reader/writer or one that also implements [`Seek`], and reports which
//! through [`ReadStream::seekable`] and [`WriteStream::seekable`]. Both implement
//! [`Seek`] unconditionally so they can be used where the bound is required;
//! seeking a stream that cannot seek fails with [`IoErrorKind::Unsupported`]
//! instead of panicking. Check `seekable()` first when the answer changes what
//! you do.
//!
//! # Finalization
//!
//! A stream is not finished when it is dropped. Hand it back to
//! [`crate::RemoteFs::on_written`] or [`crate::RemoteFs::on_read`] so the client
//! can complete the exchange the protocol expects — FTP, for one, needs the data
//! connection closed and a final reply read before the transfer counts.
//!
//! Every boxed trait object is [`Send`], so a transfer can be moved to another
//! thread once it has been opened.
//!
//! # Examples
//!
//! ```
//! use std::io::{Read, Seek};
//!
//! use remotefs::fs::ReadStream;
//!
//! # fn f(mut stream: ReadStream) -> std::io::Result<()> {
//! if stream.seekable() {
//!     stream.rewind()?;
//! }
//!
//! let mut buffer = Vec::new();
//! stream.read_to_end(&mut buffer)?;
//! # Ok(())
//! # }
//! ```

use std::io::{Error as IoError, ErrorKind as IoErrorKind, Read, Seek, Write};

// -- read stream

/// A [`Read`] that can also [`Seek`] and be sent across threads.
///
/// Implement this on a protocol's reader to let a client build a seekable
/// [`ReadStream`] from it. The trait has no methods of its own; it exists so the
/// three bounds can be named as one boxed trait object.
pub trait ReadAndSeek: Read + Seek + Send {}

/// A remote file opened for reading.
///
/// Built with [`From`] from either a `Box<dyn Read + Send>` or a
/// `Box<dyn ReadAndSeek>`, depending on whether the protocol supports random
/// access. See the [module documentation](self) for seeking and finalization.
pub struct ReadStream {
    stream: StreamReader,
}

/// Whether the wrapped reader can seek.
enum StreamReader {
    Read(Box<dyn Read + Send>),
    ReadAndSeek(Box<dyn ReadAndSeek>),
}

impl ReadStream {
    /// Return whether [`Seek`] on this stream will succeed.
    pub fn seekable(&self) -> bool {
        matches!(self.stream, StreamReader::ReadAndSeek(_))
    }
}

impl From<Box<dyn Read + Send>> for ReadStream {
    fn from(reader: Box<dyn Read + Send>) -> Self {
        Self {
            stream: StreamReader::Read(reader),
        }
    }
}

impl From<Box<dyn ReadAndSeek>> for ReadStream {
    fn from(reader: Box<dyn ReadAndSeek>) -> Self {
        Self {
            stream: StreamReader::ReadAndSeek(reader),
        }
    }
}

impl Read for ReadStream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.stream.read(buf)
    }
}

/// Fails with [`IoErrorKind::Unsupported`] when the stream is not seekable.
impl Seek for ReadStream {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        self.stream.seek(pos)
    }
}

impl Read for StreamReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::Read(r) => r.read(buf),
            Self::ReadAndSeek(r) => r.read(buf),
        }
    }
}

impl Seek for StreamReader {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        match self {
            Self::Read(_) => Err(IoError::new(
                IoErrorKind::Unsupported, // TODO: change to `NotSeekable` when stable <https://doc.rust-lang.org/stable/std/io/enum.ErrorKind.html#variant.NotSeekable>
                "the read stream for this protocol, doesn't support Seek operation",
            )),
            Self::ReadAndSeek(s) => s.seek(pos),
        }
    }
}

// -- write stream

/// A [`Write`] that can also [`Seek`] and be sent across threads.
///
/// The write-side counterpart of [`ReadAndSeek`].
pub trait WriteAndSeek: Write + Seek + Send {}

/// A remote file opened for writing or appending.
///
/// Built with [`From`] from either a `Box<dyn Write + Send>` or a
/// `Box<dyn WriteAndSeek>`, depending on whether the protocol supports random
/// access. See the [module documentation](self) for seeking and finalization.
pub struct WriteStream {
    /// The wrapped writer, exposed so a client can take it back on finalization.
    pub stream: StreamWriter,
}

/// Whether the wrapped writer can seek.
pub enum StreamWriter {
    /// A writer that cannot seek.
    Write(Box<dyn Write + Send>),
    /// A writer that can seek.
    WriteAndSeek(Box<dyn WriteAndSeek>),
}

impl WriteStream {
    /// Return whether [`Seek`] on this stream will succeed.
    pub fn seekable(&self) -> bool {
        matches!(self.stream, StreamWriter::WriteAndSeek(_))
    }
}

impl From<Box<dyn Write + Send>> for WriteStream {
    fn from(writer: Box<dyn Write + Send>) -> Self {
        Self {
            stream: StreamWriter::Write(writer),
        }
    }
}

impl From<Box<dyn WriteAndSeek>> for WriteStream {
    fn from(writer: Box<dyn WriteAndSeek>) -> Self {
        Self {
            stream: StreamWriter::WriteAndSeek(writer),
        }
    }
}

impl Write for WriteStream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.stream.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.stream.flush()
    }
}

/// Fails with [`IoErrorKind::Unsupported`] when the stream is not seekable.
impl Seek for WriteStream {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        self.stream.seek(pos)
    }
}

impl Write for StreamWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Self::Write(w) => w.write(buf),
            Self::WriteAndSeek(w) => w.write(buf),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Self::Write(w) => w.flush(),
            Self::WriteAndSeek(w) => w.flush(),
        }
    }
}

impl Seek for StreamWriter {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        match self {
            Self::Write(_) => Err(IoError::new(
                IoErrorKind::Unsupported, // TODO: change to `NotSeekable` when stable <https://doc.rust-lang.org/stable/std/io/enum.ErrorKind.html#variant.NotSeekable>
                "the read stream for this protocol, doesn't support Seek operation",
            )),
            Self::WriteAndSeek(s) => s.seek(pos),
        }
    }
}

#[cfg(test)]
mod test {

    use std::fs::File;

    use tempfile::NamedTempFile;

    use super::*;

    impl ReadAndSeek for File {}
    impl WriteAndSeek for File {}

    #[test]
    fn should_create_new_read_stream_from_read() {
        let temp = NamedTempFile::new().expect("Could not make tempfile");
        let file: Box<dyn Read + Send> =
            Box::new(File::open(temp.path()).expect("Could not open tempfile"));
        let s = ReadStream::from(file);
        assert_eq!(s.seekable(), false);
    }

    #[test]
    fn should_create_new_read_stream_from_read_and_seek() {
        let temp = NamedTempFile::new().expect("Could not make tempfile");
        let file: Box<dyn ReadAndSeek> =
            Box::new(File::open(temp.path()).expect("Could not open tempfile"));
        let s = ReadStream::from(file);
        assert_eq!(s.seekable(), true);
    }

    #[test]
    fn should_create_new_write_stream_from_write() {
        let temp = NamedTempFile::new().expect("Could not make tempfile");
        let file: Box<dyn Write + Send> =
            Box::new(File::create(temp.path()).expect("Could not open tempfile"));
        let s = WriteStream::from(file);
        assert_eq!(s.seekable(), false);
    }

    #[test]
    fn should_create_new_write_stream_from_write_and_seek() {
        let temp = NamedTempFile::new().expect("Could not make tempfile");
        let file: Box<dyn WriteAndSeek> =
            Box::new(File::create(temp.path()).expect("Could not open tempfile"));
        let s = WriteStream::from(file);
        assert_eq!(s.seekable(), true);
    }
}
