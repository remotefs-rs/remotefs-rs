mod file_handle;
mod inode;

use std::ffi::OsStr;
use std::fs;
use std::hash::{Hash as _, Hasher as _};
use std::io::{Cursor, Read as _, Seek as _};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use fuser::{
    FileAttr, FileType, Filesystem, KernelConfig, ReplyAttr, ReplyCreate, ReplyData,
    ReplyDirectory, ReplyEmpty, ReplyEntry, ReplyOpen, ReplyStatfs, ReplyWrite, ReplyXattr,
    Request, TimeOrNow,
};
use libc::c_int;
use remotefs::fs::UnixPex;
use remotefs::{File, RemoteError, RemoteErrorType, RemoteFs, RemoteResult};

pub use self::file_handle::FileHandlersDb;
pub use self::inode::InodeDb;
use super::Driver;

const BLOCK_SIZE: usize = 512;
const FMODE_EXEC: i32 = 0x20;

/// Get the inode as [`u64`] number for a [`Path`]
fn inode(path: &Path) -> u64 {
    let mut hasher = seahash::SeaHasher::new();
    path.hash(&mut hasher);
    hasher.finish()
}

/// Convert a [`remotefs::fs::FileType`] to a [`FileType`] from [`fuser`]
fn convert_remote_filetype(filetype: remotefs::fs::FileType) -> FileType {
    match filetype {
        remotefs::fs::FileType::Directory => FileType::Directory,
        remotefs::fs::FileType::File => FileType::RegularFile,
        remotefs::fs::FileType::Symlink => FileType::Symlink,
    }
}

/// Convert a [`File`] from [`remotefs`] to a [`FileAttr`] from [`fuser`]
fn convert_file(value: &File) -> FileAttr {
    FileAttr {
        ino: inode(value.path()),
        size: value.metadata().size,
        blocks: (value.metadata().size + BLOCK_SIZE as u64 - 1) / BLOCK_SIZE as u64,
        atime: value.metadata().accessed.unwrap_or(UNIX_EPOCH),
        mtime: value.metadata().modified.unwrap_or(UNIX_EPOCH),
        ctime: value.metadata().created.unwrap_or(UNIX_EPOCH),
        crtime: UNIX_EPOCH,
        kind: convert_remote_filetype(value.metadata().file_type.clone()),
        perm: value
            .metadata()
            .mode
            .map(|mode| (u32::from(mode)) as u16)
            .unwrap_or(0o777),
        nlink: 0,
        uid: value.metadata().uid.unwrap_or(0),
        gid: value.metadata().gid.unwrap_or(0),
        rdev: 0,
        blksize: BLOCK_SIZE as u32,
        flags: 0,
    }
}

/// Convert a [`TimeOrNow`] to a [`SystemTime`]
fn time_or_now(t: TimeOrNow) -> SystemTime {
    match t {
        TimeOrNow::SpecificTime(t) => t,
        TimeOrNow::Now => SystemTime::now(),
    }
}

/// Convert a mode to a [`FileType`] from [`fuser`]
fn as_file_kind(mut mode: u32) -> Option<FileType> {
    mode &= libc::S_IFMT;

    if mode == libc::S_IFREG {
        Some(FileType::RegularFile)
    } else if mode == libc::S_IFLNK {
        Some(FileType::Symlink)
    } else if mode == libc::S_IFDIR {
        Some(FileType::Directory)
    } else {
        None
    }
}

impl Driver {
    /// Get the inode for a path.
    ///
    /// If the inode is not in the database, it will be fetched from the remote filesystem.
    fn get_inode_from_path(&mut self, path: &Path) -> RemoteResult<(File, FileAttr)> {
        let (file, attrs) = self.remote.stat(path).map(|file| {
            let attrs = convert_file(&file);
            (file, attrs)
        })?;

        // Save the inode to the database
        if !self.database.has(attrs.ino) {
            self.database.put(attrs.ino, path.to_path_buf());
        }

        Ok((file, attrs))
    }

    /// Get the inode from the inode number
    fn get_inode(&mut self, inode: u64) -> RemoteResult<(File, FileAttr)> {
        let path = self
            .database
            .get(inode)
            .ok_or_else(|| {
                remotefs::RemoteError::new(remotefs::RemoteErrorType::NoSuchFileOrDirectory)
            })?
            .to_path_buf();

        self.get_inode_from_path(&path)
    }

    /// Look up a name in a directory.
    fn lookup_name(&mut self, parent: u64, name: &OsStr) -> Option<PathBuf> {
        let parent_path = self.database.get(parent)?;
        let path = parent_path.join(name);

        // Get the inode and save it to the database
        let inode = inode(&path);
        if !self.database.has(inode) {
            self.database.put(inode, path.clone());
        }

        Some(path)
    }

    /// Check whether the user has access to a file's parent.
    fn check_parent_access(&mut self, inode: u64, request: &Request, access_mask: i32) -> bool {
        let (parent, _) = match self.get_inode(inode) {
            Ok(res) => res,
            Err(err) => {
                error!("Failed to get file attributes: {err}");
                return false;
            }
        };

        Self::check_access(&parent, request.uid(), request.gid(), access_mask)
    }

