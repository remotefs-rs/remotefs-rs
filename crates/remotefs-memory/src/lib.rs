#![crate_name = "remotefs_memory"]
#![crate_type = "lib"]

//! # remotefs-memory
//!
//! remotefs-memory is a [remotefs](https://github.com/remotefs-rs/remotefs-rs)
//! client implementation backed entirely by an in-memory tree, useful for
//! tests and simulations that need a [`RemoteFs`] without a real server.
//!
//! It exposes a single type, [`MemoryFs`], which implements the [`RemoteFs`]
//! trait and can therefore be used interchangeably with any other remotefs
//! client.
//!
//! ## Getting Started
//!
//! Add `remotefs-memory` to your `Cargo.toml`:
//!
//! ```toml
//! remotefs = "1"
//! remotefs-memory = "1"
//! ```
//!
//! ## Example
//!
//! ```rust
//! use std::path::{Path, PathBuf};
//!
//! use remotefs_memory::{Inode, MemoryFs, node, Node, Tree};
//! use remotefs::RemoteFs;
//! use remotefs::fs::UnixPex;
//!
//! let tempdir = PathBuf::from("/tmp");
//! let tree = Tree::new(node!(
//!     PathBuf::from("/"),
//!     Inode::dir(0, 0, UnixPex::from(0o755)),
//!     node!(tempdir.clone(), Inode::dir(0, 0, UnixPex::from(0o755)))
//! ));
//!
//! let mut client = MemoryFs::new(tree);
//! client.connect().unwrap();
//! // Every path is absolute; there is no working directory.
//! assert!(client.stat(Path::new("/tmp")).unwrap().is_dir());
//! ```
//!
//! ## Transfers
//!
//! `open`, `create`, and `append` return owned streams. A write stream stages
//! bytes and commits them into the tree only when `finish` is called;
//! dropping it discards the staged bytes and logs at `debug` level.
//!
//! ```rust
//! use std::io::Write;
//! use std::path::{Path, PathBuf};
//!
//! use remotefs::RemoteFs;
//! use remotefs::fs::{ReadOptions, UnixPex, WriteOptions};
//! use remotefs_memory::{Inode, MemoryFs, node, Node, Tree};
//!
//! let tree = Tree::new(node!(
//!     PathBuf::from("/"),
//!     Inode::dir(0, 0, UnixPex::from(0o755))
//! ));
//! let mut client = MemoryFs::new(tree);
//! client.connect().unwrap();
//!
//! let mut stream = client
//!     .create(Path::new("/hello.txt"), &WriteOptions::default())
//!     .unwrap();
//! stream.write_all(b"hello").unwrap();
//! stream.finish().unwrap();
//!
//! let mut output = Vec::new();
//! client
//!     .read_file(Path::new("/hello.txt"), &ReadOptions::default(), &mut output)
//!     .unwrap();
//! assert_eq!(output, b"hello");
//! ```
//!
//! ## Async consumers
//!
//! `MemoryFs` is a native blocking client. Wrap it in
//! `remotefs::adapters::r#async::Unblock` (remotefs `tokio` feature) to use it
//! from an `AsyncRemoteFs` consumer.
//!

#![doc(html_playground_url = "https://play.rust-lang.org")]
#![doc(
    html_favicon_url = "https://raw.githubusercontent.com/remotefs-rs/remotefs-rs/main/assets/logo-128.png"
)]
#![doc(
    html_logo_url = "https://raw.githubusercontent.com/remotefs-rs/remotefs-rs/main/assets/logo.png"
)]

#[macro_use]
extern crate log;

mod inode;
mod stream;
#[cfg(test)]
mod test;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

pub use orange_trees::{Node, Tree, node};
use remotefs::fs::{
    Capabilities, ExecOutput, ReadOptions, ReadStream, SetMetadata, UnixPex, WriteOptions,
    WriteStream,
};
use remotefs::path::ensure_absolute;
use remotefs::{File, RemoteError, RemoteErrorType, RemoteFs, RemoteResult};

pub use self::inode::Inode;
use self::stream::{MemoryReader, MemoryWriter};

/// Alias for the filesystem tree. It is a [`Tree`] of [`PathBuf`] and [`Inode`].
pub type FsTree = Tree<PathBuf, Inode>;

/// Operations `MemoryFs` performs natively.
const CAPABILITIES: Capabilities = Capabilities::STREAM_READ
    .union(Capabilities::STREAM_WRITE)
    .union(Capabilities::APPEND)
    .union(Capabilities::RANGE_READ)
    .union(Capabilities::SEEK_READ)
    .union(Capabilities::SEEK_WRITE)
    .union(Capabilities::COPY)
    .union(Capabilities::SYMLINK)
    .union(Capabilities::SET_METADATA)
    .union(Capabilities::POSIX_MODE);

