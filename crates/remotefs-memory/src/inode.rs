use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;

use remotefs::fs::{FileType, Metadata, UnixPex};

/// Inode is the data stored in each node of the filesystem.
#[derive(Debug, Clone)]
pub struct Inode {
    /// File metadata
    pub(crate) metadata: Metadata,
    /// File content; if the node is a directory, this field is `None`.
    pub(crate) content: Option<Vec<u8>>,
    identity: Arc<()>,
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
            identity: Arc::new(()),
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
            identity: Arc::new(()),
        }
    }

    /// Create a new [`Inode`] with type **Symlink** with the given metadata and target.
    pub fn symlink(uid: u32, gid: u32, target: PathBuf) -> Self {
        Self {
            metadata: Metadata::default()
                .uid(uid)
                .gid(gid)
                .file_type(FileType::Symlink)
                .created(SystemTime::now())
                .accessed(SystemTime::now())
                .mode(UnixPex::from(0o777))
                .symlink(target.clone()),
            content: Some(target.to_string_lossy().as_bytes().to_vec()),
            identity: Arc::new(()),
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

    /// Returns a token that identifies this inode instance.
    pub(crate) fn identity(&self) -> Arc<()> {
        Arc::clone(&self.identity)
    }

    /// Clone this inode as an independent filesystem entry.
    pub(crate) fn clone_with_new_identity(&self) -> Self {
        Self {
            metadata: self.metadata.clone(),
            content: self.content.clone(),
            identity: Arc::new(()),
        }
    }

    /// Returns whether `identity` identifies this inode instance.
    pub(crate) fn has_identity(&self, identity: &Arc<()>) -> bool {
        Arc::ptr_eq(&self.identity, identity)
    }
}