    /// Check whether the user has access to a file.
    fn check_access(file: &File, uid: u32, gid: u32, mut access_mask: i32) -> bool {
        debug!("Checking access for file: {:?} {:?}; UID: {uid}; GID: {gid} access_mask: {access_mask}", file.path(), file.metadata());
        if access_mask == libc::F_OK {
            return true;
        }

        let file_mode =
            u32::from(file.metadata().mode.unwrap_or_else(|| UnixPex::from(0o777))) as i32;

        // root is allowed to read & write anything
        if uid == 0 {
            // root only allowed to exec if one of the X bits is set
            access_mask &= libc::X_OK;
            access_mask -= access_mask & (file_mode >> 6);
            access_mask -= access_mask & (file_mode >> 3);
            access_mask -= access_mask & file_mode;
            return access_mask == 0;
        }

        if uid == file.metadata().uid.unwrap_or_default() {
            access_mask -= access_mask & (file_mode >> 6);
        } else if gid == file.metadata().gid.unwrap_or_default() {
            access_mask -= access_mask & (file_mode >> 3);
        } else {
            access_mask -= access_mask & file_mode;
        }

        access_mask == 0
    }

    /// Read data from a file.
    ///
    /// If possible, this system will use the stream from remotefs directly,
    /// otherwise it will use a temporary file (*sigh*).
    /// Note that most of remotefs supports streaming, so this should be rare.
    fn read(&mut self, path: &Path, buffer: &mut [u8], offset: u64) -> RemoteResult<usize> {
        match self.remote.open(path) {
            Ok(mut reader) => {
                debug!("Reading file from stream: {:?} at {offset}", path);
                if offset > 0 {
                    // read file until offset
                    let mut offset_buff = vec![0; offset as usize];
                    reader.read_exact(&mut offset_buff).map_err(|err| {
                        remotefs::RemoteError::new_ex(
                            remotefs::RemoteErrorType::IoError,
                            err.to_string(),
                        )
                    })?;
                }

                // read file
                let bytes_read = reader.read(buffer).map_err(|err| {
                    remotefs::RemoteError::new_ex(
                        remotefs::RemoteErrorType::IoError,
                        err.to_string(),
                    )
                })?;
                debug!("Read {bytes_read} bytes from stream; closing stream");

                // close file
                self.remote.on_read(reader)?;

                Ok(bytes_read)
            }
            Err(RemoteError {
                kind: RemoteErrorType::UnsupportedFeature,
                ..
            }) => self.read_tempfile(path, buffer, offset),
            Err(err) => Err(err),
        }
    }

    /// Read data from a file using a temporary file.
    fn read_tempfile(
        &mut self,
        path: &Path,
        buffer: &mut [u8],
        offset: u64,
    ) -> RemoteResult<usize> {
        let Ok(tempfile) = tempfile::NamedTempFile::new() else {
            return Err(remotefs::RemoteError::new(
                remotefs::RemoteErrorType::IoError,
            ));
        };
        let Ok(writer) = fs::OpenOptions::new().write(true).open(tempfile.path()) else {
            error!("Failed to open temporary file");
            return Err(remotefs::RemoteError::new(
                remotefs::RemoteErrorType::IoError,
            ));
        };

        // transfer to tempfile
        self.remote.open_file(path, Box::new(writer))?;

        let Ok(mut reader) = fs::File::open(tempfile.path()) else {
            error!("Failed to open temporary file");
            return Err(remotefs::RemoteError::new(
                remotefs::RemoteErrorType::IoError,
            ));
        };

        // skip to offset
        if offset > 0 {
            let mut offset_buff = vec![0; offset as usize];
            if let Err(err) = reader.read_exact(&mut offset_buff) {
                error!("Failed to read file: {err}");
                return Err(remotefs::RemoteError::new(
                    remotefs::RemoteErrorType::IoError,
                ));
            }
        }

        // read file
        reader.read_exact(buffer).map_err(|err| {
            remotefs::RemoteError::new_ex(remotefs::RemoteErrorType::IoError, err.to_string())
        })?;

        if let Err(err) = tempfile.close() {
            error!("Failed to close temporary file: {err}");
        }

        Ok(buffer.len())
    }

    /// Write data to a file.
    fn write(&mut self, file: &File, data: &[u8], offset: u64) -> RemoteResult<u32> {
        // write data
        let mut reader = Cursor::new(data);
        let mut writer = match self.remote.create(file.path(), file.metadata()) {
            Ok(writer) => writer,
            Err(RemoteError {
                kind: RemoteErrorType::UnsupportedFeature,
                ..
            }) if offset > 0 => {
                error!("remote file system doesn't support stream, so it is not possible to write at offset");
                return Err(RemoteError::new_ex(
                    RemoteErrorType::UnsupportedFeature,
                    "remote file system doesn't support stream, so it is not possible to write at offset".to_string(),
                ));
            }
            Err(RemoteError {
                kind: RemoteErrorType::UnsupportedFeature,
                ..
            }) => {
                return self.write_wno_stream(file, data);
            }
            Err(err) => {
                error!("Failed to write file: {err}");
                return Err(err);
            }
        };
        if offset > 0 {
            // try to seek
            if let Err(err) = writer.seek(std::io::SeekFrom::Start(offset)) {
                error!("Failed to seek file: {err}. Not that not all the remote filesystems support seeking");
                return Err(RemoteError::new_ex(
                    RemoteErrorType::IoError,
                    err.to_string(),
                ));
            }
        }
        // write
        let bytes_written = match std::io::copy(&mut reader, &mut writer) {
            Ok(bytes) => bytes as u32,
            Err(err) => {
                error!("Failed to write file: {err}");
                return Err(RemoteError::new_ex(
                    RemoteErrorType::IoError,
                    err.to_string(),
                ));
            }
        };
        // on write
        self.remote
            .on_written(writer)
            .map_err(|err| RemoteError::new_ex(RemoteErrorType::IoError, err.to_string()))?;

        Ok(bytes_written)
    }

