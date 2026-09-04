use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub type Inode = u64;

pub const ROOT_INODE: Inode = 1;

/// A database to map inodes to files and back.
///
/// Inode numbers are allocated on first use from a monotonic counter, rather than derived from a
/// hash of the path. This guarantees that two different paths can never be assigned the same
/// inode number, which a hash-based scheme cannot: a hash collision would otherwise alias two
/// unrelated files onto the same inode, silently redirecting lookups for one file to the other.
#[derive(Debug, Clone)]
pub struct InodeDb {
    inode_to_path: HashMap<Inode, PathBuf>,
    path_to_inode: HashMap<PathBuf, Inode>,
    next_inode: Inode,
}

impl InodeDb {
    /// Load [`InodeDb`] with only the root inode set, which has always the value 1.
    pub fn load() -> Self {
        let mut db = Self {
            inode_to_path: HashMap::new(),
            path_to_inode: HashMap::new(),
            next_inode: ROOT_INODE + 1,
        };

        db.insert(ROOT_INODE, PathBuf::from("/"));

        db
    }

    /// Get the inode assigned to `path`, allocating a new, never-before-used one if `path` has
    /// not been seen before.
    pub fn inode_for(&mut self, path: &Path) -> Inode {
        if let Some(inode) = self.path_to_inode.get(path) {
            return *inode;
        }

        let inode = self.next_inode;
        self.next_inode += 1;
        self.insert(inode, path.to_path_buf());

        inode
    }

    /// Forget an inode, removing it (and its path) from the database.
    pub fn forget(&mut self, inode: Inode) {
        if inode == ROOT_INODE {
            error!("tried to roget 1");
            return;
        }

        if let Some(path) = self.inode_to_path.remove(&inode) {
            self.path_to_inode.remove(&path);
        }
    }

    /// Get a path from an inode
    pub fn get(&self, inode: Inode) -> Option<&Path> {
        self.inode_to_path.get(&inode).map(|x| x.as_path())
    }

    /// Insert a bidirectional inode <-> path mapping.
    fn insert(&mut self, inode: Inode, path: PathBuf) {
        debug!("inode {inode} -> {}", path.display());
        self.path_to_inode.insert(path.clone(), inode);
        self.inode_to_path.insert(inode, path);
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
        assert_eq!(db.get(ROOT_INODE), Some(Path::new("/")));

        let inode = db.inode_for(Path::new("/test"));
        assert_eq!(db.get(inode), Some(Path::new("/test")));

        db.forget(inode);
        assert_eq!(db.get(inode), None);
    }

    #[test]
    fn test_should_not_forget_root() {
        let mut db = InodeDb::load();

        db.forget(ROOT_INODE);
        assert_eq!(db.get(ROOT_INODE), Some(Path::new("/")));
    }

    #[test]
    fn test_should_reuse_inode_for_same_path() {
        let mut db = InodeDb::load();

        let a = db.inode_for(Path::new("/test"));
        let b = db.inode_for(Path::new("/test"));

        assert_eq!(a, b);
    }

    #[test]
    fn test_should_never_assign_same_inode_to_different_paths() {
        let mut db = InodeDb::load();

        let a = db.inode_for(Path::new("/foo"));
        let b = db.inode_for(Path::new("/bar"));

        assert_ne!(a, b);
        assert_eq!(db.get(a), Some(Path::new("/foo")));
        assert_eq!(db.get(b), Some(Path::new("/bar")));
    }

    #[test]
    fn test_should_allocate_new_inode_after_forgetting_path() {
        let mut db = InodeDb::load();

        let a = db.inode_for(Path::new("/test"));
        db.forget(a);

        let b = db.inode_for(Path::new("/test"));

        assert_ne!(a, b);
    }
}
