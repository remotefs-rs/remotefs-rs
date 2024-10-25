#![crate_name = "remotefs_memory"]
#![crate_type = "lib"]

//! # remotefs-memory
//!
//! A memory-based implementation of the `remotefs` crate.
//! This crate provides a simple in-memory filesystem that can be used for testing purposes.
//!
//! ## Getting Started
//!
//! Add `remotefs-memory` to your `Cargo.toml`:
//!
//! ```toml
//! remotefs = "0.3"
//! remotefs-memory = "0.1"
//! ```
//!
//! ## Example
//!
//! ```rust
//! use std::path::PathBuf;
//!
//! use remotefs_memory::{Inode, MemoryFs, node, Node, Tree};
//! use remotefs::RemoteFs;
//! use remotefs::fs::{UnixPex, Metadata};
//!
//! let tempdir = PathBuf::from("/tmp");
//! let tree = Tree::new(node!(
//!     PathBuf::from("/"),
//!     Inode::dir(0, 0, UnixPex::from(0o755)),
//!     node!(tempdir.clone(), Inode::dir(0, 0, UnixPex::from(0o755)))
//! ));
//!
//! let mut client = MemoryFs::new(tree);
//!
//! assert!(client.connect().is_ok());
//! // Change directory
//! assert!(client.change_dir(tempdir.as_path()).is_ok());
//! ```
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
#[cfg(test)]
mod test;

use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub use orange_trees::{node, Node, Tree};
use remotefs::fs::stream::{StreamWriter, WriteAndSeek};
use remotefs::fs::{FileType, Metadata, ReadStream, UnixPex, Welcome, WriteStream};
use remotefs::{File, RemoteError, RemoteErrorType, RemoteFs, RemoteResult};

pub use self::inode::Inode;

/// Alias for the filesystem tree. It is a [`Tree`] of [`PathBuf`] and [`Inode`].
pub type FsTree = Tree<PathBuf, Inode>;

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
pub struct MemoryFs {
    tree: FsTree,
    wrkdir: PathBuf,
    connected: bool,
    // Fn to get uid
    get_uid: Box<dyn Fn() -> u32 + Send + Sync>,
    // Fn to get gid
    get_gid: Box<dyn Fn() -> u32 + Send + Sync>,
}

#[derive(Debug, Clone)]
struct WriteHandle {
    path: PathBuf,
    data: Cursor<Vec<u8>>,
    mode: WriteMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WriteMode {
    Append,
    Create,
}

impl Write for WriteHandle {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.data.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl MemoryFs {
    /// Create a new instance of the [`MemoryFs`] with the provided [`FsTree`].
    pub fn new(tree: FsTree) -> Self {
        Self {
            tree,
            wrkdir: PathBuf::from("/"),
            connected: false,
            get_uid: Box::new(|| 0),
            get_gid: Box::new(|| 0),
        }
    }

    /// Set the function to get the user id (uid).
    pub fn with_get_uid<F>(mut self, get_uid: F) -> Self
    where
        F: Fn() -> u32 + Send + Sync + 'static,
    {
        self.get_uid = Box::new(get_uid);
        self
    }

    /// Set the function to get the group id (gid).
    pub fn with_get_gid<F>(mut self, get_gid: F) -> Self
    where
        F: Fn() -> u32 + Send + Sync + 'static,
    {
        self.get_gid = Box::new(get_gid);
        self
    }

    fn absolutize(&self, path: &Path) -> PathBuf {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.wrkdir.join(path)
        }
    }

    /// Downcast the write handle to a write handle.
    fn downcast_write_handle(handle: WriteStream) -> Box<WriteHandle> {
        match handle.stream {
            StreamWriter::Write(w) => {
                let raw: *mut dyn Write = Box::into_raw(w);
                unsafe { Box::from_raw(raw as *mut WriteHandle) }
            }
            StreamWriter::WriteAndSeek(w) => {
                let raw: *mut dyn WriteAndSeek = Box::into_raw(w);
                unsafe { Box::from_raw(raw as *mut WriteHandle) }
            }
        }
    }
}

impl RemoteFs for MemoryFs {
    fn connect(&mut self) -> RemoteResult<Welcome> {
        debug!("connect()");
        self.connected = true;
        Ok(Welcome::default())
    }

