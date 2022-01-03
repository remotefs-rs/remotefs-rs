//! ## stream
//!
//! ssh file stream

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
use remotefs::fs::stream::{ReadAndSeek, ReadStream, WriteAndSeek, WriteStream};
use ssh2::File as Ssh2File;
use std::io::{Read, Seek, Write};

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