    /// Write data to a file without using a stream.
    fn write_wno_stream(&mut self, file: &File, data: &[u8]) -> RemoteResult<u32> {
        debug!(
            "Writing file without stream: {:?} {} bytes",
            file.path(),
            data.len()
        );
        let reader = Cursor::new(data.to_vec());
        self.remote
            .create_file(file.path(), file.metadata(), Box::new(reader))
            .map(|len| len as u32)
    }
}

impl Filesystem for Driver {
    /// Initialize filesystem.
    /// Called before any other filesystem method.
    fn init(&mut self, _req: &Request, _config: &mut KernelConfig) -> Result<(), c_int> {
        info!("Initializing filesystem");
        if let Err(err) = self.remote.connect() {
            error!("Failed to connect to remote filesystem: {err}");
            return Err(libc::EIO);
        }
        info!("Connected to remote filesystem");

        Ok(())
    }

    /// Clean up filesystem.
    /// Called on filesystem exit.
    fn destroy(&mut self) {
        info!("Destroying filesystem");
        if let Err(err) = self.remote.disconnect() {
            error!("Failed to disconnect from remote filesystem: {err}");
        } else {
            info!("Disconnected from remote filesystem");
        }
    }

    /// Look up a directory entry by name and get its attributes.
    fn lookup(&mut self, req: &Request, parent: u64, name: &OsStr, reply: ReplyEntry) {
        debug!("lookup() called with {:?} {:?}", parent, name);
        let path = match self.lookup_name(parent, name) {
            Some(path) => path,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };

        let (file, attrs) = match self.get_inode_from_path(path.as_path()) {
            Err(err) => {
                error!("Failed to get file attributes: {err}");
                reply.error(libc::ENOENT);
                return;
            }
            Ok(res) => res,
        };

        if !Self::check_access(&file, req.uid(), req.gid(), libc::X_OK) {
            reply.error(libc::EACCES);
            return;
        }

        reply.entry(&Duration::new(0, 0), &attrs, 0)
    }

    /// Forget about an inode.
    /// The nlookup parameter indicates the number of lookups previously performed on
    /// this inode. If the filesystem implements inode lifetimes, it is recommended that
    /// inodes acquire a single reference on each lookup, and lose nlookup references on
    /// each forget. The filesystem may ignore forget calls, if the inodes don't need to
    /// have a limited lifetime. On unmount it is not guaranteed, that all referenced
    /// inodes will receive a forget message.
    fn forget(&mut self, _req: &Request, ino: u64, _nlookup: u64) {
        debug!("forget() called with {ino}");
        self.database.forget(ino);
    }

    /// Get file attributes.
    fn getattr(&mut self, _req: &Request, ino: u64, reply: ReplyAttr) {
        debug!("getattr() called with {:?}", ino);
        let attrs = match self.get_inode(ino) {
            Err(err) => {
                error!("Failed to get file attributes: {err}");
                reply.error(libc::ENOENT);
                return;
            }
            Ok((_, attrs)) => attrs,
        };

        reply.attr(&Duration::new(0, 0), &attrs);
    }

    /// Set file attributes.
    fn setattr(
        &mut self,
        req: &Request,
        ino: u64,
        mode: Option<u32>,
        uid: Option<u32>,
        gid: Option<u32>,
        size: Option<u64>,
        atime: Option<TimeOrNow>,
        mtime: Option<TimeOrNow>,
        ctime: Option<SystemTime>,
        _fh: Option<u64>,
        _crtime: Option<SystemTime>,
        _chgtime: Option<SystemTime>,
        _bkuptime: Option<SystemTime>,
        _flags: Option<u32>,
        reply: ReplyAttr,
    ) {
        debug!(
            "setattr() called with mode: {:?}, uid: {:?}, gid: {:?}, size: {:?}, atime: {:?}, mtime: {:?}, ctime: {:?}",
            mode, uid, gid, size, atime, mtime, ctime
        );
        let (mut file, _) = match self.get_inode(ino) {
            Ok(attrs) => attrs,
            Err(err) => {
                error!("Failed to get file attributes: {err}");
                reply.error(libc::ENOENT);
                return;
            }
        };

        if !Self::check_access(&file, req.uid(), req.gid(), libc::W_OK) {
            reply.error(libc::EACCES);
            return;
        }

        if let Some(mode) = mode {
            file.metadata.mode = Some(mode.into());
        }
        if let Some(uid) = uid {
            file.metadata.uid = Some(uid);
        }
        if let Some(gid) = gid {
            file.metadata.gid = Some(gid);
        }
        if let Some(size) = size {
            file.metadata.size = size;
        }
        if let Some(atime) = atime {
            file.metadata.accessed = Some(time_or_now(atime));
        }
        if let Some(mtime) = mtime {
            file.metadata.modified = Some(time_or_now(mtime));
        }
        if let Some(ctime) = ctime {
            file.metadata.created = Some(ctime);
        }

        // set attributes
        match self.remote.setstat(file.path(), file.metadata().clone()) {
            Ok(_) => {
                let attrs = convert_file(&file);
                reply.attr(&Duration::new(0, 0), &attrs);
            }
            Err(err) => {
                error!("Failed to set file attributes: {err}");
                reply.error(libc::EIO);
            }
        }
    }