    fn disconnect(&mut self) -> RemoteResult<()> {
        debug!("disconnect()");
        self.connected = false;
        Ok(())
    }

    fn is_connected(&mut self) -> bool {
        debug!("is_connected() -> {}", self.connected);
        self.connected
    }

    fn pwd(&mut self) -> RemoteResult<PathBuf> {
        if !self.connected {
            return Err(RemoteError::new(RemoteErrorType::NotConnected));
        }
        debug!("pwd() -> {:?}", self.wrkdir);

        Ok(self.wrkdir.clone())
    }

    fn change_dir(&mut self, dir: &Path) -> RemoteResult<PathBuf> {
        if !self.connected {
            return Err(RemoteError::new(RemoteErrorType::NotConnected));
        }

        let dir = self.absolutize(dir);

        debug!("change_dir({:?})", dir);

        // check if the directory exists
        let inode = self
            .tree
            .root()
            .query(&dir)
            .ok_or_else(|| RemoteError::new(RemoteErrorType::NoSuchFileOrDirectory))?
            .value()
            .clone();

        match inode.metadata().file_type {
            FileType::Directory => {
                self.wrkdir = dir.clone();
                Ok(self.wrkdir.clone())
            }
            FileType::Symlink if inode.metadata().symlink.is_some() => {
                self.change_dir(inode.metadata().symlink.as_ref().unwrap())
            }
            FileType::Symlink | FileType::File => Err(RemoteError::new(RemoteErrorType::BadFile)),
        }
    }

    fn list_dir(&mut self, path: &Path) -> RemoteResult<Vec<File>> {
        if !self.connected {
            return Err(RemoteError::new(RemoteErrorType::NotConnected));
        }

        let path = self.absolutize(path);
        debug!("list_dir({:?})", path);

        // query node
        let node = self
            .tree
            .root()
            .query(&path)
            .ok_or_else(|| RemoteError::new(RemoteErrorType::NoSuchFileOrDirectory))?;

        let mut files = vec![];

        for child in node.children() {
            let path = child.id().clone();
            let metadata = child.value().metadata().clone();
            debug!("list_dir() -> {path:?}, {metadata:?}");

            files.push(File { path, metadata })
        }

        Ok(files)
    }

    fn stat(&mut self, path: &Path) -> RemoteResult<File> {
        if !self.connected {
            return Err(RemoteError::new(RemoteErrorType::NotConnected));
        }

        let path = self.absolutize(path);
        debug!("stat({:?})", path);

        let node = self
            .tree
            .root()
            .query(&path)
            .ok_or_else(|| RemoteError::new(RemoteErrorType::NoSuchFileOrDirectory))?;

        let path = node.id().clone();
        let metadata = node.value().metadata().clone();

        debug!("stat({path:?}) -> {metadata:?}");

        Ok(File { path, metadata })
    }

    fn setstat(&mut self, path: &Path, metadata: Metadata) -> RemoteResult<()> {
        if !self.connected {
            return Err(RemoteError::new(RemoteErrorType::NotConnected));
        }

        let path = self.absolutize(path);
        debug!("setstat({:?}, {:?})", path, metadata);

        let node = self
            .tree
            .root_mut()
            .query_mut(&path)
            .ok_or_else(|| RemoteError::new(RemoteErrorType::NoSuchFileOrDirectory))?;

        node.set_value(Inode {
            metadata,
            content: node.value().content.clone(),
        });

        Ok(())
    }

    fn exists(&mut self, path: &Path) -> RemoteResult<bool> {
        if !self.connected {
            return Err(RemoteError::new(RemoteErrorType::NotConnected));
        }

        let path = self.absolutize(path);
        debug!("exists({:?})", path);

        Ok(self.tree.root().query(&path).is_some())
    }

