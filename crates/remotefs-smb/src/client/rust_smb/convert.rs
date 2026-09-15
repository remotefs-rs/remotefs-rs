//! Conversions between remotefs types and `smb` crate types.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use remotefs::fs::{File, FileType, Metadata, UnixPex};
use remotefs::{RemoteError, RemoteErrorType};
use smb::binrw_util::prelude::FileTime;
use smb::{
    FileAttributes, FileIdBothDirectoryInformation, FileNetworkOpenInformation, Status, UncPath,
};

use crate::utils::path::SharePath;

/// Seconds between 1601-01-01 (FILETIME epoch) and 1970-01-01 (Unix epoch).
const FILETIME_UNIX_OFFSET_SECS: u64 = 11_644_473_600;

/// Builds the UNC path of `path` under `share`.
pub(super) fn unc_for(share: &UncPath, path: &SharePath) -> UncPath {
    if path.is_root() {
        share.clone()
    } else {
        share.clone().with_path(&path.join("\\"))
    }
}

/// Converts a FILETIME into a `SystemTime`; zero means "unset".
pub(super) fn filetime_to_system_time(time: FileTime) -> Option<SystemTime> {
    if time.is_zero() {
        return None;
    }
    time.since_epoch()
        .checked_sub(Duration::from_secs(FILETIME_UNIX_OFFSET_SECS))
        .map(|since_unix| UNIX_EPOCH + since_unix)
}

/// Converts a `SystemTime` into a FILETIME (100 ns ticks since 1601).
///
/// Times before the Unix epoch saturate to the Unix epoch.
pub(super) fn system_time_to_filetime(time: SystemTime) -> FileTime {
    let since_unix = time.duration_since(UNIX_EPOCH).unwrap_or_default();
    let since_1601 = since_unix + Duration::from_secs(FILETIME_UNIX_OFFSET_SECS);
    let ticks = since_1601.as_nanos() / 100;
    FileTime::from(u64::try_from(ticks).unwrap_or(u64::MAX))
}

/// SMB2 carries no POSIX mode: synthesize one from the attributes.
pub(super) fn mode_for(attributes: FileAttributes) -> UnixPex {
    match (attributes.directory(), attributes.readonly()) {
        (true, false) => UnixPex::from(0o755),
        (true, true) => UnixPex::from(0o555),
        (false, false) => UnixPex::from(0o644),
        (false, true) => UnixPex::from(0o444),
    }
}

pub(super) fn file_type_for(attributes: FileAttributes) -> FileType {
    if attributes.directory() {
        FileType::Directory
    } else if attributes.reparse_point() {
        FileType::Symlink
    } else {
        FileType::File
    }
}

fn metadata_for(
    attributes: FileAttributes,
    size: u64,
    created: FileTime,
    modified: FileTime,
    accessed: FileTime,
) -> Metadata {
    let mut metadata = Metadata::default()
        .file_type(file_type_for(attributes))
        .mode(mode_for(attributes))
        .size(size);
    if let Some(created) = filetime_to_system_time(created) {
        metadata = metadata.created(created);
    }
    if let Some(modified) = filetime_to_system_time(modified) {
        metadata = metadata.modified(modified);
    }
    if let Some(accessed) = filetime_to_system_time(accessed) {
        metadata = metadata.accessed(accessed);
    }
    metadata
}

/// Builds a remotefs `File` from a `stat`-style query.
pub(super) fn open_info_to_file(path: PathBuf, info: &FileNetworkOpenInformation) -> File {
    File::new(
        path,
        metadata_for(
            info.file_attributes,
            info.end_of_file,
            info.creation_time,
            info.last_write_time,
            info.last_access_time,
        ),
    )
}

/// Builds a remotefs `File` from a directory listing entry.
pub(super) fn dir_entry_to_file(parent: &Path, entry: &FileIdBothDirectoryInformation) -> File {
    let mut path = parent.to_path_buf();
    path.push(entry.file_name.to_string());
    File::new(
        path,
        metadata_for(
            entry.file_attributes,
            entry.end_of_file,
            entry.creation_time,
            entry.last_write_time,
            entry.last_access_time,
        ),
    )
}

/// Wraps an `smb` error, refining `kind` from the NT status when known.
pub(super) fn smb_error(kind: RemoteErrorType, error: smb::Error) -> RemoteError {
    let kind = match &error {
        smb::Error::ReceivedErrorMessage(status, _)
        | smb::Error::UnexpectedMessageStatus(status) => status_kind(*status).unwrap_or(kind),
        smb::Error::ConnectionStopped => RemoteErrorType::ConnectionError,
        _ => kind,
    };
    RemoteError::with_source(kind, error)
}

fn status_kind(status: u32) -> Option<RemoteErrorType> {
    let mapped = match Status::try_from(status).ok()? {
        Status::ObjectNameNotFound | Status::ObjectPathNotFound => {
            RemoteErrorType::NoSuchFileOrDirectory
        }
        Status::AccessDenied => RemoteErrorType::PermissionDenied,
        Status::ObjectNameCollision => RemoteErrorType::AlreadyExists,
        Status::DirectoryNotEmpty => RemoteErrorType::DirectoryNotEmpty,
        Status::LogonFailure => RemoteErrorType::AuthenticationFailed,
        Status::NotSupported => RemoteErrorType::UnsupportedFeature,
        _ => return None,
    };
    Some(mapped)
}