    /// Read symbolic link.
    fn readlink(&mut self, _req: &Request, ino: u64, reply: ReplyData) {
        debug!("readlink() called with {:?}", ino);
        let (file, _) = match self.get_inode(ino) {
            Ok(attrs) => attrs,
            Err(err) => {
                error!("Failed to get file attributes: {err}");
                reply.error(libc::ENOENT);
                return;
            }
        };

        let mut buffer = vec![0; file.metadata().size as usize];
        if let Err(err) = self.read(file.path(), &mut buffer, 0) {
            error!("Failed to read file: {err}");
            reply.error(libc::EIO);
            return;
        }

        reply.data(&buffer);
    }

    /// Create file node.
    /// Create a regular file, character device, block device, fifo or socket node.
    fn mknod(
        &mut self,
        req: &Request,
        parent: u64,
        name: &OsStr,
        mode: u32,
        _umask: u32,
        _rdev: u32,
        reply: ReplyEntry,
    ) {
        debug!("mknod() called with {:?} {:?} {:o}", parent, name, mode);
        let file_type = mode & libc::S_IFMT;

        if file_type != libc::S_IFREG && file_type != libc::S_IFLNK && file_type != libc::S_IFDIR {
            warn!("mknod() implementation is incomplete. Only supports regular files, symlinks, and directories. Got {:o}", mode);
            reply.error(libc::ENOSYS);
            return;
        }

        let path = match self.lookup_name(parent, name) {
            Some(path) => path,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };

        // Check access for parent
        if !self.check_parent_access(parent, req, libc::W_OK) {
            reply.error(libc::EACCES);
            return;
        }

        // Check file type
        let res = match as_file_kind(mode) {
            Some(FileType::Directory) => self.remote.create_dir(&path, UnixPex::from(mode)),
            Some(FileType::RegularFile) => {
                let metadata = remotefs::fs::Metadata {
                    mode: Some(mode.into()),
                    gid: Some(req.gid()),
                    uid: Some(req.uid()),
                    ..Default::default()
                };
                let reader = Cursor::new(Vec::new());
                self.remote
                    .create_file(&path, &metadata, Box::new(reader))
                    .map(|_| ())
            }
            Some(_) | None => {
                warn!("mknod() implementation is incomplete. Only supports regular files and directories. Got {:o}", mode);
                reply.error(libc::ENOSYS);
                return;
            }
        };

        if let Err(err) = res {
            error!("Failed to create file: {err}");
            reply.error(libc::EIO);
            return;
        }

        // Get the inode
        match self.get_inode_from_path(path.as_path()) {
            Err(err) => {
                error!("Failed to get file attributes: {err}");
                reply.error(libc::ENOENT);
            }
            Ok((_, attrs)) => reply.entry(&Duration::new(0, 0), &attrs, 0),
        }
    }

    /// Create a directory.
    fn mkdir(
        &mut self,
        req: &Request,
        parent: u64,
        name: &OsStr,
        mode: u32,
        _umask: u32,
        reply: ReplyEntry,
    ) {
        debug!("mkdir() called with {:?} {:?} {:o}", parent, name, mode);
        let path = match self.lookup_name(parent, name) {
            Some(path) => path,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };

        // Check access for parent
        if !self.check_parent_access(parent, req, libc::W_OK) {
            reply.error(libc::EACCES);
            return;
        }

        let mode = UnixPex::from(mode);
        if let Err(err) = self.remote.create_dir(&path, mode) {
            error!("Failed to create directory: {err}");
            reply.error(libc::EIO);
            return;
        }

        // Get the inode
        match self.get_inode_from_path(path.as_path()) {
            Err(err) => {
                error!("Failed to get file attributes: {err}");
                reply.error(libc::ENOENT);
            }
            Ok((_, attrs)) => reply.entry(&Duration::new(0, 0), &attrs, 0),
        }
    }

    /// Remove a file
    fn unlink(&mut self, req: &Request, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        debug!("unlink() called with {:?} {:?}", parent, name);
        let path = match self.lookup_name(parent, name) {
            Some(path) => path,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };

        // Check access for parent
        if !self.check_parent_access(parent, req, libc::W_OK) {
            reply.error(libc::EACCES);
            return;
        }

        if let Err(err) = self.remote.remove_file(&path) {
            error!("Failed to remove file: {err}");
            reply.error(libc::EIO);
            return;
        }

        reply.ok();
    }

