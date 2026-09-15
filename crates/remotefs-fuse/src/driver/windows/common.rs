//! Pure helpers shared by synchronous and asynchronous Dokany drivers.

use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::UNIX_EPOCH;

use dokan::{FillDataError, FillDataResult, FindData, OperationResult};
use dokan_sys::win32::{
    FILE_CREATE, FILE_DELETE_ON_CLOSE, FILE_DIRECTORY_FILE, FILE_MAXIMUM_DISPOSITION,
    FILE_NON_DIRECTORY_FILE, FILE_OPEN, FILE_OPEN_IF, FILE_OVERWRITE, FILE_OVERWRITE_IF,
    FILE_SUPERSEDE,
};
use path_slash::PathBufExt;
use remotefs::{File, RemoteError, RemoteErrorType, RemoteResult};
use widestring::{U16CStr, U16CString};
use winapi::shared::ntstatus::{
    self, STATUS_ACCESS_DENIED, STATUS_BUFFER_OVERFLOW, STATUS_CANNOT_DELETE,
    STATUS_DELETE_PENDING, STATUS_FILE_IS_A_DIRECTORY, STATUS_INVALID_DEVICE_REQUEST,
    STATUS_INVALID_PARAMETER, STATUS_NOT_A_DIRECTORY, STATUS_OBJECT_NAME_COLLISION,
    STATUS_OBJECT_NAME_NOT_FOUND,
};
use winapi::um::winnt::{self, ACCESS_MASK};

use super::entry::{EntryName, Stat, StatHandle};
use super::security::SecurityDescriptor;

/// Normalized path for a Dokany request.
#[derive(Debug)]
pub(crate) struct PathInfo {
    pub path: PathBuf,
}

/// Return the stable index Dokany uses for a remote file.
pub(crate) fn file_index(file: &File) -> u64 {
    if file.path() == Path::new("/") {
        return super::ROOT_ID;
    }

    let mut hasher = seahash::SeaHasher::new();
    file.path().hash(&mut hasher);
    hasher.finish()
}

/// Convert a Dokany signed byte offset into the non-negative remote offset it represents.
pub(crate) fn nonnegative_offset(value: i64) -> OperationResult<u64> {
    u64::try_from(value).map_err(|_| STATUS_INVALID_PARAMETER)
}

/// Return a file name as a NUL-terminated UTF-16 string.
pub(crate) fn file_name(path: &Path) -> U16CString {
    let Some(file_name) = path.file_name() else {
        return U16CString::default();
    };

    U16CString::from_str(file_name.to_string_lossy()).unwrap_or_default()
}

/// Convert remote metadata into Windows file attributes.
pub(crate) fn attributes_from_file(file: &File) -> u32 {
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

    if is_readonly(file) {
        attributes |= winnt::FILE_ATTRIBUTE_READONLY;
    }

    if file.is_hidden() {
        attributes |= winnt::FILE_ATTRIBUTE_HIDDEN;
    }

    attributes
}

/// Return whether the remote mode denies all write access.
pub(crate) fn is_readonly(file: &File) -> bool {
    file.metadata()
        .mode
        .map(|mode| u32::from(mode) & 0o222 == 0)
        .unwrap_or_default()
}

/// Convert a Dokany name to a normalized remote path.
pub(crate) fn path_info(file_name: &U16CStr) -> PathInfo {
    let path = PathBuf::from(file_name.to_string_lossy());
    let slash_path = PathBuf::from(path.to_slash_lossy().to_string());
    debug!("PathInfo: {path:?} -> {slash_path:?}");

    PathInfo { path: slash_path }
}

/// Convert a remote file into Dokany directory enumeration data.
pub(crate) fn find_data(file: &File, file_name: U16CString) -> FindData {
    FindData {
        attributes: attributes_from_file(file),
        creation_time: file.metadata().created.unwrap_or(UNIX_EPOCH),
        last_access_time: file.metadata().accessed.unwrap_or(UNIX_EPOCH),
        last_write_time: file.metadata().modified.unwrap_or(UNIX_EPOCH),
        file_size: file.metadata().size.unwrap_or(0),
        file_name,
    }
}

/// Ignore overlong names while preserving the normal Dokany buffer error.
pub(crate) fn ignore_name_too_long(err: FillDataError) -> OperationResult<()> {
    match err {
        FillDataError::BufferFull => Err(STATUS_BUFFER_OVERFLOW),
        FillDataError::NameTooLong => Ok(()),
    }
}

/// Fill a Dokany directory enumeration from remote entries.
pub(crate) fn fill_entries(
    entries: Vec<File>,
    pattern: Option<&U16CStr>,
    mut fill: impl FnMut(&FindData) -> FillDataResult,
) -> OperationResult<()> {
    for child in entries {
        let file_name = file_name(child.path());
        if pattern
            .map(|pattern| dokan::is_name_in_expression(pattern, &file_name, false))
            .unwrap_or(true)
        {
            fill(&find_data(&child, file_name)).or_else(ignore_name_too_long)?;
        }
    }
    Ok(())
}

/// Run an alternate-stream operation when `context` addresses one.
pub(crate) fn try_alt_stream<P, F, U>(context: &StatHandle<P>, f: F) -> Option<OperationResult<U>>
where
    F: FnOnce(&mut super::AltStream) -> OperationResult<U>,
{
    let use_alt_stream = match context.stat.read() {
        Ok(stat) => stat.file.path().to_string_lossy().contains(':'),
        Err(_) => {
            error!("mutex poisoned");
            return Some(Err(STATUS_INVALID_DEVICE_REQUEST));
        }
    };
    if !use_alt_stream {
        return None;
    }

    let alt_stream = match context.alt_stream.read() {
        Ok(stream) => stream.clone(),
        Err(_) => {
            error!("mutex poisoned");
            return Some(Err(STATUS_INVALID_DEVICE_REQUEST));
        }
    };
    let alt_stream = alt_stream?;
    match alt_stream.write() {
        Ok(mut stream) => Some(f(&mut stream)),
        Err(_) => {
            error!("mutex poisoned");
            Some(Err(STATUS_INVALID_DEVICE_REQUEST))
        }
    }
}

