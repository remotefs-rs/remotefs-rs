use std::path::PathBuf;
use std::time::SystemTime;

use remotefs::fs::{FileType, Metadata, UnixPex};

/// Inode is the data stored in each node of the filesystem.
#[derive(Debug, Clone)]
pub struct Inode {
    /// File metadata
    pub(crate) metadata: Metadata,
    /// File content; if the node is a directory, this field is `None`.
    pub(crate) content: Option<Vec<u8>>,
}

impl Inode {
    /// Create a new [`Inode`] with type **Directory** with the given metadata and content.
    pub fn dir(uid: u32, gid: u32, mode: UnixPex) -> Self {
        Self {
            metadata: Metadata::default()
                .uid(uid)
                .gid(gid)
                .file_type(FileType::Directory)
                .created(SystemTime::now())
                .accessed(SystemTime::now())
                .mode(mode),
            content: None,
        }
    }

    /// Create a new [`Inode`] with type **File** with the given metadata and content.
    pub fn file(uid: u32, gid: u32, mode: UnixPex, data: Vec<u8>) -> Self {
        Self {
            metadata: Metadata::default()
                .uid(uid)
                .gid(gid)
                .file_type(FileType::File)
                .created(SystemTime::now())
                .accessed(SystemTime::now())
                .mode(mode)
                .size(data.len() as u64),
            content: Some(data),
        }
    }

    /// Create a new [`Inode`] with type **Symlink** with the given metadata and target.
    pub fn symlink(uid: u32, gid: u32, mode: UnixPex, target: PathBuf) -> Self {
        Self {
            metadata: Metadata::default()
                .uid(uid)
                .gid(gid)
                .file_type(FileType::Symlink)
                .created(SystemTime::now())
                .accessed(SystemTime::now())
                .mode(mode)
                .symlink(target),
            content: None,
        }
    }

    /// Return the [`Metadata`] of the file.
    pub fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    /// Return the content of the file.
    pub fn content(&self) -> Option<&[u8]> {
        self.content.as_deref()
    }
}