    fn remove_file(&mut self, path: &Path) -> RemoteResult<()> {
        if !self.connected {
            return Err(RemoteError::new(RemoteErrorType::NotConnected));
        }

        let path = self.absolutize(path);
        debug!("remove_file({:?})", path);

        // get node
        let node = self
            .tree
            .root_mut()
            .query_mut(&path)
            .ok_or_else(|| RemoteError::new(RemoteErrorType::NoSuchFileOrDirectory))?;

        // check if is a leaf and a file
        if !node.is_leaf() || node.value().metadata().file_type == FileType::Directory {
            return Err(RemoteError::new(RemoteErrorType::CouldNotRemoveFile));
        }

        let parent = self.tree.root_mut().parent_mut(&path).unwrap();
        parent.remove_child(&path);

        Ok(())
    }

    fn remove_dir(&mut self, path: &Path) -> RemoteResult<()> {
        if !self.connected {
            return Err(RemoteError::new(RemoteErrorType::NotConnected));
        }

        let path = self.absolutize(path);
        debug!("remove_dir({:?})", path);

        // get node
        let node = self
            .tree
            .root_mut()
            .query_mut(&path)
            .ok_or_else(|| RemoteError::new(RemoteErrorType::NoSuchFileOrDirectory))?;
        // check if is a leaf and is a directory
        if !node.is_leaf() {
            debug!("Directory {path:?} is not empty");
            return Err(RemoteError::new(RemoteErrorType::DirectoryNotEmpty));
        }
        if node.value().metadata().file_type != FileType::Directory {
            debug!("{path:?} is not a directory");
            return Err(RemoteError::new(RemoteErrorType::CouldNotRemoveFile));
        }

        let parent = self.tree.root_mut().parent_mut(&path).unwrap();
        parent.remove_child(&path);
        debug!("removed {:?}", path);

        Ok(())
    }

    fn remove_dir_all(&mut self, path: &Path) -> RemoteResult<()> {
        if !self.connected {
            return Err(RemoteError::new(RemoteErrorType::NotConnected));
        }

        let path = self.absolutize(path);
        debug!("remove_dir_all({:?})", path);

        let parent = self
            .tree
            .root_mut()
            .parent_mut(&path)
            .ok_or_else(|| RemoteError::new(RemoteErrorType::NoSuchFileOrDirectory))?;

        if !parent.children().iter().any(|child| *child.id() == path) {
            return Err(RemoteError::new(RemoteErrorType::NoSuchFileOrDirectory));
        }
        parent.remove_child(&path);
        debug!("removed {:?}", path);

        Ok(())
    }

    fn create_dir(&mut self, path: &Path, mode: UnixPex) -> RemoteResult<()> {
        if !self.connected {
            return Err(RemoteError::new(RemoteErrorType::NotConnected));
        }

        let path = self.absolutize(path);
        debug!("create_dir({:?})", path);
        let parent = path
            .parent()
            .unwrap_or_else(|| Path::new("/"))
            .to_path_buf();

        let dir = Inode::dir((self.get_uid)(), (self.get_gid)(), mode);

        let parent = self
            .tree
            .root_mut()
            .query_mut(&parent)
            .ok_or_else(|| RemoteError::new(RemoteErrorType::NoSuchFileOrDirectory))?;

        // check if the directory already exists
        if parent.children().iter().any(|child| *child.id() == path) {
            debug!("Directory {path:?} already exists");
            return Err(RemoteError::new(RemoteErrorType::DirectoryAlreadyExists));
        }

        // add the directory
        parent.add_child(Node::new(path.clone(), dir));
        debug!("created directory {path:?}");

        Ok(())
    }