/// Create cached Dokany state for a newly statted remote file.
pub(crate) fn new_stat(file: File) -> RemoteResult<Arc<RwLock<Stat>>> {
    let security = SecurityDescriptor::new_default()
        .map_err(|_| RemoteError::new(RemoteErrorType::ProtocolError))?;
    Ok(Arc::new(RwLock::new(Stat::new(file, security))))
}

/// Result of deciding how Dokany should handle an existing or missing path.
#[derive(Debug)]
pub(crate) enum CreatePlan {
    Open {
        stat: Arc<RwLock<Stat>>,
        alt_stream: Option<Arc<RwLock<super::AltStream>>>,
        is_dir: bool,
        new_file_created: bool,
    },
    CreateFile,
    CreateDirectory,
}

/// Decide the non-I/O portion of a Dokany create request.
pub(crate) fn plan_create(
    existing: Option<&Arc<RwLock<Stat>>>,
    file_name: &U16CStr,
    desired_access: ACCESS_MASK,
    create_disposition: u32,
    create_options: u32,
) -> OperationResult<CreatePlan> {
    if create_disposition > FILE_MAXIMUM_DISPOSITION {
        error!("invalid create disposition: {create_disposition}");
        return Err(STATUS_INVALID_PARAMETER);
    }
    let delete_on_close = create_options & FILE_DELETE_ON_CLOSE > 0;
    if let Some(stat) = existing {
        let read = match stat.read() {
            Ok(read) => read,
            Err(_) => {
                error!("mutex poisoned");
                return Err(STATUS_INVALID_DEVICE_REQUEST);
            }
        };
        let is_readonly = is_readonly(&read.file);
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
        drop(read);

        let stream_name = EntryName(file_name.to_ustring());
        let alt_stream = {
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
                    FILE_SUPERSEDE | FILE_OVERWRITE | FILE_OVERWRITE_IF
                        if create_disposition != FILE_SUPERSEDE && is_readonly =>
                    {
                        error!("file {file_name:?} is readonly");
                        return Err(STATUS_ACCESS_DENIED);
                    }
                    FILE_CREATE => {
                        error!("alt stream already exists: {file_name:?}");
                        return Err(ntstatus::STATUS_OBJECT_NAME_COLLISION);
                    }
                    _ => {}
                }
                Some((stream, false))
            } else {
                if is_readonly {
                    error!("file {file_name:?} is readonly");
                    return Err(STATUS_ACCESS_DENIED);
                }
                let stream = Arc::new(RwLock::new(super::AltStream::new()));
                stat.alt_streams.insert(stream_name, Arc::clone(&stream));
                Some((stream, true))
            }
        };

        if let Some((alt_stream, new_file_created)) = alt_stream {
            return Ok(CreatePlan::Open {
                stat: Arc::clone(stat),
                alt_stream: Some(alt_stream),
                is_dir: false,
                new_file_created,
            });
        }

        let is_file = stat
            .read()
            .ok()
            .map(|stat| stat.file.is_file())
            .unwrap_or_default();
        if is_file {
            if create_options & FILE_DIRECTORY_FILE > 0 {
                error!("file is not a directory: {file_name:?}");
                return Err(STATUS_NOT_A_DIRECTORY);
            }
            match create_disposition {
                FILE_SUPERSEDE | FILE_OVERWRITE | FILE_OVERWRITE_IF
                    if create_disposition != FILE_SUPERSEDE && is_readonly =>
                {
                    error!("file {file_name:?} is readonly");
                    return Err(STATUS_ACCESS_DENIED);
                }
                FILE_CREATE => {
                    error!("file already exists: {file_name:?}");
                    return Err(STATUS_OBJECT_NAME_COLLISION);
                }
                _ => {}
            }
            return Ok(CreatePlan::Open {
                stat: Arc::clone(stat),
                alt_stream: None,
                is_dir: false,
                new_file_created: false,
            });
        }

        if create_options & FILE_NON_DIRECTORY_FILE > 0 {
            error!("file is a directory: {file_name:?}");
            return Err(STATUS_FILE_IS_A_DIRECTORY);
        }
        match create_disposition {
            FILE_OPEN | FILE_OPEN_IF => Ok(CreatePlan::Open {
                stat: Arc::clone(stat),
                alt_stream: None,
                is_dir: true,
                new_file_created: false,
            }),
            FILE_CREATE => {
                error!("directory already exists: {file_name:?}");
                Err(STATUS_OBJECT_NAME_COLLISION)
            }
            _ => {
                error!("invalid create disposition: {create_disposition}");
                Err(STATUS_INVALID_PARAMETER)
            }
        }
    } else if create_disposition == FILE_CREATE || create_disposition == FILE_OPEN_IF {
        if create_options & FILE_NON_DIRECTORY_FILE > 0 {
            Ok(CreatePlan::CreateFile)
        } else {
            Ok(CreatePlan::CreateDirectory)
        }
    } else if create_disposition == FILE_OPEN {
        error!("tried to open non existing file: {file_name:?}");
        Err(STATUS_OBJECT_NAME_NOT_FOUND)
    } else {
        error!("invalid create disposition: {create_disposition}");
        Err(STATUS_INVALID_PARAMETER)
    }
}