/// Default POSIX mode for files and directories created without one.
const DEFAULT_MODE: u32 = 0o755;

/// Locks the shared tree, mapping a poisoned lock to a protocol error.
pub(crate) fn lock_tree(tree: &Mutex<FsTree>) -> RemoteResult<MutexGuard<'_, FsTree>> {
    tree.lock().map_err(|_| {
        RemoteError::with_message(
            RemoteErrorType::ProtocolError,
            "in-memory filesystem lock poisoned",
        )
    })
}

/// MemoryFs is a simple in-memory filesystem that can be used for testing purposes.
///
/// It implements the [`RemoteFs`] trait.
///
/// The [`MemoryFs`] is instantiated providing a [`orange_trees::Tree`] which contains the filesystem data.
///
/// When reading or writing files, the [`MemoryFs`] will use the [`orange_trees::Tree`] to store the data.
///
/// You can easily create the [`MemoryFs`] using the [`MemoryFs::new`] method, providing the tree.
/// Use the [`node!`] macro to create the tree or use the [`orange_trees`] crate to create it programmatically.
///
/// The tree contains nodes identified by a [`PathBuf`] and a value of type [`Inode`].
/// Every path passed to the client must be absolute; the tree root is `/`.
pub struct MemoryFs {
    tree: Arc<Mutex<FsTree>>,
    connected: AtomicBool,
    // Fn to get uid
    get_uid: Box<dyn Fn() -> u32 + Send + Sync>,
    // Fn to get gid
    get_gid: Box<dyn Fn() -> u32 + Send + Sync>,
}

impl MemoryFs {
    /// Create a client backed by `tree`.
    ///
    /// `uid`/`gid` default to `0` for every file until
    /// [`MemoryFs::with_get_uid`] or [`MemoryFs::with_get_gid`] is used to
    /// override them.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use std::path::PathBuf;
    ///
    /// use remotefs::fs::UnixPex;
    /// use remotefs_memory::{Inode, MemoryFs, Node, Tree, node};
    ///
    /// let tree = Tree::new(node!(
    ///     PathBuf::from("/"),
    ///     Inode::dir(0, 0, UnixPex::from(0o755))
    /// ));
    /// let client = MemoryFs::new(tree);
    /// ```
    pub fn new(tree: FsTree) -> Self {
        Self {
            tree: Arc::new(Mutex::new(tree)),
            connected: AtomicBool::new(false),
            get_uid: Box::new(|| 0),
            get_gid: Box::new(|| 0),
        }
    }

    /// Override the closure used to fill the uid of newly created files.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use std::path::PathBuf;
    ///
    /// use remotefs::fs::UnixPex;
    /// use remotefs_memory::{Inode, MemoryFs, Node, Tree, node};
    ///
    /// let tree = Tree::new(node!(
    ///     PathBuf::from("/"),
    ///     Inode::dir(0, 0, UnixPex::from(0o755))
    /// ));
    /// let client = MemoryFs::new(tree).with_get_uid(|| 1000);
    /// ```
    pub fn with_get_uid<F>(mut self, get_uid: F) -> Self
    where
        F: Fn() -> u32 + Send + Sync + 'static,
    {
        self.get_uid = Box::new(get_uid);
        self
    }

    /// Override the closure used to fill the gid of newly created files.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use std::path::PathBuf;
    ///
    /// use remotefs::fs::UnixPex;
    /// use remotefs_memory::{Inode, MemoryFs, Node, Tree, node};
    ///
    /// let tree = Tree::new(node!(
    ///     PathBuf::from("/"),
    ///     Inode::dir(0, 0, UnixPex::from(0o755))
    /// ));
    /// let client = MemoryFs::new(tree).with_get_gid(|| 1000);
    /// ```
    pub fn with_get_gid<F>(mut self, get_gid: F) -> Self
    where
        F: Fn() -> u32 + Send + Sync + 'static,
    {
        self.get_gid = Box::new(get_gid);
        self
    }

    /// Validates that `path` is absolute and that the client is connected.
    fn validate(&self, path: &Path) -> RemoteResult<()> {
        ensure_absolute(path)?;
        if self.is_connected() {
            Ok(())
        } else {
            Err(RemoteError::new(RemoteErrorType::NotConnected))
        }
    }