    /// Remove a directory
    fn rmdir(&mut self, req: &Request, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        debug!("rmdir() called with {:?} {:?}", parent, name);
        let path = match self.lookup_name(parent, name) {
            Some(path) => path,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };

        // Check access for parent
        if !self.check_parent_access(parent, req, libc::W_OK) {
            reply.error(libc::EACCES);
            return;
        }

        if let Err(err) = self.remote.remove_dir(&path) {
            error!("Failed to remove directory: {err}");
            reply.error(libc::EIO);
            return;
        }

        reply.ok();
    }

    /// Create a symbolic link
    fn symlink(
        &mut self,
        req: &Request,
        parent: u64,
        name: &OsStr,
        link: &Path,
        reply: ReplyEntry,
    ) {
        debug!("symlink() called with {:?} {:?} {:?}", parent, name, link);
        let path = match self.lookup_name(parent, name) {
            Some(path) => path,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };

        // Check access for parent
        if !self.check_parent_access(parent, req, libc::W_OK) {
            reply.error(libc::EACCES);
            return;
        }

        if let Err(err) = self.remote.symlink(&path, link) {
            error!("Failed to create symlink: {err}");
            reply.error(libc::EIO);
            return;
        }

        todo!();
    }

    /// Rename a file
    fn rename(
        &mut self,
        req: &Request,
        parent: u64,
        name: &OsStr,
        newparent: u64,
        newname: &OsStr,
        _flags: u32,
        reply: ReplyEmpty,
    ) {
        debug!(
            "rename() called with {:?} {:?} {:?} {:?}",
            parent, name, newparent, newname
        );

        // Check access for parent
        if !self.check_parent_access(parent, req, libc::W_OK) {
            reply.error(libc::EACCES);
            return;
        }

        let src = match self.lookup_name(parent, name) {
            Some(path) => path,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };

        // Check access for new parent
        if !self.check_parent_access(newparent, req, libc::W_OK) {
            reply.error(libc::EACCES);
            return;
        }

        let dest = match self.lookup_name(newparent, newname) {
            Some(path) => path,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };

        if let Err(err) = self.remote.mov(&src, &dest) {
            error!("Failed to move file: {err}");
            reply.error(libc::EIO);
            return;
        }

        // Update the database
        self.database.put(inode(&dest), dest);

        reply.ok();
    }

    /// Create a hard link
    fn link(
        &mut self,
        _req: &Request,
        _ino: u64,
        _newparent: u64,
        _newname: &OsStr,
        reply: ReplyEntry,
    ) {
        // not implemented
        reply.error(libc::ENOSYS);
    }

    /// Open a file.
    /// Open flags (with the exception of O_CREAT, O_EXCL, O_NOCTTY and O_TRUNC) are
    /// available in flags. Filesystem may store an arbitrary file handle (pointer, index,
    /// etc) in fh, and use this in other all other file operations (read, write, flush,
    /// release, fsync). Filesystem may also implement stateless file I/O and not store
    /// anything in fh. There are also some flags (direct_io, keep_cache) which the
    /// filesystem may set, to change the way the file is opened. See fuse_file_info
    /// structure in <fuse_common.h> for more details.
    fn open(&mut self, req: &Request, ino: u64, flags: i32, reply: ReplyOpen) {
        debug!("open() called for {ino}");
        let (access_mask, read, write) = match flags & libc::O_ACCMODE {
            libc::O_RDONLY => {
                // Behavior is undefined, but most filesystems return EACCES
                if flags & libc::O_TRUNC != 0 {
                    reply.error(libc::EACCES);
                    return;
                }
                if flags & FMODE_EXEC != 0 {
                    // Open is from internal exec syscall
                    (libc::X_OK, true, false)
                } else {
                    (libc::R_OK, true, false)
                }
            }
            libc::O_WRONLY => (libc::W_OK, false, true),
            libc::O_RDWR => (libc::R_OK | libc::W_OK, true, true),
            // Exactly one access mode flag must be specified
            _ => {
                reply.error(libc::EINVAL);
                return;
            }
        };

        let (file, _) = match self.get_inode(ino) {
            Ok(res) => res,
            Err(err) => {
                error!("Failed to get file attributes: {err}");
                reply.error(libc::ENOENT);
                return;
            }
        };

        if !Self::check_access(&file, req.uid(), req.gid(), access_mask) {
            reply.error(libc::EACCES);
            return;
        }

        // Set file handle and reply
        let fh = self.file_handlers.open(req.pid(), ino, read, write);
        reply.opened(fh, 0);
    }

