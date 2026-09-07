//! What a remote host reports about one of its entries.

use std::fs::Metadata as StdMetadata;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use super::{FileType, UnixPex};

/// The attributes of a remote entry, as far as the protocol could report them.
///
/// Every field a protocol may be unable to answer is an [`Option`]: an S3 bucket
/// has no owner, a Windows share has no [`UnixPex`], and most FTP servers do not
/// report a creation time. A client leaves such a field empty rather than filling
/// it with a plausible value, so `None` means "the protocol did not say", never
/// "zero".
///
/// The builder methods each set one field and return `self`, so metadata can be
/// assembled in one expression. They are also what a caller passes to
/// [`crate::RemoteFs::setstat`], and what
/// [`crate::RemoteFs::create`]/[`crate::RemoteFs::append`] read for the transfer
/// size that SCP requires up front.
///
/// # Examples
///
/// ```
/// use std::time::SystemTime;
///
/// use remotefs::fs::{FileType, Metadata};
///
/// let metadata = Metadata::default()
///     .file_type(FileType::Directory)
///     .modified(SystemTime::UNIX_EPOCH)
///     .uid(1000)
///     .gid(1000);
///
/// assert!(metadata.is_dir());
/// assert_eq!(metadata.size, 0);
/// assert!(metadata.created.is_none());
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Metadata {
    /// Last access time, when the protocol reports one.
    pub accessed: Option<SystemTime>,
    /// Creation time, when the protocol reports one.
    pub created: Option<SystemTime>,
    /// Owning group id, when the protocol has the notion.
    pub gid: Option<u32>,
    /// POSIX permissions, when the protocol has the notion.
    pub mode: Option<UnixPex>,
    /// Last modification time, when the protocol reports one.
    pub modified: Option<SystemTime>,
    /// Size in bytes; zero for entries that have no size.
    pub size: u64,
    /// For a symbolic link, the path it points at.
    pub symlink: Option<PathBuf>,
    /// Whether the entry is a directory, a file, or a link.
    pub file_type: FileType,
    /// Owning user id, when the protocol has the notion.
    pub uid: Option<u32>,
}

impl Default for Metadata {
    fn default() -> Self {
        Self {
            accessed: None,
            created: None,
            gid: None,
            mode: None,
            modified: None,
            size: 0,
            symlink: None,
            file_type: FileType::File,
            uid: None,
        }
    }
}

impl Metadata {
    /// Set the last access time, consuming and returning `self`.
    pub fn accessed(mut self, accessed: SystemTime) -> Self {
        self.accessed = Some(accessed);
        self
    }

    /// Set the creation time, consuming and returning `self`.
    pub fn created(mut self, created: SystemTime) -> Self {
        self.created = Some(created);
        self
    }

    /// Set the owning group id, consuming and returning `self`.
    pub fn gid(mut self, gid: u32) -> Self {
        self.gid = Some(gid);
        self
    }

    /// Set the POSIX permissions, consuming and returning `self`.
    pub fn mode(mut self, mode: UnixPex) -> Self {
        self.mode = Some(mode);
        self
    }

    /// Set the last modification time, consuming and returning `self`.
    pub fn modified(mut self, modified: SystemTime) -> Self {
        self.modified = Some(modified);
        self
    }

    /// Set the size in bytes, consuming and returning `self`.
    ///
    /// Set this before a [`crate::RemoteFs::create`] or
    /// [`crate::RemoteFs::append`] on a protocol such as SCP, which needs the
    /// transfer size before the first byte is sent.
    pub fn size(mut self, size: u64) -> Self {
        self.size = size;
        self
    }

    /// Set the path this link points at, consuming and returning `self`.
    pub fn symlink<P: AsRef<Path>>(mut self, p: P) -> Self {
        self.symlink = Some(p.as_ref().to_path_buf());
        self
    }

    /// Set the entry kind, consuming and returning `self`.
    pub fn file_type(mut self, t: FileType) -> Self {
        self.file_type = t;
        self
    }

    /// Set the owning user id, consuming and returning `self`.
    pub fn uid(mut self, uid: u32) -> Self {
        self.uid = Some(uid);
        self
    }

    /// Return whether the entry is a directory.
    pub fn is_dir(&self) -> bool {
        self.file_type.is_dir()
    }

    /// Return whether the entry is a regular file.
    pub fn is_file(&self) -> bool {
        self.file_type.is_file()
    }

    /// Return whether the entry is a symbolic link.
    pub fn is_symlink(&self) -> bool {
        self.file_type.is_symlink()
    }

