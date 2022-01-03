//! ## Metadata
//!
//! file metadata

/**
 * MIT License
 *
 * remotefs - Copyright (c) 2021 Christian Visintin
 *
 * Permission is hereby granted, free of charge, to any person obtaining a copy
 * of this software and associated documentation files (the "Software"), to deal
 * in the Software without restriction, including without limitation the rights
 * to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
 * copies of the Software, and to permit persons to whom the Software is
 * furnished to do so, subject to the following conditions:
 *
 * The above copyright notice and this permission notice shall be included in all
 * copies or substantial portions of the Software.
 *
 * THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
 * IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
 * FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
 * AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
 * LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
 * OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
 * SOFTWARE.
 */
use super::{FileType, UnixPex};

#[cfg(target_family = "unix")]
use std::os::unix::fs::MetadataExt;
use std::{
    fs::Metadata as StdMetadata,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

/// File metadata
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Metadata {
    /// Last access time
    pub accessed: SystemTime,
    /// Creation time
    pub created: SystemTime,
    /// Group id
    pub gid: Option<u32>,
    /// Unix permissions
    pub mode: Option<UnixPex>,
    /// Modify time
    pub modified: SystemTime,
    /// File size in bytes
    pub size: u64,
    /// If file is symlink, contains the path of the file it is pointing to
    pub symlink: Option<PathBuf>,
    /// File type
    pub file_type: FileType,
    /// User id
    pub uid: Option<u32>,
}

impl Default for Metadata {
    fn default() -> Self {
        Self {
            accessed: UNIX_EPOCH,
            created: UNIX_EPOCH,
            gid: None,
            mode: None,
            modified: UNIX_EPOCH,
            size: 0,
            symlink: None,
            file_type: FileType::File,
            uid: None,
        }
    }
}

impl Metadata {
    /// Construct metadata with accessed
    pub fn accessed(mut self, accessed: SystemTime) -> Self {
        self.accessed = accessed;
        self
    }

    /// Construct metadata with created
    pub fn created(mut self, created: SystemTime) -> Self {
        self.created = created;
        self
    }

    /// Construct metadata with group id
    pub fn gid(mut self, gid: u32) -> Self {
        self.gid = Some(gid);
        self
    }

    /// Construct metadata with UNIX permissions
    pub fn mode(mut self, mode: UnixPex) -> Self {
        self.mode = Some(mode);
        self
    }

    /// Construct metadata with modify time
    pub fn modified(mut self, modified: SystemTime) -> Self {
        self.modified = modified;
        self
    }

    /// Construct metadata with file size
    pub fn size(mut self, size: u64) -> Self {
        self.size = size;
        self
    }

    /// Construct metadata with symlink
    pub fn symlink<P: AsRef<Path>>(mut self, p: P) -> Self {
        self.symlink = Some(p.as_ref().to_path_buf());
        self
    }

    /// Construct metadata with type
    pub fn file_type(mut self, t: FileType) -> Self {
        self.file_type = t;
        self
    }

    /// Construct metadata with user id
    pub fn uid(mut self, uid: u32) -> Self {
        self.uid = Some(uid);
        self
    }

    /// Returns whether the file is a directory
    pub fn is_dir(&self) -> bool {
        self.file_type.is_dir()
    }

    /// Returns whether the file is a regular file
    pub fn is_file(&self) -> bool {
        self.file_type.is_file()
    }

    /// Returns whether the file is a symbolic link
    pub fn is_symlink(&self) -> bool {
        self.file_type.is_symlink()
    }

    /// Set symlink
    pub fn set_symlink<P: AsRef<Path>>(&mut self, p: P) {
        self.symlink = Some(p.as_ref().to_path_buf());
    }
}

#[cfg(target_family = "windows")]
impl From<StdMetadata> for Metadata {
    fn from(metadata: StdMetadata) -> Self {
        Self {
            accessed: metadata.accessed().ok().unwrap_or(SystemTime::UNIX_EPOCH),
            created: metadata.created().ok().unwrap_or(SystemTime::UNIX_EPOCH),
            gid: None,
            file_type: FileType::from(metadata.file_type()),
            modified: metadata.modified().ok().unwrap_or(SystemTime::UNIX_EPOCH),
            mode: None,
            size: metadata.len(),
            symlink: None,
            uid: None,
        }
    }
}

#[cfg(target_family = "unix")]
impl From<StdMetadata> for Metadata {
    fn from(metadata: StdMetadata) -> Self {
        Self {
            accessed: metadata.accessed().ok().unwrap_or(SystemTime::UNIX_EPOCH),
            // NOTE: on Unix based system, ctime is NEVER stored!!!
            created: metadata.created().ok().unwrap_or(SystemTime::UNIX_EPOCH),
            gid: Some(metadata.gid()),
            file_type: FileType::from(metadata.file_type()),
            modified: metadata.modified().ok().unwrap_or(SystemTime::UNIX_EPOCH),
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

    use super::super::UnixPexClass;
    use super::*;

    use pretty_assertions::assert_eq;
    use std::time::Duration;

    #[test]
    fn should_initialize_metadata() {
        let metadata = Metadata::default();
        assert_eq!(metadata.accessed, UNIX_EPOCH);
        assert_eq!(metadata.created, UNIX_EPOCH);
        assert!(metadata.gid.is_none());
        assert!(metadata.mode.is_none());
        assert_eq!(metadata.modified, UNIX_EPOCH);
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
        assert_eq!(metadata.accessed, accessed);
        assert_eq!(metadata.created, created);
        assert_eq!(metadata.gid.unwrap(), 14);
        assert!(metadata.mode.is_some());
        assert_eq!(metadata.modified, modified);
        assert_eq!(metadata.size, 1024);
        assert_eq!(metadata.is_symlink(), true);
        assert_eq!(metadata.is_dir(), false);
        assert_eq!(metadata.is_file(), false);
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
