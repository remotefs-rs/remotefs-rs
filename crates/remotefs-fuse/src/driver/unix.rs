#[cfg(feature = "tokio")]
pub(crate) mod r#async;
mod file_handle;
mod inode;
mod state;
#[cfg(test)]
mod test;

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use fuser::{
    AccessFlags as FuserAccessFlags, BsdFileFlags, FileAttr, FileHandle as FuserFileHandle,
    FileType, Filesystem, FopenFlags, Generation, INodeNo, KernelConfig, LockOwner, OpenFlags,
    RenameFlags, ReplyAttr, ReplyCreate, ReplyData, ReplyDirectory, ReplyEmpty, ReplyEntry,
    ReplyOpen, ReplyStatfs, ReplyWrite, ReplyXattr, Request, TimeOrNow, WriteFlags,
};
use inode::Inode;
use libc::mode_t;
use nix::sys::stat::SFlag;
use nix::unistd::AccessFlags;
use remotefs::fs::UnixPex;
use remotefs::{File, RemoteFs, RemoteResult};

use self::file_handle::PendingWrite;
use self::state::{BLOCK_SIZE, as_file_kind, convert_file, convert_remote_filetype};
use super::{Driver, transfer};
use crate::MountOption;

#[derive(Debug)]
pub(crate) struct DriverInner<T: RemoteFs> {
    pub(crate) state: state::UnixState<PendingWrite>,
    pub(crate) remote: T,
}

impl<T> DriverInner<T>
where
    T: RemoteFs,
{
    pub(crate) fn new(remote: T, options: Vec<MountOption>) -> Self {
        Self {
            state: state::UnixState::new(options),
            remote,
        }
    }
}

impl<T> Filesystem for Driver<T>
where
    T: RemoteFs + Send + Sync + 'static,
{
    fn init(&mut self, req: &Request, config: &mut KernelConfig) -> std::io::Result<()> {
        self.with_inner(|inner| inner.init(req, config))
    }

    fn destroy(&mut self) {
        self.with_inner(|inner| inner.destroy());
    }

    fn lookup(&self, req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEntry) {
        self.with_inner(|inner| inner.lookup(req, parent, name, reply));
    }

    fn forget(&self, req: &Request, ino: INodeNo, nlookup: u64) {
        self.with_inner(|inner| inner.forget(req, ino, nlookup));
    }

    fn getattr(&self, req: &Request, ino: INodeNo, fh: Option<FuserFileHandle>, reply: ReplyAttr) {
        self.with_inner(|inner| inner.getattr(req, ino, fh, reply));
    }

    fn setattr(
        &self,
        req: &Request,
        ino: INodeNo,
        mode: Option<u32>,
        uid: Option<u32>,
        gid: Option<u32>,
        size: Option<u64>,
        atime: Option<TimeOrNow>,
        mtime: Option<TimeOrNow>,
        ctime: Option<SystemTime>,
        fh: Option<FuserFileHandle>,
        crtime: Option<SystemTime>,
        chgtime: Option<SystemTime>,
        bkuptime: Option<SystemTime>,
        flags: Option<BsdFileFlags>,
        reply: ReplyAttr,
    ) {
        self.with_inner(|inner| {
            inner.setattr(
                req, ino, mode, uid, gid, size, atime, mtime, ctime, fh, crtime, chgtime, bkuptime,
                flags, reply,
            );
        });
    }

    fn readlink(&self, req: &Request, ino: INodeNo, reply: ReplyData) {
        self.with_inner(|inner| inner.readlink(req, ino, reply));
    }

    fn mknod(
        &self,
        req: &Request,
        parent: INodeNo,
        name: &OsStr,
        mode: u32,
        umask: u32,
        rdev: u32,
        reply: ReplyEntry,
    ) {
        self.with_inner(|inner| inner.mknod(req, parent, name, mode, umask, rdev, reply));
    }

    fn mkdir(
        &self,
        req: &Request,
        parent: INodeNo,
        name: &OsStr,
        mode: u32,
        umask: u32,
        reply: ReplyEntry,
    ) {
        self.with_inner(|inner| inner.mkdir(req, parent, name, mode, umask, reply));
    }

    fn unlink(&self, req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEmpty) {
        self.with_inner(|inner| inner.unlink(req, parent, name, reply));
    }

    fn rmdir(&self, req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEmpty) {
        self.with_inner(|inner| inner.rmdir(req, parent, name, reply));
    }

    fn symlink(
        &self,
        req: &Request,
        parent: INodeNo,
        link_name: &OsStr,
        target: &Path,
        reply: ReplyEntry,
    ) {
        self.with_inner(|inner| inner.symlink(req, parent, link_name, target, reply));
    }

    fn rename(
        &self,
        req: &Request,
        parent: INodeNo,
        name: &OsStr,
        newparent: INodeNo,
        newname: &OsStr,
        flags: RenameFlags,
        reply: ReplyEmpty,
    ) {
        self.with_inner(|inner| inner.rename(req, parent, name, newparent, newname, flags, reply));
    }

    fn link(
        &self,
        req: &Request,
        ino: INodeNo,
        newparent: INodeNo,
        newname: &OsStr,
        reply: ReplyEntry,
    ) {
        self.with_inner(|inner| inner.link(req, ino, newparent, newname, reply));
    }

    fn open(&self, req: &Request, ino: INodeNo, flags: OpenFlags, reply: ReplyOpen) {
        self.with_inner(|inner| inner.open(req, ino, flags, reply));
    }

    fn read(
        &self,
        req: &Request,
        ino: INodeNo,
        fh: FuserFileHandle,
        offset: u64,
        size: u32,
        flags: OpenFlags,
        lock_owner: Option<LockOwner>,
        reply: ReplyData,
    ) {
        self.with_inner(|inner| inner.read(req, ino, fh, offset, size, flags, lock_owner, reply));
    }

    fn write(
        &self,
        req: &Request,
        ino: INodeNo,
        fh: FuserFileHandle,
        offset: u64,
        data: &[u8],
        write_flags: WriteFlags,
        flags: OpenFlags,
        lock_owner: Option<LockOwner>,
        reply: ReplyWrite,
    ) {
        self.with_inner(|inner| {
            inner.write(
                req,
                ino,
                fh,
                offset,
                data,
                write_flags,
                flags,
                lock_owner,
                reply,
            );
        });
    }

    fn flush(
        &self,
        req: &Request,
        ino: INodeNo,
        fh: FuserFileHandle,
        lock_owner: LockOwner,
        reply: ReplyEmpty,
    ) {
        self.with_inner(|inner| inner.flush(req, ino, fh, lock_owner, reply));
    }

    fn release(
        &self,
        req: &Request,
        ino: INodeNo,
        fh: FuserFileHandle,
        flags: OpenFlags,
        lock_owner: Option<LockOwner>,
        flush: bool,
        reply: ReplyEmpty,
    ) {
        self.with_inner(|inner| inner.release(req, ino, fh, flags, lock_owner, flush, reply));
    }

    fn fsync(
        &self,
        req: &Request,
        ino: INodeNo,
        fh: FuserFileHandle,
        datasync: bool,
        reply: ReplyEmpty,
    ) {
        self.with_inner(|inner| inner.fsync(req, ino, fh, datasync, reply));
    }

    fn opendir(&self, req: &Request, ino: INodeNo, flags: OpenFlags, reply: ReplyOpen) {
        self.with_inner(|inner| inner.opendir(req, ino, flags, reply));
    }

    fn readdir(
        &self,
        req: &Request,
        ino: INodeNo,
        fh: FuserFileHandle,
        offset: u64,
        reply: ReplyDirectory,
    ) {
        self.with_inner(|inner| inner.readdir(req, ino, fh, offset, reply));
    }

    fn releasedir(
        &self,
        req: &Request,
        ino: INodeNo,
        fh: FuserFileHandle,
        flags: OpenFlags,
        reply: ReplyEmpty,
    ) {
        self.with_inner(|inner| inner.releasedir(req, ino, fh, flags, reply));
    }

    fn fsyncdir(
        &self,
        req: &Request,
        ino: INodeNo,
        fh: FuserFileHandle,
        datasync: bool,
        reply: ReplyEmpty,
    ) {
        self.with_inner(|inner| inner.fsyncdir(req, ino, fh, datasync, reply));
    }

    fn statfs(&self, req: &Request, ino: INodeNo, reply: ReplyStatfs) {
        self.with_inner(|inner| inner.statfs(req, ino, reply));
    }

    fn setxattr(
        &self,
        req: &Request,
        ino: INodeNo,
        name: &OsStr,
        value: &[u8],
        flags: i32,
        position: u32,
        reply: ReplyEmpty,
    ) {
        self.with_inner(|inner| inner.setxattr(req, ino, name, value, flags, position, reply));
    }

    fn getxattr(&self, req: &Request, ino: INodeNo, name: &OsStr, size: u32, reply: ReplyXattr) {
        self.with_inner(|inner| inner.getxattr(req, ino, name, size, reply));
    }

    fn listxattr(&self, req: &Request, ino: INodeNo, size: u32, reply: ReplyXattr) {
        self.with_inner(|inner| inner.listxattr(req, ino, size, reply));
    }

    fn removexattr(&self, req: &Request, ino: INodeNo, name: &OsStr, reply: ReplyEmpty) {
        self.with_inner(|inner| inner.removexattr(req, ino, name, reply));
    }

    fn access(&self, req: &Request, ino: INodeNo, mask: FuserAccessFlags, reply: ReplyEmpty) {
        self.with_inner(|inner| inner.access(req, ino, mask, reply));
    }

    fn create(
        &self,
        req: &Request,
        parent: INodeNo,
        name: &OsStr,
        mode: u32,
        umask: u32,
        flags: i32,
        reply: ReplyCreate,
    ) {
        self.with_inner(|inner| inner.create(req, parent, name, mode, umask, flags, reply));
    }
}

