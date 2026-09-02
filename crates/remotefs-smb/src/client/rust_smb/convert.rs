//! Conversions between remotefs types and `smb` crate types.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use remotefs::fs::{File, FileType, Metadata, UnixPex};
use remotefs::{RemoteError, RemoteErrorType};
use smb::binrw_util::prelude::FileTime;
use smb::{FileAttributes, FileIdBothDirectoryInformation, FileNetworkOpenInformation, UncPath};

use crate::utils::path as path_utils;

/// Seconds between 1601-01-01 (FILETIME epoch) and 1970-01-01 (Unix epoch).
const FILETIME_UNIX_OFFSET_SECS: u64 = 11_644_473_600;

/// Absolutizes `path` against `wrkdir` and returns it together with the
/// share-relative name: `/`-separated, no leading or trailing slash, empty
/// for the share root.
pub(super) fn relative_unc_path(wrkdir: &Path, path: &Path) -> (PathBuf, String) {
    let absolute = path_utils::absolutize(wrkdir, path);
    let relative = absolute
        .to_string_lossy()
        .trim_matches(['/', '\\'])
        .replace('\\', "/");
    (absolute, relative)
}

/// Builds the UNC path of `relative` under `share`.
pub(super) fn unc_for(share: &UncPath, relative: &str) -> UncPath {
    if relative.is_empty() {
        share.clone()
    } else {
        share.clone().with_path(&to_backslash_path(relative))
    }
}

/// Converts a `/`-separated relative path into the `\`-separated form used
/// by `FileRenameInformation`.
pub(super) fn to_backslash_path(relative: &str) -> String {
    relative.replace('/', "\\")
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
    File {
        path,
        metadata: metadata_for(
            info.file_attributes,
            info.end_of_file,
            info.creation_time,
            info.last_write_time,
            info.last_access_time,
        ),
    }
}

/// Builds a remotefs `File` from a directory listing entry.
pub(super) fn dir_entry_to_file(parent: &Path, entry: &FileIdBothDirectoryInformation) -> File {
    let mut path = parent.to_path_buf();
    path.push(entry.file_name.to_string());
    File {
        path,
        metadata: metadata_for(
            entry.file_attributes,
            entry.end_of_file,
            entry.creation_time,
            entry.last_write_time,
            entry.last_access_time,
        ),
    }
}

/// Wraps an `smb` error into a `RemoteError` of the given kind.
pub(super) fn smb_error(kind: RemoteErrorType, error: smb::Error) -> RemoteError {
    RemoteError::new_ex(kind, error)
}

#[cfg(test)]
mod test {
    use std::path::{Path, PathBuf};
    use std::str::FromStr;
    use std::time::{Duration, UNIX_EPOCH};

    use pretty_assertions::assert_eq;
    use remotefs::fs::{FileType, UnixPex};
    use smb::binrw_util::prelude::FileTime;
    use smb::{FileAttributes, FileNetworkOpenInformation, UncPath};

    use super::*;

    #[test]
    fn should_build_relative_unc_paths() {
        let wrkdir = Path::new("/");
        assert_eq!(
            relative_unc_path(wrkdir, Path::new("/cargo-test/a.txt")),
            (
                PathBuf::from("/cargo-test/a.txt"),
                "cargo-test/a.txt".to_string()
            )
        );
        assert_eq!(
            relative_unc_path(wrkdir, Path::new("/cargo-test/")),
            (PathBuf::from("/cargo-test/"), "cargo-test".to_string())
        );
        assert_eq!(
            relative_unc_path(wrkdir, Path::new("/")),
            (PathBuf::from("/"), String::new())
        );
        assert_eq!(
            relative_unc_path(Path::new("/cargo-test"), Path::new("a.txt")),
            (
                PathBuf::from("/cargo-test/a.txt"),
                "cargo-test/a.txt".to_string()
            )
        );
    }

    #[test]
    fn should_build_unc_from_relative() {
        let share = UncPath::from_str(r"\\localhost\temp").unwrap();
        assert_eq!(unc_for(&share, "").to_string(), r"\\localhost\temp");
        assert_eq!(
            unc_for(&share, "cargo-test/a.txt").to_string(),
            r"\\localhost\temp\cargo-test\a.txt"
        );
    }

    #[test]
    fn should_convert_slashes() {
        assert_eq!(to_backslash_path("cargo-test/b.txt"), r"cargo-test\b.txt");
    }

    #[test]
    fn should_convert_filetime() {
        assert_eq!(filetime_to_system_time(FileTime::ZERO), None);
        // 1970-01-01T00:00:01Z in 100 ns ticks since 1601.
        let one_second_after_unix_epoch = FileTime::from(116_444_736_010_000_000u64);
        assert_eq!(
            filetime_to_system_time(one_second_after_unix_epoch),
            Some(UNIX_EPOCH + Duration::from_secs(1))
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
        assert_eq!(file.metadata().size, 10);
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
    fn should_wrap_smb_error() {
        let err = smb_error(RemoteErrorType::StatFailed, smb::Error::Other("boom"));
        assert_eq!(err.kind, RemoteErrorType::StatFailed);
        assert!(err.msg.as_deref().unwrap().contains("boom"));
    }
}
