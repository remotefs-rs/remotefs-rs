//! ## stream
//!
//! ssh file stream

use std::io::{Read, Seek, Write};

use remotefs::fs::stream::{ReadAndSeek, ReadStream, WriteAndSeek, WriteStream};
use ssh2::File as Ssh2File;

// -- read stream

pub struct SftpReadStream {
    file: Ssh2File,
}

impl From<Ssh2File> for SftpReadStream {
    fn from(file: Ssh2File) -> Self {
        Self { file }
    }
}

impl Read for SftpReadStream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.file.read(buf)
    }
}

impl Seek for SftpReadStream {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        self.file.seek(pos)
    }
}

impl ReadAndSeek for SftpReadStream {}

impl From<SftpReadStream> for ReadStream {
    fn from(stream: SftpReadStream) -> Self {
        ReadStream::from(Box::new(stream) as Box<dyn ReadAndSeek>)
    }
}

// -- write stream

pub struct SftpWriteStream {
    file: Ssh2File,
}

impl From<Ssh2File> for SftpWriteStream {
    fn from(file: Ssh2File) -> Self {
        Self { file }
    }
}

impl Write for SftpWriteStream {
    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }

    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.file.write(buf)
    }
}

impl Seek for SftpWriteStream {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        self.file.seek(pos)
    }
}

impl WriteAndSeek for SftpWriteStream {}

impl From<SftpWriteStream> for WriteStream {
    fn from(stream: SftpWriteStream) -> Self {
        WriteStream::from(Box::new(stream) as Box<dyn WriteAndSeek>)
    }
}
