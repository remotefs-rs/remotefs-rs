use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub type Inode = u64;

type Database = HashMap<Inode, PathBuf>;

pub const ROOT_INODE: Inode = 1;

/// A database to map inodes to files
///
/// The database is saved to a file when the instance is dropped
#[derive(Debug, Clone)]
pub struct InodeDb {
    database: Database,
}

impl InodeDb {
    /// Load [`InodeDb`] from a file
    ///
    /// It will initialize an empty database with only one inode set: the root inode which has always the value 1
    pub fn load() -> Self {
        let mut db = Self {
            database: Database::new(),
        };

        db.put(ROOT_INODE, PathBuf::from("/"));

        db
    }

    /// Check if the database contains an inode
    pub fn has(&self, inode: Inode) -> bool {
        self.database.contains_key(&inode)
    }

    /// Put a new inode into the database
    pub fn put(&mut self, inode: Inode, path: PathBuf) {
        debug!("inode {inode} -> {}", path.display());
        self.database.insert(inode, path);
    }

    /// Forget an inode
    pub fn forget(&mut self, inode: Inode) {
        if inode == ROOT_INODE {
            error!("tried to roget 1");
            return;
        }

        self.database.remove(&inode);
    }

    /// Get a path from an inode
    pub fn get(&self, inode: Inode) -> Option<&Path> {
        self.database.get(&inode).map(|x| x.as_path())
    }
}

#[cfg(test)]
mod test {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn test_inode_db() {
        let mut db = InodeDb::load();

        // should have root inode
        assert_eq!(db.has(ROOT_INODE), true);
        assert_eq!(db.get(ROOT_INODE), Some(Path::new("/")));

        db.put(3, PathBuf::from("/test"));
        assert_eq!(db.get(3), Some(Path::new("/test")));
        assert_eq!(db.has(3), true);

        db.forget(3);
        assert_eq!(db.get(3), None);
        assert_eq!(db.has(3), false);
    }

    #[test]
    fn test_should_not_forget_root() {
        let mut db = InodeDb::load();

        db.forget(ROOT_INODE);
        assert_eq!(db.has(ROOT_INODE), true);
    }
}
