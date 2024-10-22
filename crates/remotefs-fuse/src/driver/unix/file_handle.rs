use std::collections::HashMap;

use super::inode::Inode;

/// Pid is a process identifier.
pub type Pid = u32;
/// Fh is a file handle number.
pub type Fh = u64;

/// FileHandlersDb is a database of file handles for each process.
#[derive(Debug, Default)]
pub struct FileHandlersDb {
    /// Database of file handles for each process.
    handlers: HashMap<Pid, ProcessFileHandlers>,
}

impl FileHandlersDb {
    /// Put a new file handle into the database.
    pub fn put(&mut self, pid: Pid, inode: Inode, read: bool, write: bool) -> u64 {
        self.handlers
            .entry(pid)
            .or_insert_with(ProcessFileHandlers::default)
            .put(inode, read, write)
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
            handlers.close(fh);
        }

        // remove the process if it has no more file handles
        if self
            .handlers
            .get(&pid)
            .map(|handlers| handlers.handles.is_empty())
            .unwrap_or_default()
        {
            self.handlers.remove(&pid);
        }
    }
}

/// ProcessFileHandlers is a database of file handles. It is used to store file handles for open files.
///
/// It is a map between the file handle number and the [`FileHandle`] struct.
#[derive(Debug, Default)]
struct ProcessFileHandlers {
    handles: HashMap<Fh, FileHandle>,
    /// Next file handle number
    next: u64,
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
    /// Put a new [`FileHandle`] into the database.
    ///
    /// Returns the created file handle number.
    fn put(&mut self, inode: Inode, read: bool, write: bool) -> u64 {
        let fh = self.next;
        self.handles.insert(fh, FileHandle { inode, read, write });
        self.next = self.handles.len() as u64;
        fh
    }

    /// Get a [`FileHandle`] from the database.
    fn get(&self, fh: u64) -> Option<&FileHandle> {
        self.handles.get(&fh)
    }

    /// Close a file handle.
    ///
    /// This will remove the file handle from the database.
    /// The file handle number will be reused next.
    fn close(&mut self, fh: u64) {
        self.handles.remove(&fh);
        self.next = fh;
    }
}

#[cfg(test)]
mod test {

    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn test_should_store_handlers_for_pid() {
        let mut db = FileHandlersDb::default();

        let fh = db.put(1, 1, true, false);
        assert_eq!(
            db.get(1, fh),
            Some(&FileHandle {
                inode: 1,
                read: true,
                write: false
            })
        );

        assert_eq!(db.get(2, fh), None);

        let fh = db.put(1, 2, true, false);
        assert_eq!(
            db.get(1, fh),
            Some(&FileHandle {
                inode: 2,
                read: true,
                write: false
            })
        );

        let fh = db.put(2, 3, true, false);

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

        let fh = db.put(1, 1, true, false);
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

        db.put(1, 2, true, false);
        db.put(1, 3, true, false);
        db.close(1, 2);

        assert!(db.handlers.contains_key(&1));
    }

    #[test]
    fn test_file_handle_db() {
        let mut db = ProcessFileHandlers::default();

        let fh = db.put(1, true, false);
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

        let _fh1 = db.put(1, true, false);
        let fh2 = db.put(2, true, false);
        let _fh3 = db.put(3, true, false);

        db.close(fh2);

        let fh4 = db.put(4, true, false);

        assert_eq!(fh4, fh2);
        assert_eq!(
            db.get(fh2),
            Some(&FileHandle {
                inode: 4,
                read: true,
                write: false
            })
        );

        // next should be 5
        let fh5 = db.put(5, true, false);
        assert_eq!(fh5, 5);
    }
}
