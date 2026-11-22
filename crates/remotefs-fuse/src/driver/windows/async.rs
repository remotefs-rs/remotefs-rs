//! Native async Dokany driver over an [`AsyncRemoteFs`].
//!
//! Dokany callbacks return synchronously, so each callback runs its async body
//! on the runtime handle supplied by [`AsyncDriver::new`]. Remote operations
//! inside those bodies are awaited natively.

use std::path::Path;
use std::sync::{Arc, RwLock};
use std::time::UNIX_EPOCH;

use dashmap::DashMap;
use dashmap::mapref::one::Ref;
use dokan::{
    CreateFileInfo, DiskSpaceInfo, FileInfo, FileSystemHandler, FileTimeOperation, FillDataResult,
    FindData, FindStreamData, OperationInfo, OperationResult, VolumeInfo,
};
use dokan_sys::win32::FILE_DELETE_ON_CLOSE;
use remotefs::fs::{SetMetadata, UnixPex};
use remotefs::{AsyncRemoteFs, File, RemoteResult};
use tokio::runtime::Handle;
use widestring::{U16CStr, U16CString, U16Str, U16String};
use winapi::shared::ntstatus::{
    self, STATUS_ACCESS_DENIED, STATUS_CANNOT_DELETE, STATUS_DELETE_PENDING,
    STATUS_DIRECTORY_NOT_EMPTY, STATUS_INVALID_DEVICE_REQUEST, STATUS_INVALID_PARAMETER,
    STATUS_NOT_A_DIRECTORY, STATUS_NOT_IMPLEMENTED, STATUS_OBJECT_NAME_COLLISION,
};
use winapi::um::winnt::{ACCESS_MASK, FILE_CASE_PRESERVED_NAMES, FILE_CASE_SENSITIVE_SEARCH};

use super::AltStream;
use super::common::{self, CreatePlan};
use super::entry::{Stat, StatHandle};
use crate::MountOption;
use crate::driver::transfer::r#async::{self as transfer, AsyncPendingWriteState};

/// Per-open-handle context of the async driver.
pub(crate) type AsyncStatHandle = StatHandle<tokio::sync::Mutex<Option<AsyncPendingWriteState>>>;

/// Remote filesystem driver over an [`AsyncRemoteFs`] for Dokany.
pub(crate) struct AsyncDriver<T>
where
    T: AsyncRemoteFs + 'static,
{
    options: Vec<MountOption>,
    remote: Arc<tokio::sync::RwLock<T>>,
    file_handlers: DashMap<U16CString, Arc<RwLock<Stat>>>,
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
    pub(crate) fn new(
        remote: Arc<tokio::sync::RwLock<T>>,
        options: Vec<MountOption>,
        handle: Handle,
    ) -> Self {
        Self {
            options,
            remote,
            file_handlers: DashMap::new(),
            handle,
        }
    }

    pub(crate) fn options(&self) -> &[MountOption] {
        &self.options
    }

    /// Run one async callback body on the calling Dokany thread.
    fn block_on<F: std::future::Future>(&self, future: F) -> F::Output {
        self.handle.block_on(future)
    }

    async fn stat(
        &self,
        file_name: &U16CStr,
    ) -> RemoteResult<Ref<'_, U16CString, Arc<RwLock<Stat>>>> {
        let key = file_name.to_ucstring();
        if let Some(stat) = self.file_handlers.get(&key) {
            return Ok(stat);
        }

        let path_info = common::path_info(file_name);
        let file = self.remote.read().await.stat(&path_info.path).await?;
        self.file_handlers
            .insert(key.clone(), common::new_stat(file)?);
        Ok(self
            .file_handlers
            .get(&key)
            .expect("stat was just inserted"))
    }

    async fn read(&self, path: &Path, buffer: &mut [u8], offset: u64) -> RemoteResult<usize> {
        transfer::read_at(&*self.remote.read().await, path, buffer, offset).await
    }

    pub(super) async fn write_to_handle(
        &self,
        context: &AsyncStatHandle,
        file: &File,
        data: &[u8],
        offset: u64,
    ) -> RemoteResult<u32> {
        let mut pending = context.pending_write.lock().await;
        if pending.is_none() {
            *pending = Some(transfer::start_pending_write(&*self.remote.read().await, file).await?);
        }
        transfer::write_to_pending(
            pending.as_mut().expect("pending write was just inserted"),
            data,
            offset,
        )
        .await
    }

    async fn create_empty_file(&self, path: &Path) -> RemoteResult<()> {
        transfer::create_empty_file(&*self.remote.read().await, path, Some(UnixPex::from(0o644)))
            .await
    }

    async fn finalize_pending_write(
        &self,
        context: &AsyncStatHandle,
        file: &File,
    ) -> RemoteResult<()> {
        let pending = context.pending_write.lock().await.take();
        let Some(pending) = pending else {
            return Ok(());
        };
        transfer::finalize_pending_write(&*self.remote.read().await, file, pending).await
    }

    pub(super) async fn cleanup_pending_write(
        &self,
        context: &AsyncStatHandle,
        file: &File,
        will_delete: bool,
    ) {
        if let Err(err) = self.finalize_pending_write(context, file).await {
            let action = if will_delete {
                "before delete"
            } else {
                "on cleanup"
            };
            error!("failed to finalize write {action}: {err}");
        }
    }

    async fn append(&self, file: &File, data: &[u8]) -> RemoteResult<u32> {
        transfer::append_data(&*self.remote.read().await, file, data).await
    }

    async fn find_files(
        &self,
        ctx: &File,
        pattern: Option<&U16CStr>,
        fill: impl FnMut(&FindData) -> FillDataResult,
    ) -> OperationResult<()> {
        if ctx.is_file() {
            return Err(STATUS_NOT_A_DIRECTORY);
        }
        let entries = self
            .remote
            .read()
            .await
            .list_dir(ctx.path())
            .await
            .map_err(|err| {
                error!("list_dir failed: {err}");
                STATUS_INVALID_DEVICE_REQUEST
            })?;
        common::fill_entries(entries, pattern, fill)
    }
}

