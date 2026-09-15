//! Owned blocking streams over a file opened through the Windows redirector.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::time::SystemTime;

use remotefs::fs::{RemoteRead, RemoteWrite};
use remotefs::{RemoteError, RemoteResult};

/// Reader that stops after an optional byte budget.
pub struct WNetReadStream {
    file: File,
    remaining: Option<u64>,
}

impl WNetReadStream {
    /// Wraps `file`, already positioned at the requested offset.
    pub fn new(file: File, remaining: Option<u64>) -> Self {
        Self { file, remaining }
    }
}

impl Read for WNetReadStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let allowed = match self.remaining {
            Some(0) => return Ok(0),
            Some(remaining) => {
                usize::try_from(remaining).map_or(buf.len(), |limit| limit.min(buf.len()))
            }
            None => buf.len(),
        };
        let count = self.file.read(&mut buf[..allowed])?;
        if let Some(remaining) = self.remaining.as_mut() {
            *remaining -= count as u64;
        }
        Ok(count)
    }
}

impl RemoteRead for WNetReadStream {
    fn seekable(&self) -> bool {
        true
    }

    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.file.seek(position)
    }
}

/// Writer that syncs the file on `finish`.
pub struct WNetWriteStream {
    file: File,
    modified: Option<SystemTime>,
}

impl WNetWriteStream {
    /// Wraps an open, writable `file`.
    pub fn new(file: File, modified: Option<SystemTime>) -> Self {
        Self { file, modified }
    }
}

impl Write for WNetWriteStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.file.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

impl RemoteWrite for WNetWriteStream {
    fn seekable(&self) -> bool {
        true
    }

    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.file.seek(position)
    }

    fn finish(self: Box<Self>) -> RemoteResult<()> {
        let mut this = *self;
        this.file.flush().map_err(RemoteError::from)?;
        this.file.sync_all().map_err(RemoteError::from)?;
        if let Some(modified) = this.modified {
            this.file
                .set_modified(modified)
                .map_err(RemoteError::from)?;
            this.file.sync_all().map_err(RemoteError::from)?;
        }
        Ok(())
    }
}