    fn symlink(&mut self, path: &Path, target: &Path) -> RemoteResult<()> {
        if !self.connected {
            return Err(RemoteError::new(RemoteErrorType::NotConnected));
        }
        let path = self.absolutize(path);
        let target = self.absolutize(target);
        debug!("symlink({:?}, {:?})", path, target);
        // check if `target` exists
        if self.tree.root().query(&target).is_none() {
            debug!("target {target:?} does not exist");
            return Err(RemoteError::new(RemoteErrorType::NoSuchFileOrDirectory));
        }

        let parent = path
            .parent()
            .unwrap_or_else(|| Path::new("/"))
            .to_path_buf();

        let symlink = Inode::symlink((self.get_uid)(), (self.get_gid)(), target.to_path_buf());

        let parent = self
            .tree
            .root_mut()
            .query_mut(&parent)
            .ok_or_else(|| RemoteError::new(RemoteErrorType::NoSuchFileOrDirectory))?;

        // check if the file already exists
        if parent.children().iter().any(|child| *child.id() == path) {
            debug!("symbolic link {path:?} already exists");
            return Err(RemoteError::new(RemoteErrorType::FileCreateDenied));
        }

        // add the directory
        parent.add_child(Node::new(path.clone(), symlink));
        debug!("symlink {path:?} -> {target:?}");

        Ok(())
    }

    fn copy(&mut self, src: &Path, dest: &Path) -> RemoteResult<()> {
        if !self.connected {
            return Err(RemoteError::new(RemoteErrorType::NotConnected));
        }
        let src = self.absolutize(src);
        let dest = self.absolutize(dest);
        debug!("copy({:?}, {:?})", src, dest);

        let dest_parent = dest
            .parent()
            .unwrap_or_else(|| Path::new("/"))
            .to_path_buf();

        let dest_inode = self
            .tree
            .root()
            .query(&src)
            .ok_or_else(|| RemoteError::new(RemoteErrorType::NoSuchFileOrDirectory))?
            .value()
            .clone();

        let dest_parent = self
            .tree
            .root_mut()
            .query_mut(&dest_parent)
            .ok_or_else(|| RemoteError::new(RemoteErrorType::NoSuchFileOrDirectory))?;

        debug!("copied {src:?} to {dest:?}");
        dest_parent.add_child(Node::new(dest, dest_inode));

        Ok(())
    }

    fn mov(&mut self, src: &Path, dest: &Path) -> RemoteResult<()> {
        if !self.connected {
            return Err(RemoteError::new(RemoteErrorType::NotConnected));
        }
        let src = self.absolutize(src);
        let dest = self.absolutize(dest);
        debug!("mov({:?}, {:?})", src, dest);

        let dest_parent = dest
            .parent()
            .unwrap_or_else(|| Path::new("/"))
            .to_path_buf();

        let dest_inode = self
            .tree
            .root()
            .query(&src)
            .ok_or_else(|| RemoteError::new(RemoteErrorType::NoSuchFileOrDirectory))?
            .value()
            .clone();

        let dest_parent = self
            .tree
            .root_mut()
            .query_mut(&dest_parent)
            .ok_or_else(|| RemoteError::new(RemoteErrorType::NoSuchFileOrDirectory))?;

        dest_parent.add_child(Node::new(dest.clone(), dest_inode));

        // remove src
        let src_parent = self
            .tree
            .root_mut()
            .parent_mut(&src)
            .ok_or_else(|| RemoteError::new(RemoteErrorType::NoSuchFileOrDirectory))?;

        src_parent.remove_child(&src);
        debug!("moved {src:?} to {dest:?}");

        Ok(())
    }

    fn exec(&mut self, _cmd: &str) -> RemoteResult<(u32, String)> {
        Err(RemoteError::new(RemoteErrorType::UnsupportedFeature))
    }

