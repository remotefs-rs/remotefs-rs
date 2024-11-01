mod entry;
mod security;
#[cfg(test)]
mod test;

use std::io::{Cursor, Read as _, Seek as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, RwLock};

use dashmap::mapref::one::Ref;
use dokan::{
    CreateFileInfo, DiskSpaceInfo, FileInfo, FileSystemHandler, FileTimeOperation, FillDataResult,
    FindData, FindStreamData, OperationInfo, OperationResult, VolumeInfo,
};
use dokan_sys::win32::{
    FILE_CREATE, FILE_DELETE_ON_CLOSE, FILE_DIRECTORY_FILE, FILE_MAXIMUM_DISPOSITION,
    FILE_NON_DIRECTORY_FILE, FILE_OPEN, FILE_OPEN_IF, FILE_OVERWRITE, FILE_OVERWRITE_IF,
    FILE_SUPERSEDE,
};
use entry::{EntryName, StatHandle};
use remotefs::fs::{Metadata, UnixPex};
use remotefs::{File, RemoteError, RemoteErrorType, RemoteFs, RemoteResult};
use widestring::{U16CStr, U16CString};
use winapi::shared::ntstatus::{
    self, STATUS_ACCESS_DENIED, STATUS_CANNOT_DELETE, STATUS_DELETE_PENDING,
    STATUS_FILE_IS_A_DIRECTORY, STATUS_INVALID_DEVICE_REQUEST, STATUS_INVALID_PARAMETER,
    STATUS_NOT_A_DIRECTORY, STATUS_NOT_IMPLEMENTED, STATUS_OBJECT_NAME_COLLISION,
    STATUS_OBJECT_NAME_NOT_FOUND,
};
use winapi::um::winnt::{self, ACCESS_MASK};

pub use self::entry::Stat;
use self::security::SecurityDescriptor;
use super::Driver;

struct PathInfo {
    path: PathBuf,
    file_name: U16CString,
    parent: PathBuf,
}

#[derive(Debug)]
struct AltStream {
    handle_count: u32,
    delete_pending: bool,
}

impl AltStream {
    fn new() -> Self {
        Self {
            handle_count: 0,
            delete_pending: false,
        }
    }
}

