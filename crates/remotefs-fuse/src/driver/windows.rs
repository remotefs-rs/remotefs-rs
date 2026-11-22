#[cfg(feature = "tokio")]
pub(crate) mod r#async;
mod common;
mod entry;
mod security;
#[cfg(test)]
mod test;

use std::path::Path;
use std::sync::{Arc, Mutex, RwLock};
use std::time::UNIX_EPOCH;

use dashmap::mapref::one::Ref;
use dokan::{
    CreateFileInfo, DiskSpaceInfo, FileInfo, FileSystemHandler, FileTimeOperation, FillDataResult,
    FindData, FindStreamData, OperationInfo, OperationResult, VolumeInfo,
};
use dokan_sys::win32::FILE_DELETE_ON_CLOSE;
use entry::SyncStatHandle;
use remotefs::fs::{SetMetadata, UnixPex};
use remotefs::{File, RemoteError, RemoteErrorType, RemoteFs, RemoteResult};
use widestring::{U16CStr, U16CString, U16Str, U16String};
use winapi::shared::ntstatus::{
    self, STATUS_ACCESS_DENIED, STATUS_CANNOT_DELETE, STATUS_DELETE_PENDING,
    STATUS_DIRECTORY_NOT_EMPTY, STATUS_INVALID_DEVICE_REQUEST, STATUS_INVALID_PARAMETER,
    STATUS_NOT_A_DIRECTORY, STATUS_NOT_IMPLEMENTED, STATUS_OBJECT_NAME_COLLISION,
};
use winapi::um::winnt::{ACCESS_MASK, FILE_CASE_PRESERVED_NAMES, FILE_CASE_SENSITIVE_SEARCH};

use self::common::CreatePlan;
pub use self::entry::Stat;
pub(crate) use super::transfer::PendingWriteState;
use super::{Driver, transfer};

const ROOT_ID: u64 = 1;

#[derive(Debug)]
pub struct AltStream {
    delete_pending: bool,
    data: Vec<u8>,
}

impl AltStream {
    fn new() -> Self {
        Self {
            delete_pending: false,
            data: Vec::new(),
        }
    }
}

impl<T> Driver<T>
where
    T: RemoteFs + Sync + Send,
{
    /// Get the Stat object for a given `file_name`.
    fn stat(&self, file_name: &U16CStr) -> RemoteResult<Ref<'_, U16CString, Arc<RwLock<Stat>>>> {
        let key = file_name.to_ucstring();
        if let Some(stat) = self.file_handlers.get(&key) {
            return Ok(stat);
        }

        let path_info = common::path_info(file_name);

        let file = self.remote(|remote| remote.stat(&path_info.path))?;

        // insert the file into the file handlers
        self.file_handlers
            .insert(key.clone(), common::new_stat(file)?);

        Ok(self.file_handlers.get(&key).unwrap())
    }

    /// Read up to `buffer.len()` bytes of `path` at `offset`; see [`transfer::read_at`].
    fn read(&self, path: &Path, buffer: &mut [u8], offset: u64) -> RemoteResult<usize> {
        debug!("Read file: {:?} {} bytes at {offset}", path, buffer.len());
        self.remote(|remote| transfer::read_at(remote, path, buffer, offset))
    }

    /// Write `data` at `offset` to the pending write staged on `context`,
    /// starting a new one (against `file`) if this is the first write to this
    /// handle. The remote write stays staged until
    /// [`Self::finalize_pending_write`] runs.
    fn write_to_handle(
        &self,
        context: &SyncStatHandle,
        file: &File,
        data: &[u8],
        offset: u64,
    ) -> RemoteResult<u32> {
        let mut pending = context
            .pending_write
            .lock()
            .map_err(|_| RemoteError::with_message(RemoteErrorType::IoError, "mutex poisoned"))?;

        if pending.is_none() {
            *pending = Some(self.remote(|remote| transfer::start_pending_write(remote, file))?);
        }

        transfer::write_to_pending(
            pending.as_mut().expect("pending write was just inserted"),
            data,
            offset,
        )
    }

    /// Create a new, empty remote file in a single call, without staging a pending write.
    fn create_empty_file(&self, path: &Path) -> RemoteResult<()> {
        self.remote(|remote| transfer::create_empty_file(remote, path, Some(UnixPex::from(0o644))))
    }

    /// Finalize the write staged on `context`, if any, actually persisting it to the remote
    /// filesystem.
    fn finalize_pending_write(&self, context: &SyncStatHandle, file: &File) -> RemoteResult<()> {
        let pending = {
            let mut guard = context.pending_write.lock().map_err(|_| {
                RemoteError::with_message(RemoteErrorType::IoError, "mutex poisoned")
            })?;
            guard.take()
        };

        let Some(pending) = pending else {
            return Ok(());
        };

        self.remote(|remote| transfer::finalize_pending_write(remote, file, pending))
    }

    /// Append data to a file.
    fn append(&self, file: &File, data: &[u8]) -> RemoteResult<u32> {
        debug!("Append to file: {:?} {} bytes", file.path(), data.len());
        self.remote(|remote| transfer::append_data(remote, file, data))
    }

    /// Find files at path with the optional pattern.
    fn find_files<F>(&self, ctx: &File, pattern: Option<&U16CStr>, fill: F) -> OperationResult<()>
    where
        F: FnMut(&FindData) -> FillDataResult,
    {
        debug!("find_files({ctx:?}, {pattern:?})");
        if ctx.is_file() {
            return Err(STATUS_NOT_A_DIRECTORY);
        }

        // list directory
        let entries = match self.remote(|remote| remote.list_dir(ctx.path())) {
            Ok(entries) => entries,
            Err(err) => {
                error!("list_dir failed: {err}");
                return Err(STATUS_INVALID_DEVICE_REQUEST);
            }
        };

        common::fill_entries(entries, pattern, fill)
    }

    /// Execute a function on the remote filesystem.
    fn remote<F, U>(&self, f: F) -> RemoteResult<U>
    where
        F: FnOnce(&T) -> RemoteResult<U>,
    {
        let remote = self
            .remote
            .read()
            .map_err(|_| RemoteError::with_message(RemoteErrorType::IoError, "mutex poisoned"))?;
        f(&remote)
    }

    /// Execute a lifecycle function (`connect` / `disconnect`) under the exclusive write lock.
    fn remote_mut<F, U>(&self, f: F) -> RemoteResult<U>
    where
        F: FnOnce(&mut T) -> RemoteResult<U>,
    {
        let mut remote = self
            .remote
            .write()
            .map_err(|_| RemoteError::with_message(RemoteErrorType::IoError, "mutex poisoned"))?;
        f(&mut remote)
    }
}

