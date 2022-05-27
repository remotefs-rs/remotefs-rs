//! # smb utils
//!
//! SMB protocol utilities

use libc::mode_t;
use pavao::SmbStat;
use remotefs::{
    fs::{FileType, Metadata, UnixPex},
    File,
};
use std::path::PathBuf;

/// Convert `SmbStat` to `File`
pub fn smbstat_to_file<S: AsRef<str>>(uri: S, stat: SmbStat) -> File {
    let mode = mode_t::from(stat.mode);
    File {
        path: PathBuf::from(uri.as_ref()),
        metadata: Metadata::default()
            .accessed(stat.accessed)
            .created(stat.created)
            .file_type(get_file_type_from_stat(&stat))
            .gid(stat.gid)
            .mode(UnixPex::from(mode as u32))
            .modified(stat.modified)
            .size(stat.size)
            .uid(stat.uid),
    }
}

fn get_file_type_from_stat(stat: &SmbStat) -> FileType {
    match stat.mode {
        mode if mode.is_dir() => FileType::Directory,
        mode if mode.is_symlink() => FileType::Symlink,
        _ => FileType::File,
    }
}