impl<T> DriverInner<T>
where
    T: RemoteFs,
{
    /// Get the inode for a path.
    ///
    /// If the inode is not in the database, it will be fetched from the remote filesystem.
    fn get_inode_from_path(&mut self, path: &Path) -> RemoteResult<(File, FileAttr)> {
        let inode = self.state.database.inode_for(path);
        let (file, attrs) = self.remote.stat(path).map(|file| {
            let attrs = convert_file(&file, inode, self.state.uid(), self.state.gid());
            (file, attrs)
        })?;

        Ok((file, attrs))
    }

    /// Get the inode from the [`Inode`] number
    fn get_inode(&mut self, inode: Inode) -> RemoteResult<(File, FileAttr)> {
        let path = self.state.inode_path(inode).ok_or_else(|| {
            remotefs::RemoteError::new(remotefs::RemoteErrorType::NoSuchFileOrDirectory)
        })?;

        self.get_inode_from_path(&path)
    }

    /// Check whether the user has access to a inode.
    fn check_inode_access(
        &mut self,
        inode: Inode,
        request: &Request,
        access_mask: AccessFlags,
    ) -> bool {
        let (parent, _) = match self.get_inode(inode) {
            Ok(res) => res,
            Err(err) => {
                error!("Failed to get file attributes: {err}");
                return false;
            }
        };

        self.state
            .check_access(&parent, request.uid(), request.gid(), access_mask)
    }

    /// Read up to `buffer.len()` bytes of `path` at `offset`; see [`transfer::read_at`].
    fn read_remote_file(
        &mut self,
        path: &Path,
        buffer: &mut [u8],
        offset: u64,
    ) -> RemoteResult<usize> {
        transfer::read_at(&self.remote, path, buffer, offset)
    }

    /// Write `data` at `offset` to the pending write staged for `(pid, fh)`,
    /// starting a new one (against `file`) if this is the first write to this
    /// handle. The remote write is finalized by [`Self::finalize_pending_write`]
    /// from `flush()`/`release()`.
    fn write_to_handle(
        &mut self,
        pid: u32,
        fh: u64,
        file: &File,
        data: &[u8],
        offset: u64,
    ) -> RemoteResult<u32> {
        if self.state.file_handlers.handle_state(pid, fh).is_none() {
            let state = transfer::start_pending_write(&self.remote, file)?;
            self.state.file_handlers.set_handle_state(
                pid,
                fh,
                PendingWrite {
                    file: file.clone(),
                    state,
                },
            );
        }

        let pending = self
            .state
            .file_handlers
            .handle_state(pid, fh)
            .expect("pending write was just inserted");
        transfer::write_to_pending(&mut pending.state, data, offset)
    }

    /// Finalize a pending write, actually persisting it to the remote filesystem.
    fn finalize_pending_write(&mut self, pending: PendingWrite) -> RemoteResult<()> {
        transfer::finalize_pending_write(&self.remote, &pending.file, pending.state)
    }
}

