use std::fs::File;
use std::io::{Read, Seek, Write};

use remotefs::fs::stream::{ReadAndSeek, WriteAndSeek};

pub struct FileStream {
    file: File,
}

impl From<File> for FileStream {
    fn from(file: File) -> Self {
        Self { file }
    }
}

impl Read for FileStream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.file.read(buf)
    }
}

impl Seek for FileStream {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        self.file.seek(pos)
    }
}

impl ReadAndSeek for FileStream {}

impl Write for FileStream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.file.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

impl WriteAndSeek for FileStream {}
