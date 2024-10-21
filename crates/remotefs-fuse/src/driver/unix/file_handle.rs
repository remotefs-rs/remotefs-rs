use std::collections::HashMap;

use super::inode::Inode;

/// FileHandleDb is a database of file handles. It is used to store file handles for open files.
///
/// It is a map between the file handle number and the [`FileHandle`] struct.
#[derive(Debug, Default)]
pub struct FileHandleDb {
    handles: HashMap<u64, FileHandle>,
    /// Next file handle number
    next: u64,
}

/// FileHandle is a handle to an open file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileHandle {
    /// Inode of the file
    pub inode: u64,
    /// Read permission
    pub read: bool,
    /// Write permission
    pub write: bool,
}

impl FileHandleDb {
    /// Put a new [`FileHandle`] into the database.
    ///
    /// Returns the created file handle number.
    pub fn put(&mut self, inode: Inode, read: bool, write: bool) -> u64 {
        let fh = self.next;
        self.handles.insert(fh, FileHandle { inode, read, write });
        self.next = self.handles.len() as u64;
        fh
    }

    /// Get a [`FileHandle`] from the database.
    pub fn get(&self, fh: u64) -> Option<&FileHandle> {
        self.handles.get(&fh)
    }

    /// Close a file handle.
    ///
    /// This will remove the file handle from the database.
    /// The file handle number will be reused next.
    pub fn close(&mut self, fh: u64) {
        self.handles.remove(&fh);
        self.next = fh;
    }
}

#[cfg(test)]
mod test {

    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn test_file_handle_db() {
        let mut db = FileHandleDb::default();

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
        let mut db = FileHandleDb::default();

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
