//! Bookkeeping and pure conversions shared by the Unix filesystem drivers.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(feature = "tokio")]
use fuser::Request;
use fuser::{FileAttr, FileType, TimeOrNow};
use libc::c_int;
use nix::fcntl::OFlag;
use nix::sys::stat::SFlag;
use nix::unistd::AccessFlags;
use remotefs::File;
use remotefs::fs::SetMetadata;

use super::file_handle::FileHandlersDb;
use super::inode::{Inode, InodeDb};
use crate::MountOption;

pub(crate) const BLOCK_SIZE: usize = 512;
pub(crate) const FMODE_EXEC: c_int = 0x20;
pub(crate) const ROOT_UID: u32 = 0;

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use pretty_assertions::assert_eq;
    use remotefs::File;
    use remotefs::fs::Metadata;

    use super::convert_file;

    #[test]
    fn test_convert_file_should_apply_uid_and_gid_overrides() {
        let file = File::new(
            PathBuf::from("/remote.txt"),
            Metadata::default().uid(1002).gid(1003),
        );

        let attrs = convert_file(&file, 7, Some(1000), Some(1001));

        assert_eq!(attrs.uid, 1000);
        assert_eq!(attrs.gid, 1001);
    }

    #[test]
    fn test_convert_file_should_apply_uid_override_independently() {
        let file = File::new(
            PathBuf::from("/remote.txt"),
            Metadata::default().uid(1002).gid(1003),
        );

        let attrs = convert_file(&file, 7, Some(1000), None);

        assert_eq!(attrs.uid, 1000);
        assert_eq!(attrs.gid, 1003);
    }

    #[test]
    fn test_convert_file_should_apply_gid_override_independently() {
        let file = File::new(
            PathBuf::from("/remote.txt"),
            Metadata::default().uid(1002).gid(1003),
        );

        let attrs = convert_file(&file, 7, None, Some(1001));

        assert_eq!(attrs.uid, 1002);
        assert_eq!(attrs.gid, 1001);
    }

    #[test]
    fn test_convert_file_should_preserve_remote_ids_without_overrides() {
        let file = File::new(
            PathBuf::from("/remote.txt"),
            Metadata::default().uid(1002).gid(1003),
        );

        let attrs = convert_file(&file, 7, None, None);

        assert_eq!(attrs.uid, 1002);
        assert_eq!(attrs.gid, 1003);
    }

    #[test]
    fn test_convert_file_should_default_missing_ids_to_zero() {
        let file = File::new(PathBuf::from("/remote.txt"), Metadata::default());

        let attrs = convert_file(&file, 7, None, None);

        assert_eq!(attrs.uid, 0);
        assert_eq!(attrs.gid, 0);
    }
}

/// Convert a remote file type to a FUSE file type.
pub(crate) fn convert_remote_filetype(filetype: remotefs::fs::FileType) -> FileType {
    match filetype {
        remotefs::fs::FileType::Directory => FileType::Directory,
        remotefs::fs::FileType::File => FileType::RegularFile,
        remotefs::fs::FileType::Symlink => FileType::Symlink,
        _ => FileType::RegularFile,
    }
}

/// Convert a remote file to FUSE attributes using `inode` and ownership overrides.
pub(crate) fn convert_file(
    value: &File,
    inode: Inode,
    uid: Option<u32>,
    gid: Option<u32>,
) -> FileAttr {
    let size = value.metadata().size.unwrap_or(0);
    FileAttr {
        ino: fuser::INodeNo(inode),
        size,
        blocks: size.div_ceil(BLOCK_SIZE as u64),
        atime: value.metadata().accessed.unwrap_or(UNIX_EPOCH),
        mtime: value.metadata().modified.unwrap_or(UNIX_EPOCH),
        ctime: value.metadata().created.unwrap_or(UNIX_EPOCH),
        crtime: UNIX_EPOCH,
        kind: convert_remote_filetype(value.metadata().file_type),
        perm: value
            .metadata()
            .mode
            .map(|mode| u32::from(mode) as u16)
            .unwrap_or(0o777),
        nlink: 0,
        uid: uid.or(value.metadata().uid).unwrap_or(0),
        gid: gid.or(value.metadata().gid).unwrap_or(0),
        rdev: 0,
        blksize: BLOCK_SIZE as u32,
        flags: 0,
    }
}

