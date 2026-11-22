//! Owned read and write streams over the in-memory tree.
//!
//! [`MemoryReader`] reads a snapshot of a file's bytes and is always seekable.
//! [`MemoryWriter`] stages bytes in a cursor and commits them into the shared
//! tree when its stream is finished; a dropped writer discards the staged
//! bytes.

use std::io::{self, Cursor, Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use remotefs::fs::{RemoteRead, RemoteWrite};
use remotefs::{RemoteError, RemoteErrorType, RemoteResult};

use crate::{FsTree, lock_tree};

/// Seekable reader over a snapshot of a file's bytes.
pub(crate) struct MemoryReader {
    cursor: Cursor<Vec<u8>>,
}

impl MemoryReader {
    /// Creates a reader over `bytes`.
    pub(crate) fn new(bytes: Vec<u8>) -> Self {
        Self {
            cursor: Cursor::new(bytes),
        }
    }
}

impl Read for MemoryReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.cursor.read(buffer)
    }
}

impl RemoteRead for MemoryReader {
    fn seekable(&self) -> bool {
        true
    }

    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        Seek::seek(&mut self.cursor, position)
    }
}

/// Writer that stages bytes and commits them into the tree on `finish`.
pub(crate) struct MemoryWriter {
    tree: Arc<Mutex<FsTree>>,
    path: PathBuf,
    cursor: Cursor<Vec<u8>>,
    modified: Option<SystemTime>,
    identity: Arc<()>,
    finished: bool,
}

impl MemoryWriter {
    /// Creates a writer for `path` whose buffer starts as `initial`.
    ///
    /// `modified` overrides the modification time applied on `finish`.
    pub(crate) fn new(
        tree: Arc<Mutex<FsTree>>,
        path: PathBuf,
        initial: Vec<u8>,
        modified: Option<SystemTime>,
        identity: Arc<()>,
    ) -> Self {
        Self {
            tree,
            path,
            cursor: Cursor::new(initial),
            modified,
            identity,
            finished: false,
        }
    }

    /// Moves the cursor to the end of the staged bytes.
    pub(crate) fn seek_to_end(&mut self) {
        let end = self.cursor.get_ref().len() as u64;
        self.cursor.set_position(end);
    }
}

impl Write for MemoryWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.cursor.write(buffer)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.cursor.flush()
    }
}

impl RemoteWrite for MemoryWriter {
    fn seekable(&self) -> bool {
        true
    }

    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        Seek::seek(&mut self.cursor, position)
    }

    fn finish(mut self: Box<Self>) -> RemoteResult<()> {
        self.finished = true;
        let bytes = std::mem::take(self.cursor.get_mut());
        let mut tree = lock_tree(&self.tree)?;
        let node = tree
            .root_mut()
            .query_mut(&self.path)
            .ok_or_else(|| RemoteError::new(RemoteErrorType::NoSuchFileOrDirectory))?;
        if !node.value().has_identity(&self.identity) {
            return Err(RemoteError::with_message(
                RemoteErrorType::ProtocolError,
                "file changed while write stream was open",
            ));
        }
        let mut inode = node.value().clone();
        if inode.metadata().is_dir() {
            return Err(RemoteError::with_message(
                RemoteErrorType::BadFile,
                "cannot write a directory",
            ));
        }
        debug!(
            "committing {len} bytes to {path:?}",
            len = bytes.len(),
            path = self.path
        );
        inode.metadata.size = Some(bytes.len() as u64);
        inode.metadata.modified = Some(self.modified.unwrap_or_else(SystemTime::now));
        inode.content = Some(bytes);
        node.set_value(inode);
        Ok(())
    }
}

impl Drop for MemoryWriter {
    fn drop(&mut self) {
        if !self.finished {
            debug!(
                "write stream for {path:?} dropped without finish; discarding {len} staged bytes",
                path = self.path,
                len = self.cursor.get_ref().len()
            );
        }
    }
}
