//! Stream fixtures backed by the in-memory filesystem state.

use std::io::{self, Cursor, Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::Arc;

use super::{Entry, State};
use crate::fs::{FileType, Metadata, RemoteError, RemoteRead, RemoteResult, RemoteWrite};

/// A seekable or non-seekable reader over staged fixture bytes.
pub(crate) struct MockReader {
    state: Arc<std::sync::Mutex<State>>,
    cursor: Cursor<Vec<u8>>,
    seekable: bool,
    finished: bool,
}

impl MockReader {
    pub(crate) fn new(state: Arc<std::sync::Mutex<State>>, bytes: Vec<u8>, seekable: bool) -> Self {
        Self {
            state,
            cursor: Cursor::new(bytes),
            seekable,
            finished: false,
        }
    }
}

impl Read for MockReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.cursor.read(buffer)
    }
}

impl RemoteRead for MockReader {
    fn seekable(&self) -> bool {
        self.seekable
    }

    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        if self.seekable {
            Seek::seek(&mut self.cursor, position)
        } else {
            Err(io::ErrorKind::Unsupported.into())
        }
    }

    fn finish(mut self: Box<Self>) -> RemoteResult<()> {
        self.finished = true;
        let mut state = self.state.lock().expect("mock state lock poisoned");
        state.finish_count += 1;
        match state.fail_finish {
            Some(kind) => Err(RemoteError::new(kind)),
            None => Ok(()),
        }
    }
}

impl Drop for MockReader {
    fn drop(&mut self) {
        if !self.finished
            && let Ok(mut state) = self.state.lock()
        {
            state.unfinished_count += 1;
        }
    }
}

/// A staged writer that commits bytes when its stream is finished.
pub(crate) struct MockWriter {
    state: Arc<std::sync::Mutex<State>>,
    path: PathBuf,
    cursor: Cursor<Vec<u8>>,
    metadata: Metadata,
    seekable: bool,
    finished: bool,
}

impl MockWriter {
    pub(crate) fn new(
        state: Arc<std::sync::Mutex<State>>,
        path: PathBuf,
        bytes: Vec<u8>,
        metadata: Metadata,
        seekable: bool,
    ) -> Self {
        Self {
            state,
            path,
            cursor: Cursor::new(bytes),
            metadata,
            seekable,
            finished: false,
        }
    }

    pub(crate) fn seek_to_end(&mut self) {
        Seek::seek(&mut self.cursor, SeekFrom::End(0)).expect("cursor end is always seekable");
    }
}

impl Write for MockWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.cursor.write(buffer)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.cursor.flush()
    }
}

impl Seek for MockWriter {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        if self.seekable {
            Seek::seek(&mut self.cursor, position)
        } else {
            Err(io::ErrorKind::Unsupported.into())
        }
    }
}

impl RemoteWrite for MockWriter {
    fn seekable(&self) -> bool {
        self.seekable
    }

    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        Seek::seek(self, position)
    }

    fn finish(mut self: Box<Self>) -> RemoteResult<()> {
        self.finished = true;
        let mut state = self.state.lock().expect("mock state lock poisoned");
        state.finish_count += 1;
        if let Some(kind) = state.fail_finish {
            return Err(RemoteError::new(kind));
        }
        let mut metadata = std::mem::take(&mut self.metadata);
        metadata.file_type = FileType::File;
        metadata.size = Some(self.cursor.get_ref().len() as u64);
        metadata.symlink = None;
        state.entries.insert(
            self.path.clone(),
            Entry {
                metadata,
                bytes: std::mem::take(self.cursor.get_mut()),
            },
        );
        Ok(())
    }
}

impl Drop for MockWriter {
    fn drop(&mut self) {
        if !self.finished
            && let Ok(mut state) = self.state.lock()
        {
            state.unfinished_count += 1;
        }
    }
}