    /// Set the path this link points at, in place.
    ///
    /// Use this when the link target is resolved after the metadata was built,
    /// which is what a client does when it stats a link in a second round trip.
    pub fn set_symlink<P: AsRef<Path>>(&mut self, p: P) {
        self.symlink = Some(p.as_ref().to_path_buf());
    }
}

/// Windows has no notion of owner or POSIX mode, so those fields stay empty.
#[cfg(target_family = "windows")]
impl From<StdMetadata> for Metadata {
    fn from(metadata: StdMetadata) -> Self {
        Self {
            accessed: metadata.accessed().ok(),
            created: metadata.created().ok(),
            gid: None,
            file_type: FileType::from(metadata.file_type()),
            modified: metadata.modified().ok(),
            mode: None,
            size: metadata.len(),
            symlink: None,
            uid: None,
        }
    }
}

/// A directory reports its block size rather than its length, as `ls` does.
#[cfg(target_family = "unix")]
impl From<StdMetadata> for Metadata {
    fn from(metadata: StdMetadata) -> Self {
        use std::os::unix::fs::MetadataExt;

        Self {
            accessed: metadata.accessed().ok(),
            created: metadata.created().ok(),
            gid: Some(metadata.gid()),
            file_type: FileType::from(metadata.file_type()),
            modified: metadata.modified().ok(),
            mode: Some(UnixPex::from(metadata.mode())),
            size: if metadata.is_dir() {
                metadata.blksize()
            } else {
                metadata.len()
            },
            symlink: None,
            uid: Some(metadata.uid()),
        }
    }
}

#[cfg(test)]
mod test {

    use std::time::{Duration, UNIX_EPOCH};

    use pretty_assertions::assert_eq;

    use super::super::UnixPexClass;
    use super::*;

    #[test]
    fn should_initialize_metadata() {
        let metadata = Metadata::default();
        assert!(metadata.accessed.is_none());
        assert!(metadata.created.is_none());
        assert!(metadata.gid.is_none());
        assert!(metadata.mode.is_none());
        assert!(metadata.modified.is_none());
        assert_eq!(metadata.size, 0);
        assert!(metadata.symlink.is_none());
        assert_eq!(metadata.file_type, FileType::File);
        assert!(metadata.uid.is_none());
    }

    #[test]
    fn should_construct_metadata() {
        let accessed = UNIX_EPOCH.checked_add(Duration::from_secs(86400)).unwrap();
        let created = UNIX_EPOCH
            .checked_add(Duration::from_secs(4238673))
            .unwrap();
        let modified = UNIX_EPOCH
            .checked_add(Duration::from_secs(9048045687))
            .unwrap();
        let metadata = Metadata::default()
            .accessed(accessed)
            .created(created)
            .gid(14)
            .mode(UnixPex::new(
                UnixPexClass::from(6),
                UnixPexClass::from(4),
                UnixPexClass::from(0),
            ))
            .modified(modified)
            .size(1024)
            .symlink(Path::new("/tmp/a.txt"))
            .file_type(FileType::Symlink)
            .uid(10);
        assert_eq!(metadata.accessed, Some(accessed));
        assert_eq!(metadata.created, Some(created));
        assert_eq!(metadata.gid.unwrap(), 14);
        assert!(metadata.mode.is_some());
        assert_eq!(metadata.modified, Some(modified));
        assert_eq!(metadata.size, 1024);
        assert!(metadata.is_symlink());
        assert!(!metadata.is_dir());
        assert!(!metadata.is_file());
        assert_eq!(
            metadata.symlink.as_deref().unwrap(),
            Path::new("/tmp/a.txt")
        );
        assert_eq!(metadata.uid.unwrap(), 10);
    }

    #[test]
    #[cfg(target_family = "windows")]
    fn should_make_metadata_from_std_metadata() {
        let tempfile = tempfile::NamedTempFile::new().ok().unwrap();
        let metadata = std::fs::metadata(tempfile.path()).ok().unwrap();
        let metadata = Metadata::from(metadata);
        assert!(metadata.is_file());
        assert!(metadata.symlink.is_none());
        assert_eq!(metadata.size, 0);
        assert!(metadata.gid.is_none());
        assert!(metadata.uid.is_none());
        assert!(metadata.mode.is_none());
    }

    #[test]
    #[cfg(target_family = "unix")]
    fn should_make_metadata_from_std_metadata() {
        let tempfile = tempfile::NamedTempFile::new().ok().unwrap();
        let metadata = std::fs::metadata(tempfile.path()).ok().unwrap();
        let metadata = Metadata::from(metadata);
        assert!(metadata.is_file());
        assert!(metadata.symlink.is_none());
        assert_eq!(metadata.size, 0);
        assert!(metadata.gid.is_some());
        assert!(metadata.uid.is_some());
        assert!(metadata.mode.is_some());
    }
}