/// Convert a FUSE time value to a system time.
pub(crate) fn time_or_now(time: TimeOrNow) -> SystemTime {
    match time {
        TimeOrNow::SpecificTime(time) => time,
        TimeOrNow::Now => SystemTime::now(),
    }
}

/// Convert a POSIX mode type to a FUSE file kind.
pub(crate) fn as_file_kind(mut mode: SFlag) -> Option<FileType> {
    mode &= SFlag::S_IFMT;

    if mode == SFlag::S_IFREG {
        Some(FileType::RegularFile)
    } else if mode == SFlag::S_IFLNK {
        Some(FileType::Symlink)
    } else if mode == SFlag::S_IFDIR {
        Some(FileType::Directory)
    } else {
        None
    }
}

/// Request identity copied out of a [`fuser::Request`].
#[cfg(feature = "tokio")]
#[derive(Debug, Clone, Copy)]
pub(crate) struct RequestMeta {
    pub uid: u32,
    pub gid: u32,
    pub pid: u32,
}

#[cfg(feature = "tokio")]
impl From<&Request> for RequestMeta {
    fn from(req: &Request) -> Self {
        Self {
            uid: req.uid(),
            gid: req.gid(),
            pid: req.pid(),
        }
    }
}

/// Access mode requested by an `open`-family call.
#[derive(Debug, Clone, Copy)]
pub(crate) struct OpenMode {
    pub access_mask: AccessFlags,
    pub read: bool,
    pub write: bool,
}

/// Parse `open(2)` flags into access permissions.
pub(crate) fn parse_open_flags(flags: i32) -> Result<OpenMode, fuser::Errno> {
    let execute = flags & FMODE_EXEC != 0;
    let flags = OFlag::from_bits_truncate(flags);
    match flags & OFlag::O_ACCMODE {
        OFlag::O_RDONLY => {
            if flags.intersects(OFlag::O_TRUNC) {
                return Err(fuser::Errno::EACCES);
            }
            let access_mask = if execute {
                AccessFlags::X_OK
            } else {
                AccessFlags::R_OK
            };
            Ok(OpenMode {
                access_mask,
                read: true,
                write: false,
            })
        }
        OFlag::O_WRONLY => Ok(OpenMode {
            access_mask: AccessFlags::W_OK,
            read: false,
            write: true,
        }),
        OFlag::O_RDWR => Ok(OpenMode {
            access_mask: AccessFlags::R_OK | AccessFlags::W_OK,
            read: true,
            write: true,
        }),
        _ => Err(fuser::Errno::EINVAL),
    }
}

/// Parse `opendir` flags into access permissions.
pub(crate) fn parse_opendir_flags(flags: i32) -> Result<OpenMode, fuser::Errno> {
    let flags = OFlag::from_bits_truncate(flags);
    match flags & OFlag::O_ACCMODE {
        OFlag::O_RDONLY if flags.intersects(OFlag::O_TRUNC) => Err(fuser::Errno::EACCES),
        OFlag::O_RDONLY => Ok(OpenMode {
            access_mask: AccessFlags::R_OK,
            read: true,
            write: false,
        }),
        OFlag::O_WRONLY => Ok(OpenMode {
            access_mask: AccessFlags::W_OK,
            read: false,
            write: true,
        }),
        OFlag::O_RDWR => Ok(OpenMode {
            access_mask: AccessFlags::R_OK | AccessFlags::W_OK,
            read: true,
            write: true,
        }),
        _ => Err(fuser::Errno::EINVAL),
    }
}

/// Parse `create` flags into read and write permissions.
pub(crate) fn parse_create_flags(flags: i32) -> Result<(bool, bool), fuser::Errno> {
    match OFlag::from_bits_truncate(flags) & OFlag::O_ACCMODE {
        OFlag::O_RDONLY => Ok((true, false)),
        OFlag::O_WRONLY => Ok((false, true)),
        OFlag::O_RDWR => Ok((true, true)),
        _ => Err(fuser::Errno::EINVAL),
    }
}

