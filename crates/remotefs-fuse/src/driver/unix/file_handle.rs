use std::collections::HashMap;

use remotefs::File;
use remotefs::fs::WriteStream;

use super::inode::Inode;

/// Pid is a process identifier.
pub type Pid = u32;
/// Fh is a file handle number.
pub type Fh = u64;

/// Staged state for a write in progress on a file handle, spanning possibly many `write()`
/// calls between `open()` and `flush()`/`release()`. The remote write is only finalized (and,
/// for [`PendingWriteState::Buffered`], actually uploaded) once the handle is flushed.
pub(crate) struct PendingWrite {
    pub(crate) file: File,
    pub(crate) state: PendingWriteState,
}

pub(crate) enum PendingWriteState {
    /// The remote exposes a streaming writer, opened once and kept alive across writes.
    /// `next_offset` is the stream's current write cursor, used to seek only when a write isn't
    /// a simple continuation of the previous one.
    Stream {
        stream: WriteStream,
        next_offset: u64,
    },
    /// The remote doesn't support streaming writes; data is staged in memory and uploaded as a
    /// single write when the handle is flushed.
    Buffered(Vec<u8>),
}

/// FileHandlersDb is a database of file handles for each process.
#[derive(Default)]
pub struct FileHandlersDb {
    /// Database of file handles for each process.
    handlers: HashMap<Pid, ProcessFileHandlers>,
}

impl std::fmt::Debug for FileHandlersDb {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileHandlersDb")
            .field("handlers", &self.handlers.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl FileHandlersDb {
    /// Open a new file handle into the database.
    pub fn open(&mut self, pid: Pid, inode: Inode, read: bool, write: bool) -> u64 {
        let fh = self
            .handlers
            .entry(pid)
            .or_default()
            .open(inode, read, write);

        debug!(
            "opened file handle {fh} for pid {pid} and inode {inode}; read: {read}, write: {write}",
        );

        fh
    }

    /// Get a file handle from the database.
    pub fn get(&self, pid: Pid, fh: u64) -> Option<&FileHandle> {
        self.handlers
            .get(&pid)
            .and_then(|handlers| handlers.get(fh))
    }

    /// Close a file handle.
    pub fn close(&mut self, pid: Pid, fh: u64) {
        if let Some(handlers) = self.handlers.get_mut(&pid) {
            debug!("closing file handle {fh} for pid {pid}");
            handlers.close(fh);
        }

        // remove the process if it has no more file handles
        if self
            .handlers
            .get(&pid)
            .map(|handlers| handlers.handles.is_empty())
            .unwrap_or_default()
        {
            debug!("removing file handlers for pid {pid}");
            self.handlers.remove(&pid);
        }
    }

    /// Get the pending write staged for a file handle, if a write has started one.
    pub(crate) fn pending_write(&mut self, pid: Pid, fh: Fh) -> Option<&mut PendingWrite> {
        self.handlers.get_mut(&pid)?.pending_writes.get_mut(&fh)
    }

    /// Stage a new pending write for a file handle. Replaces any previous one for the same
    /// handle, if any.
    pub(crate) fn start_pending_write(&mut self, pid: Pid, fh: Fh, pending: PendingWrite) {
        self.handlers
            .entry(pid)
            .or_default()
            .pending_writes
            .insert(fh, pending);
    }

    /// Remove and return the pending write staged for a file handle, if any.
    pub(crate) fn take_pending_write(&mut self, pid: Pid, fh: Fh) -> Option<PendingWrite> {
        self.handlers.get_mut(&pid)?.pending_writes.remove(&fh)
    }
}

/// ProcessFileHandlers is a database of file handles. It is used to store file handles for open files.
///
/// It is a map between the file handle number and the [`FileHandle`] struct.
#[derive(Default)]
struct ProcessFileHandlers {
    handles: HashMap<Fh, FileHandle>,
    /// Next file handle number that has never been assigned before
    next: u64,
    /// Previously assigned file handle numbers that were closed and can be reused
    free: Vec<Fh>,
    /// Write staged for a handle but not yet flushed to the remote.
    pending_writes: HashMap<Fh, PendingWrite>,
}

/// FileHandle is a handle to an open file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileHandle {
    /// Inode of the file
    pub inode: Inode,
    /// Read permission
    pub read: bool,
    /// Write permission
    pub write: bool,
}

impl ProcessFileHandlers {
    /// Open a new [`FileHandle`] into the database.
    ///
    /// Returns the created file handle number.
    fn open(&mut self, inode: Inode, read: bool, write: bool) -> u64 {
        let fh = self.free.pop().unwrap_or_else(|| {
            let fh = self.next;
            self.next += 1;
            fh
        });
        self.handles.insert(fh, FileHandle { inode, read, write });

        fh
    }

