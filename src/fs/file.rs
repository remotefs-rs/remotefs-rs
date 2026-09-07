//! The description of a single entry on the remote file system.
//!
//! A [`File`] is a path plus its [`Metadata`], and it stands for any kind of
//! entry: a regular file, a directory, or a symbolic link. Which one it is comes
//! from [`Metadata::file_type`], not from a separate type, so a listing is a flat
//! `Vec<File>` that a caller filters as it sees fit.
//!
//! The metadata deliberately mirrors the subset of `std::fs::Metadata` that a
//! network protocol can actually answer. Fields no protocol may know are
//! [`Option`] — [`Metadata::mode`] is empty on a Windows share, and
//! [`Metadata::created`] is empty on most FTP servers — so a client reports what
//! it learned instead of inventing a plausible value.

use std::path::{Path, PathBuf};

mod file_type;
mod metadata;
mod permissions;

#[doc(inline)]
pub use file_type::FileType;
#[doc(inline)]
pub use metadata::Metadata;
#[doc(inline)]
pub use permissions::{UnixPex, UnixPexClass};

/// An entry on the remote file system: its path and its metadata.
///
/// The path is absolute as far as the protocol is concerned; a client resolves a
/// relative path against the working directory before it builds a `File`.
///
/// # Examples
///
/// ```
/// use remotefs::fs::{File, FileType, Metadata};
///
/// let file = File {
///     path: "/var/log/syslog".into(),
///     metadata: Metadata::default().file_type(FileType::File).size(512),
/// };
///
/// assert_eq!(file.name(), "syslog");
/// assert_eq!(file.extension(), None);
/// assert!(file.is_file());
/// assert!(!file.is_hidden());
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct File {
    /// The absolute path of the entry on the remote host.
    pub path: PathBuf,
    /// What the remote host reported about the entry.
    pub metadata: Metadata,
}

impl File {
    /// Return the absolute path of the entry.
    pub fn path(&self) -> &Path {
        self.path.as_path()
    }

    /// Return the last component of the path, or `/` for the root.
    ///
    /// The name is lossily converted to UTF-8, because a remote host may name an
    /// entry with bytes that are not valid UTF-8.
    ///
    /// # Examples
    ///
    /// ```
    /// use remotefs::fs::{File, Metadata};
    ///
    /// let root = File {
    ///     path: "/".into(),
    ///     metadata: Metadata::default(),
    /// };
    ///
    /// assert_eq!(root.name(), "/");
    /// ```
    pub fn name(&self) -> String {
        self.path()
            .file_name()
            .map(|x| x.to_string_lossy().to_string())
            .unwrap_or_else(|| "/".to_string())
    }

    /// Return what the remote host reported about the entry.
    pub fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    /// Return the path extension, when the entry has one.
    ///
    /// Like [`File::name`], the extension is lossily converted to UTF-8.
    ///
    /// # Examples
    ///
    /// ```
    /// use remotefs::fs::{File, Metadata};
    ///
    /// let file = File {
    ///     path: "/tmp/archive.tar.gz".into(),
    ///     metadata: Metadata::default(),
    /// };
    ///
    /// assert_eq!(file.extension().as_deref(), Some("gz"));
    /// ```
    pub fn extension(&self) -> Option<String> {
        self.path()
            .extension()
            .map(|x| x.to_string_lossy().to_string())
    }

    /// Return whether the entry is a directory.
    pub fn is_dir(&self) -> bool {
        self.metadata().is_dir()
    }

    /// Return whether the entry is a regular file.
    pub fn is_file(&self) -> bool {
        self.metadata().is_file()
    }

    /// Return whether the entry is a symbolic link.
    ///
    /// A link is reported as a link even when it points at a directory; follow
    /// [`Metadata::symlink`] to learn what it points at.
    pub fn is_symlink(&self) -> bool {
        self.metadata().is_symlink()
    }

    /// Return whether the name starts with a dot, as POSIX hides it.
    ///
    /// This is a naming convention, not a protocol flag: a Windows share can mark
    /// an entry hidden without naming it with a leading dot, and this method will
    /// not see it.
    pub fn is_hidden(&self) -> bool {
        self.name().starts_with('.')
    }
}

#[cfg(test)]
mod tests {

    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn should_create_file() {
        let entry = File {
            path: PathBuf::from("/bar.txt"),
            metadata: Metadata::default(),
        };
        assert_eq!(entry.path(), Path::new("/bar.txt"));
        assert_eq!(entry.name(), String::from("bar.txt"));
        assert_eq!(entry.extension().as_deref(), Some("txt"));
        assert_eq!(entry.metadata(), &Metadata::default());
        assert_eq!(entry.is_dir(), false);
        assert_eq!(entry.is_file(), true);
        assert_eq!(entry.is_hidden(), false);
    }

    #[test]
    fn should_return_is_hidden_for_hidden_files() {
        let entry = File {
            path: PathBuf::from("/.bar.txt"),
            metadata: Metadata::default(),
        };
        assert_eq!(entry.is_hidden(), true);
    }
}
