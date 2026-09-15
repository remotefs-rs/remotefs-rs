//! Native async fuser driver over an [`AsyncRemoteFs`].

#[cfg(test)]
mod test;
mod turnstile;

use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use fuser::{
    AccessFlags as FuserAccessFlags, BsdFileFlags, FileAttr, FileHandle as FuserFileHandle,
    FileType, Filesystem, FopenFlags, Generation, INodeNo, KernelConfig, LockOwner, OpenFlags,
    RenameFlags, ReplyAttr, ReplyCreate, ReplyData, ReplyDirectory, ReplyEmpty, ReplyEntry,
    ReplyOpen, ReplyStatfs, ReplyWrite, ReplyXattr, Request, TimeOrNow, WriteFlags,
};
use nix::sys::stat::SFlag;
use nix::unistd::AccessFlags;
use remotefs::fs::UnixPex;
use remotefs::{AsyncRemoteFs, File, RemoteError, RemoteErrorType, RemoteResult};
use tokio::runtime::Handle;
use tokio::sync::RwLock;

use self::turnstile::Turnstile;
use super::inode::Inode;
use super::state::{
    BLOCK_SIZE, RequestMeta, UnixState, as_file_kind, convert_file, convert_remote_filetype,
    parse_create_flags, parse_open_flags, parse_opendir_flags, setattr_changes,
};
use crate::MountOption;
use crate::driver::transfer::r#async::{self as transfer, AsyncPendingWriteState};

/// Per-open-handle state of the async driver.
#[derive(Debug, Default)]
pub(crate) struct HandleSlot {
    turnstile: Turnstile,
    pending: tokio::sync::Mutex<Option<AsyncPendingWrite>>,
}

/// A staged write and the file it targets.
#[derive(Debug)]
pub(crate) struct AsyncPendingWrite {
    file: File,
    state: AsyncPendingWriteState,
}

/// Shared core of the asynchronous Unix driver.
pub(crate) struct AsyncInner<T> {
    state: Mutex<UnixState<Arc<HandleSlot>>>,
    remote: Arc<RwLock<T>>,
}

/// Remote filesystem driver over an [`AsyncRemoteFs`] client.
pub(crate) struct AsyncDriver<T>
where
    T: AsyncRemoteFs + 'static,
{
    pub(super) inner: Arc<AsyncInner<T>>,
    handle: Handle,
}

impl<T> std::fmt::Debug for AsyncDriver<T>
where
    T: AsyncRemoteFs + 'static,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AsyncDriver").finish_non_exhaustive()
    }
}

impl<T> AsyncDriver<T>
where
    T: AsyncRemoteFs + 'static,
{
    /// Build a driver over an already connected remote client.
    pub(crate) fn new(remote: Arc<RwLock<T>>, options: Vec<MountOption>, handle: Handle) -> Self {
        Self {
            inner: Arc::new(AsyncInner {
                state: Mutex::new(UnixState::new(options)),
                remote,
            }),
            handle,
        }
    }

    fn spawn<F>(&self, request: impl FnOnce(Arc<AsyncInner<T>>) -> F)
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        self.handle.spawn(request(Arc::clone(&self.inner)));
    }
}

impl<T> AsyncInner<T>
where
    T: AsyncRemoteFs + 'static,
{
    /// Lock bookkeeping state. The guard must not cross an await point.
    pub(super) fn state(&self) -> std::sync::MutexGuard<'_, UnixState<Arc<HandleSlot>>> {
        self.state
            .lock()
            .expect("Unix async driver state lock poisoned")
    }

    /// Stat `path` and pair it with its inode.
    pub(super) async fn get_inode_from_path(&self, path: &Path) -> RemoteResult<(File, FileAttr)> {
        let inode = { self.state().database.inode_for(path) };
        let file = self.remote.read().await.stat(path).await?;
        let attrs = convert_file(&file, inode);
        Ok((file, attrs))
    }

    /// Resolve `inode` to its path and stat it.
    pub(super) async fn get_inode(&self, inode: Inode) -> RemoteResult<(File, FileAttr)> {
        let path = self
            .state()
            .inode_path(inode)
            .ok_or_else(|| RemoteError::new(RemoteErrorType::NoSuchFileOrDirectory))?;
        self.get_inode_from_path(&path).await
    }

    async fn check_inode_access(
        &self,
        inode: Inode,
        request: RequestMeta,
        access_mask: AccessFlags,
    ) -> bool {
        match self.get_inode(inode).await {
            Ok((file, _)) => {
                self.state()
                    .check_access(&file, request.uid, request.gid, access_mask)
            }
            Err(err) => {
                error!("Failed to get file attributes: {err}");
                false
            }
        }
    }

    pub(super) fn slot(&self, pid: u32, fh: u64) -> Option<Arc<HandleSlot>> {
        let mut state = self.state();
        state.file_handlers.handle_state(pid, fh).cloned()
    }

    pub(super) fn open_handle(&self, pid: u32, inode: Inode, read: bool, write: bool) -> u64 {
        let mut state = self.state();
        let fh = state.file_handlers.open(pid, inode, read, write);
        state
            .file_handlers
            .set_handle_state(pid, fh, Arc::new(HandleSlot::default()));
        fh
    }

    async fn write_to_handle(
        &self,
        slot: &HandleSlot,
        file: &File,
        data: &[u8],
        offset: u64,
    ) -> RemoteResult<u32> {
        let mut pending = slot.pending.lock().await;
        if pending.is_none() {
            let state = transfer::start_pending_write(&*self.remote.read().await, file).await?;
            *pending = Some(AsyncPendingWrite {
                file: file.clone(),
                state,
            });
        }
        let pending = pending.as_mut().expect("pending write was just inserted");
        transfer::write_to_pending(&mut pending.state, data, offset).await
    }

    async fn finalize_pending_write(&self, slot: &HandleSlot) -> RemoteResult<()> {
        let pending = slot.pending.lock().await.take();
        match pending {
            Some(AsyncPendingWrite { file, state }) => {
                transfer::finalize_pending_write(&*self.remote.read().await, &file, state).await
            }
            None => Ok(()),
        }
    }

    async fn fsync_pending_write(&self, slot: &HandleSlot) -> RemoteResult<()> {
        self.finalize_pending_write(slot).await
    }

    pub(super) async fn read_remote_file(
        &self,
        path: &Path,
        buffer: &mut [u8],
        offset: u64,
    ) -> RemoteResult<usize> {
        transfer::read_at(&*self.remote.read().await, path, buffer, offset).await
    }
}