    /// Read data.
    /// Read should send exactly the number of bytes requested except on EOF or error,
    /// otherwise the rest of the data will be substituted with zeroes. An exception to
    /// this is when the file has been opened in 'direct_io' mode, in which case the
    /// return value of the read system call will reflect the return value of this
    /// operation. fh will contain the value set by the open method, or will be undefined
    /// if the open method didn't set any value.
    fn read(
        &mut self,
        req: &Request,
        ino: u64,
        fh: u64,
        offset: i64,
        size: u32,
        _flags: i32,
        _lock_owner: Option<u64>,
        reply: ReplyData,
    ) {
        debug!("read() called for {ino} {size} bytes at {offset}");
        // check access
        if !self
            .file_handlers
            .get(req.pid(), fh)
            .map(|handler| handler.read)
            .unwrap_or_default()
        {
            debug!("No read permission for fh {fh}");
            reply.error(libc::EACCES);
            return;
        }
        // check offset
        if offset < 0 {
            debug!("Invalid offset {offset}");
            reply.error(libc::EINVAL);
            return;
        }

        let (file, _) = match self.get_inode(ino) {
            Ok(attrs) => attrs,
            Err(err) => {
                error!("Failed to get file attributes: {err}");
                reply.error(libc::ENOENT);
                return;
            }
        };

        let read_size = (size as u64).min(file.metadata().size.saturating_sub(offset as u64));
        debug!("Reading {read_size} bytes from at {offset}");
        let mut buffer = vec![0; read_size as usize];
        if let Err(err) = self.read(file.path(), &mut buffer, offset as u64) {
            error!("Failed to read file: {err}");
            reply.error(libc::EIO);
            return;
        }

        reply.data(&buffer);
    }

    /// Write data.
    /// Write should return exactly the number of bytes requested except on error. An
    /// exception to this is when the file has been opened in 'direct_io' mode, in
    /// which case the return value of the write system call will reflect the return
    /// value of this operation. fh will contain the value set by the open method, or
    /// will be undefined if the open method didn't set any value.
    fn write(
        &mut self,
        req: &Request,
        ino: u64,
        fh: u64,
        offset: i64,
        data: &[u8],
        _write_flags: u32,
        _flags: i32,
        _lock_owner: Option<u64>,
        reply: ReplyWrite,
    ) {
        debug!("write() called for {ino} {} bytes at {offset}", data.len());
        // check access
        if !self
            .file_handlers
            .get(req.pid(), fh)
            .map(|handler| handler.write)
            .unwrap_or_default()
        {
            debug!("No write permission for fh {fh}");
            reply.error(libc::EACCES);
            return;
        }
        // check offset
        if offset < 0 {
            debug!("Invalid offset {offset}");
            reply.error(libc::EINVAL);
            return;
        }

        let (file, _) = match self.get_inode(ino) {
            Ok(attrs) => attrs,
            Err(err) => {
                error!("Failed to get file attributes: {err}");
                reply.error(libc::ENOENT);
                return;
            }
        };

        // write data
        let bytes_written = match self.write(&file, data, offset as u64) {
            Ok(bytes) => bytes,
            Err(err) => {
                error!("Failed to write file: {err}");
                reply.error(libc::EIO);
                return;
            }
        };

        reply.written(bytes_written);
    }

    /// Flush method.
    /// This is called on each close() of the opened file. Since file descriptors can
    /// be duplicated (dup, dup2, fork), for one open call there may be many flush
    /// calls. Filesystems shouldn't assume that flush will always be called after some
    /// writes, or that if will be called at all. fh will contain the value set by the
    /// open method, or will be undefined if the open method didn't set any value.
    /// NOTE: the name of the method is misleading, since (unlike fsync) the filesystem
    /// is not forced to flush pending writes. One reason to flush data, is if the
    /// filesystem wants to return write errors. If the filesystem supports file locking
    /// operations (setlk, getlk) it should remove all locks belonging to 'lock_owner'.
    fn flush(&mut self, req: &Request, ino: u64, fh: u64, _lock_owner: u64, reply: ReplyEmpty) {
        debug!("flush() called for {ino}");

        // get fh
        if self.file_handlers.get(req.pid(), fh).is_none() {
            reply.error(libc::ENOENT);
            return;
        }

        // nop and ok
        reply.ok();
    }

    /// Release an open file.
    /// Release is called when there are no more references to an open file: all file
    /// descriptors are closed and all memory mappings are unmapped. For every open
    /// call there will be exactly one release call. The filesystem may reply with an
    /// error, but error values are not returned to close() or munmap() which triggered
    /// the release. fh will contain the value set by the open method, or will be undefined
    /// if the open method didn't set any value. flags will contain the same flags as for
    /// open.
    fn release(
        &mut self,
        req: &Request,
        _ino: u64,
        fh: u64,
        _flags: i32,
        _lock_owner: Option<u64>,
        _flush: bool,
        reply: ReplyEmpty,
    ) {
        // get fh
        if self.file_handlers.get(req.pid(), fh).is_none() {
            reply.error(libc::ENOENT);
            return;
        }

        // remove fh and ok
        self.file_handlers.close(req.pid(), fh);
        reply.ok();
    }

    /// Synchronize file contents.
    /// If the datasync parameter is non-zero, then only the user data should be flushed,
    /// not the meta data.
    fn fsync(&mut self, _req: &Request, _ino: u64, _fh: u64, _datasync: bool, reply: ReplyEmpty) {
        reply.ok();
    }

