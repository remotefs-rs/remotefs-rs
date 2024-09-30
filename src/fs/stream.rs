//! ## Stream
//!
//! this module exposes the streams returned by create, append and open methods

use std::io::{Error as IoError, ErrorKind as IoErrorKind, Read, Seek, Write};

// -- read stream

/// A trait which combines `io::Read` and `io::Seek` together
pub trait ReadAndSeek: Read + Seek {}

/// The stream returned by RemoteFs to read a file from the remote server
pub struct ReadStream {
    stream: StreamReader,
}

/// The kind of stream contained in the stream. Can be Read only or Read + Seek
enum StreamReader {
    Read(Box<dyn Read>),
    ReadAndSeek(Box<dyn ReadAndSeek>),
}

impl ReadStream {
    /// Returns whether `ReadStream` is seekable
    pub fn seekable(&self) -> bool {
        matches!(self.stream, StreamReader::ReadAndSeek(_))
    }
}

impl From<Box<dyn Read>> for ReadStream {
    fn from(reader: Box<dyn Read>) -> Self {
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

/// A trait which combines `io::Write` and `io::Seek` together
pub trait WriteAndSeek: Write + Seek {}

/// The stream returned by RemoteFs to write a file from the remote server
pub struct WriteStream {
    stream: StreamWriter,
}

/// The kind of stream contained in the stream. Can be Write only or Write + Seek
enum StreamWriter {
    Write(Box<dyn Write>),
    WriteAndSeek(Box<dyn WriteAndSeek>),
}

impl WriteStream {
    /// Returns whether `WriteStream` is seekable
    pub fn seekable(&self) -> bool {
        matches!(self.stream, StreamWriter::WriteAndSeek(_))
    }
}

impl From<Box<dyn Write>> for WriteStream {
    fn from(writer: Box<dyn Write>) -> Self {
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
        let file: Box<dyn Read> =
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
        let file: Box<dyn Write> =
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
