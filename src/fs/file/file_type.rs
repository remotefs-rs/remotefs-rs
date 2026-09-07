//! The kind of entry a [`super::File`] stands for.

use std::fs::FileType as StdFileType;

/// The kind of an entry: a directory, a regular file, or a symbolic link.
///
/// Every entry is exactly one of the three. A link is classified as
/// [`FileType::Symlink`] whatever it points at, so resolving a link is the
/// caller's decision, not the client's.
///
/// Anything a protocol reports that is none of the three — a socket, a FIFO, a
/// device node — is reported as [`FileType::File`], which is also the default.
///
/// # Examples
///
/// ```
/// use remotefs::fs::FileType;
///
/// assert!(FileType::Directory.is_dir());
/// assert!(!FileType::Symlink.is_file());
/// assert_eq!(FileType::default(), FileType::File);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FileType {
    /// A directory.
    Directory,
    /// A regular file, and anything that is neither a directory nor a link.
    File,
    /// A symbolic link, whatever it points at.
    Symlink,
}

impl Default for FileType {
    fn default() -> Self {
        Self::File
    }
}

impl FileType {
    /// Return whether the entry is a directory.
    pub fn is_dir(&self) -> bool {
        matches!(self, Self::Directory)
    }

    /// Return whether the entry is a regular file.
    pub fn is_file(&self) -> bool {
        matches!(self, Self::File)
    }

    /// Return whether the entry is a symbolic link.
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