impl<'c, 'h: 'c, T> FileSystemHandler<'c, 'h> for AsyncDriver<T>
where
    T: AsyncRemoteFs + 'static + 'h,
{
    type Context = AsyncStatHandle;

    fn mounted(
        &'h self,
        _mount_point: &U16CStr,
        _info: &OperationInfo<'c, 'h, Self>,
    ) -> OperationResult<()> {
        info!("mounted()");
        Ok(())
    }

    fn unmounted(&'h self, _info: &OperationInfo<'c, 'h, Self>) -> OperationResult<()> {
        info!("unmounted()");
        Ok(())
    }

    fn create_file(
        &'h self,
        file_name: &U16CStr,
        _security_context: &dokan_sys::DOKAN_IO_SECURITY_CONTEXT,
        desired_access: ACCESS_MASK,
        _file_attributes: u32,
        _share_access: u32,
        create_disposition: u32,
        create_options: u32,
        _info: &mut OperationInfo<'c, 'h, Self>,
    ) -> OperationResult<CreateFileInfo<Self::Context>> {
        self.block_on(async {
            let path_info = common::path_info(file_name);
            let delete_on_close = create_options & FILE_DELETE_ON_CLOSE > 0;
            let existing = self
                .stat(file_name)
                .await
                .ok()
                .map(|stat| stat.value().clone());
            match common::plan_create(
                existing.as_ref(),
                file_name,
                desired_access,
                create_disposition,
                create_options,
            )? {
                CreatePlan::Open {
                    stat,
                    alt_stream,
                    is_dir,
                    new_file_created,
                } => Ok(CreateFileInfo {
                    context: AsyncStatHandle {
                        stat,
                        alt_stream: RwLock::new(alt_stream),
                        delete_on_close,
                        pending_write: tokio::sync::Mutex::new(None),
                    },
                    is_dir,
                    new_file_created,
                }),
                CreatePlan::CreateFile => {
                    self.create_empty_file(&path_info.path)
                        .await
                        .map_err(|err| {
                            error!("failed to create empty file: {err}");
                            ntstatus::STATUS_CONNECTION_DISCONNECTED
                        })?;
                    let stat = self.stat(file_name).await.map_err(|err| {
                        error!("stat failed: {err}");
                        ntstatus::STATUS_CONNECTION_DISCONNECTED
                    })?;
                    Ok(CreateFileInfo {
                        context: AsyncStatHandle {
                            stat: stat.value().clone(),
                            alt_stream: RwLock::new(None),
                            delete_on_close,
                            pending_write: tokio::sync::Mutex::new(None),
                        },
                        is_dir: false,
                        new_file_created: true,
                    })
                }
                CreatePlan::CreateDirectory => {
                    self.remote
                        .read()
                        .await
                        .create_dir(&path_info.path, Some(UnixPex::from(0o755)))
                        .await
                        .map_err(|err| {
                            error!("create_dir failed: {err}");
                            ntstatus::STATUS_CONNECTION_DISCONNECTED
                        })?;
                    let stat = self.stat(file_name).await.map_err(|err| {
                        error!("stat failed: {err}");
                        ntstatus::STATUS_CONNECTION_DISCONNECTED
                    })?;
                    Ok(CreateFileInfo {
                        context: AsyncStatHandle {
                            stat: stat.value().clone(),
                            alt_stream: RwLock::new(None),
                            delete_on_close,
                            pending_write: tokio::sync::Mutex::new(None),
                        },
                        is_dir: true,
                        new_file_created: true,
                    })
                }
            }
        })
    }

    fn cleanup(
        &'h self,
        file_name: &U16CStr,
        info: &OperationInfo<'c, 'h, Self>,
        context: &'c Self::Context,
    ) {
        self.block_on(async {
            debug!("cleanup({file_name:?}, {context:?})");
            let (file, stat_delete_on_close, stat_delete_pending) = {
                let stat = match context.stat.read() {
                    Ok(stat) => stat,
                    Err(_) => {
                        error!("mutex poisoned");
                        return;
                    }
                };
                (stat.file.clone(), stat.delete_on_close, stat.delete_pending)
            };

            let alt_stream_delete =
                common::try_alt_stream(context, |alt_stream| Ok(alt_stream.delete_pending))
                    .transpose()
                    .unwrap_or_default()
                    .unwrap_or_default();
            if alt_stream_delete {
                if let Ok(mut alt_stream) = context.alt_stream.write() {
                    alt_stream.take();
                } else {
                    error!("mutex poisoned");
                }
                return;
            }

            let will_delete = context.delete_on_close
                || stat_delete_on_close
                || stat_delete_pending
                || info.delete_on_close();
            self.cleanup_pending_write(context, &file, will_delete)
                .await;

            if will_delete {
                let result = if file.is_dir() {
                    self.remote.read().await.remove_dir(file.path()).await
                } else {
                    self.remote.read().await.remove_file(file.path()).await
                };
                if let Err(err) = result {
                    error!("delete failed: {err}");
                }
            }
        });
    }

    fn close_file(
        &'h self,
        file_name: &U16CStr,
        _info: &OperationInfo<'c, 'h, Self>,
        _context: &'c Self::Context,
    ) {
        debug!("close_file({file_name:?})");
        self.file_handlers.remove(&file_name.to_ucstring());
    }

    fn read_file(
        &'h self,
        file_name: &U16CStr,
        offset: i64,
        buffer: &mut [u8],
        _info: &OperationInfo<'c, 'h, Self>,
        context: &'c Self::Context,
    ) -> OperationResult<u32> {
        self.block_on(async {
            debug!("read_file({file_name:?}, {offset})");
            let file = match context.stat.read() {
                Ok(stat) => stat.file.clone(),
                Err(_) => {
                    error!("mutex poisoned");
                    return Err(STATUS_INVALID_DEVICE_REQUEST);
                }
            };
            if let Some(result) = common::try_alt_stream(context, |alt_stream| {
                let offset = usize::try_from(common::nonnegative_offset(offset)?)
                    .map_err(|_| STATUS_INVALID_PARAMETER)?;
                let Some(available) = alt_stream.data.len().checked_sub(offset) else {
                    return Ok(0);
                };
                let len = std::cmp::min(buffer.len(), available);
                buffer[..len].copy_from_slice(&alt_stream.data[offset..offset + len]);
                Ok(len as u32)
            }) {
                return result;
            }
            self.read(file.path(), buffer, common::nonnegative_offset(offset)?)
                .await
                .map(|len| len as u32)
                .map_err(|err| {
                    error!("read failed: {err}");
                    STATUS_INVALID_DEVICE_REQUEST
                })
        })
    }

    fn write_file(
        &'h self,
        file_name: &U16CStr,
        offset: i64,
        buffer: &[u8],
        info: &OperationInfo<'c, 'h, Self>,
        context: &'c Self::Context,
    ) -> OperationResult<u32> {
        self.block_on(async {
            debug!("write_file({file_name:?}, {offset})");
            let file = match context.stat.read() {
                Ok(stat) => stat.file.clone(),
                Err(_) => {
                    error!("mutex poisoned");
                    return Err(STATUS_INVALID_DEVICE_REQUEST);
                }
            };
            if let Some(result) = common::try_alt_stream(context, |alt_stream| {
                let offset = if info.write_to_eof() {
                    alt_stream.data.len()
                } else {
                    usize::try_from(common::nonnegative_offset(offset)?)
                        .map_err(|_| STATUS_INVALID_PARAMETER)?
                };
                if offset + buffer.len() > alt_stream.data.len() {
                    alt_stream.data.resize(offset + buffer.len(), 0);
                }
                alt_stream.data[offset..offset + buffer.len()].copy_from_slice(buffer);
                Ok(buffer.len() as u32)
            }) {
                return result;
            }
            let result = if info.write_to_eof() {
                self.append(&file, buffer).await
            } else {
                self.write_to_handle(context, &file, buffer, common::nonnegative_offset(offset)?)
                    .await
            };
            result.map_err(|err| {
                error!("write failed: {err}");
                STATUS_INVALID_DEVICE_REQUEST
            })
        })
    }

    fn flush_file_buffers(
        &'h self,
        file_name: &U16CStr,
        _info: &OperationInfo<'c, 'h, Self>,
        context: &'c Self::Context,
    ) -> OperationResult<()> {
        self.block_on(async {
            debug!("flush_file_buffers({file_name:?}, {context:?})");
            let file = match context.stat.read() {
                Ok(stat) => stat.file.clone(),
                Err(_) => {
                    error!("mutex poisoned");
                    return Err(STATUS_INVALID_DEVICE_REQUEST);
                }
            };
            self.finalize_pending_write(context, &file)
                .await
                .map_err(|err| {
                    error!("failed to flush write: {err}");
                    STATUS_INVALID_DEVICE_REQUEST
                })
        })
    }

    fn get_file_information(
        &'h self,
        file_name: &U16CStr,
        _info: &OperationInfo<'c, 'h, Self>,
        context: &'c Self::Context,
    ) -> OperationResult<FileInfo> {
        debug!("get_file_information({file_name:?}, {context:?})");
        let file = match context.stat.read() {
            Ok(stat) => stat.file.clone(),
            Err(_) => {
                error!("mutex poisoned");
                return Err(STATUS_INVALID_DEVICE_REQUEST);
            }
        };
        Ok(FileInfo {
            attributes: common::attributes_from_file(&file),
            creation_time: file.metadata().created.unwrap_or(UNIX_EPOCH),
            last_access_time: file.metadata().accessed.unwrap_or(UNIX_EPOCH),
            last_write_time: file.metadata().modified.unwrap_or(UNIX_EPOCH),
            file_size: file.metadata().size.unwrap_or(0),
            number_of_links: 1,
            file_index: common::file_index(&file),
        })
    }

    fn find_files(
        &'h self,
        file_name: &U16CStr,
        fill_find_data: impl FnMut(&FindData) -> FillDataResult,
        _info: &OperationInfo<'c, 'h, Self>,
        context: &'c Self::Context,
    ) -> OperationResult<()> {
        self.block_on(async {
            debug!("find_files({file_name:?}, {context:?})");
            let alt_stream = match context.alt_stream.read() {
                Ok(stream) => stream.clone(),
                Err(_) => {
                    error!("mutex poisoned");
                    return Err(STATUS_INVALID_DEVICE_REQUEST);
                }
            };
            if alt_stream.is_some() {
                return Err(STATUS_INVALID_DEVICE_REQUEST);
            }
            let file = match context.stat.read() {
                Ok(stat) => stat.file.clone(),
                Err(_) => {
                    error!("mutex poisoned");
                    return Err(STATUS_INVALID_DEVICE_REQUEST);
                }
            };
            self.find_files(&file, None, fill_find_data).await
        })
    }

    fn find_files_with_pattern(
        &'h self,
        file_name: &U16CStr,
        pattern: &U16CStr,
        fill_find_data: impl FnMut(&FindData) -> FillDataResult,
        _info: &OperationInfo<'c, 'h, Self>,
        context: &'c Self::Context,
    ) -> OperationResult<()> {
        self.block_on(async {
            debug!("find_files_with_pattern({file_name:?}, {pattern:?}, {context:?})");
            let file = match context.stat.read() {
                Ok(stat) => stat.file.clone(),
                Err(_) => {
                    error!("mutex poisoned");
                    return Err(STATUS_INVALID_DEVICE_REQUEST);
                }
            };
            self.find_files(&file, Some(pattern), fill_find_data).await
        })
    }

    fn set_file_attributes(
        &'h self,
        file_name: &U16CStr,
        file_attributes: u32,
        _info: &OperationInfo<'c, 'h, Self>,
        _context: &'c Self::Context,
    ) -> OperationResult<()> {
        debug!("set_file_attributes({file_name:?}, {file_attributes:?})");
        Ok(())
    }

    fn set_file_time(
        &'h self,
        file_name: &U16CStr,
        creation_time: FileTimeOperation,
        last_access_time: FileTimeOperation,
        last_write_time: FileTimeOperation,
        _info: &OperationInfo<'c, 'h, Self>,
        context: &'c Self::Context,
    ) -> OperationResult<()> {
        self.block_on(async {
            debug!(
                "set_file_time({file_name:?}, {creation_time:?}, {last_access_time:?}, {last_write_time:?})"
            );
            let file = match context.stat.read() {
                Ok(stat) => stat.file.clone(),
                Err(_) => {
                    error!("mutex poisoned");
                    return Err(STATUS_INVALID_DEVICE_REQUEST);
                }
            };
            let mut changes = SetMetadata::default();
            let mut any_change = false;
            if let FileTimeOperation::SetTime(time) = last_access_time {
                changes = changes.accessed(time);
                any_change = true;
            }
            if let FileTimeOperation::SetTime(time) = last_write_time {
                changes = changes.modified(time);
                any_change = true;
            }
            if let FileTimeOperation::SetTime(_) = creation_time {
                debug!("creation time is not supported by remotefs; ignoring");
            }
            if any_change {
                self.remote
                    .read()
                    .await
                    .set_metadata(file.path(), &changes)
                    .await
                    .map_err(|err| {
                        error!("set_metadata failed: {err}");
                        STATUS_INVALID_DEVICE_REQUEST
                    })?;
            }
            Ok(())
        })
    }

    fn delete_file(
        &'h self,
        file_name: &U16CStr,
        info: &OperationInfo<'c, 'h, Self>,
        context: &'c Self::Context,
    ) -> OperationResult<()> {
        debug!("delete_file({file_name:?}, {context:?})");
        let is_dir = match context.stat.read() {
            Ok(stat) => stat.file.is_dir(),
            Err(_) => {
                error!("mutex poisoned");
                return Err(STATUS_INVALID_DEVICE_REQUEST);
            }
        };
        if is_dir {
            return Err(STATUS_CANNOT_DELETE);
        }
        if let Some(result) = common::try_alt_stream(context, |alt_stream| {
            if alt_stream.delete_pending {
                return Err(STATUS_DELETE_PENDING);
            }
            Ok(())
        }) {
            return result;
        }
        match context.stat.write() {
            Ok(mut stat) => stat.delete_pending = info.delete_on_close(),
            Err(_) => {
                error!("mutex poisoned");
                return Err(STATUS_INVALID_DEVICE_REQUEST);
            }
        }
        Ok(())
    }

    fn delete_directory(
        &'h self,
        file_name: &U16CStr,
        info: &OperationInfo<'c, 'h, Self>,
        context: &'c Self::Context,
    ) -> OperationResult<()> {
        self.block_on(async {
            debug!("delete_directory({file_name:?}, {context:?})");
            if common::try_alt_stream(context, |_alt_stream| Ok(())).is_some() {
                return Err(STATUS_INVALID_DEVICE_REQUEST);
            }
            let file = match context.stat.read() {
                Ok(stat) => stat.file.clone(),
                Err(_) => {
                    error!("mutex poisoned");
                    return Err(STATUS_INVALID_DEVICE_REQUEST);
                }
            };
            if !file.is_dir() {
                return Err(STATUS_NOT_A_DIRECTORY);
            }
            let is_empty = self
                .remote
                .read()
                .await
                .list_dir(file.path())
                .await
                .map(|entries| entries.is_empty())
                .map_err(|err| {
                    error!("list_dir failed: {err}");
                    STATUS_INVALID_DEVICE_REQUEST
                })?;
            if !is_empty && info.delete_on_close() {
                return Err(STATUS_DIRECTORY_NOT_EMPTY);
            }
            if let Some(result) = common::try_alt_stream(context, |alt_stream| {
                alt_stream.delete_pending = info.delete_on_close();
                Ok(())
            }) {
                return result;
            }
            match context.stat.write() {
                Ok(mut stat) => stat.delete_pending = info.delete_on_close(),
                Err(_) => {
                    error!("mutex poisoned");
                    return Err(STATUS_INVALID_DEVICE_REQUEST);
                }
            }
            Ok(())
        })
    }

    fn move_file(
        &'h self,
        file_name: &U16CStr,
        new_file_name: &U16CStr,
        replace_if_existing: bool,
        _info: &OperationInfo<'c, 'h, Self>,
        context: &'c Self::Context,
    ) -> OperationResult<()> {
        self.block_on(async {
            debug!("move_file({file_name:?}, {new_file_name:?}, {replace_if_existing:?})");
            let dest = common::path_info(new_file_name);
            if !replace_if_existing
                && self
                    .remote
                    .read()
                    .await
                    .exists(&dest.path)
                    .await
                    .unwrap_or(true)
            {
                return Err(STATUS_OBJECT_NAME_COLLISION);
            }
            let file = match context.stat.read() {
                Ok(stat) => stat.file.clone(),
                Err(_) => {
                    error!("mutex poisoned");
                    return Err(STATUS_INVALID_DEVICE_REQUEST);
                }
            };
            self.remote
                .read()
                .await
                .rename(file.path(), &dest.path)
                .await
                .map_err(|err| {
                    error!("move failed: {err}");
                    STATUS_ACCESS_DENIED
                })
        })
    }

    fn set_end_of_file(
        &'h self,
        file_name: &U16CStr,
        offset: i64,
        _info: &OperationInfo<'c, 'h, Self>,
        context: &'c Self::Context,
    ) -> OperationResult<()> {
        self.block_on(async {
            debug!("set_end_of_file({file_name:?}, {offset})");
            if let Some(result) = common::try_alt_stream(context, |alt_stream| {
                let offset = usize::try_from(common::nonnegative_offset(offset)?)
                    .map_err(|_| STATUS_INVALID_PARAMETER)?;
                alt_stream.data.truncate(offset);
                Ok(())
            }) {
                return result;
            }
            let file = match context.stat.read() {
                Ok(stat) => stat.file.clone(),
                Err(_) => {
                    error!("mutex poisoned");
                    return Err(STATUS_INVALID_DEVICE_REQUEST);
                }
            };
            context.pending_write.lock().await.take();
            transfer::truncate_file(
                &*self.remote.read().await,
                &file,
                common::nonnegative_offset(offset)?,
            )
            .await
            .map_err(|err| {
                error!("truncate failed: {err}");
                STATUS_INVALID_DEVICE_REQUEST
            })
        })
    }

    fn set_allocation_size(
        &'h self,
        file_name: &U16CStr,
        alloc_size: i64,
        _info: &OperationInfo<'c, 'h, Self>,
        context: &'c Self::Context,
    ) -> OperationResult<()> {
        debug!("set_allocation_size({file_name:?}, {alloc_size})");
        common::try_alt_stream(context, |alt_stream: &mut AltStream| {
            let alloc_size = usize::try_from(common::nonnegative_offset(alloc_size)?)
                .map_err(|_| STATUS_INVALID_PARAMETER)?;
            alt_stream.data = vec![0; alloc_size];
            Ok(())
        })
        .unwrap_or(Ok(()))
    }

    fn get_file_security(
        &'h self,
        file_name: &U16CStr,
        security_information: u32,
        security_descriptor: winapi::um::winnt::PSECURITY_DESCRIPTOR,
        buffer_length: u32,
        _info: &OperationInfo<'c, 'h, Self>,
        context: &'c Self::Context,
    ) -> OperationResult<u32> {
        debug!("get_file_security({file_name:?}, {security_information:?}, {buffer_length})");
        let stat = match context.stat.read() {
            Ok(stat) => stat,
            Err(_) => {
                error!("mutex poisoned");
                return Err(STATUS_INVALID_DEVICE_REQUEST);
            }
        };
        stat.sec_desc
            .get_security_info(security_information, security_descriptor, buffer_length)
    }

    fn set_file_security(
        &'h self,
        file_name: &U16CStr,
        security_information: u32,
        security_descriptor: winapi::um::winnt::PSECURITY_DESCRIPTOR,
        _buffer_length: u32,
        _info: &OperationInfo<'c, 'h, Self>,
        context: &'c Self::Context,
    ) -> OperationResult<()> {
        debug!("set_file_security({file_name:?}, {security_information:?})");
        let mut stat = match context.stat.write() {
            Ok(stat) => stat,
            Err(_) => {
                error!("mutex poisoned");
                return Err(STATUS_INVALID_DEVICE_REQUEST);
            }
        };
        stat.sec_desc
            .set_security_info(security_information, security_descriptor)
    }

    fn find_streams(
        &'h self,
        file_name: &U16CStr,
        mut fill_find_stream_data: impl FnMut(&FindStreamData) -> FillDataResult,
        _info: &OperationInfo<'c, 'h, Self>,
        context: &'c Self::Context,
    ) -> OperationResult<()> {
        debug!("find_streams({file_name:?})");
        let file = match context.stat.read() {
            Ok(stat) => stat.file.clone(),
            Err(_) => {
                error!("mutex poisoned");
                return Err(STATUS_INVALID_DEVICE_REQUEST);
            }
        };
        fill_find_stream_data(&FindStreamData {
            size: file.metadata().size.unwrap_or(0) as i64,
            name: U16CString::from_str("::$DATA").expect("valid stream name"),
        })
        .or_else(common::ignore_name_too_long)?;
        let alt_streams = match context.stat.read() {
            Ok(stat) => stat.alt_streams.clone(),
            Err(_) => {
                error!("mutex poisoned");
                return Err(STATUS_INVALID_DEVICE_REQUEST);
            }
        };
        for (name, stream) in &alt_streams {
            let mut name_buf = vec![':' as u16];
            name_buf.extend_from_slice(name.0.as_slice());
            name_buf.extend_from_slice(U16String::from_str(":$DATA").as_slice());
            fill_find_stream_data(&FindStreamData {
                size: stream
                    .read()
                    .map(|data| data.data.len() as i64)
                    .unwrap_or_default(),
                name: U16CString::from_ustr(U16Str::from_slice(&name_buf))
                    .expect("valid stream name"),
            })
            .or_else(common::ignore_name_too_long)?;
        }
        Ok(())
    }

    fn get_volume_information(
        &'h self,
        _info: &OperationInfo<'c, 'h, Self>,
    ) -> OperationResult<VolumeInfo> {
        Ok(VolumeInfo {
            name: U16CString::from_str("remotefs-fuse").expect("valid volume name"),
            serial_number: 0,
            max_component_length: 255,
            fs_flags: FILE_CASE_SENSITIVE_SEARCH | FILE_CASE_PRESERVED_NAMES,
            fs_name: U16CString::from_str("DOKANY").expect("valid filesystem name"),
        })
    }

    fn lock_file(
        &'h self,
        _file_name: &U16CStr,
        _offset: i64,
        _length: i64,
        _info: &OperationInfo<'c, 'h, Self>,
        _context: &'c Self::Context,
    ) -> OperationResult<()> {
        Err(STATUS_NOT_IMPLEMENTED)
    }

    fn unlock_file(
        &'h self,
        _file_name: &U16CStr,
        _offset: i64,
        _length: i64,
        _info: &OperationInfo<'c, 'h, Self>,
        _context: &'c Self::Context,
    ) -> OperationResult<()> {
        Err(STATUS_NOT_IMPLEMENTED)
    }

    fn get_disk_free_space(
        &'h self,
        _info: &OperationInfo<'c, 'h, Self>,
    ) -> OperationResult<DiskSpaceInfo> {
        const DEFAULT_SIZE: u64 = 1024 * 1024 * 1024 * 128;
        Ok(DiskSpaceInfo {
            free_byte_count: DEFAULT_SIZE,
            byte_count: DEFAULT_SIZE,
            available_byte_count: DEFAULT_SIZE,
        })
    }
}