#[cfg(test)]
mod test {
    use std::path::{Path, PathBuf};
    use std::str::FromStr;
    use std::time::{Duration, UNIX_EPOCH};

    use pretty_assertions::assert_eq;
    use remotefs::fs::{FileType, UnixPex};
    use remotefs::RemoteErrorType;
    use smb::binrw_util::prelude::FileTime;
    use smb::{FileAttributes, FileNetworkOpenInformation, Status, UncPath};

    use super::*;
    use crate::utils::path::SharePath;

    #[test]
    fn should_build_unc_from_share_path() {
        let share = UncPath::from_str(r"\\localhost\temp").unwrap();
        let root = SharePath::parse(Path::new("/")).unwrap();
        assert_eq!(unc_for(&share, &root).to_string(), r"\\localhost\temp");
        let nested = SharePath::parse(Path::new("/cargo-test/a.txt")).unwrap();
        assert_eq!(
            unc_for(&share, &nested).to_string(),
            r"\\localhost\temp\cargo-test\a.txt"
        );
    }

    #[test]
    fn should_convert_filetime_both_ways() {
        assert_eq!(filetime_to_system_time(FileTime::ZERO), None);
        let one_second_after_unix_epoch = FileTime::from(116_444_736_010_000_000u64);
        let system_time = UNIX_EPOCH + Duration::from_secs(1);
        assert_eq!(
            filetime_to_system_time(one_second_after_unix_epoch),
            Some(system_time)
        );
        assert_eq!(
            filetime_to_system_time(system_time_to_filetime(system_time)),
            Some(system_time)
        );
    }

    #[test]
    fn should_map_attributes_to_mode_and_type() {
        let dir = FileAttributes::new().with_directory(true);
        assert_eq!(mode_for(dir), UnixPex::from(0o755));
        assert_eq!(file_type_for(dir), FileType::Directory);
        let ro = FileAttributes::new().with_readonly(true);
        assert_eq!(mode_for(ro), UnixPex::from(0o444));
        assert_eq!(file_type_for(ro), FileType::File);
        let link = FileAttributes::new().with_reparse_point(true);
        assert_eq!(file_type_for(link), FileType::Symlink);
    }

    #[test]
    fn should_map_open_info_to_file() {
        let info = FileNetworkOpenInformation {
            creation_time: FileTime::from(116_444_736_010_000_000u64),
            last_access_time: FileTime::ZERO,
            last_write_time: FileTime::from(116_444_736_020_000_000u64),
            change_time: FileTime::ZERO,
            allocation_size: 4096,
            end_of_file: 10,
            file_attributes: FileAttributes::new(),
        };
        let file = open_info_to_file(PathBuf::from("/cargo-test/a.txt"), &info);
        assert_eq!(file.name(), "a.txt");
        assert_eq!(file.path(), Path::new("/cargo-test/a.txt"));
        assert_eq!(file.metadata().size, Some(10));
        assert_eq!(file.metadata().file_type, FileType::File);
        assert_eq!(file.metadata().accessed, None);
        assert_eq!(
            file.metadata().modified,
            Some(UNIX_EPOCH + Duration::from_secs(2))
        );
        assert_eq!(file.metadata().mode, Some(UnixPex::from(0o644)));
        assert_eq!(file.metadata().uid, None);
    }

    #[test]
    fn should_map_nt_status_to_error_kinds() {
        for (status, expected) in [
            (
                Status::ObjectNameNotFound,
                RemoteErrorType::NoSuchFileOrDirectory,
            ),
            (
                Status::ObjectPathNotFound,
                RemoteErrorType::NoSuchFileOrDirectory,
            ),
            (Status::AccessDenied, RemoteErrorType::PermissionDenied),
            (Status::ObjectNameCollision, RemoteErrorType::AlreadyExists),
            (
                Status::DirectoryNotEmpty,
                RemoteErrorType::DirectoryNotEmpty,
            ),
            (Status::LogonFailure, RemoteErrorType::AuthenticationFailed),
            (Status::NotSupported, RemoteErrorType::UnsupportedFeature),
        ] {
            let error = smb_error(
                RemoteErrorType::ProtocolError,
                smb::Error::UnexpectedMessageStatus(status as u32),
            );
            assert_eq!(error.kind(), expected, "{status:?}");
            assert!(std::error::Error::source(&error).is_some());
        }
        let fallback = smb_error(RemoteErrorType::StatFailed, smb::Error::Other("boom"));
        assert_eq!(fallback.kind(), RemoteErrorType::StatFailed);
        assert!(fallback.to_string().contains("boom"));
        let stopped = smb_error(RemoteErrorType::StatFailed, smb::Error::ConnectionStopped);
        assert_eq!(stopped.kind(), RemoteErrorType::ConnectionError);
    }
}