impl<T> Driver<T>
where
    T: RemoteFs,
{
    #[cfg(test)]
    fn get_inode_from_path(&self, path: &Path) -> RemoteResult<(File, FileAttr)> {
        self.with_inner(|inner| inner.get_inode_from_path(path))
    }

    #[cfg(test)]
    fn get_inode(&self, inode: INodeNo) -> RemoteResult<(File, FileAttr)> {
        self.with_inner(|inner| inner.get_inode(inode.0))
    }

    #[cfg(test)]
    fn lookup_name(&self, parent: INodeNo, name: &OsStr) -> Option<PathBuf> {
        self.with_inner(|inner| inner.state.lookup_name(parent.0, name))
    }

    #[cfg(test)]
    fn inode_for(&self, path: &Path) -> Inode {
        self.with_inner(|inner| inner.state.database.inode_for(path))
    }

    #[cfg(test)]
    fn read_remote_file(&self, path: &Path, buffer: &mut [u8], offset: u64) -> RemoteResult<usize> {
        self.with_inner(|inner| inner.read_remote_file(path, buffer, offset))
    }

    #[cfg(test)]
    fn check_access(&self, file: &File, uid: u32, gid: u32, access_mask: AccessFlags) -> bool {
        self.with_inner(|inner| inner.state.check_access(file, uid, gid, access_mask))
    }

    #[cfg(test)]
    fn write_to_handle(
        &self,
        pid: u32,
        fh: u64,
        file: &File,
        data: &[u8],
        offset: u64,
    ) -> RemoteResult<u32> {
        self.with_inner(|inner| inner.write_to_handle(pid, fh, file, data, offset))
    }

    #[cfg(test)]
    fn finalize_write(&self, pid: u32, fh: u64) -> RemoteResult<()> {
        self.with_inner(|inner| {
            let pending = inner
                .state
                .file_handlers
                .take_handle_state(pid, fh)
                .expect("no pending write to finalize");
            inner.finalize_pending_write(pending)
        })
    }

    #[cfg(test)]
    fn uid(&self) -> Option<u32> {
        self.with_inner(|inner| inner.state.uid())
    }

    #[cfg(test)]
    fn gid(&self) -> Option<u32> {
        self.with_inner(|inner| inner.state.gid())
    }
}

