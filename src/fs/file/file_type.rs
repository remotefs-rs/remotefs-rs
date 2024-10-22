//! ## File type
//!
//! represents the file type

use std::fs::FileType as StdFileType;

/// Describes the file type (directory, regular file or symlink)
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FileType {
    /// A directory
    Directory,
    /// Regular file
    File,
    /// Symbolic link. If the file is a symlink pointing to a directory,
    /// this will be still considered a Symlink.
    Symlink,
}

impl Default for FileType {
    fn default() -> Self {
        Self::File
    }
}

impl FileType {
    /// Returns whether file is a directory
    pub fn is_dir(&self) -> bool {
        matches!(self, Self::Directory)
    }

    /// Returns whether file is a regular file
    pub fn is_file(&self) -> bool {
        matches!(self, Self::File)
    }

    /// Returns whether file is symlink
    pub fn is_symlink(&self) -> bool {
        matches!(self, Self::Symlink)
    }
}

impl From<StdFileType> for FileType {
    fn from(t: StdFileType) -> Self {
        if t.is_symlink() {
            Self::Symlink
        } else if t.is_dir() {
            Self::Directory
        } else {
            Self::File
        }
    }
}

#[cfg(test)]
mod test {

    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn should_check_file_type() {
        assert_eq!(FileType::Directory.is_dir(), true);
        assert_eq!(FileType::Directory.is_file(), false);
        assert_eq!(FileType::Directory.is_symlink(), false);
        assert_eq!(FileType::File.is_dir(), false);
        assert_eq!(FileType::File.is_file(), true);
        assert_eq!(FileType::File.is_symlink(), false);
        assert_eq!(FileType::Symlink.is_dir(), false);
        assert_eq!(FileType::Symlink.is_file(), false);
        assert_eq!(FileType::Symlink.is_symlink(), true);
    }
}