impl<T> Driver<T>
where
    T: RemoteFs + Sync + Send,
{
    fn path_to_u16string(path: &Path) -> U16CString {
        U16CString::from_str(path.to_string_lossy()).expect("failed to convert path to U16CString")
    }

    fn stat(&self, file_name: &U16CStr) -> RemoteResult<Ref<'_, U16CString, Arc<RwLock<Stat>>>> {
        let key = file_name.to_ucstring();
        if let Some(stat) = self.file_handlers.get(&key) {
            return Ok(stat);
        }

        let path_info = self.path_info(file_name);

        let Ok(mut lock) = self.remote.lock() else {
            error!("mutex poisoned");
            return Err(RemoteError::new(remotefs::RemoteErrorType::ProtocolError));
        };

        let file = lock.stat(&path_info.path)?;

        // insert the file into the file handlers
        self.file_handlers.insert(
            key.clone(),
            Arc::new(RwLock::new(Stat::new(
                file,
                SecurityDescriptor::new_default()
                    .map_err(|_| RemoteError::new(remotefs::RemoteErrorType::ProtocolError))?,
            ))),
        );

        Ok(self.file_handlers.get(&key).unwrap())
    }

    fn path_info(&self, file_name: &U16CStr) -> PathInfo {
        let p = PathBuf::from(file_name.to_string_lossy());
        let parent = p
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("/"));

        PathInfo {
            path: p,
            parent,
            file_name: file_name.to_ucstring(),
        }
    }

    /// Read data from a file.
    ///
    /// If possible, this system will use the stream from remotefs directly,
    /// otherwise it will use a temporary file (*sigh*).
    /// Note that most of remotefs supports streaming, so this should be rare.
    fn read(&self, path: &Path, buffer: &mut [u8], offset: u64) -> RemoteResult<usize> {
        let Ok(mut remote) = self.remote.lock() else {
            error!("mutex poisoned");
            return Err(RemoteError::new(remotefs::RemoteErrorType::ProtocolError));
        };
        match remote.open(path) {
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
                remote.on_read(reader)?;

                Ok(bytes_read)
            }
            Err(RemoteError {
                kind: RemoteErrorType::UnsupportedFeature,
                ..
            }) => {
                drop(remote);
                self.read_tempfile(path, buffer, offset)
            }
            Err(err) => Err(err),
        }
    }

    /// Read data from a file using a temporary file.
    fn read_tempfile(&self, path: &Path, buffer: &mut [u8], offset: u64) -> RemoteResult<usize> {
        let Ok(mut remote) = self.remote.lock() else {
            error!("mutex poisoned");
            return Err(RemoteError::new(remotefs::RemoteErrorType::ProtocolError));
        };

        let Ok(tempfile) = tempfile::NamedTempFile::new() else {
            return Err(remotefs::RemoteError::new(
                remotefs::RemoteErrorType::IoError,
            ));
        };
        let Ok(writer) = std::fs::OpenOptions::new()
            .write(true)
            .open(tempfile.path())
        else {
            error!("Failed to open temporary file");
            return Err(remotefs::RemoteError::new(
                remotefs::RemoteErrorType::IoError,
            ));
        };

        // transfer to tempfile
        remote.open_file(path, Box::new(writer))?;

        let Ok(mut reader) = std::fs::File::open(tempfile.path()) else {
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
    fn write(&self, file: &File, data: &[u8], offset: u64) -> RemoteResult<u32> {
        // write data
        let Ok(mut remote) = self.remote.lock() else {
            error!("mutex poisoned");
            return Err(RemoteError::new(remotefs::RemoteErrorType::ProtocolError));
        };

        let mut reader = Cursor::new(data);
        let mut writer = match remote.create(file.path(), file.metadata()) {
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
                drop(remote);
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
        remote
            .on_written(writer)
            .map_err(|err| RemoteError::new_ex(RemoteErrorType::IoError, err.to_string()))?;

        Ok(bytes_written)
    }

    /// Write data to a file without using a stream.
    fn write_wno_stream(&self, file: &File, data: &[u8]) -> RemoteResult<u32> {
        debug!(
            "Writing file without stream: {:?} {} bytes",
            file.path(),
            data.len()
        );
        let Ok(mut remote) = self.remote.lock() else {
            error!("mutex poisoned");
            return Err(RemoteError::new(remotefs::RemoteErrorType::ProtocolError));
        };
        let reader = Cursor::new(data.to_vec());
        remote
            .create_file(file.path(), file.metadata(), Box::new(reader))
            .map(|len| len as u32)
    }
}

// For reference <https://github.com/dokan-dev/dokan-rust/blob/master/dokan/examples/memfs/main.rs>
impl<'c, 'h: 'c, T> FileSystemHandler<'c, 'h> for Driver<T>
where
    T: RemoteFs + Sync + Send + 'h,
{
    /// Type of the context associated with an open file object.
    type Context = StatHandle;

    /// Called when Dokan has successfully mounted the volume.
    fn mounted(
        &'h self,
        _mount_point: &U16CStr,
        _info: &OperationInfo<'c, 'h, Self>,
    ) -> OperationResult<()> {
        info!("mounted()");
        match self.remote.lock().map(|mut rem| rem.connect()) {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(e)) => {
                error!("connection failed: {e}",);
                Err(ntstatus::STATUS_CONNECTION_DISCONNECTED)
            }
            Err(_) => {
                error!("mutex poisoned");
                Err(ntstatus::STATUS_CONNECTION_DISCONNECTED)
            }
        }
    }

    /// Called when Dokan is unmounting the volume.
    fn unmounted(&'h self, _info: &OperationInfo<'c, 'h, Self>) -> OperationResult<()> {
        info!("unmounted()");
        match self.remote.lock().map(|mut rem| rem.disconnect()) {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(e)) => {
                error!("disconnection failed: {e}",);
                Err(ntstatus::STATUS_CONNECTION_DISCONNECTED)
            }
            Err(_) => {
                error!("mutex poisoned");
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
        info!("create_file({file_name:?}, {desired_access:?}, {file_attributes:?}, {share_access:?}, {create_disposition:?}, {create_options:?})");

        let stat = self.stat(file_name).ok();

        if create_disposition > FILE_MAXIMUM_DISPOSITION {
            error!("invalid create disposition: {create_disposition}");
            return Err(STATUS_INVALID_PARAMETER);
        }
        let delete_on_close = create_options & FILE_DELETE_ON_CLOSE > 0;
        if let Some(stat) = stat {
            let stat = stat.value();
            let read = stat.read().unwrap();

            let is_readonly = read
                .file
                .metadata()
                .mode
                .map(|m| (u32::from(m)) & 0o222 == 0)
                .unwrap_or_default();

            if is_readonly
                && (desired_access & winnt::FILE_WRITE_DATA > 0
                    || desired_access & winnt::FILE_APPEND_DATA > 0)
            {
                error!("file {file_name:?} is readonly");
                return Err(STATUS_ACCESS_DENIED);
            }
            if read.delete_pending {
                error!("delete pending: {file_name:?}");
                return Err(STATUS_DELETE_PENDING);
            }
            if is_readonly && delete_on_close {
                error!("delete on close: {file_name:?}");
                return Err(STATUS_CANNOT_DELETE);
            }
            std::mem::drop(read);

            let stream_name = EntryName(file_name.to_ustring());
            let ret = {
                let mut stat = stat.write().unwrap();
                if let Some(stream) = stat.alt_streams.get(&stream_name).map(|s| Arc::clone(s)) {
                    if stream.read().unwrap().delete_pending {
                        error!("delete pending: {file_name:?}");
                        return Err(STATUS_DELETE_PENDING);
                    }
                    match create_disposition {
                        FILE_SUPERSEDE | FILE_OVERWRITE | FILE_OVERWRITE_IF => {
                            if create_disposition != FILE_SUPERSEDE && is_readonly {
                                error!("file {file_name:?} is readonly");
                                return Err(STATUS_ACCESS_DENIED);
                            }
                        }
                        FILE_CREATE => return Err(ntstatus::STATUS_OBJECT_NAME_COLLISION),
                        _ => (),
                    }
                    Some((stream, false))
                } else {
                    if create_disposition == FILE_OPEN || create_disposition == FILE_OVERWRITE {
                        error!("alt stream not found: {file_name:?}");
                        return Err(STATUS_OBJECT_NAME_NOT_FOUND);
                    }
                    if is_readonly {
                        error!("file {file_name:?} is readonly");
                        return Err(STATUS_ACCESS_DENIED);
                    }
                    let stream = Arc::new(RwLock::new(AltStream::new()));
                    stat.alt_streams.insert(stream_name, Arc::clone(&stream));

                    Some((stream, true))
                }
            };

            if let Some((stream, new_file_created)) = ret {
                let handle = StatHandle {
                    stat: stat.clone(),
                    alt_stream: RwLock::new(Some(stream)),
                    delete_on_close,
                    mtime_delayed: Mutex::new(None),
                    atime_delayed: Mutex::new(None),
                    ctime_enabled: AtomicBool::new(false),
                    mtime_enabled: AtomicBool::new(false),
                    atime_enabled: AtomicBool::new(false),
                };
                return Ok(CreateFileInfo {
                    context: handle,
                    is_dir: false,
                    new_file_created,
                });
            }
            let is_file = stat
                .read()
                .ok()
                .map(|r| r.file.is_file())
                .unwrap_or_default();
            match is_file {
                true => {
                    if create_options & FILE_DIRECTORY_FILE > 0 {
                        error!("file is not a directory: {file_name:?}");
                        return Err(STATUS_NOT_A_DIRECTORY);
                    }
                    match create_disposition {
                        FILE_SUPERSEDE | FILE_OVERWRITE | FILE_OVERWRITE_IF => {
                            if create_disposition != FILE_SUPERSEDE && is_readonly {
                                error!("file {file_name:?} is readonly");
                                return Err(STATUS_ACCESS_DENIED);
                            }
                        }
                        FILE_CREATE => return Err(STATUS_OBJECT_NAME_COLLISION),
                        _ => (),
                    }
                    debug!("open file: {file_name:?}");
                    let handle = StatHandle {
                        stat: stat.clone(),
                        alt_stream: RwLock::new(None),
                        delete_on_close,
                        mtime_delayed: Mutex::new(None),
                        atime_delayed: Mutex::new(None),
                        ctime_enabled: AtomicBool::new(false),
                        mtime_enabled: AtomicBool::new(false),
                        atime_enabled: AtomicBool::new(false),
                    };
                    return Ok(CreateFileInfo {
                        context: handle,
                        is_dir: false,
                        new_file_created: false,
                    });
                }
                false => {
                    if create_options & FILE_NON_DIRECTORY_FILE > 0 {
                        return Err(STATUS_FILE_IS_A_DIRECTORY);
                    }
                    match create_disposition {
                        FILE_OPEN | FILE_OPEN_IF => {
                            debug!("open directory: {file_name:?}");
                            let handle = StatHandle {
                                stat: stat.clone(),
                                alt_stream: RwLock::new(None),
                                delete_on_close,
                                mtime_delayed: Mutex::new(None),
                                atime_delayed: Mutex::new(None),
                                ctime_enabled: AtomicBool::new(false),
                                mtime_enabled: AtomicBool::new(false),
                                atime_enabled: AtomicBool::new(false),
                            };
                            Ok(CreateFileInfo {
                                context: handle,
                                is_dir: true,
                                new_file_created: false,
                            })
                        }
                        FILE_CREATE => Err(STATUS_OBJECT_NAME_COLLISION),
                        _ => Err(STATUS_INVALID_PARAMETER),
                    }
                }
            }
        } else {
            if create_disposition == FILE_OPEN || create_disposition == FILE_OPEN_IF {
                if create_options & FILE_NON_DIRECTORY_FILE > 0 {
                    debug!("create file: {file_name:?}");
                    let path_info = self.path_info(file_name);
                    if let Err(err) = self.write(
                        &File {
                            path: path_info.path,
                            metadata: Metadata::default().mode(UnixPex::from(0o644)).size(0),
                        },
                        &[],
                        0,
                    ) {
                        error!("write failed: {err}");
                        return Err(ntstatus::STATUS_CONNECTION_DISCONNECTED);
                    }

                    let stat = match self.stat(file_name) {
                        Ok(stat) => stat,
                        Err(err) => {
                            error!("stat failed: {err}");
                            return Err(ntstatus::STATUS_CONNECTION_DISCONNECTED);
                        }
                    };

                    let handle = StatHandle {
                        stat: stat.value().clone(),
                        alt_stream: RwLock::new(None),
                        delete_on_close,
                        mtime_delayed: Mutex::new(None),
                        atime_delayed: Mutex::new(None),
                        ctime_enabled: AtomicBool::new(false),
                        mtime_enabled: AtomicBool::new(false),
                        atime_enabled: AtomicBool::new(false),
                    };

                    Ok(CreateFileInfo {
                        context: handle,
                        is_dir: false,
                        new_file_created: true,
                    })
                } else {
                    // create directory
                    debug!("create directory: {file_name:?}");
                    let stat = {
                        let path_info = self.path_info(file_name);
                        let mut lock = self.remote.lock().unwrap();
                        if let Err(err) = lock.create_dir(&path_info.path, UnixPex::from(0o755)) {
                            error!("create_dir failed: {err}");
                            return Err(ntstatus::STATUS_CONNECTION_DISCONNECTED);
                        }

                        match self.stat(file_name) {
                            Ok(stat) => stat,
                            Err(err) => {
                                error!("stat failed: {err}");
                                return Err(ntstatus::STATUS_CONNECTION_DISCONNECTED);
                            }
                        }
                    };

                    let handle = StatHandle {
                        stat: stat.value().clone(),
                        alt_stream: RwLock::new(None),
                        delete_on_close,
                        mtime_delayed: Mutex::new(None),
                        atime_delayed: Mutex::new(None),
                        ctime_enabled: AtomicBool::new(false),
                        mtime_enabled: AtomicBool::new(false),
                        atime_enabled: AtomicBool::new(false),
                    };
                    Ok(CreateFileInfo {
                        context: handle,
                        is_dir: true,
                        new_file_created: true,
                    })
                }
            } else {
                Err(STATUS_INVALID_PARAMETER)
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
        // TODO: everything necessary, finaly remove key

        todo!();
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
        info!("close_file({file_name:?}, {context:?})");

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
        info!("read_file({file_name:?}, {offset})");
        // read file
        let file = match context.stat.read() {
            Err(_) => {
                error!("mutex poisoned");
                return Err(STATUS_INVALID_DEVICE_REQUEST);
            }
            Ok(stat) => stat.file.clone(),
        };

        self.read(&file.path, buffer, offset as u64)
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
        todo!()
    }

    /// Flushes the buffer of the file and causes all buffered data to be written to the file.
    ///
    /// See [`FlushFileBuffers`] for more information.
    ///
    /// [`FlushFileBuffers`]: https://docs.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-flushfilebuffers
    fn flush_file_buffers(
        &'h self,
        file_name: &U16CStr,
        info: &OperationInfo<'c, 'h, Self>,
        context: &'c Self::Context,
    ) -> OperationResult<()> {
        todo!()
    }

    /// Gets information about the file.
    ///
    /// See [`GetFileInformationByHandle`] for more information.
    ///
    /// [`GetFileInformationByHandle`]: https://docs.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-getfileinformationbyhandle
    fn get_file_information(
        &'h self,
        file_name: &U16CStr,
        info: &OperationInfo<'c, 'h, Self>,
        context: &'c Self::Context,
    ) -> OperationResult<FileInfo> {
        todo!()
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
        info: &OperationInfo<'c, 'h, Self>,
        context: &'c Self::Context,
    ) -> OperationResult<()> {
        todo!()
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
        info: &OperationInfo<'c, 'h, Self>,
        context: &'c Self::Context,
    ) -> OperationResult<()> {
        todo!()
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
        info: &OperationInfo<'c, 'h, Self>,
        context: &'c Self::Context,
    ) -> OperationResult<()> {
        todo!()
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
        info: &OperationInfo<'c, 'h, Self>,
        context: &'c Self::Context,
    ) -> OperationResult<()> {
        todo!()
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
        info!("delete_file({file_name:?}, {context:?})");
        if context.stat.read().expect("failed to read").file.is_dir() {
            error!("file is a directory: {file_name:?}");
            return Err(STATUS_CANNOT_DELETE);
        }
        let alt_stream = context.alt_stream.read().unwrap();
        if let Some(stream) = alt_stream.as_ref() {
            stream.write().unwrap().delete_pending = info.delete_on_close();
        } else {
            context.stat.write().unwrap().delete_pending = info.delete_on_close();
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
        todo!()
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
        info: &OperationInfo<'c, 'h, Self>,
        context: &'c Self::Context,
    ) -> OperationResult<()> {
        todo!()
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
        info: &OperationInfo<'c, 'h, Self>,
        context: &'c Self::Context,
    ) -> OperationResult<()> {
        todo!()
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
        info: &OperationInfo<'c, 'h, Self>,
        context: &'c Self::Context,
    ) -> OperationResult<()> {
        todo!()
    }

    /// Locks the file for exclusive access.
    ///
    /// It will only be called if [`MountFlags::FILELOCK_USER_MODE`] was specified when mounting the
    /// volume, otherwise Dokan will take care of file locking.
    ///
    /// See [`LockFile`] for more information.
    ///
    /// [`MountFlags::FILELOCK_USER_MODE`]: crate::MountFlags::FILELOCK_USER_MODE
    /// [`LockFile`]: https://docs.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-lockfile
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

    /// Unlocks the previously locked file.
    ///
    /// It will only be called if [`MountFlags::FILELOCK_USER_MODE`] was specified when mounting the
    /// volume, otherwise Dokan will take care of file locking.
    ///
    /// See [`UnlockFile`] for more information.
    ///
    /// [`MountFlags::FILELOCK_USER_MODE`]: crate::MountFlags::FILELOCK_USER_MODE
    /// [`UnlockFile`]: https://docs.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-unlockfile
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

    /// Gets free space information about the disk.
    ///
    /// See [`GetDiskFreeSpaceEx`] for more information.
    ///
    /// [`GetDiskFreeSpaceEx`]: https://docs.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-getdiskfreespaceexw
    fn get_disk_free_space(
        &'h self,
        _info: &OperationInfo<'c, 'h, Self>,
    ) -> OperationResult<DiskSpaceInfo> {
        Err(STATUS_NOT_IMPLEMENTED)
    }

    /// Gets information about the volume and file system.
    ///
    /// See [`GetVolumeInformation`] for more information.
    ///
    /// [`GetVolumeInformation`]: https://docs.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-getvolumeinformationbyhandlew
    fn get_volume_information(
        &'h self,
        _info: &OperationInfo<'c, 'h, Self>,
    ) -> OperationResult<VolumeInfo> {
        Err(STATUS_NOT_IMPLEMENTED)
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
        _file_name: &U16CStr,
        _security_information: u32,
        _security_descriptor: winapi::um::winnt::PSECURITY_DESCRIPTOR,
        _buffer_length: u32,
        _info: &OperationInfo<'c, 'h, Self>,
        _context: &'c Self::Context,
    ) -> OperationResult<u32> {
        Err(STATUS_NOT_IMPLEMENTED)
    }

    /// Sets security information of a file.
    ///
    /// See [`SetFileSecurity`] for more information.
    ///
    /// [`SetFileSecurity`]: https://docs.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-setfilesecuritya
    fn set_file_security(
        &'h self,
        _file_name: &U16CStr,
        _security_information: u32,
        _security_descriptor: winapi::um::winnt::PSECURITY_DESCRIPTOR,
        _buffer_length: u32,
        _info: &OperationInfo<'c, 'h, Self>,
        _context: &'c Self::Context,
    ) -> OperationResult<()> {
        Err(STATUS_NOT_IMPLEMENTED)
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
        _file_name: &U16CStr,
        _fill_find_stream_data: impl FnMut(&FindStreamData) -> FillDataResult,
        _info: &OperationInfo<'c, 'h, Self>,
        _context: &'c Self::Context,
    ) -> OperationResult<()> {
        Err(STATUS_NOT_IMPLEMENTED)
    }
}
