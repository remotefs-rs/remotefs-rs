use std::collections::HashMap;
use std::path::{Path, PathBuf};

use thiserror::Error;

/// Error type for InodeDb
#[derive(Error, Debug)]
pub enum InodeDbError {
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
    #[error("Serde error: {0}")]
    SerdeError(#[from] serde_json::Error),
}

pub type InodeDbResult<T> = Result<T, InodeDbError>;
pub type Inode = u64;

type Database = HashMap<Inode, PathBuf>;

/// A database to map inodes to files
///
/// The database is saved to a file when the instance is dropped
#[derive(Debug, Default, Clone)]
pub struct InodeDb {
    database: Database,
    path: PathBuf,
}

impl InodeDb {
    /// Load [`InodeDb`] from a file
    pub fn load(path: &Path) -> Result<Self, InodeDbError> {
        let data = std::fs::read_to_string(path)?;
        let database: Database = serde_json::from_str(&data)?;

        Ok(Self {
            database,
            path: path.to_path_buf(),
        })
    }

    /// Check if the database contains an inode
    pub fn has(&self, inode: Inode) -> bool {
        self.database.contains_key(&inode)
    }

    /// Put a new inode into the database
    pub fn put(&mut self, inode: Inode, path: PathBuf) {
        self.database.insert(inode, path);
    }

    /// Get a path from an inode
    pub fn get(&self, inode: Inode) -> Option<&Path> {
        self.database.get(&inode).map(|x| x.as_path())
    }

    /// Save [`InodeDb`] to a file
    fn save(&self) -> InodeDbResult<()> {
        let data = serde_json::to_string(&self.database)?;
        std::fs::write(&self.path, data)?;

        Ok(())
    }
}

impl Drop for InodeDb {
    fn drop(&mut self) {
        if let Err(err) = self.save() {
            error!("Failed to save InodeDb: {err}");
        }
    }
}