/// Build metadata changes for `setattr`, or `None` when unchanged.
pub(crate) fn setattr_changes(
    mode: Option<u32>,
    uid: Option<u32>,
    gid: Option<u32>,
    atime: Option<TimeOrNow>,
    mtime: Option<TimeOrNow>,
) -> Option<SetMetadata> {
    let mut changes = SetMetadata::default();
    let mut any = false;
    if let Some(mode) = mode {
        changes = changes.mode(mode.into());
        any = true;
    }
    if let Some(uid) = uid {
        changes = changes.uid(uid);
        any = true;
    }
    if let Some(gid) = gid {
        changes = changes.gid(gid);
        any = true;
    }
    if let Some(atime) = atime {
        changes = changes.accessed(time_or_now(atime));
        any = true;
    }
    if let Some(mtime) = mtime {
        changes = changes.modified(time_or_now(mtime));
        any = true;
    }
    any.then_some(changes)
}

/// Unix bookkeeping shared by synchronous and asynchronous drivers.
#[derive(Debug)]
pub(crate) struct UnixState<S> {
    pub database: InodeDb,
    pub file_handlers: FileHandlersDb<S>,
    pub options: Vec<MountOption>,
}

impl<S> UnixState<S> {
    pub(crate) fn new(options: Vec<MountOption>) -> Self {
        Self {
            database: InodeDb::load(),
            file_handlers: FileHandlersDb::default(),
            options,
        }
    }

    /// Return the path registered for `inode`, if any.
    pub(crate) fn inode_path(&self, inode: Inode) -> Option<PathBuf> {
        self.database.get(inode).map(Path::to_path_buf)
    }

    /// Look up a child path and register its inode.
    pub(crate) fn lookup_name(&mut self, parent: Inode, name: &OsStr) -> Option<PathBuf> {
        let parent_path = self.database.get(parent)?;
        let path = parent_path.join(name);
        self.database.inode_for(&path);

        debug!("lookup_name() called with {parent:?} {name:?} -> {path:?}");

        Some(path)
    }

    /// Check whether a user may access a file with `access_mask`.
    pub(crate) fn check_access(
        &self,
        file: &File,
        uid: u32,
        gid: u32,
        mut access_mask: AccessFlags,
    ) -> bool {
        debug!(
            "Checking access for file: {:?} {:?}; UID: {uid}; GID: {gid} access_mask: {access_mask:?}",
            file.path(),
            file.metadata()
        );
        if access_mask == AccessFlags::F_OK {
            return true;
        }

        let file_mode = file
            .metadata()
            .mode
            .map(u32::from)
            .unwrap_or_else(|| self.default_mode()) as i32;

        debug!("file mode for {}: {file_mode:o}", file.path().display());

        if uid == ROOT_UID {
            debug!("Root access to file: {}", file.path().display());
            access_mask &= AccessFlags::X_OK;
            let mut access_mask = access_mask.bits();
            access_mask -= access_mask & (file_mode >> 6);
            access_mask -= access_mask & (file_mode >> 3);
            access_mask -= access_mask & file_mode;
            return access_mask == 0;
        }

        let mut access_mask = access_mask.bits();

        let file_uid = self
            .uid()
            .unwrap_or_else(|| file.metadata().uid.unwrap_or_default());
        let file_gid = self
            .gid()
            .unwrap_or_else(|| file.metadata().gid.unwrap_or_default());

        if uid == file_uid {
            access_mask -= access_mask & (file_mode >> 6);
            debug!("UID access to file: {}", file.path().display());
        } else if gid == file_gid {
            access_mask -= access_mask & (file_mode >> 3);
            debug!("GID access to file: {}", file.path().display());
        } else {
            debug!("Other access to file: {}", file.path().display());
            access_mask -= access_mask & file_mode;
        }

        debug!("Access mask: {access_mask}");

        access_mask == 0
    }

    /// Return an explicitly configured UID override.
    pub(crate) fn uid(&self) -> Option<u32> {
        self.options.iter().find_map(|opt| match opt {
            MountOption::Uid(uid) => Some(*uid),
            _ => None,
        })
    }

    /// Return an explicitly configured GID override.
    pub(crate) fn gid(&self) -> Option<u32> {
        self.options.iter().find_map(|opt| match opt {
            MountOption::Gid(gid) => Some(*gid),
            _ => None,
        })
    }

    /// Return the configured default mode, or `0755`.
    pub(crate) fn default_mode(&self) -> u32 {
        self.options
            .iter()
            .find_map(|opt| match opt {
                MountOption::DefaultMode(mode) => Some(*mode),
                _ => None,
            })
            .unwrap_or(0o755)
    }
}