    /// Open a directory.
    /// Filesystem may store an arbitrary file handle (pointer, index, etc) in fh, and
    /// use this in other all other directory stream operations (readdir, releasedir,
    /// fsyncdir). Filesystem may also implement stateless directory I/O and not store
    /// anything in fh, though that makes it impossible to implement standard conforming
    /// directory stream operations in case the contents of the directory can change
    /// between opendir and releasedir.
    fn opendir(&mut self, req: &Request, ino: u64, flags: i32, reply: ReplyOpen) {
        debug!("opendir() called on {:?}", ino);
        let (access_mask, read, write) = match flags & libc::O_ACCMODE {
            libc::O_RDONLY => {
                // Behavior is undefined, but most filesystems return EACCES
                if flags & libc::O_TRUNC != 0 {
                    reply.error(libc::EACCES);
                    return;
                }
                (libc::R_OK, true, false)
            }
            libc::O_WRONLY => (libc::W_OK, false, true),
            libc::O_RDWR => (libc::R_OK | libc::W_OK, true, true),
            // Exactly one access mode flag must be specified
            _ => {
                reply.error(libc::EINVAL);
                return;
            }
        };

        let (file, _) = match self.get_inode(ino) {
            Ok(attrs) => attrs,
            Err(err) => {
                error!("Failed to get file attributes: {err}");
                reply.error(libc::ENOENT);
                return;
            }
        };

        if Self::check_access(&file, req.uid(), req.gid(), access_mask) {
            let fh = self.file_handlers.open(req.pid(), ino, read, write);
            reply.opened(fh, 0);
        } else {
            reply.error(libc::EACCES);
        }
    }

    /// Read directory.
    /// Send a buffer filled using buffer.fill(), with size not exceeding the
    /// requested size. Send an empty buffer on end of stream. fh will contain the
    /// value set by the opendir method, or will be undefined if the opendir method
    /// didn't set any value.
    fn readdir(
        &mut self,
        req: &Request,
        ino: u64,
        fh: u64,
        offset: i64,
        mut reply: ReplyDirectory,
    ) {
        debug!("readdir() called on {:?}", ino);
        // check fh with read permissions
        match self.file_handlers.get(req.pid(), fh) {
            Some(handler) if !handler.read => {
                reply.error(libc::EACCES);
                return;
            }
            None => {
                reply.error(libc::ENOENT);
                return;
            }
            _ => {}
        }

        // get directory
        let file = match self.get_inode(ino) {
            Ok((file, _)) => file,
            Err(err) => {
                error!("Failed to get file attributes: {err}");
                reply.error(libc::ENOENT);
                return;
            }
        };

        // list directory
        let entries = match self.remote.list_dir(file.path()) {
            Ok(entries) => entries,
            Err(err) => {
                error!("Failed to list directory: {err}");
                reply.error(libc::EIO);
                return;
            }
        };

        for (index, entry) in entries.into_iter().skip(offset as usize).enumerate() {
            let inode = inode(entry.path());
            let name = match entry.path().file_name() {
                Some(name) => OsStr::from_bytes(name.as_bytes()),
                None => {
                    error!("Failed to get file name");

                    continue;
                }
            };
            let buffer_full = reply.add(
                inode,
                offset + index as i64 + 1,
                convert_remote_filetype(entry.metadata().file_type.clone()),
                name,
            );

            if buffer_full {
                debug!("buffer is full");
                break;
            }
        }

        reply.ok();
    }

    /// Release an open directory.
    /// For every opendir call there will be exactly one releasedir call. fh will
    /// contain the value set by the opendir method, or will be undefined if the
    /// opendir method didn't set any value.
    fn releasedir(&mut self, req: &Request, _ino: u64, fh: u64, _flags: i32, reply: ReplyEmpty) {
        // get fh
        if self.file_handlers.get(req.pid(), fh).is_none() {
            reply.error(libc::ENOENT);
            return;
        }

        // remove fh and ok
        self.file_handlers.close(req.pid(), fh);
        reply.ok();
    }

    /// Synchronize directory contents.
    /// If the datasync parameter is set, then only the directory contents should
    /// be flushed, not the meta data. fh will contain the value set by the opendir
    /// method, or will be undefined if the opendir method didn't set any value.
    fn fsyncdir(&mut self, req: &Request, ino: u64, fh: u64, _datasync: bool, reply: ReplyEmpty) {
        debug!("fsyncdir() called for {ino}");
        // get fh
        if self.file_handlers.get(req.pid(), fh).is_none() {
            reply.error(libc::ENOENT);
            return;
        }
        reply.ok();
    }

    /// Get file system statistics.
    fn statfs(&mut self, _req: &Request, ino: u64, reply: ReplyStatfs) {
        debug!("statfs() called for {ino}");

        // get statfs
        struct FsStats {
            files: u64,
            size: u64,
        }

        let path = match self.get_inode(ino) {
            Ok((file, _)) => file.path().to_path_buf(),
            Err(_) => PathBuf::from("/"),
        };
        debug!("Getting filesystem statistics for {path:?}");

        // recursive directory iteration
        fn iter_dir(
            remote: &mut Box<dyn RemoteFs>,
            p: &Path,
            stats: &mut FsStats,
        ) -> RemoteResult<()> {
            let entries = remote.list_dir(p)?;
            for entry in entries {
                stats.files += 1;
                stats.size += entry.metadata().size;
                if entry.metadata().file_type == remotefs::fs::FileType::Directory {
                    iter_dir(remote, entry.path(), stats)?;
                }
            }
            Ok(())
        }

        let mut stats = FsStats { files: 0, size: 0 };
        if let Err(err) = iter_dir(&mut self.remote, &path, &mut stats) {
            error!("Failed to get filesystem statistics: {err}");
            reply.error(libc::EIO);
            return;
        }

        reply.statfs(
            stats.size / BLOCK_SIZE as u64,
            u64::MAX - stats.size / BLOCK_SIZE as u64,
            u64::MAX - stats.size / BLOCK_SIZE as u64,
            stats.files,
            0,
            BLOCK_SIZE as u32,
            255,
            0,
        );
    }

