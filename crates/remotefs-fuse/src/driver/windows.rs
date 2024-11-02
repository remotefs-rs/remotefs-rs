mod entry;
mod security;
#[cfg(test)]
mod test;

use std::hash::{Hash as _, Hasher as _};
use std::io::{Cursor, Read as _, Seek as _};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::UNIX_EPOCH;

use dashmap::mapref::one::Ref;
use dokan::{
    CreateFileInfo, FileInfo, FileSystemHandler, FileTimeOperation, FillDataError, FillDataResult,
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
use widestring::{U16CStr, U16CString, U16Str, U16String};
use winapi::shared::ntstatus::{
    self, STATUS_ACCESS_DENIED, STATUS_BUFFER_OVERFLOW, STATUS_CANNOT_DELETE,
    STATUS_DELETE_PENDING, STATUS_DIRECTORY_NOT_EMPTY, STATUS_FILE_IS_A_DIRECTORY,
    STATUS_INVALID_DEVICE_REQUEST, STATUS_INVALID_PARAMETER, STATUS_NOT_A_DIRECTORY,
    STATUS_NOT_IMPLEMENTED, STATUS_OBJECT_NAME_COLLISION, STATUS_OBJECT_NAME_NOT_FOUND,
};
use winapi::um::winnt::{self, ACCESS_MASK, FILE_CASE_PRESERVED_NAMES, FILE_CASE_SENSITIVE_SEARCH};

pub use self::entry::Stat;
use self::security::SecurityDescriptor;
use super::Driver;

const ROOT_ID: u64 = 1;

#[derive(Debug)]
#[allow(dead_code)]
struct PathInfo {
    path: PathBuf,
    file_name: U16CString,
    parent: PathBuf,
}

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
    /// Get the file index as [`u64`] number for a [`Path`]
    fn file_index(file: &File) -> u64 {
        if file.path() == Path::new("/") {
            return ROOT_ID;
        }

        let mut hasher = seahash::SeaHasher::new();
        file.path().hash(&mut hasher);
        hasher.finish()
    }

    /// Get file name from a path.
    fn file_name(path: &Path) -> U16CString {
        U16CString::from_str(path.file_name().unwrap().to_string_lossy())
            .unwrap_or_else(|_| U16CString::default())
    }

    /// Get windows attributes from a file.
    fn attributes_from_file(file: &File) -> u32 {
        let mut attributes = 0;
        if file.metadata().is_dir() {
            attributes |= winnt::FILE_ATTRIBUTE_DIRECTORY;
        }

        if file.metadata().is_file() {
            attributes |= winnt::FILE_ATTRIBUTE_NORMAL;
        }

        if file.metadata().is_symlink() {
            attributes |= winnt::FILE_ATTRIBUTE_REPARSE_POINT;
        }

        if file
            .metadata
            .mode
            .map(|m| (u32::from(m)) & 0o222 == 0)
            .unwrap_or_default()
        {
            attributes |= winnt::FILE_ATTRIBUTE_READONLY;
        }

        if file.is_hidden() {
            attributes |= winnt::FILE_ATTRIBUTE_HIDDEN;
        }

        attributes
    }

    /// Get the Stat object for a given `file_name`.
    fn stat(&self, file_name: &U16CStr) -> RemoteResult<Ref<'_, U16CString, Arc<RwLock<Stat>>>> {
        let key = file_name.to_ucstring();
        if let Some(stat) = self.file_handlers.get(&key) {
            return Ok(stat);
        }

        let path_info = Self::path_info(file_name);

        let file = self.remote(|remote| remote.stat(&path_info.path))?;

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

    /// Get the path information for a given `file_name`.
    fn path_info(file_name: &U16CStr) -> PathInfo {
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
        debug!("Read file: {:?} {} bytes at {offset}", path, buffer.len());

        match self.remote(|remote| remote.open(path)) {
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
                self.remote(|remote| remote.on_read(reader))?;

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
    fn read_tempfile(&self, path: &Path, buffer: &mut [u8], offset: u64) -> RemoteResult<usize> {
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
        self.remote(|remote| remote.open_file(path, Box::new(writer)))?;

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
        debug!(
            "Write to file: {:?} {} bytes at {offset}",
            file.path(),
            data.len(),
        );
        // write data

        let mut reader = Cursor::new(data);
        let mut writer = match self.remote(|remote| remote.create(file.path(), file.metadata())) {
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
        self.remote(|remote| remote.on_written(writer))
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

        let reader = Cursor::new(data.to_vec());
        self.remote(|remote| remote.create_file(file.path(), file.metadata(), Box::new(reader)))
            .map(|len| len as u32)
    }

    /// Append data to a file.
    fn append(&self, file: &File, data: &[u8]) -> RemoteResult<u32> {
        debug!("Append to file: {:?} {} bytes", file.path(), data.len());
        // write data

        let mut reader = Cursor::new(data);
        let mut writer = match self.remote(|remote| remote.append(file.path(), file.metadata())) {
            Ok(writer) => writer,
            Err(RemoteError {
                kind: RemoteErrorType::UnsupportedFeature,
                ..
            }) => {
                return self.append_wno_stream(file, data);
            }
            Err(err) => {
                error!("Failed to write file: {err}");
                return Err(err);
            }
        };

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
        self.remote(|remote| remote.on_written(writer))
            .map_err(|err| RemoteError::new_ex(RemoteErrorType::IoError, err.to_string()))?;

        Ok(bytes_written)
    }

    /// Append data to a file without using a stream.
    fn append_wno_stream(&self, file: &File, data: &[u8]) -> RemoteResult<u32> {
        debug!(
            "Append to file without stream: {:?} {} bytes",
            file.path(),
            data.len()
        );
        let reader = Cursor::new(data.to_vec());
        self.remote(|remote| remote.append_file(file.path(), file.metadata(), Box::new(reader)))
            .map(|len| len as u32)
    }

    /// Find files at path with the optional pattern.
    fn find_files<F>(&self, ctx: &File, pattern: Option<&U16CStr>, fill: F) -> OperationResult<()>
    where
        F: FnMut(&FindData) -> FillDataResult,
    {
        if ctx.is_file() {
            return Err(STATUS_NOT_A_DIRECTORY);
        }
        self.find_files_acc(ctx.path(), pattern, fill)?;

        Ok(())
    }

    fn find_files_acc<F>(
        &self,
        p: &Path,
        pattern: Option<&U16CStr>,
        mut acc: F,
    ) -> OperationResult<F>
    where
        F: FnMut(&FindData) -> FillDataResult,
    {
        debug!("find_files_acc({p:?}, {pattern:?})");

        // list directory
        let entries = match self.remote(|remote| remote.list_dir(p)) {
            Ok(entries) => entries,
            Err(err) => {
                error!("list_dir failed: {err}");
                return Err(STATUS_INVALID_DEVICE_REQUEST);
            }
        };

        // iter children and fill data
        let mut dirs = Vec::with_capacity(entries.len());
        for child in entries {
            // push entry
            let file_name = Self::file_name(child.path());
            if pattern
                .map(|pattern| dokan::is_name_in_expression(pattern, &file_name, false))
                .unwrap_or(true)
            {
                (acc)(&Self::find_data(&child)).or_else(Self::ignore_name_too_long)?;
            }

            if child.is_dir() {
                dirs.push(child);
            }
        }

        // iter dirs
        for dir in dirs {
            acc = self.find_files_acc(dir.path(), pattern, acc)?;
        }

        Ok(acc)
    }

    fn find_data(file: &File) -> FindData {
        FindData {
            attributes: Self::attributes_from_file(file),
            creation_time: file.metadata().created.unwrap_or(UNIX_EPOCH),
            last_access_time: file.metadata().accessed.unwrap_or(UNIX_EPOCH),
            last_write_time: file.metadata().modified.unwrap_or(UNIX_EPOCH),
            file_size: file.metadata().size,
            file_name: Self::file_name(file.path()),
        }
    }

    /// Return the error in case of a [`FillDataError`] for [`FillDataError::NameTooLong`] and [`FillDataError::BufferFull`].
    fn ignore_name_too_long(err: FillDataError) -> OperationResult<()> {
        match err {
            // Normal behavior.
            FillDataError::BufferFull => Err(STATUS_BUFFER_OVERFLOW),
            // Silently ignore this error because 1) file names passed to create_file should have been checked
            // by Windows. 2) We don't want an error on a single file to make the whole directory unreadable.
            FillDataError::NameTooLong => Ok(()),
        }
    }

    /// Execute a function on the remote filesystem.
    fn remote<F, U>(&self, f: F) -> RemoteResult<U>
    where
        F: FnOnce(&mut T) -> RemoteResult<U>,
    {
        let mut remote = self
            .remote
            .lock()
            .map_err(|_| RemoteError::new_ex(RemoteErrorType::IoError, "mutex poisoned"))?;
        f(&mut remote)
    }

    /// Try to execute a function on the alt stream.
    fn try_alt_stream<F, U>(context: &StatHandle, f: F) -> Option<OperationResult<U>>
    where
        F: FnOnce(&mut AltStream) -> OperationResult<U>,
    {
        let alt_stream = match context.alt_stream.read() {
            Err(_) => {
                error!("mutex poisoned");
                return Some(Err(STATUS_INVALID_DEVICE_REQUEST));
            }
            Ok(stream) => stream.clone(),
        };

        if let Some(alt_stream) = alt_stream.as_ref() {
            match alt_stream.write() {
                Ok(mut stream) => {
                    let ret = f(&mut stream);

                    Some(ret)
                }
                Err(_) => {
                    error!("mutex poisoned");
                    Some(Err(STATUS_INVALID_DEVICE_REQUEST))
                }
            }
        } else {
            None
        }
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
        match self.remote(|remote| remote.connect()) {
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
        match self.remote(|rem| rem.disconnect()) {
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
        info!("create_file({file_name:?}, {desired_access:?}, {file_attributes:?}, {share_access:?}, {create_disposition:?}, {create_options:?})");

        let stat = self.stat(file_name).ok();

        if create_disposition > FILE_MAXIMUM_DISPOSITION {
            error!("invalid create disposition: {create_disposition}");
            return Err(STATUS_INVALID_PARAMETER);
        }
        let delete_on_close = create_options & FILE_DELETE_ON_CLOSE > 0;
        if let Some(stat) = stat {
            let stat = stat.value();
            let read = match stat.read() {
                Ok(read) => read,
                Err(_) => {
                    error!("mutex poisoned");
                    return Err(STATUS_INVALID_DEVICE_REQUEST);
                }
            };

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
                let mut stat = match stat.write() {
                    Ok(stat) => stat,
                    Err(_) => {
                        error!("mutex poisoned");
                        return Err(STATUS_INVALID_DEVICE_REQUEST);
                    }
                };
                if let Some(stream) = stat.alt_streams.get(&stream_name).cloned() {
                    let inner_stream = match stream.read() {
                        Ok(stream) => stream,
                        Err(_) => {
                            error!("mutex poisoned");
                            return Err(STATUS_INVALID_DEVICE_REQUEST);
                        }
                    };
                    if inner_stream.delete_pending {
                        error!("delete pending: {file_name:?}");
                        return Err(STATUS_DELETE_PENDING);
                    }
                    drop(inner_stream);
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
                    };
                    Ok(CreateFileInfo {
                        context: handle,
                        is_dir: false,
                        new_file_created: false,
                    })
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
        } else if create_disposition == FILE_OPEN || create_disposition == FILE_OPEN_IF {
            if create_options & FILE_NON_DIRECTORY_FILE > 0 {
                debug!("create file: {file_name:?}");
                let path_info = Self::path_info(file_name);
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
                    let path_info = Self::path_info(file_name);

                    if let Err(err) = self
                        .remote(|remote| remote.create_dir(&path_info.path, UnixPex::from(0o755)))
                    {
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
        info!("cleanup({file_name:?}, {context:?})");
        let stat = match context.stat.read() {
            Err(_) => {
                error!("mutex poisoned");
                return;
            }
            Ok(stat) => stat,
        };

        let alt_stream_delete =
            Self::try_alt_stream(context, |alt_stream| Ok(alt_stream.delete_pending))
                .transpose()
                .unwrap_or_default()
                .unwrap_or_default();

        if context.delete_on_close
            || stat.delete_on_close
            || stat.delete_pending
            || info.delete_on_close()
            || alt_stream_delete
        {
            debug!("removing file: {file_name:?}");
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

        // check alt stream
        if let Some(res) = Self::try_alt_stream(context, |alt_stream| {
            let offset = offset as usize;
            let len = std::cmp::min(buffer.len(), alt_stream.data.len() - offset);
            buffer[0..len].copy_from_slice(&alt_stream.data[offset..offset + len]);
            Ok(len as u32)
        }) {
            return res;
        }

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
        info!("write_file({file_name:?}, {offset})");
        // read file
        let file = match context.stat.read() {
            Err(_) => {
                error!("mutex poisoned");
                return Err(STATUS_INVALID_DEVICE_REQUEST);
            }
            Ok(stat) => stat.file.clone(),
        };

        // check alt stream
        if let Some(res) = Self::try_alt_stream(context, |alt_stream| {
            let offset = if info.write_to_eof() {
                alt_stream.data.len()
            } else {
                offset as usize
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
            self.append(&file, buffer)
        } else {
            self.write(&file, buffer, offset as u64)
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
        info!("flush_file_buffers({file_name:?}, {context:?})");

        Ok(())
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
        info!("get_file_information({file_name:?}, {context:?})");

        let file = match context.stat.read() {
            Err(_) => {
                error!("mutex poisoned");
                return Err(STATUS_INVALID_DEVICE_REQUEST);
            }
            Ok(stat) => stat.file.clone(),
        };

        Ok(FileInfo {
            attributes: Self::attributes_from_file(&file),
            creation_time: file.metadata().created.unwrap_or(UNIX_EPOCH),
            last_access_time: file.metadata().accessed.unwrap_or(UNIX_EPOCH),
            last_write_time: file.metadata().modified.unwrap_or(UNIX_EPOCH),
            file_size: file.metadata().size,
            number_of_links: 1,
            file_index: Self::file_index(&file),
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
        info!("find_files({file_name:?}, {context:?})");

        let alt_stream = match context.alt_stream.read() {
            Err(_) => {
                error!("mutex poisoned");
                return Err(STATUS_INVALID_DEVICE_REQUEST);
            }
            Ok(stream) => stream.clone(),
        };
        if alt_stream.is_some() {
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
        info!("find_files_with_pattern({file_name:?}, {pattern:?}, {context:?})");

        let alt_stream = match context.alt_stream.read() {
            Err(_) => {
                error!("mutex poisoned");
                return Err(STATUS_INVALID_DEVICE_REQUEST);
            }
            Ok(stream) => stream.clone(),
        };
        if alt_stream.is_some() {
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
        info!("set_file_attributes({file_name:?}, {file_attributes:?}, {context:?})");

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
        info!("set_file_time({file_name:?}, {creation_time:?}, {last_access_time:?}, {last_write_time:?}, {context:?})");
        let file = match context.stat.read() {
            Err(_) => {
                error!("mutex poisoned");
                return Err(STATUS_INVALID_DEVICE_REQUEST);
            }
            Ok(stat) => stat.file.clone(),
        };

        let mut metadata = file.metadata().clone();

        // set metadata
        if let FileTimeOperation::SetTime(time) = creation_time {
            metadata.created = Some(time);
        }

        if let FileTimeOperation::SetTime(time) = last_access_time {
            metadata.accessed = Some(time);
        }

        if let FileTimeOperation::SetTime(time) = last_write_time {
            metadata.modified = Some(time);
        }

        if let Err(err) = self.remote(|remote| remote.setstat(file.path(), metadata)) {
            error!("setstat failed: {err}");
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
        info!("delete_file({file_name:?}, {context:?})");
        if context.stat.read().expect("failed to read").file.is_dir() {
            error!("file is a directory: {file_name:?}");
            return Err(STATUS_CANNOT_DELETE);
        }

        if let Some(res) = Self::try_alt_stream(context, |alt_stream| {
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
        info!("delete_directory({file_name:?}, {context:?})");

        if Self::try_alt_stream(context, |_alt_stream| Ok(())).is_some() {
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
        if let Some(res) = Self::try_alt_stream(context, |alt_stream| {
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
        info!("move_file({file_name:?}, {new_file_name:?}, {replace_if_existing:?}, {context:?})");

        let dest = Self::path_info(new_file_name);
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

        self.remote(|remote| remote.mov(&file.path, &dest.path))
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
        info!("set_end_of_file({file_name:?}, {offset}, {context:?})");

        Self::try_alt_stream(context, |alt_stream| {
            alt_stream.data.truncate(offset as usize);

            Ok(())
        })
        .unwrap_or(Err(STATUS_NOT_IMPLEMENTED))
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
        info!("set_allocation_size({file_name:?}, {alloc_size}, {context:?})");

        Self::try_alt_stream(context, |alt_stream: &mut AltStream| {
            alt_stream.data = vec![0; alloc_size as usize];

            Ok(())
        })
        .unwrap_or(Err(STATUS_NOT_IMPLEMENTED))
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
        info!("get_file_security({file_name:?}, {security_information:?}, {buffer_length}, {context:?})");
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
        info!("set_file_security({file_name:?}, {security_information:?}, {context:?})");

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
        info!("find_streams({file_name:?}, {context:?})");

        let file = match context.stat.read() {
            Err(_) => {
                error!("mutex poisoned");
                return Err(STATUS_INVALID_DEVICE_REQUEST);
            }
            Ok(stat) => stat.file.clone(),
        };

        fill_find_stream_data(&FindStreamData {
            size: file.metadata().size as i64,
            name: U16CString::from_str("::$DATA").unwrap(),
        })
        .or_else(Self::ignore_name_too_long)?;

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
            .or_else(Self::ignore_name_too_long)?;
        }
        Ok(())
    }

    fn get_volume_information(
        &'h self,
        _info: &OperationInfo<'c, 'h, Self>,
    ) -> OperationResult<VolumeInfo> {
        info!("get_volume_information()");

        Ok(VolumeInfo {
            name: U16CString::from_str("remotefs-fuse").expect("failed to create U16CString"),
            serial_number: 0,
            max_component_length: 255,
            fs_flags: FILE_CASE_SENSITIVE_SEARCH | FILE_CASE_PRESERVED_NAMES,
            fs_name: U16CString::from_str("DOKANY").expect("failed to create U16CString"),
        })
    }
}