    fn append(&mut self, path: &Path, metadata: &Metadata) -> RemoteResult<WriteStream> {
        if !self.connected {
            return Err(RemoteError::new(RemoteErrorType::NotConnected));
        }
        let path = self.absolutize(path);
        debug!("append({:?},{:?})", path, metadata);
        let parent = path
            .parent()
            .unwrap_or_else(|| Path::new("/"))
            .to_path_buf();

        let parent = self
            .tree
            .root_mut()
            .query_mut(&parent)
            .ok_or_else(|| RemoteError::new(RemoteErrorType::NoSuchFileOrDirectory))?;

        // get current content if any
        let content = match parent.query(&path) {
            Some(node) => node.value().content.clone(),
            None => None,
        };

        let file = Inode::file(
            metadata.uid.unwrap_or((self.get_uid)()),
            metadata.gid.unwrap_or((self.get_gid)()),
            metadata.mode.unwrap_or_else(|| UnixPex::from(0o755)),
            content.clone().unwrap_or_default(),
        );

        // add new file
        parent.add_child(Node::new(path.clone(), file));

        // make stream
        debug!("file {path:?} opened for append");
        let handle = WriteHandle {
            path,
            data: Cursor::new(content.unwrap_or_default()),
            mode: WriteMode::Append,
        };

        let stream = Box::new(handle) as Box<dyn Write + Send>;

        Ok(WriteStream::from(stream))
    }

    fn create(&mut self, path: &Path, metadata: &Metadata) -> RemoteResult<WriteStream> {
        if !self.connected {
            return Err(RemoteError::new(RemoteErrorType::NotConnected));
        }
        let path = self.absolutize(path);
        debug!("create({:?},{:?})", path, metadata);
        let parent = path
            .parent()
            .unwrap_or_else(|| Path::new("/"))
            .to_path_buf();

        let parent = self
            .tree
            .root_mut()
            .query_mut(&parent)
            .ok_or_else(|| RemoteError::new(RemoteErrorType::NoSuchFileOrDirectory))?;

        let file = Inode::file(
            metadata.uid.unwrap_or((self.get_uid)()),
            metadata.gid.unwrap_or((self.get_gid)()),
            metadata.mode.unwrap_or_else(|| UnixPex::from(0o755)),
            vec![],
        );

        // add new file
        parent.add_child(Node::new(path.clone(), file));
        debug!("{:?} created", path);

        // make stream
        let handle = WriteHandle {
            path,
            data: Cursor::new(vec![]),
            mode: WriteMode::Create,
        };

        let stream = Box::new(handle) as Box<dyn Write + Send>;

        Ok(WriteStream::from(stream))
    }

    fn open(&mut self, path: &Path) -> RemoteResult<ReadStream> {
        if !self.connected {
            return Err(RemoteError::new(RemoteErrorType::NotConnected));
        }
        let path = self.absolutize(path);
        debug!("open({:?})", path);

        let node = self
            .tree
            .root()
            .query(&path)
            .ok_or_else(|| RemoteError::new(RemoteErrorType::NoSuchFileOrDirectory))?;

        debug!("{:?} opened", path);

        let stream = Cursor::new(node.value().content.as_ref().cloned().unwrap_or_default());
        let stream = Box::new(stream) as Box<dyn Read + Send>;

        Ok(ReadStream::from(stream))
    }

    fn on_written(&mut self, writable: WriteStream) -> RemoteResult<()> {
        let handle = Self::downcast_write_handle(writable);
        debug!("on_written({:?}, {:?})", handle.path, handle.mode);

        // get node
        let node = self
            .tree
            .root_mut()
            .query_mut(&handle.path)
            .ok_or_else(|| RemoteError::new(RemoteErrorType::NoSuchFileOrDirectory))?;

        let mut value = node.value().clone();

        value.content = match handle.mode {
            WriteMode::Append => {
                let mut content = value.content.as_ref().cloned().unwrap_or_default();
                content.extend_from_slice(handle.data.get_ref());
                Some(content)
            }
            WriteMode::Create => Some(handle.data.get_ref().to_vec()),
        };
        value.metadata.size = match handle.mode {
            WriteMode::Append => {
                let mut size = value.metadata.size;
                size += handle.data.get_ref().len() as u64;
                size
            }
            WriteMode::Create => handle.data.get_ref().len() as u64,
        };
        value.metadata.modified = Some(SystemTime::now());

        debug!("{:?} written {:?}", handle.path, value);
        node.set_value(value);

        Ok(())
    }
}