    /// Locks the shared tree, mapping a poisoned lock to a protocol error.
    fn lock_tree(&self) -> RemoteResult<MutexGuard<'_, FsTree>> {
        lock_tree(&self.tree)
    }

    fn missing() -> RemoteError {
        RemoteError::new(RemoteErrorType::NoSuchFileOrDirectory)
    }

    /// Returns the parent of `path`, or `InvalidPath` for a root path.
    fn parent_of(path: &Path) -> RemoteResult<PathBuf> {
        path.parent().map(Path::to_path_buf).ok_or_else(|| {
            RemoteError::with_message(RemoteErrorType::InvalidPath, "path has no parent")
        })
    }

    /// Returns a mutable directory node, or `NoSuchFileOrDirectory`.
    fn directory_mut<'a>(
        tree: &'a mut FsTree,
        path: &PathBuf,
    ) -> RemoteResult<&'a mut Node<PathBuf, Inode>> {
        tree.root_mut()
            .query_mut(path)
            .filter(|node| node.value().metadata().is_dir())
            .ok_or_else(Self::missing)
    }

    /// Clones the subtree rooted at `node`, rewriting ids from `src` to `dest`.
    fn rekey(node: &Node<PathBuf, Inode>, src: &Path, dest: &Path) -> Node<PathBuf, Inode> {
        let id = match node.id().strip_prefix(src) {
            Ok(suffix) if !suffix.as_os_str().is_empty() => dest.join(suffix),
            _ => dest.to_path_buf(),
        };
        let children = node
            .children()
            .iter()
            .map(|child| Self::rekey(child, src, dest))
            .collect();
        Node::new(id, node.value().clone_with_new_identity()).with_children(children)
    }

    /// Copies the subtree at `src` under `dest`, replacing an existing `dest`.
    fn clone_subtree(tree: &mut FsTree, src: &PathBuf, dest: &PathBuf) -> RemoteResult<()> {
        if dest.starts_with(src) || src.starts_with(dest) {
            return Err(RemoteError::with_message(
                RemoteErrorType::BadFile,
                "source and destination overlap",
            ));
        }
        let dest_parent = Self::parent_of(dest)?;
        let subtree = Self::rekey(tree.root().query(src).ok_or_else(Self::missing)?, src, dest);
        let parent = Self::directory_mut(tree, &dest_parent)?;
        parent.remove_child(dest);
        parent.add_child(subtree);
        Ok(())
    }

    /// Validates every path before checking whether the client is connected.
    fn validate_paths(&self, paths: &[&Path]) -> RemoteResult<()> {
        for path in paths {
            ensure_absolute(path)?;
        }
        if self.is_connected() {
            Ok(())
        } else {
            Err(RemoteError::new(RemoteErrorType::NotConnected))
        }
    }

    /// Slices `bytes` by the read options, clamping to the file length.
    fn ranged(bytes: &[u8], opts: &ReadOptions) -> Vec<u8> {
        let length = bytes.len() as u64;
        let offset = opts.offset.unwrap_or(0).min(length);
        let end = opts.length.map_or(length, |requested| {
            offset.saturating_add(requested).min(length)
        });
        bytes[offset as usize..end as usize].to_vec()
    }

    /// Inserts or reuses the inode at `path` and returns a staged writer.
    fn open_writer(
        &self,
        path: &Path,
        opts: &WriteOptions,
        append: bool,
    ) -> RemoteResult<WriteStream> {
        let path = path.to_path_buf();
        let parent_path = Self::parent_of(&path)?;
        let mut tree = self.lock_tree()?;
        let parent = Self::directory_mut(&mut tree, &parent_path)?;
        let existing = parent
            .children()
            .iter()
            .find(|child| *child.id() == path)
            .map(|child| child.value().clone());
        if existing
            .as_ref()
            .is_some_and(|inode| inode.metadata().is_dir())
        {
            return Err(RemoteError::with_message(
                RemoteErrorType::BadFile,
                "is a directory",
            ));
        }
        let (inode, initial) = match existing {
            Some(inode) if append => {
                if inode.metadata().is_symlink() {
                    return Err(RemoteError::with_message(
                        RemoteErrorType::BadFile,
                        "cannot append to a symlink",
                    ));
                }
                let initial = inode.content().map(<[u8]>::to_vec).unwrap_or_default();
                (inode.clone_with_new_identity(), initial)
            }
            _ => (
                Inode::file(
                    (self.get_uid)(),
                    (self.get_gid)(),
                    opts.mode.unwrap_or_else(|| UnixPex::from(DEFAULT_MODE)),
                    Vec::new(),
                ),
                Vec::new(),
            ),
        };
        let identity = inode.identity();
        parent.add_child(Node::new(path.clone(), inode));
        drop(tree);
        let mut writer = MemoryWriter::new(
            Arc::clone(&self.tree),
            path,
            initial,
            opts.modified,
            identity,
        );
        if append {
            writer.seek_to_end();
        }
        Ok(WriteStream::new(writer))
    }
}