impl<T> Filesystem for AsyncDriver<T>
where
    T: AsyncRemoteFs + 'static,
{
    fn init(&mut self, _req: &Request, _config: &mut KernelConfig) -> std::io::Result<()> {
        info!("Async filesystem initialized");
        Ok(())
    }

    fn destroy(&mut self) {
        info!("Async filesystem destroyed");
    }

    fn lookup(&self, req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEntry) {
        let request = RequestMeta::from(req);
        let name = name.to_owned();
        self.spawn(move |inner| async move {
            inner.lookup(request, parent, name, reply).await;
        });
    }

    fn forget(&self, _req: &Request, ino: INodeNo, _nlookup: u64) {
        self.inner.state().database.forget(ino.0);
    }

    fn getattr(
        &self,
        _req: &Request,
        ino: INodeNo,
        _fh: Option<FuserFileHandle>,
        reply: ReplyAttr,
    ) {
        self.spawn(move |inner| async move {
            inner.getattr(ino, reply).await;
        });
    }

    #[allow(clippy::too_many_arguments)]
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
        _fh: Option<FuserFileHandle>,
        _crtime: Option<SystemTime>,
        _chgtime: Option<SystemTime>,
        _bkuptime: Option<SystemTime>,
        _flags: Option<BsdFileFlags>,
        reply: ReplyAttr,
    ) {
        let request = RequestMeta::from(req);
        self.spawn(move |inner| async move {
            inner
                .setattr(
                    request, ino, mode, uid, gid, size, atime, mtime, ctime, reply,
                )
                .await;
        });
    }

    fn readlink(&self, _req: &Request, ino: INodeNo, reply: ReplyData) {
        self.spawn(move |inner| async move {
            inner.readlink(ino, reply).await;
        });
    }

    #[allow(clippy::too_many_arguments)]
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
        let request = RequestMeta::from(req);
        let name = name.to_owned();
        self.spawn(move |inner| async move {
            inner
                .mknod(request, parent, name, mode, umask, rdev, reply)
                .await;
        });
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
        let request = RequestMeta::from(req);
        let name = name.to_owned();
        self.spawn(move |inner| async move {
            inner.mkdir(request, parent, name, mode, umask, reply).await;
        });
    }

    fn unlink(&self, req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEmpty) {
        let request = RequestMeta::from(req);
        let name = name.to_owned();
        self.spawn(move |inner| async move {
            inner.unlink(request, parent, name, reply).await;
        });
    }

    fn rmdir(&self, req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEmpty) {
        let request = RequestMeta::from(req);
        let name = name.to_owned();
        self.spawn(move |inner| async move {
            inner.rmdir(request, parent, name, reply).await;
        });
    }

    fn symlink(
        &self,
        req: &Request,
        parent: INodeNo,
        link_name: &OsStr,
        target: &Path,
        reply: ReplyEntry,
    ) {
        let request = RequestMeta::from(req);
        let link_name = link_name.to_owned();
        let target = target.to_path_buf();
        self.spawn(move |inner| async move {
            inner
                .symlink(request, parent, link_name, target, reply)
                .await;
        });
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
        let request = RequestMeta::from(req);
        let name = name.to_owned();
        let newname = newname.to_owned();
        self.spawn(move |inner| async move {
            inner
                .rename(request, parent, name, newparent, newname, flags, reply)
                .await;
        });
    }

    fn link(
        &self,
        _req: &Request,
        _ino: INodeNo,
        _newparent: INodeNo,
        _newname: &OsStr,
        reply: ReplyEntry,
    ) {
        reply.error(fuser::Errno::ENOSYS);
    }

    fn open(&self, req: &Request, ino: INodeNo, flags: OpenFlags, reply: ReplyOpen) {
        let request = RequestMeta::from(req);
        self.spawn(move |inner| async move {
            inner.open(request, ino, flags, reply).await;
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn read(
        &self,
        req: &Request,
        ino: INodeNo,
        fh: FuserFileHandle,
        offset: u64,
        size: u32,
        _flags: OpenFlags,
        _lock_owner: Option<LockOwner>,
        reply: ReplyData,
    ) {
        let request = RequestMeta::from(req);
        self.spawn(move |inner| async move {
            inner.read(request, ino, fh, offset, size, reply).await;
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn write(
        &self,
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
        let request = RequestMeta::from(req);
        let data = data.to_vec();
        let Some(slot) = self.inner.slot(request.pid, fh.0) else {
            error!("no file handler found for {fh} and pid {}", request.pid);
            reply.error(fuser::Errno::ENOENT);
            return;
        };
        let ticket = slot.turnstile.ticket();
        self.spawn(move |inner| async move {
            let _turn = slot.turnstile.wait(ticket).await;
            inner
                .write(request, ino, fh, slot.as_ref(), offset, data, reply)
                .await;
        });
    }

    fn flush(
        &self,
        req: &Request,
        ino: INodeNo,
        fh: FuserFileHandle,
        _lock_owner: LockOwner,
        reply: ReplyEmpty,
    ) {
        let request = RequestMeta::from(req);
        let Some(slot) = self.inner.slot(request.pid, fh.0) else {
            error!("no file handler found for {fh} and pid {}", request.pid);
            reply.error(fuser::Errno::ENOENT);
            return;
        };
        let ticket = slot.turnstile.ticket();
        self.spawn(move |inner| async move {
            let _turn = slot.turnstile.wait(ticket).await;
            inner.flush(request, ino, fh, slot.as_ref(), reply).await;
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn release(
        &self,
        req: &Request,
        ino: INodeNo,
        fh: FuserFileHandle,
        _flags: OpenFlags,
        _lock_owner: Option<LockOwner>,
        _flush: bool,
        reply: ReplyEmpty,
    ) {
        let request = RequestMeta::from(req);
        let Some(slot) = self.inner.slot(request.pid, fh.0) else {
            error!("no file handler found for {fh} and pid {}", request.pid);
            reply.error(fuser::Errno::ENOENT);
            return;
        };
        let ticket = slot.turnstile.ticket();
        self.spawn(move |inner| async move {
            let _turn = slot.turnstile.wait(ticket).await;
            inner.release(request, ino, fh, slot.as_ref(), reply).await;
        });
    }

    fn fsync(
        &self,
        req: &Request,
        ino: INodeNo,
        fh: FuserFileHandle,
        _datasync: bool,
        reply: ReplyEmpty,
    ) {
        let request = RequestMeta::from(req);
        let Some(slot) = self.inner.slot(request.pid, fh.0) else {
            error!("no file handler found for {fh} and pid {}", request.pid);
            reply.error(fuser::Errno::ENOENT);
            return;
        };
        let ticket = slot.turnstile.ticket();
        self.spawn(move |inner| async move {
            let _turn = slot.turnstile.wait(ticket).await;
            inner.fsync(request, ino, fh, slot.as_ref(), reply).await;
        });
    }

    fn opendir(&self, req: &Request, ino: INodeNo, flags: OpenFlags, reply: ReplyOpen) {
        let request = RequestMeta::from(req);
        self.spawn(move |inner| async move {
            inner.opendir(request, ino, flags, reply).await;
        });
    }

    fn readdir(
        &self,
        req: &Request,
        ino: INodeNo,
        fh: FuserFileHandle,
        offset: u64,
        reply: ReplyDirectory,
    ) {
        let request = RequestMeta::from(req);
        self.spawn(move |inner| async move {
            inner.readdir(request, ino, fh, offset, reply).await;
        });
    }

    fn releasedir(
        &self,
        req: &Request,
        ino: INodeNo,
        fh: FuserFileHandle,
        _flags: OpenFlags,
        reply: ReplyEmpty,
    ) {
        let request = RequestMeta::from(req);
        self.inner.state().file_handlers.close(request.pid, fh.0);
        let _ = ino;
        reply.ok();
    }

    fn fsyncdir(
        &self,
        req: &Request,
        ino: INodeNo,
        fh: FuserFileHandle,
        _datasync: bool,
        reply: ReplyEmpty,
    ) {
        let request = RequestMeta::from(req);
        if self
            .inner
            .state()
            .file_handlers
            .get(request.pid, fh.0)
            .is_none()
        {
            error!("no file handler found for {fh} and pid {}", request.pid);
            reply.error(fuser::Errno::ENOENT);
            return;
        }
        let _ = ino;
        reply.ok();
    }

    fn statfs(&self, req: &Request, ino: INodeNo, reply: ReplyStatfs) {
        let request = RequestMeta::from(req);
        self.spawn(move |inner| async move {
            inner.statfs(request, ino, reply).await;
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn setxattr(
        &self,
        _req: &Request,
        ino: INodeNo,
        name: &OsStr,
        value: &[u8],
        _flags: i32,
        _position: u32,
        reply: ReplyEmpty,
    ) {
        debug!("setxattr() called on {ino:?} {name:?} {value:?}");
        reply.error(fuser::Errno::ENOSYS);
    }

    fn getxattr(&self, _req: &Request, ino: INodeNo, name: &OsStr, _size: u32, reply: ReplyXattr) {
        debug!("getxattr() called on {ino:?} {name:?}");
        reply.error(fuser::Errno::ENOSYS);
    }

    fn listxattr(&self, _req: &Request, ino: INodeNo, size: u32, reply: ReplyXattr) {
        debug!("listxattr() called on {ino:?} {size:?}");
        reply.error(fuser::Errno::ENOSYS);
    }

    fn removexattr(&self, _req: &Request, ino: INodeNo, name: &OsStr, reply: ReplyEmpty) {
        debug!("removexattr() called on {ino:?} {name:?}");
        reply.error(fuser::Errno::ENOSYS);
    }

    fn access(&self, req: &Request, ino: INodeNo, mask: FuserAccessFlags, reply: ReplyEmpty) {
        let request = RequestMeta::from(req);
        self.spawn(move |inner| async move {
            inner.access(request, ino, mask, reply).await;
        });
    }

    #[allow(clippy::too_many_arguments)]
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
        let request = RequestMeta::from(req);
        let name = name.to_owned();
        self.spawn(move |inner| async move {
            inner
                .create(request, parent, name, mode, umask, flags, reply)
                .await;
        });
    }
}

impl<T> AsyncInner<T>
where
    T: AsyncRemoteFs + 'static,
{
    async fn lookup(
        &self,
        request: RequestMeta,
        parent: INodeNo,
        name: OsString,
        reply: ReplyEntry,
    ) {
        debug!("lookup() called with {parent:?} {name:?}");
        let path = match self.state().lookup_name(parent.0, &name) {
            Some(path) => path,
            None => {
                error!("Failed to lookup file: {name:?}");
                reply.error(fuser::Errno::ENOENT);
                return;
            }
        };
        let (file, attrs) = match self.get_inode_from_path(&path).await {
            Ok(result) => result,
            Err(err) => {
                error!("Failed to get file attributes: {err}");
                reply.error(fuser::Errno::ENOENT);
                return;
            }
        };
        if !self
            .state()
            .check_access(&file, request.uid, request.gid, AccessFlags::F_OK)
        {
            error!("No access to file: {path:?}");
            reply.error(fuser::Errno::EACCES);
            return;
        }
        reply.entry(&Duration::new(0, 0), &attrs, Generation(0));
    }

    async fn getattr(&self, ino: INodeNo, reply: ReplyAttr) {
        debug!("getattr() called with {ino}");
        match self.get_inode(ino.0).await {
            Ok((_, attrs)) => reply.attr(&Duration::new(0, 0), &attrs),
            Err(err) => {
                error!("Failed to get file attributes for {ino}: {err}");
                reply.error(fuser::Errno::ENOENT);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn setattr(
        &self,
        request: RequestMeta,
        ino: INodeNo,
        mode: Option<u32>,
        uid: Option<u32>,
        gid: Option<u32>,
        size: Option<u64>,
        atime: Option<TimeOrNow>,
        mtime: Option<TimeOrNow>,
        ctime: Option<SystemTime>,
        reply: ReplyAttr,
    ) {
        debug!(
            "setattr() called with mode: {mode:?}, uid: {uid:?}, gid: {gid:?}, size: {size:?}, atime: {atime:?}, mtime: {mtime:?}, ctime: {ctime:?}"
        );
        let (file, _) = match self.get_inode(ino.0).await {
            Ok(result) => result,
            Err(err) => {
                error!("Failed to get file attributes: {err}");
                reply.error(fuser::Errno::ENOENT);
                return;
            }
        };
        if !self
            .state()
            .check_access(&file, request.uid, request.gid, AccessFlags::W_OK)
        {
            error!("No access to file: {}", file.path().display());
            reply.error(fuser::Errno::EACCES);
            return;
        }
        if let Some(changes) = setattr_changes(mode, uid, gid, atime, mtime)
            && let Err(err) = self
                .remote
                .read()
                .await
                .set_metadata(file.path(), &changes)
                .await
        {
            error!("Failed to set file attributes: {err}");
            reply.error(fuser::Errno::EIO);
            return;
        }
        if let Some(size) = size
            && let Err(err) = transfer::truncate_file(&*self.remote.read().await, &file, size).await
        {
            error!("Failed to truncate file: {err}");
            reply.error(fuser::Errno::EIO);
            return;
        }
        match self.get_inode_from_path(file.path()).await {
            Ok((_, attrs)) => reply.attr(&Duration::new(0, 0), &attrs),
            Err(err) => {
                error!("Failed to get file attributes: {err}");
                reply.error(fuser::Errno::EIO);
            }
        }
    }

    async fn readlink(&self, ino: INodeNo, reply: ReplyData) {
        let (file, _) = match self.get_inode(ino.0).await {
            Ok(result) => result,
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

    #[allow(clippy::too_many_arguments)]
    #[cfg_attr(
        target_os = "linux",
        expect(
            clippy::unnecessary_cast,
            reason = "mode_t is u32 on Linux but narrower on macOS and BSD"
        )
    )]
    async fn mknod(
        &self,
        request: RequestMeta,
        parent: INodeNo,
        name: OsString,
        mode: u32,
        _umask: u32,
        _rdev: u32,
        reply: ReplyEntry,
    ) {
        debug!("mknod() called with {parent:?} {name:?} {mode:o}");
        let mode = SFlag::from_bits_retain(mode as libc::mode_t);
        let file_type = mode & SFlag::S_IFMT;
        if file_type != SFlag::S_IFREG && file_type != SFlag::S_IFLNK && file_type != SFlag::S_IFDIR
        {
            warn!("mknod() implementation is incomplete for mode {mode:o}");
            reply.error(fuser::Errno::ENOSYS);
            return;
        }
        let path = match self.state().lookup_name(parent.0, &name) {
            Some(path) => path,
            None => {
                error!("Failed to lookup file: {name:?}");
                reply.error(fuser::Errno::ENOENT);
                return;
            }
        };
        if !self
            .check_inode_access(parent.0, request, AccessFlags::W_OK)
            .await
        {
            error!("No access to parent: {parent}");
            reply.error(fuser::Errno::EACCES);
            return;
        }
        let result = match as_file_kind(mode) {
            Some(FileType::Directory) => {
                self.remote
                    .read()
                    .await
                    .create_dir(&path, Some(UnixPex::from(mode.bits() as u32)))
                    .await
            }
            Some(FileType::RegularFile) => {
                transfer::create_empty_file(
                    &*self.remote.read().await,
                    &path,
                    Some(UnixPex::from(mode.bits() as u32)),
                )
                .await
            }
            Some(_) | None => {
                warn!("mknod() implementation is incomplete for mode {mode:o}");
                reply.error(fuser::Errno::ENOSYS);
                return;
            }
        };
        if let Err(err) = result {
            error!("Failed to create file: {err}");
            reply.error(fuser::Errno::EIO);
            return;
        }
        match self.get_inode_from_path(&path).await {
            Ok((_, attrs)) => reply.entry(&Duration::new(0, 0), &attrs, Generation(0)),
            Err(err) => {
                error!("Failed to get file attributes: {err}");
                reply.error(fuser::Errno::ENOENT);
            }
        }
    }

    async fn mkdir(
        &self,
        request: RequestMeta,
        parent: INodeNo,
        name: OsString,
        mode: u32,
        _umask: u32,
        reply: ReplyEntry,
    ) {
        let path = match self.state().lookup_name(parent.0, &name) {
            Some(path) => path,
            None => {
                error!("Failed to lookup file: {name:?}");
                reply.error(fuser::Errno::ENOENT);
                return;
            }
        };
        if !self
            .check_inode_access(parent.0, request, AccessFlags::W_OK)
            .await
        {
            error!("No access to parent: {parent}");
            reply.error(fuser::Errno::EACCES);
            return;
        }
        if let Err(err) = self
            .remote
            .read()
            .await
            .create_dir(&path, Some(UnixPex::from(mode)))
            .await
        {
            error!("Failed to create directory: {err}");
            reply.error(fuser::Errno::EIO);
            return;
        }
        match self.get_inode_from_path(&path).await {
            Ok((_, attrs)) => reply.entry(&Duration::new(0, 0), &attrs, Generation(0)),
            Err(err) => {
                error!("Failed to get file attributes: {err}");
                reply.error(fuser::Errno::ENOENT);
            }
        }
    }

    async fn unlink(
        &self,
        request: RequestMeta,
        parent: INodeNo,
        name: OsString,
        reply: ReplyEmpty,
    ) {
        let path = match self.state().lookup_name(parent.0, &name) {
            Some(path) => path,
            None => {
                error!("Failed to lookup file: {name:?}");
                reply.error(fuser::Errno::ENOENT);
                return;
            }
        };
        if !self
            .check_inode_access(parent.0, request, AccessFlags::W_OK)
            .await
        {
            error!("No access to parent: {parent}");
            reply.error(fuser::Errno::EACCES);
            return;
        }
        if let Err(err) = self.remote.read().await.remove_file(&path).await {
            error!("Failed to remove file: {err}");
            reply.error(fuser::Errno::EIO);
            return;
        }
        reply.ok();
    }

    async fn rmdir(
        &self,
        request: RequestMeta,
        parent: INodeNo,
        name: OsString,
        reply: ReplyEmpty,
    ) {
        let path = match self.state().lookup_name(parent.0, &name) {
            Some(path) => path,
            None => {
                error!("Failed to lookup file: {name:?}");
                reply.error(fuser::Errno::ENOENT);
                return;
            }
        };
        if !self
            .check_inode_access(parent.0, request, AccessFlags::W_OK)
            .await
        {
            error!("No access to parent: {parent}");
            reply.error(fuser::Errno::EACCES);
            return;
        }
        if let Err(err) = self.remote.read().await.remove_dir(&path).await {
            error!("Failed to remove directory: {err}");
            reply.error(fuser::Errno::EIO);
            return;
        }
        reply.ok();
    }

    async fn symlink(
        &self,
        request: RequestMeta,
        parent: INodeNo,
        name: OsString,
        target: PathBuf,
        reply: ReplyEntry,
    ) {
        let path = match self.state().lookup_name(parent.0, &name) {
            Some(path) => path,
            None => {
                error!("Failed to lookup file: {name:?}");
                reply.error(fuser::Errno::ENOENT);
                return;
            }
        };
        if !self
            .check_inode_access(parent.0, request, AccessFlags::W_OK)
            .await
        {
            error!("No access to parent: {parent}");
            reply.error(fuser::Errno::EACCES);
            return;
        }
        if let Err(err) = self.remote.read().await.symlink(&path, &target).await {
            error!("Failed to create symlink: {err}");
            reply.error(fuser::Errno::EIO);
            return;
        }
        match self.get_inode_from_path(&path).await {
            Ok((_, attrs)) => reply.entry(&Duration::new(0, 0), &attrs, Generation(0)),
            Err(err) => {
                error!("Failed to get file attributes: {err}");
                reply.error(fuser::Errno::ENOENT);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn rename(
        &self,
        request: RequestMeta,
        parent: INodeNo,
        name: OsString,
        newparent: INodeNo,
        newname: OsString,
        _flags: RenameFlags,
        reply: ReplyEmpty,
    ) {
        if !self
            .check_inode_access(parent.0, request, AccessFlags::W_OK)
            .await
        {
            error!("No access to parent: {parent}");
            reply.error(fuser::Errno::EACCES);
            return;
        }
        let src = match self.state().lookup_name(parent.0, &name) {
            Some(path) => path,
            None => {
                error!("Failed to lookup file: {name:?}");
                reply.error(fuser::Errno::ENOENT);
                return;
            }
        };
        if !self
            .check_inode_access(newparent.0, request, AccessFlags::W_OK)
            .await
        {
            error!("No access to new parent: {newparent}");
            reply.error(fuser::Errno::EACCES);
            return;
        }
        let dest = match self.state().lookup_name(newparent.0, &newname) {
            Some(path) => path,
            None => {
                error!("Failed to lookup file: {newname:?}");
                reply.error(fuser::Errno::ENOENT);
                return;
            }
        };
        if let Err(err) = self.remote.read().await.rename(&src, &dest).await {
            error!("Failed to move file: {err}");
            reply.error(fuser::Errno::EIO);
            return;
        }
        reply.ok();
    }

    async fn open(&self, request: RequestMeta, ino: INodeNo, flags: OpenFlags, reply: ReplyOpen) {
        let mode = match parse_open_flags(flags.0) {
            Ok(mode) => mode,
            Err(errno) => {
                error!("Invalid access mode flags: {flags:?}");
                reply.error(errno);
                return;
            }
        };
        let (file, _) = match self.get_inode(ino.0).await {
            Ok(result) => result,
            Err(err) => {
                error!("Failed to get file attributes: {err}");
                reply.error(fuser::Errno::ENOENT);
                return;
            }
        };
        if !self
            .state()
            .check_access(&file, request.uid, request.gid, mode.access_mask)
        {
            error!("No access to file: {}", file.path().display());
            reply.error(fuser::Errno::EACCES);
            return;
        }
        let fh = self.open_handle(request.pid, ino.0, mode.read, mode.write);
        reply.opened(FuserFileHandle(fh), FopenFlags::empty());
    }

    async fn read(
        &self,
        request: RequestMeta,
        ino: INodeNo,
        fh: FuserFileHandle,
        offset: u64,
        size: u32,
        reply: ReplyData,
    ) {
        debug!("read() called for {ino} {size} bytes at {offset}");
        if !self
            .state()
            .file_handlers
            .get(request.pid, fh.0)
            .map(|handler| handler.read)
            .unwrap_or_default()
        {
            error!("No read permission for fh {fh} and pid {}", request.pid);
            reply.error(fuser::Errno::EACCES);
            return;
        }
        let (file, _) = match self.get_inode(ino.0).await {
            Ok(result) => result,
            Err(err) => {
                error!("Failed to get file attributes: {err}");
                reply.error(fuser::Errno::ENOENT);
                return;
            }
        };
        let read_size = (size as u64).min(file.metadata().size.unwrap_or(0).saturating_sub(offset));
        let mut buffer = vec![0; read_size as usize];
        match self
            .read_remote_file(file.path(), &mut buffer, offset)
            .await
        {
            Ok(read) => reply.data(&buffer[..read]),
            Err(err) => {
                error!("Failed to read file: {err}");
                reply.error(fuser::Errno::EIO);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn write(
        &self,
        request: RequestMeta,
        ino: INodeNo,
        fh: FuserFileHandle,
        slot: &HandleSlot,
        offset: u64,
        data: Vec<u8>,
        reply: ReplyWrite,
    ) {
        debug!("write() called for {ino} {} bytes at {offset}", data.len());
        if !self
            .state()
            .file_handlers
            .get(request.pid, fh.0)
            .map(|handler| handler.write)
            .unwrap_or_default()
        {
            debug!("No write permission for fh {fh}");
            reply.error(fuser::Errno::EACCES);
            return;
        }
        let (file, _) = match self.get_inode(ino.0).await {
            Ok(result) => result,
            Err(err) => {
                error!("Failed to get file attributes: {err}");
                reply.error(fuser::Errno::ENOENT);
                return;
            }
        };
        match self.write_to_handle(slot, &file, &data, offset).await {
            Ok(written) => reply.written(written),
            Err(err) => {
                error!("Failed to write file: {err}");
                reply.error(fuser::Errno::EIO);
            }
        }
    }

    async fn flush(
        &self,
        request: RequestMeta,
        ino: INodeNo,
        fh: FuserFileHandle,
        slot: &HandleSlot,
        reply: ReplyEmpty,
    ) {
        debug!("flush() called for {ino}");
        if self.state().file_handlers.get(request.pid, fh.0).is_none() {
            error!("no file handler found for {fh} and pid {}", request.pid);
            reply.error(fuser::Errno::ENOENT);
            return;
        }
        if let Err(err) = self.finalize_pending_write(slot).await {
            error!("Failed to flush write: {err}");
            reply.error(fuser::Errno::EIO);
            return;
        }
        reply.ok();
    }

    async fn release(
        &self,
        request: RequestMeta,
        ino: INodeNo,
        fh: FuserFileHandle,
        slot: &HandleSlot,
        reply: ReplyEmpty,
    ) {
        if self.state().file_handlers.get(request.pid, fh.0).is_none() {
            error!("no file handler found for {fh} and pid {}", request.pid);
            reply.error(fuser::Errno::ENOENT);
            return;
        }
        if let Err(err) = self.finalize_pending_write(slot).await {
            error!("Failed to finalize write on release for {ino}: {err}");
        }
        self.state().file_handlers.close(request.pid, fh.0);
        reply.ok();
    }

    async fn fsync(
        &self,
        _request: RequestMeta,
        _ino: INodeNo,
        _fh: FuserFileHandle,
        slot: &HandleSlot,
        reply: ReplyEmpty,
    ) {
        if let Err(err) = self.fsync_pending_write(slot).await {
            error!("Failed to fsync write: {err}");
            reply.error(fuser::Errno::EIO);
            return;
        }
        reply.ok();
    }

    async fn opendir(
        &self,
        request: RequestMeta,
        ino: INodeNo,
        flags: OpenFlags,
        reply: ReplyOpen,
    ) {
        let mode = match parse_opendir_flags(flags.0) {
            Ok(mode) => mode,
            Err(errno) => {
                error!("Invalid flags: {flags:?}");
                reply.error(errno);
                return;
            }
        };
        let (file, _) = match self.get_inode(ino.0).await {
            Ok(result) => result,
            Err(err) => {
                error!("Failed to get file attributes: {err}");
                reply.error(fuser::Errno::ENOENT);
                return;
            }
        };
        if self
            .state()
            .check_access(&file, request.uid, request.gid, mode.access_mask)
        {
            let fh = self.open_handle(request.pid, ino.0, mode.read, mode.write);
            reply.opened(FuserFileHandle(fh), FopenFlags::empty());
        } else {
            error!("No access to file: {ino}");
            reply.error(fuser::Errno::EACCES);
        }
    }

    async fn readdir(
        &self,
        request: RequestMeta,
        ino: INodeNo,
        fh: FuserFileHandle,
        offset: u64,
        mut reply: ReplyDirectory,
    ) {
        match self.state().file_handlers.get(request.pid, fh.0) {
            Some(handler) if !handler.read => {
                error!("No read permission for fh {fh} and pid {}", request.pid);
                reply.error(fuser::Errno::EACCES);
                return;
            }
            None => {
                error!("no file handler found for {fh} and pid {}", request.pid);
                reply.error(fuser::Errno::ENOENT);
                return;
            }
            _ => {}
        }
        let file = match self.get_inode(ino.0).await {
            Ok((file, _)) => file,
            Err(err) => {
                error!("Failed to get file attributes: {err}");
                reply.error(fuser::Errno::ENOENT);
                return;
            }
        };
        let entries = match self.remote.read().await.list_dir(file.path()).await {
            Ok(entries) => entries,
            Err(err) => {
                error!("Failed to list directory: {err}");
                reply.error(fuser::Errno::EIO);
                return;
            }
        };
        for (index, entry) in entries.into_iter().skip(offset as usize).enumerate() {
            let inode = self.state().database.inode_for(entry.path());
            let name = match entry.path().file_name() {
                Some(name) => OsStr::from_bytes(name.as_bytes()),
                None => {
                    error!("Failed to get file name {:?}", entry.path().display());
                    continue;
                }
            };
            if reply.add(
                INodeNo(inode),
                offset + index as u64 + 1,
                convert_remote_filetype(entry.metadata().file_type),
                name,
            ) {
                break;
            }
        }
        reply.ok();
    }

    async fn statfs(&self, _request: RequestMeta, ino: INodeNo, reply: ReplyStatfs) {
        struct FsStats {
            files: u64,
            size: u64,
        }

        fn iter_dir<'a, T: AsyncRemoteFs>(
            remote: &'a T,
            path: &'a Path,
            stats: &'a mut FsStats,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = RemoteResult<()>> + Send + 'a>>
        {
            Box::pin(async move {
                for entry in remote.list_dir(path).await? {
                    stats.files += 1;
                    stats.size += entry.metadata().size.unwrap_or(0);
                    if entry.metadata().file_type == remotefs::fs::FileType::Directory {
                        iter_dir(remote, entry.path(), stats).await?;
                    }
                }
                Ok(())
            })
        }

        let path = self
            .get_inode(ino.0)
            .await
            .map(|(file, _)| file.path().to_path_buf())
            .unwrap_or_else(|_| PathBuf::from("/"));
        let mut stats = FsStats { files: 0, size: 0 };
        if let Err(err) = iter_dir(&*self.remote.read().await, &path, &mut stats).await {
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

    async fn access(
        &self,
        request: RequestMeta,
        ino: INodeNo,
        mask: FuserAccessFlags,
        reply: ReplyEmpty,
    ) {
        let file = match self.get_inode(ino.0).await {
            Ok((file, _)) => file,
            Err(err) => {
                error!("Failed to get file attributes: {err}");
                reply.error(fuser::Errno::ENOENT);
                return;
            }
        };
        if self.state().check_access(
            &file,
            request.uid,
            request.gid,
            AccessFlags::from_bits_truncate(mask.bits()),
        ) {
            reply.ok();
        } else {
            error!("No access to file: {}", file.path().display());
            reply.error(fuser::Errno::EACCES);
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn create(
        &self,
        request: RequestMeta,
        parent: INodeNo,
        name: OsString,
        mode: u32,
        _umask: u32,
        flags: i32,
        reply: ReplyCreate,
    ) {
        let (read, write) = match parse_create_flags(flags) {
            Ok(mode) => mode,
            Err(errno) => {
                error!("Invalid access mode flag: {flags:?}");
                reply.error(errno);
                return;
            }
        };
        let path = match self.state().lookup_name(parent.0, &name) {
            Some(path) => path,
            None => {
                error!("Failed to lookup name {name:?}");
                reply.error(fuser::Errno::ENOENT);
                return;
            }
        };
        if let Err(err) = transfer::create_empty_file(
            &*self.remote.read().await,
            &path,
            Some(UnixPex::from(mode)),
        )
        .await
        {
            error!("Failed to create file: {err}");
            reply.error(fuser::Errno::EIO);
            return;
        }
        let inode = self.state().database.inode_for(&path);
        match self.get_inode(inode).await {
            Ok((_, attrs)) => {
                let fh = self.open_handle(request.pid, inode, read, write);
                reply.created(
                    &Duration::new(0, 0),
                    &attrs,
                    Generation(0),
                    FuserFileHandle(fh),
                    FopenFlags::empty(),
                );
            }
            Err(err) => {
                debug!("Failed to get file attributes: {err}");
                reply.error(fuser::Errno::ENOENT);
            }
        }
    }
}
