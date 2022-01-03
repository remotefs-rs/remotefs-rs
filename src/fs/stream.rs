//! ## Stream
//!
//! this module exposes the streams returned by create, append and open methods

/**
 * MIT License
 *
 * remotefs - Copyright (c) 2021 Christian Visintin
 *
 * Permission is hereby granted, free of charge, to any person obtaining a copy
 * of this software and associated documentation files (the "Software"), to deal
 * in the Software without restriction, including without limitation the rights
 * to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
 * copies of the Software, and to permit persons to whom the Software is
 * furnished to do so, subject to the following conditions:
 *
 * The above copyright notice and this permission notice shall be included in all
 * copies or substantial portions of the Software.
 *
 * THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
 * IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
 * FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
 * AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
 * LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
 * OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
 * SOFTWARE.
 */
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
    /// Instantiates a new `ReadStream` which supports the `Read` trait only.
    pub fn read(reader: Box<dyn Read>) -> Self {
        Self {
            stream: StreamReader::Read(reader),
        }
    }

    /// Instantiates a new `ReadStream` which supports both `Read` and `Seek` traits.
    pub fn read_and_seek(reader: Box<dyn ReadAndSeek>) -> Self {
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
    /// Instantiates a new `WriteStream` which supports the `Write` trait only.
    pub fn write(writer: Box<dyn Write>) -> Self {
        Self {
            stream: StreamWriter::Write(writer),
        }
    }

    /// Instantiates a new `WriteStream` which supports both `Write` and `Seek` traits.
    pub fn write_and_seek(writer: Box<dyn WriteAndSeek>) -> Self {
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