impl<T> DriverInner<T>
where
    T: RemoteFs,
{
    /// Initialize filesystem.
    /// Called before any other filesystem method.
    fn init(&mut self, _req: &Request, _config: &mut KernelConfig) -> std::io::Result<()> {
        info!("Initializing filesystem");
        if let Err(err) = self.remote.connect() {
            error!("Failed to connect to remote filesystem: {err}");
            return Err(std::io::Error::other(err.to_string()));
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
    fn lookup(&mut self, req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEntry) {
        debug!("lookup() called with {:?} {:?}", parent, name);
        let path = match self.state.lookup_name(parent.0, name) {
            Some(path) => path,
            None => {
                error!("Failed to lookup file: {name:?}");
                reply.error(fuser::Errno::ENOENT);
                return;
            }
        };

        let (file, attrs) = match self.get_inode_from_path(path.as_path()) {
            Err(err) => {
                error!("Failed to get file attributes: {err}");
                reply.error(fuser::Errno::ENOENT);
                return;
            }
            Ok(res) => res,
        };

        if !self
            .state
            .check_access(&file, req.uid(), req.gid(), AccessFlags::F_OK)
        {
            error!("No access to file: {path:?}");
            reply.error(fuser::Errno::EACCES);
            return;
        }

        reply.entry(&Duration::new(0, 0), &attrs, Generation(0))
    }

    /// Forget about an inode.
    /// The nlookup parameter indicates the number of lookups previously performed on
    /// this inode. If the filesystem implements inode lifetimes, it is recommended that
    /// inodes acquire a single reference on each lookup, and lose nlookup references on
    /// each forget. The filesystem may ignore forget calls, if the inodes don't need to
    /// have a limited lifetime. On unmount it is not guaranteed, that all referenced
    /// inodes will receive a forget message.
    fn forget(&mut self, _req: &Request, ino: INodeNo, _nlookup: u64) {
        debug!("forget() called with {ino}");
        self.state.database.forget(ino.0);
    }

    /// Get file attributes.
    fn getattr(
        &mut self,
        _req: &Request,
        ino: INodeNo,
        _fh: Option<FuserFileHandle>,
        reply: ReplyAttr,
    ) {
        debug!("getattr() called with {ino}");
        let attrs = match self.get_inode(ino.0) {
            Err(err) => {
                error!("Failed to get file attributes for {ino}: {err}");
                reply.error(fuser::Errno::ENOENT);
                return;
            }
            Ok((_, attrs)) => attrs,
        };

        reply.attr(&Duration::new(0, 0), &attrs);
    }

    /// Set file attributes.
    #[expect(
        clippy::too_many_arguments,
        reason = "mirrors the fixed argument list of fuser::Filesystem::setattr"
    )]
    fn setattr(
        &mut self,
        req: &Request,
        ino: INodeNo,
        mode: Option<u32>,
        uid: Option<u32>,
        gid: Option<u32>,
        size: Option<u64>,
        atime: Option<TimeOrNow>,
        mtime: Option<TimeOrNow>,
        ctime: Option<SystemTime>,
        _fh: Option<FuserFileHandle>,
        _crtime: Option<SystemTime>,
        _chgtime: Option<SystemTime>,
        _bkuptime: Option<SystemTime>,
        _flags: Option<BsdFileFlags>,
        reply: ReplyAttr,
    ) {
        debug!(
            "setattr() called with mode: {:?}, uid: {:?}, gid: {:?}, size: {:?}, atime: {:?}, mtime: {:?}, ctime: {:?}",
            mode, uid, gid, size, atime, mtime, ctime
        );
        let (file, _) = match self.get_inode(ino.0) {
            Ok(attrs) => attrs,
            Err(err) => {
                error!("Failed to get file attributes: {err}");
                reply.error(fuser::Errno::ENOENT);
                return;
            }
        };

        if !self
            .state
            .check_access(&file, req.uid(), req.gid(), AccessFlags::W_OK)
        {
            error!("No access to file: {}", file.path().display());
            reply.error(fuser::Errno::EACCES);
            return;
        }

        if let Some(changes) = state::setattr_changes(mode, uid, gid, atime, mtime)
            && let Err(err) = self.remote.set_metadata(file.path(), &changes)
        {
            error!("Failed to set file attributes: {err}");
            reply.error(fuser::Errno::EIO);
            return;
        }

        // `SetMetadata` carries no size: a size change is a truncate (or extend),
        // done by rewriting the file.
        if let Some(size) = size
            && let Err(err) = transfer::truncate_file(&self.remote, &file, size)
        {
            error!("Failed to truncate file: {err}");
            reply.error(fuser::Errno::EIO);
            return;
        }

        // Re-stat so the reply reflects what the remote actually stored.
        match self.get_inode_from_path(file.path()) {
            Ok((_, attrs)) => reply.attr(&Duration::new(0, 0), &attrs),
            Err(err) => {
                error!("Failed to get file attributes: {err}");
                reply.error(fuser::Errno::EIO);
            }
        }
    }

    /// Read symbolic link.
    fn readlink(&mut self, _req: &Request, ino: INodeNo, reply: ReplyData) {
        debug!("readlink() called with {:?}", ino);
        let (file, _) = match self.get_inode(ino.0) {
            Ok(attrs) => attrs,
            Err(err) => {
                error!("Failed to get file attributes: {err}");
                reply.error(fuser::Errno::ENOENT);
                return;
            }
        };

        match file.metadata().symlink.as_deref() {
            Some(target) => reply.data(target.as_os_str().as_bytes()),
            None => {
                error!(
                    "{} is not a symlink or has no target",
                    file.path().display()
                );
                reply.error(fuser::Errno::EINVAL);
            }
        }
    }

    /// Create file node.
    /// Create a regular file, character device, block device, fifo or socket node.
    #[expect(
        clippy::too_many_arguments,
        reason = "mirrors the fixed argument list of fuser::Filesystem::mknod"
    )]
    #[cfg_attr(
        target_os = "linux",
        expect(
            clippy::unnecessary_cast,
            reason = "mode_t is u32 on Linux (same as `mode`'s type) but a narrower type on macOS/BSD, so the cast is only redundant on Linux"
        )
    )]
    fn mknod(
        &mut self,
        req: &Request,
        parent: INodeNo,
        name: &OsStr,
        mode: u32,
        _umask: u32,
        _rdev: u32,
        reply: ReplyEntry,
    ) {
        debug!("mknod() called with {:?} {:?} {:o}", parent, name, mode);

        let mode = SFlag::from_bits_retain(mode as mode_t);
        let file_type = mode & SFlag::S_IFMT;

        if file_type != SFlag::S_IFREG && file_type != SFlag::S_IFLNK && file_type != SFlag::S_IFDIR
        {
            warn!(
                "mknod() implementation is incomplete. Only supports regular files, symlinks, and directories. Got {:o}",
                mode
            );
            reply.error(fuser::Errno::ENOSYS);
            return;
        }

        let path = match self.state.lookup_name(parent.0, name) {
            Some(path) => path,
            None => {
                error!("Failed to lookup file: {name:?}");
                reply.error(fuser::Errno::ENOENT);
                return;
            }
        };

        // Check access for parent
        if !self.check_inode_access(parent.0, req, AccessFlags::W_OK) {
            error!("No access to parent: {parent}");
            reply.error(fuser::Errno::EACCES);
            return;
        }

        // Check file type
        let res = match as_file_kind(mode) {
            Some(FileType::Directory) => self
                .remote
                .create_dir(&path, Some(UnixPex::from(mode.bits() as u32))),
            Some(FileType::RegularFile) => transfer::create_empty_file(
                &self.remote,
                &path,
                Some(UnixPex::from(mode.bits() as u32)),
            ),
            Some(_) | None => {
                warn!(
                    "mknod() implementation is incomplete. Only supports regular files and directories. Got {:o}",
                    mode
                );
                reply.error(fuser::Errno::ENOSYS);
                return;
            }
        };

        if let Err(err) = res {
            error!("Failed to create file: {err}");
            reply.error(fuser::Errno::EIO);
            return;
        }

        // Get the inode
        match self.get_inode_from_path(path.as_path()) {
            Err(err) => {
                error!("Failed to get file attributes: {err}");
                reply.error(fuser::Errno::ENOENT);
            }
            Ok((_, attrs)) => reply.entry(&Duration::new(0, 0), &attrs, Generation(0)),
        }
    }

    /// Create a directory.
    fn mkdir(
        &mut self,
        req: &Request,
        parent: INodeNo,
        name: &OsStr,
        mode: u32,
        _umask: u32,
        reply: ReplyEntry,
    ) {
        debug!("mkdir() called with {:?} {:?} {:o}", parent, name, mode);
        let path = match self.state.lookup_name(parent.0, name) {
            Some(path) => path,
            None => {
                error!("Failed to lookup file: {name:?}");
                reply.error(fuser::Errno::ENOENT);
                return;
            }
        };

        // Check access for parent
        if !self.check_inode_access(parent.0, req, AccessFlags::W_OK) {
            error!("No access to parent: {parent}");
            reply.error(fuser::Errno::EACCES);
            return;
        }

        let mode = UnixPex::from(mode);
        if let Err(err) = self.remote.create_dir(&path, Some(mode)) {
            error!("Failed to create directory: {err}");
            reply.error(fuser::Errno::EIO);
            return;
        }

        // Get the inode
        match self.get_inode_from_path(path.as_path()) {
            Err(err) => {
                error!("Failed to get file attributes: {err}");
                reply.error(fuser::Errno::ENOENT);
            }
            Ok((_, attrs)) => reply.entry(&Duration::new(0, 0), &attrs, Generation(0)),
        }
    }

    /// Remove a file
    fn unlink(&mut self, req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEmpty) {
        debug!("unlink() called with {:?} {:?}", parent, name);
        let path = match self.state.lookup_name(parent.0, name) {
            Some(path) => path,
            None => {
                error!("Failed to lookup file: {name:?}");
                reply.error(fuser::Errno::ENOENT);
                return;
            }
        };

        // Check access for parent
        if !self.check_inode_access(parent.0, req, AccessFlags::W_OK) {
            error!("No access to parent: {parent}");
            reply.error(fuser::Errno::EACCES);
            return;
        }

        if let Err(err) = self.remote.remove_file(&path) {
            error!("Failed to remove file: {err}");
            reply.error(fuser::Errno::EIO);
            return;
        }

        reply.ok();
    }

    /// Remove a directory
    fn rmdir(&mut self, req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEmpty) {
        debug!("rmdir() called with {:?} {:?}", parent, name);
        let path = match self.state.lookup_name(parent.0, name) {
            Some(path) => path,
            None => {
                error!("Failed to lookup file: {name:?}");
                reply.error(fuser::Errno::ENOENT);
                return;
            }
        };

        // Check access for parent
        if !self.check_inode_access(parent.0, req, AccessFlags::W_OK) {
            error!("No access to parent: {parent}");
            reply.error(fuser::Errno::EACCES);
            return;
        }

        if let Err(err) = self.remote.remove_dir(&path) {
            error!("Failed to remove directory: {err}");
            reply.error(fuser::Errno::EIO);
            return;
        }

        reply.ok();
    }

    /// Create a symbolic link
    fn symlink(
        &mut self,
        req: &Request,
        parent: INodeNo,
        name: &OsStr,
        link: &Path,
        reply: ReplyEntry,
    ) {
        debug!("symlink() called with {:?} {:?} {:?}", parent, name, link);
        let path = match self.state.lookup_name(parent.0, name) {
            Some(path) => path,
            None => {
                error!("Failed to lookup file: {name:?}");
                reply.error(fuser::Errno::ENOENT);
                return;
            }
        };

        // Check access for parent
        if !self.check_inode_access(parent.0, req, AccessFlags::W_OK) {
            error!("No access to parent: {parent}");
            reply.error(fuser::Errno::EACCES);
            return;
        }

        if let Err(err) = self.remote.symlink(&path, link) {
            error!("Failed to create symlink: {err}");
            reply.error(fuser::Errno::EIO);
            return;
        }

        // Get the inode
        match self.get_inode_from_path(path.as_path()) {
            Err(err) => {
                error!("Failed to get file attributes: {err}");
                reply.error(fuser::Errno::ENOENT);
            }
            Ok((_, attrs)) => reply.entry(&Duration::new(0, 0), &attrs, Generation(0)),
        }
    }

    /// Rename a file
    #[expect(
        clippy::too_many_arguments,
        reason = "mirrors the fixed argument list of fuser::Filesystem::rename"
    )]
    fn rename(
        &mut self,
        req: &Request,
        parent: INodeNo,
        name: &OsStr,
        newparent: INodeNo,
        newname: &OsStr,
        _flags: RenameFlags,
        reply: ReplyEmpty,
    ) {
        debug!(
            "rename() called with {:?} {:?} {:?} {:?}",
            parent, name, newparent, newname
        );

        // Check access for parent
        if !self.check_inode_access(parent.0, req, AccessFlags::W_OK) {
            error!("No access to parent: {parent}");
            reply.error(fuser::Errno::EACCES);
            return;
        }

        let src = match self.state.lookup_name(parent.0, name) {
            Some(path) => path,
            None => {
                error!("Failed to lookup file: {name:?}");
                reply.error(fuser::Errno::ENOENT);
                return;
            }
        };

        // Check access for new parent
        if !self.check_inode_access(newparent.0, req, AccessFlags::W_OK) {
            error!("No access to new parent: {newparent}");
            reply.error(fuser::Errno::EACCES);
            return;
        }

        let dest = match self.state.lookup_name(newparent.0, newname) {
            Some(path) => path,
            None => {
                error!("Failed to lookup file: {newname:?}");
                reply.error(fuser::Errno::ENOENT);
                return;
            }
        };

        if let Err(err) = self.remote.rename(&src, &dest) {
            error!("Failed to move file: {err}");
            reply.error(fuser::Errno::EIO);
            return;
        }

        reply.ok();
    }

    /// Create a hard link
    fn link(
        &mut self,
        _req: &Request,
        _ino: INodeNo,
        _newparent: INodeNo,
        _newname: &OsStr,
        reply: ReplyEntry,
    ) {
        debug!("link() called");
        // not implemented
        reply.error(fuser::Errno::ENOSYS);
    }

    /// Open a file.
    /// Open flags (with the exception of O_CREAT, O_EXCL, O_NOCTTY and O_TRUNC) are
    /// available in flags. Filesystem may store an arbitrary file handle (pointer, index,
    /// etc) in fh, and use this in other all other file operations (read, write, flush,
    /// release, fsync). Filesystem may also implement stateless file I/O and not store
    /// anything in fh. There are also some flags (direct_io, keep_cache) which the
    /// filesystem may set, to change the way the file is opened. See fuse_file_info
    /// structure in <fuse_common.h> for more details.
    fn open(&mut self, req: &Request, ino: INodeNo, flags: OpenFlags, reply: ReplyOpen) {
        debug!("open() called for {ino}");
        let mode = match state::parse_open_flags(flags.0) {
            Ok(mode) => mode,
            Err(errno) => {
                error!("Invalid access mode flags: {flags:?}");
                reply.error(errno);
                return;
            }
        };

        let (file, _) = match self.get_inode(ino.0) {
            Ok(res) => res,
            Err(err) => {
                error!("Failed to get file attributes: {err}");
                reply.error(fuser::Errno::ENOENT);
                return;
            }
        };

        if !self
            .state
            .check_access(&file, req.uid(), req.gid(), mode.access_mask)
        {
            error!("No access to file: {}", file.path().display());
            reply.error(fuser::Errno::EACCES);
            return;
        }

        // Set file handle and reply
        let fh = self
            .state
            .file_handlers
            .open(req.pid(), ino.0, mode.read, mode.write);
        reply.opened(FuserFileHandle(fh), FopenFlags::empty());
    }

    /// Read data.
    /// Read should send exactly the number of bytes requested except on EOF or error,
    /// otherwise the rest of the data will be substituted with zeroes. An exception to
    /// this is when the file has been opened in 'direct_io' mode, in which case the
    /// return value of the read system call will reflect the return value of this
    /// operation. fh will contain the value set by the open method, or will be undefined
    /// if the open method didn't set any value.
    #[expect(
        clippy::too_many_arguments,
        reason = "mirrors the fixed argument list of fuser::Filesystem::read"
    )]
    fn read(
        &mut self,
        req: &Request,
        ino: INodeNo,
        fh: FuserFileHandle,
        offset: u64,
        size: u32,
        _flags: OpenFlags,
        _lock_owner: Option<LockOwner>,
        reply: ReplyData,
    ) {
        debug!("read() called for {ino} {size} bytes at {offset}");
        // check access
        if !self
            .state
            .file_handlers
            .get(req.pid(), fh.0)
            .map(|handler| handler.read)
            .unwrap_or_default()
        {
            error!("No read permission for fh {fh} and pid {}", req.pid());
            reply.error(fuser::Errno::EACCES);
            return;
        }
        let (file, _) = match self.get_inode(ino.0) {
            Ok(attrs) => attrs,
            Err(err) => {
                error!("Failed to get file attributes: {err}");
                reply.error(fuser::Errno::ENOENT);
                return;
            }
        };

        let read_size = (size as u64).min(file.metadata().size.unwrap_or(0).saturating_sub(offset));
        debug!("Reading {read_size} bytes from at {offset}");
        let mut buffer = vec![0; read_size as usize];
        if let Err(err) = self.read_remote_file(file.path(), &mut buffer, offset) {
            error!("Failed to read file: {err}");
            reply.error(fuser::Errno::EIO);
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
    #[expect(
        clippy::too_many_arguments,
        reason = "mirrors the fixed argument list of fuser::Filesystem::write"
    )]
    fn write(
        &mut self,
        req: &Request,
        ino: INodeNo,
        fh: FuserFileHandle,
        offset: u64,
        data: &[u8],
        _write_flags: WriteFlags,
        _flags: OpenFlags,
        _lock_owner: Option<LockOwner>,
        reply: ReplyWrite,
    ) {
        debug!("write() called for {ino} {} bytes at {offset}", data.len());
        // check access
        if !self
            .state
            .file_handlers
            .get(req.pid(), fh.0)
            .map(|handler| handler.write)
            .unwrap_or_default()
        {
            debug!("No write permission for fh {fh}");
            reply.error(fuser::Errno::EACCES);
            return;
        }
        let (file, _) = match self.get_inode(ino.0) {
            Ok(attrs) => attrs,
            Err(err) => {
                error!("Failed to get file attributes: {err}");
                reply.error(fuser::Errno::ENOENT);
                return;
            }
        };

        // stage the write; it is only persisted to the remote once the handle is flushed
        let bytes_written = match self.write_to_handle(req.pid(), fh.0, &file, data, offset) {
            Ok(bytes) => bytes,
            Err(err) => {
                error!("Failed to write file: {err}");
                reply.error(fuser::Errno::EIO);
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
    fn flush(
        &mut self,
        req: &Request,
        ino: INodeNo,
        fh: FuserFileHandle,
        _lock_owner: LockOwner,
        reply: ReplyEmpty,
    ) {
        debug!("flush() called for {ino}");

        // get fh
        if self.state.file_handlers.get(req.pid(), fh.0).is_none() {
            error!("no file handler found for {fh} and pid {}", req.pid());
            reply.error(fuser::Errno::ENOENT);
            return;
        }

        // persist any write staged by write() and surface errors to the caller's close();
        // note this can leave the handle able to stage a new (truncating) write if more writes
        // follow this flush on the same, dup'd, file descriptor
        if let Some(pending) = self.state.file_handlers.take_handle_state(req.pid(), fh.0)
            && let Err(err) = self.finalize_pending_write(pending)
        {
            error!("Failed to flush write: {err}");
            reply.error(fuser::Errno::EIO);
            return;
        }

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
    #[expect(
        clippy::too_many_arguments,
        reason = "mirrors the fixed argument list of fuser::Filesystem::release"
    )]
    fn release(
        &mut self,
        req: &Request,
        _ino: INodeNo,
        fh: FuserFileHandle,
        _flags: OpenFlags,
        _lock_owner: Option<LockOwner>,
        _flush: bool,
        reply: ReplyEmpty,
    ) {
        // get fh
        if self.state.file_handlers.get(req.pid(), fh.0).is_none() {
            error!("no file handler found for {fh} and pid {}", req.pid());
            reply.error(fuser::Errno::ENOENT);
            return;
        }

        // defensively finalize any write that flush() never got a chance to (per fuser's docs,
        // flush is not guaranteed to be called); errors here can't be reported back to the
        // process that called close(), so just log them
        if let Some(pending) = self.state.file_handlers.take_handle_state(req.pid(), fh.0)
            && let Err(err) = self.finalize_pending_write(pending)
        {
            error!("Failed to finalize write on release: {err}");
        }

        // remove fh and ok
        self.state.file_handlers.close(req.pid(), fh.0);
        reply.ok();
    }

    /// Synchronize file contents.
    /// If the datasync parameter is non-zero, then only the user data should be flushed,
    /// not the meta data.
    fn fsync(
        &mut self,
        _req: &Request,
        _ino: INodeNo,
        _fh: FuserFileHandle,
        _datasync: bool,
        reply: ReplyEmpty,
    ) {
        reply.ok();
    }

    /// Open a directory.
    /// Filesystem may store an arbitrary file handle (pointer, index, etc) in fh, and
    /// use this in other all other directory stream operations (readdir, releasedir,
    /// fsyncdir). Filesystem may also implement stateless directory I/O and not store
    /// anything in fh, though that makes it impossible to implement standard conforming
    /// directory stream operations in case the contents of the directory can change
    /// between opendir and releasedir.
    fn opendir(&mut self, req: &Request, ino: INodeNo, flags: OpenFlags, reply: ReplyOpen) {
        debug!("opendir() called on {:?}", ino);
        let mode = match state::parse_opendir_flags(flags.0) {
            Ok(mode) => mode,
            Err(errno) => {
                error!("Invalid flags: {flags:?}");
                reply.error(errno);
                return;
            }
        };

        let (file, _) = match self.get_inode(ino.0) {
            Ok(attrs) => attrs,
            Err(err) => {
                error!("Failed to get file attributes: {err}");
                reply.error(fuser::Errno::ENOENT);
                return;
            }
        };

        if self
            .state
            .check_access(&file, req.uid(), req.gid(), mode.access_mask)
        {
            let fh = self
                .state
                .file_handlers
                .open(req.pid(), ino.0, mode.read, mode.write);
            reply.opened(FuserFileHandle(fh), FopenFlags::empty());
        } else {
            error!("No access to file: {ino}");
            reply.error(fuser::Errno::EACCES);
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
        ino: INodeNo,
        fh: FuserFileHandle,
        offset: u64,
        mut reply: ReplyDirectory,
    ) {
        debug!("readdir() called on {:?}", ino);
        // check fh with read permissions
        match self.state.file_handlers.get(req.pid(), fh.0) {
            Some(handler) if !handler.read => {
                error!("No read permission for fh {fh} and pid {}", req.pid());
                reply.error(fuser::Errno::EACCES);
                return;
            }
            None => {
                error!("no file handler found for {fh} and pid {}", req.pid());
                reply.error(fuser::Errno::ENOENT);
                return;
            }
            _ => {}
        }

        // get directory
        let file = match self.get_inode(ino.0) {
            Ok((file, _)) => file,
            Err(err) => {
                error!("Failed to get file attributes: {err}");
                reply.error(fuser::Errno::ENOENT);
                return;
            }
        };
        debug!("Reading directory {ino}: {}", file.path().display());

        // list directory
        let entries = match self.remote.list_dir(file.path()) {
            Ok(entries) => entries,
            Err(err) => {
                error!("Failed to list directory: {err}");
                reply.error(fuser::Errno::EIO);
                return;
            }
        };

        for (index, entry) in entries.into_iter().skip(offset as usize).enumerate() {
            let inode = self.state.database.inode_for(entry.path());
            debug!("Reading entry {inode} {index} {}", entry.path().display());
            let name = match entry.path().file_name() {
                Some(name) => OsStr::from_bytes(name.as_bytes()),
                None => {
                    error!("Failed to get file name {:?}", entry.path().display());
                    continue;
                }
            };
            let buffer_full = reply.add(
                INodeNo(inode),
                offset + index as u64 + 1,
                convert_remote_filetype(entry.metadata().file_type),
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
    fn releasedir(
        &mut self,
        req: &Request,
        _ino: INodeNo,
        fh: FuserFileHandle,
        _flags: OpenFlags,
        reply: ReplyEmpty,
    ) {
        // get fh
        if self.state.file_handlers.get(req.pid(), fh.0).is_none() {
            error!(
                "Failed to get file handler for {fh} and process {}",
                req.pid()
            );
            reply.error(fuser::Errno::ENOENT);
            return;
        }

        // remove fh and ok
        self.state.file_handlers.close(req.pid(), fh.0);
        reply.ok();
    }

    /// Synchronize directory contents.
    /// If the datasync parameter is set, then only the directory contents should
    /// be flushed, not the meta data. fh will contain the value set by the opendir
    /// method, or will be undefined if the opendir method didn't set any value.
    fn fsyncdir(
        &mut self,
        req: &Request,
        ino: INodeNo,
        fh: FuserFileHandle,
        _datasync: bool,
        reply: ReplyEmpty,
    ) {
        debug!("fsyncdir() called for {ino}");
        // get fh
        if self.state.file_handlers.get(req.pid(), fh.0).is_none() {
            error!(
                "Failed to get file handler for {fh} and process {}",
                req.pid()
            );
            reply.error(fuser::Errno::ENOENT);
            return;
        }
        reply.ok();
    }

    /// Get file system statistics.
    fn statfs(&mut self, _req: &Request, ino: INodeNo, reply: ReplyStatfs) {
        debug!("statfs() called for {ino}");

        // get statfs
        struct FsStats {
            files: u64,
            size: u64,
        }

        let path = match self.get_inode(ino.0) {
            Ok((file, _)) => file.path().to_path_buf(),
            Err(_) => PathBuf::from("/"),
        };
        debug!("Getting filesystem statistics for {path:?}");

        // recursive directory iteration
        fn iter_dir<T>(remote: &T, p: &Path, stats: &mut FsStats) -> RemoteResult<()>
        where
            T: RemoteFs,
        {
            let entries = remote.list_dir(p)?;
            for entry in entries {
                stats.files += 1;
                stats.size += entry.metadata().size.unwrap_or(0);
                if entry.metadata().file_type == remotefs::fs::FileType::Directory {
                    iter_dir(remote, entry.path(), stats)?;
                }
            }
            Ok(())
        }

        let mut stats = FsStats { files: 0, size: 0 };
        if let Err(err) = iter_dir(&self.remote, &path, &mut stats) {
            error!("Failed to get filesystem statistics: {err}");
            reply.error(fuser::Errno::EIO);
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
    #[expect(
        clippy::too_many_arguments,
        reason = "mirrors the fixed argument list of fuser::Filesystem::setxattr"
    )]
    fn setxattr(
        &mut self,
        _req: &Request,
        ino: INodeNo,
        name: &OsStr,
        value: &[u8],
        _flags: i32,
        _position: u32,
        reply: ReplyEmpty,
    ) {
        debug!("setxattr() called on {:?} {:?} {:?}", ino, name, value);
        // not supported
        reply.error(fuser::Errno::ENOSYS);
    }

    /// Get an extended attribute.
    /// If `size` is 0, the size of the value should be sent with `reply.size()`.
    /// If `size` is not 0, and the value fits, send it with `reply.data()`, or
    /// `reply.error(ERANGE)` if it doesn't.
    fn getxattr(
        &mut self,
        _req: &Request,
        ino: INodeNo,
        name: &OsStr,
        _size: u32,
        reply: ReplyXattr,
    ) {
        debug!("getxattr() called on {:?} {:?}", ino, name);
        // not supported
        reply.error(fuser::Errno::ENOSYS);
    }

    /// List extended attribute names.
    /// If `size` is 0, the size of the value should be sent with `reply.size()`.
    /// If `size` is not 0, and the value fits, send it with `reply.data()`, or
    /// `reply.error(ERANGE)` if it doesn't.
    fn listxattr(&mut self, _req: &Request, ino: INodeNo, size: u32, reply: ReplyXattr) {
        debug!("listxattr() called on {:?} {:?}", ino, size);
        // not supported
        reply.error(fuser::Errno::ENOSYS);
    }

    /// Remove an extended attribute.
    fn removexattr(&mut self, _req: &Request, ino: INodeNo, name: &OsStr, reply: ReplyEmpty) {
        debug!("removexattr() called on {:?} {:?}", ino, name);
        // not supported
        reply.error(fuser::Errno::ENOSYS);
    }

    /// Check file access permissions.
    /// This will be called for the access() system call. If the 'default_permissions'
    /// mount option is given, this method is not called. This method is not called
    /// under Linux kernel versions 2.4.x
    fn access(&mut self, req: &Request, ino: INodeNo, mask: FuserAccessFlags, reply: ReplyEmpty) {
        debug!("access() called on {:?} {:o}", ino, mask);
        let file = match self.get_inode(ino.0) {
            Ok((file, _)) => file,
            Err(err) => {
                error!("Failed to get file attributes: {err}");
                reply.error(fuser::Errno::ENOENT);
                return;
            }
        };

        if self.state.check_access(
            &file,
            req.uid(),
            req.gid(),
            AccessFlags::from_bits_truncate(mask.bits()),
        ) {
            reply.ok();
        } else {
            error!("No access to file: {}", file.path().display());
            reply.error(fuser::Errno::EACCES);
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
    #[expect(
        clippy::too_many_arguments,
        reason = "mirrors the fixed argument list of fuser::Filesystem::create"
    )]
    fn create(
        &mut self,
        req: &Request,
        parent: INodeNo,
        name: &OsStr,
        mode: u32,
        _umask: u32,
        flags: i32,
        reply: ReplyCreate,
    ) {
        debug!("create() called with {:?} {:?} {:o}", parent, name, mode);

        let (read, write) = match state::parse_create_flags(flags) {
            Ok(mode) => mode,
            Err(errno) => {
                error!("Invalid access mode flag: {flags:?}");
                reply.error(errno);
                return;
            }
        };

        let path = match self.state.lookup_name(parent.0, name) {
            Some(path) => path,
            None => {
                error!("Failed to lookup name {name:?}");
                reply.error(fuser::Errno::ENOENT);
                return;
            }
        };

        if let Err(err) =
            transfer::create_empty_file(&self.remote, &path, Some(UnixPex::from(mode)))
        {
            error!("Failed to create file: {err}");
            reply.error(fuser::Errno::EIO);
            return;
        }

        let inode = self.state.database.inode_for(&path);

        // return created
        match self.get_inode(inode) {
            Err(err) => {
                debug!("Failed to get file attributes: {err}");
                reply.error(fuser::Errno::ENOENT);
            }
            Ok((_, attrs)) => {
                let fh = self.state.file_handlers.open(req.pid(), inode, read, write);
                reply.created(
                    &Duration::new(0, 0),
                    &attrs,
                    Generation(0),
                    FuserFileHandle(fh),
                    FopenFlags::empty(),
                );
            }
        }
    }
}