// For reference <https://github.com/dokan-dev/dokan-rust/blob/master/dokan/examples/memfs/main.rs>
impl<'c, 'h: 'c, T> FileSystemHandler<'c, 'h> for Driver<T>
where
    T: RemoteFs + Sync + Send + 'h,
{
    /// Type of the context associated with an open file object.
    type Context = SyncStatHandle;

    /// Called when Dokan has successfully mounted the volume.
    fn mounted(
        &'h self,
        _mount_point: &U16CStr,
        _info: &OperationInfo<'c, 'h, Self>,
    ) -> OperationResult<()> {
        info!("mounted()");
        match self.remote_mut(|remote| remote.connect()) {
            Ok(_) => Ok(()),
            Err(e) => {
                error!("connection failed: {e}",);
                Err(ntstatus::STATUS_CONNECTION_DISCONNECTED)
            }
        }
    }

    /// Called when Dokan is unmounting the volume.
    fn unmounted(&'h self, _info: &OperationInfo<'c, 'h, Self>) -> OperationResult<()> {
        info!("unmounted()");
        match self.remote_mut(|remote| remote.disconnect()) {
            Ok(_) => Ok(()),
            Err(e) => {
                error!("disconnection failed: {e}",);
                Err(ntstatus::STATUS_CONNECTION_DISCONNECTED)
            }
        }
    }

    /// Called when a file object is created.
    ///
    /// The flags p-them to flags accepted by [`CreateFile`] using the
    /// [`map_kernel_to_user_create_file_flags`] helper function.
    ///
    /// [`ZwCreateFile`]: https://docs.microsoft.com/en-us/windows-hardware/drivers/ddi/wdm/nf-wdm-zwcreatefile
    /// [`CreateFile`]: https://docs.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-createfilew
    /// [`map_kernel_to_user_create_file_flags`]: crate::map_kernel_to_user_create_file_flags
    fn create_file(
        &'h self,
        file_name: &U16CStr,
        _security_context: &dokan_sys::DOKAN_IO_SECURITY_CONTEXT,
        desired_access: ACCESS_MASK,
        file_attributes: u32,
        share_access: u32,
        create_disposition: u32,
        create_options: u32,
        _info: &mut OperationInfo<'c, 'h, Self>,
    ) -> OperationResult<CreateFileInfo<Self::Context>> {
        let path_info = common::path_info(file_name);
        debug!(
            "create_file({:?}, {desired_access:?}, {file_attributes:?}, {share_access:?}, {create_disposition:?}, {create_options:?})",
            path_info.path
        );
        let delete_on_close = create_options & FILE_DELETE_ON_CLOSE > 0;
        let existing = self.stat(file_name).ok().map(|stat| stat.value().clone());
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
                context: SyncStatHandle {
                    stat,
                    alt_stream: RwLock::new(alt_stream),
                    delete_on_close,
                    pending_write: Mutex::new(None),
                },
                is_dir,
                new_file_created,
            }),
            CreatePlan::CreateFile => {
                self.create_empty_file(&path_info.path).map_err(|err| {
                    error!("failed to create empty file: {err}");
                    ntstatus::STATUS_CONNECTION_DISCONNECTED
                })?;
                let stat = self.stat(file_name).map_err(|err| {
                    error!("stat failed: {err}");
                    ntstatus::STATUS_CONNECTION_DISCONNECTED
                })?;
                Ok(CreateFileInfo {
                    context: SyncStatHandle {
                        stat: stat.value().clone(),
                        alt_stream: RwLock::new(None),
                        delete_on_close,
                        pending_write: Mutex::new(None),
                    },
                    is_dir: false,
                    new_file_created: true,
                })
            }
            CreatePlan::CreateDirectory => {
                self.remote(|remote| {
                    remote.create_dir(&path_info.path, Some(UnixPex::from(0o755)))
                })
                .map_err(|err| {
                    error!("create_dir failed: {err}");
                    ntstatus::STATUS_CONNECTION_DISCONNECTED
                })?;
                let stat = self.stat(file_name).map_err(|err| {
                    error!("stat failed: {err}");
                    ntstatus::STATUS_CONNECTION_DISCONNECTED
                })?;
                Ok(CreateFileInfo {
                    context: SyncStatHandle {
                        stat: stat.value().clone(),
                        alt_stream: RwLock::new(None),
                        delete_on_close,
                        pending_write: Mutex::new(None),
                    },
                    is_dir: true,
                    new_file_created: true,
                })
            }
        }
    }

    /// Called when the last handle for the file object has been closed.
    ///
    /// If [`info.delete_on_close`] returns `true`, the file should be deleted in this function. As the function doesn't
    /// have a return value, you should make sure the file is deletable in [`delete_file`] or [`delete_directory`].
    ///
    /// Note that the file object hasn't been released and there might be more I/O operations before
    /// [`close_file`] gets called. (This typically happens when the file is memory-mapped.)
    ///
    /// Normally [`close_file`] will be called shortly after this function. However, the file object
    /// may also be reused, and in that case [`create_file`] will be called instead.
    ///
    /// [`info.delete_on_close`]: OperationInfo::delete_on_close
    /// [`delete_file`]: Self::delete_file
    /// [`delete_directory`]: Self::delete_directory
    /// [`close_file`]: Self::close_file
    /// [`create_file`]: Self::create_file
    fn cleanup(
        &'h self,
        file_name: &U16CStr,
        info: &OperationInfo<'c, 'h, Self>,
        context: &'c Self::Context,
    ) {
        debug!("cleanup({file_name:?}, {context:?})");
        let stat = match context.stat.read() {
            Err(_) => {
                error!("mutex poisoned");
                return;
            }
            Ok(stat) => stat,
        };

        let alt_stream_delete =
            common::try_alt_stream(context, |alt_stream| Ok(alt_stream.delete_pending))
                .transpose()
                .unwrap_or_default()
                .unwrap_or_default();

        if alt_stream_delete {
            let mut alt_stream = match context.alt_stream.write() {
                Ok(alt_stream) => alt_stream,
                Err(_) => {
                    error!("mutex poisoned");
                    return;
                }
            };
            alt_stream.take();
            return;
        }

        let will_delete = context.delete_on_close
            || stat.delete_on_close
            || stat.delete_pending
            || info.delete_on_close();

        // defensively finalize any write flush_file_buffers never got a chance to persist (per
        // Dokan's docs, cleanup can run with more I/O still pending, but this is the last
        // reliable point before the handle, and possibly the file, goes away); skip it entirely
        // if the file is about to be deleted anyway
        if !will_delete && let Err(err) = self.finalize_pending_write(context, &stat.file) {
            error!("failed to finalize write on cleanup: {err}");
        }

        if will_delete {
            debug!(
                "removing file: {}; delete_on_close: {}; stat.delete_on_close: {}; delete_pending: {}",
                stat.file.path().display(),
                context.delete_on_close,
                stat.delete_on_close,
                stat.delete_pending
            );
            if let Err(err) = self.remote(|remote| {
                if stat.file.is_dir() {
                    remote.remove_dir(&stat.file.path)
                } else {
                    remote.remove_file(&stat.file.path)
                }
            }) {
                error!("delete failed: {err}");
            }
        }
    }

    /// Called when the last handle for the handle object has been closed and released.
    ///
    /// This is the last function called during the lifetime of the file object. You can safely
    /// release any resources allocated for it (such as file handles, buffers, etc.). The associated
    /// [`context`] object will also be dropped once this function returns. In case the file object is
    /// reused and thus this function isn't called, the [`context`] will be dropped before
    /// [`FileSystemHandler::create_file`] gets called.
    ///
    /// [`context`]: [`Self::Context`]
    /// [`create_file`]: [`FileSystemHandler::create_file`]
    fn close_file(
        &'h self,
        file_name: &U16CStr,
        _info: &OperationInfo<'c, 'h, Self>,
        context: &'c Self::Context,
    ) {
        debug!("close_file({file_name:?}, {context:?})");

        let key = file_name.to_ucstring();
        self.file_handlers.remove(&key);
    }

    /// Reads data from the file.
    ///
    /// The number of bytes that actually gets read should be returned.
    ///
    /// See [`ReadFile`] for more information.
    ///
    /// [`ReadFile`]: https://docs.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-readfile
    fn read_file(
        &'h self,
        file_name: &U16CStr,
        offset: i64,
        buffer: &mut [u8],
        _info: &OperationInfo<'c, 'h, Self>,
        context: &'c Self::Context,
    ) -> OperationResult<u32> {
        debug!("read_file({file_name:?}, {offset})");
        // read file
        let file = match context.stat.read() {
            Err(_) => {
                error!("mutex poisoned");
                return Err(STATUS_INVALID_DEVICE_REQUEST);
            }
            Ok(stat) => stat.file.clone(),
        };

        // check alt stream
        if let Some(res) = common::try_alt_stream(context, |alt_stream| {
            let offset = usize::try_from(common::nonnegative_offset(offset)?)
                .map_err(|_| STATUS_INVALID_PARAMETER)?;
            // reading past the end of the stream yields zero bytes, matching regular file reads
            let Some(available) = alt_stream.data.len().checked_sub(offset) else {
                return Ok(0);
            };
            let len = std::cmp::min(buffer.len(), available);
            buffer[0..len].copy_from_slice(&alt_stream.data[offset..offset + len]);
            Ok(len as u32)
        }) {
            return res;
        }

        self.read(&file.path, buffer, common::nonnegative_offset(offset)?)
            .map_err(|err| {
                error!("read failed: {err}");
                STATUS_INVALID_DEVICE_REQUEST
            })
            .map(|len| len as u32)
    }

    /// Writes data to the file.
    ///
    /// The number of bytes that actually gets written should be returned.
    ///
    /// If [`info.write_to_eof`] returns `true`, data should be written to the end of file and the
    /// `offset` parameter should be ignored.
    ///
    /// See [`WriteFile`] for more information.
    ///
    /// [`info.write_to_eof`]: OperationInfo::write_to_eof
    /// [`WriteFile`]: https://docs.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-writefile
    fn write_file(
        &'h self,
        file_name: &U16CStr,
        offset: i64,
        buffer: &[u8],
        info: &OperationInfo<'c, 'h, Self>,
        context: &'c Self::Context,
    ) -> OperationResult<u32> {
        debug!("write_file({file_name:?}, {offset})");
        // read file
        let file = match context.stat.read() {
            Err(_) => {
                error!("mutex poisoned");
                return Err(STATUS_INVALID_DEVICE_REQUEST);
            }
            Ok(stat) => stat.file.clone(),
        };

        // check alt stream
        if let Some(res) = common::try_alt_stream(context, |alt_stream| {
            debug!("write alt stream: {file_name:?}");
            let offset = if info.write_to_eof() {
                alt_stream.data.len()
            } else {
                usize::try_from(common::nonnegative_offset(offset)?)
                    .map_err(|_| STATUS_INVALID_PARAMETER)?
            };
            let len = buffer.len();
            if offset + len > alt_stream.data.len() {
                alt_stream.data.resize(offset + len, 0);
            }
            alt_stream.data[offset..offset + len].copy_from_slice(buffer);

            Ok(len as u32)
        }) {
            return res;
        }

        if info.write_to_eof() {
            debug!("append file: {file_name:?}");
            self.append(&file, buffer)
        } else {
            debug!("write file: {file_name:?}");
            self.write_to_handle(context, &file, buffer, common::nonnegative_offset(offset)?)
        }
        .map_err(|err| {
            error!("write failed: {err}");
            STATUS_INVALID_DEVICE_REQUEST
        })
    }

    /// Flushes the buffer of the file and causes all buffered data to be written to the file.
    ///
    /// See [`FlushFileBuffers`] for more information.
    ///
    /// [`FlushFileBuffers`]: https://docs.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-flushfilebuffers
    fn flush_file_buffers(
        &'h self,
        file_name: &U16CStr,
        _info: &OperationInfo<'c, 'h, Self>,
        context: &'c Self::Context,
    ) -> OperationResult<()> {
        debug!("flush_file_buffers({file_name:?}, {context:?})");

        let file = match context.stat.read() {
            Err(_) => {
                error!("mutex poisoned");
                return Err(STATUS_INVALID_DEVICE_REQUEST);
            }
            Ok(stat) => stat.file.clone(),
        };

        self.finalize_pending_write(context, &file).map_err(|err| {
            error!("failed to flush write: {err}");
            STATUS_INVALID_DEVICE_REQUEST
        })
    }

    /// Gets information about the file.
    ///
    /// See [`GetFileInformationByHandle`] for more information.
    ///
    /// [`GetFileInformationByHandle`]: https://docs.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-getfileinformationbyhandle
    fn get_file_information(
        &'h self,
        file_name: &U16CStr,
        _info: &OperationInfo<'c, 'h, Self>,
        context: &'c Self::Context,
    ) -> OperationResult<FileInfo> {
        debug!("get_file_information({file_name:?}, {context:?})");

        let file = match context.stat.read() {
            Err(_) => {
                error!("mutex poisoned");
                return Err(STATUS_INVALID_DEVICE_REQUEST);
            }
            Ok(stat) => stat.file.clone(),
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

    /// Lists all child items in the directory.
    ///
    /// `fill_find_data` should be called for every child item in the directory.
    ///
    /// It will only be called if [`find_files_with_pattern`] returns [`STATUS_NOT_IMPLEMENTED`].
    ///
    /// See [`FindFirstFile`] for more information.
    ///
    /// [`find_files_with_pattern`]: Self::find_files_with_pattern
    /// [`FindFirstFile`]: https://docs.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-findfirstfilew
    fn find_files(
        &'h self,
        file_name: &U16CStr,
        fill_find_data: impl FnMut(&FindData) -> FillDataResult,
        _info: &OperationInfo<'c, 'h, Self>,
        context: &'c Self::Context,
    ) -> OperationResult<()> {
        debug!("find_files({file_name:?}, {context:?})");

        let alt_stream = match context.alt_stream.read() {
            Err(_) => {
                error!("mutex poisoned");
                return Err(STATUS_INVALID_DEVICE_REQUEST);
            }
            Ok(stream) => stream.clone(),
        };
        if alt_stream.is_some() {
            error!("alt stream found");
            return Err(STATUS_INVALID_DEVICE_REQUEST);
        }
        drop(alt_stream);

        let file = match context.stat.read() {
            Err(_) => {
                error!("mutex poisoned");
                return Err(STATUS_INVALID_DEVICE_REQUEST);
            }
            Ok(stat) => stat.file.clone(),
        };

        self.find_files(&file, None, fill_find_data)
    }

    /// Lists all child items that matches the specified `pattern` in the directory.
    ///
    /// `fill_find_data` should be called for every matching child item in the directory.
    ///
    /// [`is_name_in_expression`] can be used to determine if a file name matches the pattern.
    ///
    /// If this function returns [`STATUS_NOT_IMPLEMENTED`], [`find_files`] will be called instead and
    /// pattern matching will be handled directly by Dokan.
    ///
    /// See [`FindFirstFile`] for more information.
    ///
    /// [`is_name_in_expression`]: crate::is_name_in_expression
    /// [`find_files`]: Self::find_files
    /// [`FindFirstFile`]: https://docs.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-findfirstfilew
    fn find_files_with_pattern(
        &'h self,
        file_name: &U16CStr,
        pattern: &U16CStr,
        fill_find_data: impl FnMut(&FindData) -> FillDataResult,
        _info: &OperationInfo<'c, 'h, Self>,
        context: &'c Self::Context,
    ) -> OperationResult<()> {
        debug!("find_files_with_pattern({file_name:?}, {pattern:?}, {context:?})");

        /*
        let alt_stream = match context.alt_stream.read() {
            Err(_) => {
                error!("mutex poisoned");
                return Err(STATUS_INVALID_DEVICE_REQUEST);
            }
            Ok(stream) => stream.clone(),
        };
        if alt_stream.is_some() {
            error!("alt stream found");
            return Err(STATUS_INVALID_DEVICE_REQUEST);
        }
        drop(alt_stream);
         */

        let file = match context.stat.read() {
            Err(_) => {
                error!("mutex poisoned");
                return Err(STATUS_INVALID_DEVICE_REQUEST);
            }
            Ok(stat) => stat.file.clone(),
        };

        self.find_files(&file, Some(pattern), fill_find_data)
    }

    /// Sets attributes of the file.
    ///
    /// `file_attributes` can be combination of one or more [file attribute constants] defined by
    /// Windows.
    ///
    /// See [`SetFileAttributes`] for more information.
    ///
    /// [file attribute constants]: https://docs.microsoft.com/en-us/windows/win32/fileio/file-attribute-constants
    /// [`SetFileAttributes`]: https://docs.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-setfileattributesw
    fn set_file_attributes(
        &'h self,
        file_name: &U16CStr,
        file_attributes: u32,
        _info: &OperationInfo<'c, 'h, Self>,
        context: &'c Self::Context,
    ) -> OperationResult<()> {
        debug!("set_file_attributes({file_name:?}, {file_attributes:?}, {context:?})");

        Ok(())
    }

    /// Sets the time when the file was created, last accessed and last written.
    ///
    /// See [`SetFileTime`] for more information.
    ///
    /// [`SetFileTime`]: https://docs.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-setfiletime
    fn set_file_time(
        &'h self,
        file_name: &U16CStr,
        creation_time: FileTimeOperation,
        last_access_time: FileTimeOperation,
        last_write_time: FileTimeOperation,
        _info: &OperationInfo<'c, 'h, Self>,
        context: &'c Self::Context,
    ) -> OperationResult<()> {
        debug!(
            "set_file_time({file_name:?}, {creation_time:?}, {last_access_time:?}, {last_write_time:?}, {context:?})"
        );
        let file = match context.stat.read() {
            Err(_) => {
                error!("mutex poisoned");
                return Err(STATUS_INVALID_DEVICE_REQUEST);
            }
            Ok(stat) => stat.file.clone(),
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

        if any_change
            && let Err(err) = self.remote(|remote| remote.set_metadata(file.path(), &changes))
        {
            error!("set_metadata failed: {err}");
            return Err(STATUS_INVALID_DEVICE_REQUEST);
        }

        Ok(())
    }

    /// Checks if the file can be deleted.
    ///
    /// The file should not be deleted in this function. Instead, it should only check if the file
    /// can be deleted and return `Ok` if that is possible.
    ///
    /// It will also be called with [`info.delete_on_close`] returning `false` to notify that the
    /// file is no longer requested to be deleted.
    ///
    /// [`info.delete_on_close`]: OperationInfo::delete_on_close
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
            error!("file is a directory: {file_name:?}");
            return Err(STATUS_CANNOT_DELETE);
        }

        if let Some(res) = common::try_alt_stream(context, |alt_stream| {
            if alt_stream.delete_pending {
                error!("delete pending: {file_name:?}");
                return Err(STATUS_DELETE_PENDING);
            }
            Ok(())
        }) {
            return res;
        }

        match context.stat.write() {
            Ok(mut stream) => {
                stream.delete_pending = info.delete_on_close();
            }
            Err(_) => {
                error!("mutex poisoned");
                return Err(STATUS_INVALID_DEVICE_REQUEST);
            }
        }

        Ok(())
    }

    /// Checks if the directory can be deleted.
    ///
    /// Similar to [`delete_file`], it should only check if the directory can be deleted and delay
    /// the actual deletion to the [`cleanup`] function.
    ///
    /// It will also be called with [`info.delete_on_close`] returning `false` to notify that the
    /// directory is no longer requested to be deleted.
    ///
    /// [`delete_file`]: Self::delete_file
    /// [`cleanup`]: Self::cleanup
    /// [`info.delete_on_close`]: OperationInfo::delete_on_close
    fn delete_directory(
        &'h self,
        file_name: &U16CStr,
        info: &OperationInfo<'c, 'h, Self>,
        context: &'c Self::Context,
    ) -> OperationResult<()> {
        debug!("delete_directory({file_name:?}, {context:?})");

        if common::try_alt_stream(context, |_alt_stream| Ok(())).is_some() {
            error!("alt stream found: {file_name:?}");
            return Err(STATUS_INVALID_DEVICE_REQUEST);
        }

        let file = match context.stat.read() {
            Err(_) => {
                error!("mutex poisoned");
                return Err(STATUS_INVALID_DEVICE_REQUEST);
            }
            Ok(stat) => stat.file.clone(),
        };

        if !file.is_dir() {
            error!("file is not a directory: {file_name:?}");
            return Err(STATUS_NOT_A_DIRECTORY);
        }

        // check if directory is empty
        let is_empty = match self.remote(|remote| remote.list_dir(&file.path)) {
            Ok(entries) => entries.is_empty(),
            Err(err) => {
                error!("list_dir failed: {err}");
                return Err(STATUS_INVALID_DEVICE_REQUEST);
            }
        };

        if !is_empty && info.delete_on_close() {
            error!("directory is not empty: {file_name:?}");
            return Err(STATUS_DIRECTORY_NOT_EMPTY);
        }

        // set delete pending
        if let Some(res) = common::try_alt_stream(context, |alt_stream| {
            alt_stream.delete_pending = info.delete_on_close();
            Ok(())
        }) {
            return res;
        }

        match context.stat.write() {
            Ok(mut stat) => {
                stat.delete_pending = info.delete_on_close();
            }
            Err(_) => {
                error!("mutex poisoned");
                return Err(STATUS_INVALID_DEVICE_REQUEST);
            }
        };

        Ok(())
    }

    /// Moves the file.
    ///
    /// If the `new_file_name` already exists, the function should only replace the existing file
    /// when `replace_if_existing` is `true`, otherwise it should return appropriate error.
    ///
    /// Note that renaming is a special kind of moving and is also handled by this function.
    ///
    /// See [`MoveFileEx`] for more information.
    ///
    /// [`MoveFileEx`]: https://docs.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-movefileexw
    fn move_file(
        &'h self,
        file_name: &U16CStr,
        new_file_name: &U16CStr,
        replace_if_existing: bool,
        _info: &OperationInfo<'c, 'h, Self>,
        context: &'c Self::Context,
    ) -> OperationResult<()> {
        debug!("move_file({file_name:?}, {new_file_name:?}, {replace_if_existing:?}, {context:?})");

        let dest = common::path_info(new_file_name);
        // check if destination exists
        if !replace_if_existing
            && self
                .remote(|remote| remote.exists(&dest.path))
                .unwrap_or(true)
        {
            error!("destination already exists: {new_file_name:?}");
            return Err(STATUS_OBJECT_NAME_COLLISION);
        }

        let file = match context.stat.read() {
            Err(_) => {
                error!("mutex poisoned");
                return Err(STATUS_INVALID_DEVICE_REQUEST);
            }
            Ok(stat) => stat.file.clone(),
        };

        debug!("move file: {file_name:?} -> {new_file_name:?}");

        self.remote(|remote| remote.rename(&file.path, &dest.path))
            .map_err(|err| {
                error!("move failed: {err}");
                STATUS_ACCESS_DENIED
            })
    }

    /// Sets end-of-file position of the file.
    ///
    /// The `offset` value is zero-based, so it actually refers to the offset to the byte
    /// immediately following the last valid byte in the file.
    ///
    /// See [`FILE_END_OF_FILE_INFORMATION`] for more information.
    ///
    /// [`FILE_END_OF_FILE_INFORMATION`]: https://docs.microsoft.com/en-us/windows-hardware/drivers/ddi/ntddk/ns-ntddk-_file_end_of_file_information
    fn set_end_of_file(
        &'h self,
        file_name: &U16CStr,
        offset: i64,
        _info: &OperationInfo<'c, 'h, Self>,
        context: &'c Self::Context,
    ) -> OperationResult<()> {
        debug!("set_end_of_file({file_name:?}, {offset}, {context:?})");

        if let Some(res) = common::try_alt_stream(context, |alt_stream| {
            let offset = usize::try_from(common::nonnegative_offset(offset)?)
                .map_err(|_| STATUS_INVALID_PARAMETER)?;
            alt_stream.data.truncate(offset);

            Ok(())
        }) {
            return res;
        }

        let file = match context.stat.read() {
            Err(_) => {
                error!("mutex poisoned");
                return Err(STATUS_INVALID_DEVICE_REQUEST);
            }
            Ok(stat) => stat.file.clone(),
        };

        // A staged write for this handle supersedes the file on the remote; drop it so the
        // truncate below is not overwritten on flush.
        if let Ok(mut pending) = context.pending_write.lock() {
            pending.take();
        }

        let offset = common::nonnegative_offset(offset)?;
        self.remote(|remote| transfer::truncate_file(remote, &file, offset))
            .map_err(|err| {
                error!("truncate failed: {err}");
                STATUS_INVALID_DEVICE_REQUEST
            })
    }

    /// Sets allocation size of the file.
    ///
    /// The allocation size is the number of bytes allocated in the underlying physical device for
    /// the file.
    ///
    /// See [`FILE_ALLOCATION_INFORMATION`] for more information.
    ///
    /// [`FILE_ALLOCATION_INFORMATION`]: https://docs.microsoft.com/en-us/windows-hardware/drivers/ddi/ntifs/ns-ntifs-_file_allocation_information
    fn set_allocation_size(
        &'h self,
        file_name: &U16CStr,
        alloc_size: i64,
        _info: &OperationInfo<'c, 'h, Self>,
        context: &'c Self::Context,
    ) -> OperationResult<()> {
        debug!("set_allocation_size({file_name:?}, {alloc_size}, {context:?})");

        common::try_alt_stream(context, |alt_stream: &mut AltStream| {
            let alloc_size = usize::try_from(common::nonnegative_offset(alloc_size)?)
                .map_err(|_| STATUS_INVALID_PARAMETER)?;
            alt_stream.data = vec![0; alloc_size];

            Ok(())
        })
        .unwrap_or({
            debug!("cant set allocation size: not implemented");
            Ok(())
        })
    }

    /// Gets security information of a file.
    ///
    /// Size of the security descriptor in bytes should be returned on success. If the buffer is not
    /// large enough, the number should still be returned, and [`STATUS_BUFFER_OVERFLOW`] will be
    /// automatically passed to Dokan if it is larger than `buffer_length`.
    ///
    /// See [`GetFileSecurity`] for more information.
    ///
    /// [`STATUS_BUFFER_OVERFLOW`]: winapi::shared::ntstatus::STATUS_BUFFER_OVERFLOW
    /// [`GetFileSecurity`]: https://docs.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-getfilesecuritya
    fn get_file_security(
        &'h self,
        file_name: &U16CStr,
        security_information: u32,
        security_descriptor: winapi::um::winnt::PSECURITY_DESCRIPTOR,
        buffer_length: u32,
        _info: &OperationInfo<'c, 'h, Self>,
        context: &'c Self::Context,
    ) -> OperationResult<u32> {
        debug!(
            "get_file_security({file_name:?}, {security_information:?}, {buffer_length}, {context:?})"
        );
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

    /// Sets security information of a file.
    ///
    /// See [`SetFileSecurity`] for more information.
    ///
    /// [`SetFileSecurity`]: https://docs.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-setfilesecuritya
    fn set_file_security(
        &'h self,
        file_name: &U16CStr,
        security_information: u32,
        security_descriptor: winapi::um::winnt::PSECURITY_DESCRIPTOR,
        _buffer_length: u32,
        _info: &OperationInfo<'c, 'h, Self>,
        context: &'c Self::Context,
    ) -> OperationResult<()> {
        debug!("set_file_security({file_name:?}, {security_information:?}, {context:?})");

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

    /// Lists all alternative streams of the file.
    ///
    /// `fill_find_stream_data` should be called for every stream of the file, including the default
    /// data stream `::$DATA`.
    ///
    /// See [`FindFirstStream`] for more information.
    ///
    /// [`FindFirstStream`]: https://docs.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-findfirststreamw
    fn find_streams(
        &'h self,
        file_name: &U16CStr,
        mut fill_find_stream_data: impl FnMut(&FindStreamData) -> FillDataResult,
        _info: &OperationInfo<'c, 'h, Self>,
        context: &'c Self::Context,
    ) -> OperationResult<()> {
        debug!("find_streams({file_name:?}, {context:?})");

        let file = match context.stat.read() {
            Err(_) => {
                error!("mutex poisoned");
                return Err(STATUS_INVALID_DEVICE_REQUEST);
            }
            Ok(stat) => stat.file.clone(),
        };

        fill_find_stream_data(&FindStreamData {
            size: file.metadata().size.unwrap_or(0) as i64,
            name: U16CString::from_str("::$DATA").unwrap(),
        })
        .or_else(common::ignore_name_too_long)?;

        let alt_streams = match context.stat.read() {
            Err(_) => {
                error!("mutex poisoned");
                return Err(STATUS_INVALID_DEVICE_REQUEST);
            }
            Ok(stat) => stat.alt_streams.clone(),
        };

        for (k, v) in alt_streams.iter() {
            let mut name_buf = vec![':' as u16];
            name_buf.extend_from_slice(k.0.as_slice());
            name_buf.extend_from_slice(U16String::from_str(":$DATA").as_slice());
            fill_find_stream_data(&FindStreamData {
                size: v
                    .read()
                    .map(|data| data.data.len() as i64)
                    .unwrap_or_default(),
                name: U16CString::from_ustr(U16Str::from_slice(&name_buf)).unwrap(),
            })
            .or_else(common::ignore_name_too_long)?;
        }
        Ok(())
    }

    fn get_volume_information(
        &'h self,
        _info: &OperationInfo<'c, 'h, Self>,
    ) -> OperationResult<VolumeInfo> {
        debug!("get_volume_information()");

        Ok(VolumeInfo {
            name: U16CString::from_str("remotefs-fuse").expect("failed to create U16CString"),
            serial_number: 0,
            max_component_length: 255,
            fs_flags: FILE_CASE_SENSITIVE_SEARCH | FILE_CASE_PRESERVED_NAMES,
            fs_name: U16CString::from_str("DOKANY").expect("failed to create U16CString"),
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
        error!("lock_file not implemented");
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
        error!("unlock_file not implemented");
        Err(STATUS_NOT_IMPLEMENTED)
    }

    fn get_disk_free_space(
        &'h self,
        _info: &OperationInfo<'c, 'h, Self>,
    ) -> OperationResult<DiskSpaceInfo> {
        const DEFAULT_SIZE: u64 = 1024 * 1024 * 1024 * 128; // 128GB
        debug!("get_disk_free_space()");
        Ok(DiskSpaceInfo {
            free_byte_count: DEFAULT_SIZE,
            byte_count: DEFAULT_SIZE,
            available_byte_count: DEFAULT_SIZE,
        })
    }
}