    /// Get a [`FileHandle`] from the database.
    fn get(&self, fh: u64) -> Option<&FileHandle> {
        self.handles.get(&fh)
    }

    /// Close a file handle.
    ///
    /// This will remove the file handle from the database.
    /// The file handle number becomes eligible for reuse by a future [`ProcessFileHandlers::open`] call.
    fn close(&mut self, fh: u64) {
        if self.handles.remove(&fh).is_some() {
            self.free.push(fh);
        }
        self.pending_writes.remove(&fh);
    }
}

#[cfg(test)]
mod test {

    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn test_should_store_handlers_for_pid() {
        let mut db = FileHandlersDb::default();

        let fh = db.open(1, 1, true, false);
        assert_eq!(
            db.get(1, fh),
            Some(&FileHandle {
                inode: 1,
                read: true,
                write: false
            })
        );

        assert_eq!(db.get(2, fh), None);

        let fh = db.open(1, 2, true, false);
        assert_eq!(
            db.get(1, fh),
            Some(&FileHandle {
                inode: 2,
                read: true,
                write: false
            })
        );

        let fh = db.open(2, 3, true, false);

        assert_eq!(
            db.get(2, fh),
            Some(&FileHandle {
                inode: 3,
                read: true,
                write: false
            })
        );
    }

    #[test]
    fn test_should_remove_pid_if_has_no_more_handles() {
        let mut db = FileHandlersDb::default();

        let fh = db.open(1, 1, true, false);
        assert_eq!(
            db.get(1, fh),
            Some(&FileHandle {
                inode: 1,
                read: true,
                write: false
            })
        );

        db.close(1, fh);
        assert_eq!(db.get(1, fh), None);

        db.open(1, 2, true, false);
        db.open(1, 3, true, false);
        db.close(1, 2);

        assert!(db.handlers.contains_key(&1));
    }

    #[test]
    fn test_file_handle_db() {
        let mut db = ProcessFileHandlers::default();

        let fh = db.open(1, true, false);
        assert_eq!(
            db.get(fh),
            Some(&FileHandle {
                inode: 1,
                read: true,
                write: false
            })
        );

        db.close(fh);
        assert_eq!(db.get(fh), None);
    }

    #[test]
    fn test_should_reuse_fhs() {
        let mut db = ProcessFileHandlers::default();

        let _fh1 = db.open(1, true, false);
        let fh2 = db.open(2, true, false);
        let _fh3 = db.open(3, true, false);

        db.close(fh2);

        let fh4 = db.open(4, true, false);

        assert_eq!(fh4, fh2);
        assert_eq!(
            db.get(fh2),
            Some(&FileHandle {
                inode: 4,
                read: true,
                write: false
            })
        );

        // next should be 3
        let fh5 = db.open(5, true, false);
        assert_eq!(fh5, 3);
    }

    #[test]
    fn test_should_not_collide_still_open_fh_after_closing_lower_fhs() {
        let mut db = ProcessFileHandlers::default();

        let fh0 = db.open(10, true, false);
        let fh1 = db.open(11, true, false);
        let fh2 = db.open(12, true, false);

        db.close(fh1);
        db.close(fh0);

        let reopened_a = db.open(20, true, false);
        let reopened_b = db.open(21, true, false);

        assert_ne!(reopened_a, fh2);
        assert_ne!(reopened_b, fh2);
        assert_eq!(
            db.get(fh2),
            Some(&FileHandle {
                inode: 12,
                read: true,
                write: false
            })
        );
    }
}