impl RemoteFs for MemoryFs {
    fn connect(&mut self) -> RemoteResult<()> {
        debug!("connect()");
        if self.connected.swap(true, Ordering::AcqRel) {
            return Err(RemoteError::new(RemoteErrorType::AlreadyConnected));
        }
        Ok(())
    }

    fn disconnect(&mut self) -> RemoteResult<()> {
        debug!("disconnect()");
        if self.connected.swap(false, Ordering::AcqRel) {
            Ok(())
        } else {
            Err(RemoteError::new(RemoteErrorType::NotConnected))
        }
    }

    fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Acquire)
    }

    fn capabilities(&self) -> Capabilities {
        CAPABILITIES
    }

    fn list_dir(&self, path: &Path) -> RemoteResult<Vec<File>> {
        self.validate(path)?;
        debug!("list_dir({path:?})");
        let tree = self.lock_tree()?;
        let node = tree
            .root()
            .query(&path.to_path_buf())
            .ok_or_else(Self::missing)?;
        if !node.value().metadata().is_dir() {
            return Err(RemoteError::with_message(
                RemoteErrorType::BadFile,
                "not a directory",
            ));
        }
        Ok(node
            .children()
            .iter()
            .map(|child| File::new(child.id().clone(), child.value().metadata().clone()))
            .collect())
    }

    fn stat(&self, path: &Path) -> RemoteResult<File> {
        self.validate(path)?;
        debug!("stat({path:?})");
        let tree = self.lock_tree()?;
        let node = tree
            .root()
            .query(&path.to_path_buf())
            .ok_or_else(Self::missing)?;
        Ok(File::new(
            node.id().clone(),
            node.value().metadata().clone(),
        ))
    }

    fn exists(&self, path: &Path) -> RemoteResult<bool> {
        self.validate(path)?;
        debug!("exists({path:?})");
        let tree = self.lock_tree()?;
        Ok(tree.root().query(&path.to_path_buf()).is_some())
    }

    fn set_metadata(&self, path: &Path, metadata: &SetMetadata) -> RemoteResult<()> {
        self.validate(path)?;
        debug!("set_metadata({path:?}, {metadata:?})");
        let mut tree = self.lock_tree()?;
        let node = tree
            .root_mut()
            .query_mut(&path.to_path_buf())
            .ok_or_else(Self::missing)?;
        let mut inode = node.value().clone();
        if let Some(mode) = metadata.mode {
            inode.metadata.mode = Some(mode);
        }
        if let Some(uid) = metadata.uid {
            inode.metadata.uid = Some(uid);
        }
        if let Some(gid) = metadata.gid {
            inode.metadata.gid = Some(gid);
        }
        if let Some(accessed) = metadata.accessed {
            inode.metadata.accessed = Some(accessed);
        }
        if let Some(modified) = metadata.modified {
            inode.metadata.modified = Some(modified);
        }
        node.set_value(inode);
        Ok(())
    }

    fn create_dir(&self, path: &Path, mode: Option<UnixPex>) -> RemoteResult<()> {
        self.validate(path)?;
        debug!("create_dir({path:?}, {mode:?})");
        let path = path.to_path_buf();
        let parent_path = Self::parent_of(&path)?;
        let dir = Inode::dir(
            (self.get_uid)(),
            (self.get_gid)(),
            mode.unwrap_or_else(|| UnixPex::from(DEFAULT_MODE)),
        );
        let mut tree = self.lock_tree()?;
        let parent = Self::directory_mut(&mut tree, &parent_path)?;
        if parent.children().iter().any(|child| *child.id() == path) {
            return Err(RemoteError::new(RemoteErrorType::AlreadyExists));
        }
        parent.add_child(Node::new(path, dir));
        Ok(())
    }

    fn remove_file(&self, path: &Path) -> RemoteResult<()> {
        self.validate(path)?;
        debug!("remove_file({path:?})");
        let path = path.to_path_buf();
        let mut tree = self.lock_tree()?;
        let node = tree.root().query(&path).ok_or_else(Self::missing)?;
        if node.value().metadata().is_dir() {
            return Err(RemoteError::with_message(
                RemoteErrorType::CouldNotRemoveFile,
                "is a directory",
            ));
        }
        tree.root_mut()
            .parent_mut(&path)
            .ok_or_else(Self::missing)?
            .remove_child(&path);
        Ok(())
    }

    fn remove_dir(&self, path: &Path) -> RemoteResult<()> {
        self.validate(path)?;
        debug!("remove_dir({path:?})");
        let path = path.to_path_buf();
        let mut tree = self.lock_tree()?;
        let node = tree.root().query(&path).ok_or_else(Self::missing)?;
        if !node.value().metadata().is_dir() {
            return Err(RemoteError::with_message(
                RemoteErrorType::CouldNotRemoveFile,
                "not a directory",
            ));
        }
        if !node.is_leaf() {
            return Err(RemoteError::new(RemoteErrorType::DirectoryNotEmpty));
        }
        tree.root_mut()
            .parent_mut(&path)
            .ok_or_else(Self::missing)?
            .remove_child(&path);
        Ok(())
    }

    fn remove_dir_all(&self, path: &Path) -> RemoteResult<()> {
        self.validate(path)?;
        debug!("remove_dir_all({path:?})");
        let path = path.to_path_buf();
        let mut tree = self.lock_tree()?;
        let parent = tree
            .root_mut()
            .parent_mut(&path)
            .ok_or_else(Self::missing)?;
        if !parent.children().iter().any(|child| *child.id() == path) {
            return Err(Self::missing());
        }
        parent.remove_child(&path);
        Ok(())
    }

    fn rename(&self, src: &Path, dest: &Path) -> RemoteResult<()> {
        self.validate_paths(&[src, dest])?;
        debug!("rename({src:?}, {dest:?})");
        let (src, dest) = (src.to_path_buf(), dest.to_path_buf());
        let mut tree = self.lock_tree()?;
        Self::clone_subtree(&mut tree, &src, &dest)?;
        tree.root_mut()
            .parent_mut(&src)
            .ok_or_else(Self::missing)?
            .remove_child(&src);
        Ok(())
    }

    fn copy(&self, src: &Path, dest: &Path) -> RemoteResult<()> {
        self.validate_paths(&[src, dest])?;
        debug!("copy({src:?}, {dest:?})");
        let mut tree = self.lock_tree()?;
        Self::clone_subtree(&mut tree, &src.to_path_buf(), &dest.to_path_buf())
    }

    fn symlink(&self, path: &Path, target: &Path) -> RemoteResult<()> {
        self.validate_paths(&[path, target])?;
        debug!("symlink({path:?}, {target:?})");
        let path = path.to_path_buf();
        let parent_path = Self::parent_of(&path)?;
        let link = Inode::symlink((self.get_uid)(), (self.get_gid)(), target.to_path_buf());
        let mut tree = self.lock_tree()?;
        if tree.root().query(&target.to_path_buf()).is_none() {
            return Err(Self::missing());
        }
        let parent = Self::directory_mut(&mut tree, &parent_path)?;
        if parent.children().iter().any(|child| *child.id() == path) {
            return Err(RemoteError::new(RemoteErrorType::AlreadyExists));
        }
        parent.add_child(Node::new(path, link));
        Ok(())
    }

    fn open(&self, path: &Path, opts: &ReadOptions) -> RemoteResult<ReadStream> {
        self.validate(path)?;
        debug!("open({path:?}, {opts:?})");
        let tree = self.lock_tree()?;
        let node = tree
            .root()
            .query(&path.to_path_buf())
            .ok_or_else(Self::missing)?;
        let content = node.value().content().ok_or_else(|| {
            RemoteError::with_message(RemoteErrorType::BadFile, "cannot open a directory")
        })?;
        Ok(ReadStream::new(MemoryReader::new(Self::ranged(
            content, opts,
        ))))
    }

    fn create(&self, path: &Path, opts: &WriteOptions) -> RemoteResult<WriteStream> {
        self.validate(path)?;
        debug!("create({path:?}, {opts:?})");
        self.open_writer(path, opts, false)
    }

    fn append(&self, path: &Path, opts: &WriteOptions) -> RemoteResult<WriteStream> {
        self.validate(path)?;
        debug!("append({path:?}, {opts:?})");
        self.open_writer(path, opts, true)
    }

    fn exec(&self, _cmd: &str) -> RemoteResult<ExecOutput> {
        if !self.is_connected() {
            return Err(RemoteError::new(RemoteErrorType::NotConnected));
        }
        Err(RemoteError::new(RemoteErrorType::UnsupportedFeature))
    }
}