    /// Set an extended attribute.
    fn setxattr(
        &mut self,
        _req: &Request,
        ino: u64,
        name: &OsStr,
        value: &[u8],
        _flags: i32,
        _position: u32,
        reply: ReplyEmpty,
    ) {
        debug!("setxattr() called on {:?} {:?} {:?}", ino, name, value);
        // not supported
        reply.error(libc::ENOSYS);
    }

    /// Get an extended attribute.
    /// If `size` is 0, the size of the value should be sent with `reply.size()`.
    /// If `size` is not 0, and the value fits, send it with `reply.data()`, or
    /// `reply.error(ERANGE)` if it doesn't.
    fn getxattr(&mut self, _req: &Request, ino: u64, name: &OsStr, _size: u32, reply: ReplyXattr) {
        debug!("getxattr() called on {:?} {:?}", ino, name);
        // not supported
        reply.error(libc::ENOSYS);
    }

    /// List extended attribute names.
    /// If `size` is 0, the size of the value should be sent with `reply.size()`.
    /// If `size` is not 0, and the value fits, send it with `reply.data()`, or
    /// `reply.error(ERANGE)` if it doesn't.
    fn listxattr(&mut self, _req: &Request, ino: u64, size: u32, reply: ReplyXattr) {
        debug!("listxattr() called on {:?} {:?}", ino, size);
        // not supported
        reply.error(libc::ENOSYS);
    }

    /// Remove an extended attribute.
    fn removexattr(&mut self, _req: &Request, ino: u64, name: &OsStr, reply: ReplyEmpty) {
        debug!("removexattr() called on {:?} {:?}", ino, name);
        // not supported
        reply.error(libc::ENOSYS);
    }

    /// Check file access permissions.
    /// This will be called for the access() system call. If the 'default_permissions'
    /// mount option is given, this method is not called. This method is not called
    /// under Linux kernel versions 2.4.x
    fn access(&mut self, req: &Request, ino: u64, mask: i32, reply: ReplyEmpty) {
        debug!("access() called on {:?} {:o}", ino, mask);
        let file = match self.get_inode(ino) {
            Ok((file, _)) => file,
            Err(err) => {
                error!("Failed to get file attributes: {err}");
                reply.error(libc::ENOENT);
                return;
            }
        };

        if Self::check_access(&file, req.uid(), req.gid(), mask) {
            reply.ok();
        } else {
            reply.error(libc::EACCES);
        }
    }

    /// Create and open a file.
    /// If the file does not exist, first create it with the specified mode, and then
    /// open it. Open flags (with the exception of O_NOCTTY) are available in flags.
    /// Filesystem may store an arbitrary file handle (pointer, index, etc) in fh,
    /// and use this in other all other file operations (read, write, flush, release,
    /// fsync). There are also some flags (direct_io, keep_cache) which the
    /// filesystem may set, to change the way the file is opened. See fuse_file_info
    /// structure in <fuse_common.h> for more details. If this method is not
    /// implemented or under Linux kernel versions earlier than 2.6.15, the mknod()
    /// and open() methods will be called instead.
    fn create(
        &mut self,
        req: &Request,
        parent: u64,
        name: &OsStr,
        mode: u32,
        _umask: u32,
        flags: i32,
        reply: ReplyCreate,
    ) {
        debug!("create() called with {:?} {:?} {:o}", parent, name, mode);

        let (read, write) = match flags & libc::O_ACCMODE {
            libc::O_RDONLY => (true, false),
            libc::O_WRONLY => (false, true),
            libc::O_RDWR => (true, true),
            // Exactly one access mode flag must be specified
            _ => {
                reply.error(libc::EINVAL);
                return;
            }
        };

        let path = match self.lookup_name(parent, name) {
            Some(path) => path,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };

        let metadata = remotefs::fs::Metadata {
            mode: Some(mode.into()),
            gid: Some(req.gid()),
            uid: Some(req.uid()),
            ..Default::default()
        };
        let reader = Cursor::new(Vec::new());
        if let Err(err) = self.remote.create_file(&path, &metadata, Box::new(reader)) {
            error!("Failed to create file: {err}");
            reply.error(libc::EIO);
            return;
        }

        let inode = inode(&path);

        // return created
        match self.get_inode(inode) {
            Err(err) => {
                debug!("Failed to get file attributes: {err}");
                reply.error(libc::ENOENT);
            }
            Ok((_, attrs)) => {
                let fh = self.file_handlers.open(req.pid(), inode, read, write);
                reply.created(&Duration::new(0, 0), &attrs, 0, fh, 0);
            }
        }
    }
}
