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
mod set_metadata;

#[doc(inline)]
pub use file_type::FileType;
#[doc(inline)]
pub use metadata::Metadata;
#[doc(inline)]
pub use permissions::{UnixPex, UnixPexClass};
#[doc(inline)]
pub use set_metadata::SetMetadata;

/// An entry on the remote file system: its path and its metadata.
///
/// The path is absolute on the remote host. Backends must supply an absolute
/// path when constructing an entry; the constructor stores it unchanged.
///
/// # Examples
///
/// ```
/// use remotefs::fs::{File, FileType, Metadata};
///
/// let file = File::new(
///     "/var/log/syslog",
///     Metadata::default().file_type(FileType::File).size(512),
/// );
///
/// assert_eq!(file.name(), "syslog");
/// assert_eq!(file.extension(), None);
/// assert!(file.is_file());
/// assert!(!file.is_hidden());
/// ```
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct File {
    /// The absolute path of the entry on the remote host.
    pub path: PathBuf,
    /// What the remote host reported about the entry.
    pub metadata: Metadata,
}

impl File {
    /// Creates an entry from its path and metadata.
    pub fn new(path: impl Into<PathBuf>, metadata: Metadata) -> Self {
        Self {
            path: path.into(),
            metadata,
        }
    }

    /// Return the absolute path of the entry.
    pub fn path(&self) -> &Path {
        self.path.as_path()
    }

    /// Return the last component of the path, or `/` for the root.
    ///
    /// The name is lossily converted to UTF-8, because a remote host may name an
    /// entry with bytes that are not valid UTF-8. Slash-rooted POSIX paths treat
    /// backslashes as literal name characters; fully qualified drive and UNC
    /// paths treat both slashes and backslashes as separators, on every client
    /// platform. Trailing separators and `.` components are ignored.
    ///
    /// # Examples
    ///
    /// ```
    /// use remotefs::fs::{File, Metadata};
    ///
    /// let root = File::new("/", Metadata::default());
    ///
    /// assert_eq!(root.name(), "/");
    /// ```
    pub fn name(&self) -> String {
        crate::path::file_name(self.path()).unwrap_or_else(|| "/".to_string())
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
    /// let file = File::new("/tmp/archive.tar.gz", Metadata::default());
    ///
    /// assert_eq!(file.extension().as_deref(), Some("gz"));
    /// ```
    pub fn extension(&self) -> Option<String> {
        let name = crate::path::file_name(self.path())?;
        let (stem, extension) = name.rsplit_once('.')?;
        (!stem.is_empty()).then(|| extension.to_owned())
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
        let entry = File::new(PathBuf::from("/bar.txt"), Metadata::default());
        assert_eq!(entry.path(), Path::new("/bar.txt"));
        assert_eq!(entry.name(), String::from("bar.txt"));
        assert_eq!(entry.extension().as_deref(), Some("txt"));
        assert_eq!(entry.metadata(), &Metadata::default());
        assert!(!entry.is_dir());
        assert!(entry.is_file());
        assert!(!entry.is_hidden());
    }

    #[test]
    fn should_return_is_hidden_for_hidden_files() {
        let entry = File::new(PathBuf::from("/.bar.txt"), Metadata::default());
        assert!(entry.is_hidden());
    }

    #[test]
    fn names_use_remote_root_syntax_on_every_platform() {
        for (path, name, extension, hidden) in [
            (r"C:\logs\report.txt", "report.txt", Some("txt"), false),
            (r"z:/logs\.hidden.log", ".hidden.log", Some("log"), true),
            (
                r"\\server\share\report.txt",
                "report.txt",
                Some("txt"),
                false,
            ),
            (r"\\server/share\logs/.secret", ".secret", None, true),
            (
                r"/logs/report\part.txt",
                r"report\part.txt",
                Some("txt"),
                false,
            ),
            (r"/logs/.secret\part", r".secret\part", None, true),
            ("/logs/archive.tar.gz", "archive.tar.gz", Some("gz"), false),
            ("/logs/file.", "file.", Some(""), false),
            ("/logs/.config", ".config", None, true),
            (r"C:\logs\report.txt\", "report.txt", Some("txt"), false),
            ("/logs/report.txt/./", "report.txt", Some("txt"), false),
            ("/", "/", None, false),
            ("//", "/", None, false),
            (r"C:\", "/", None, false),
            ("z:/", "/", None, false),
            (r"\\server\share", "/", None, false),
            (r"\\server\share\", "/", None, false),
            ("/logs/..", "/", None, false),
        ] {
            let file = File::new(path, Metadata::default());
            assert_eq!(file.name(), name, "path: {path:?}");
            assert_eq!(file.extension().as_deref(), extension, "path: {path:?}");
            assert_eq!(file.is_hidden(), hidden, "path: {path:?}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn remote_names_convert_non_utf8_lossily() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        for prefix in [b"/logs/".as_slice(), br"C:\logs\", br"\\server\share\"] {
            let mut bytes = prefix.to_vec();
            bytes.extend_from_slice(b".file\xff.ext\xff");
            let file = File::new(OsString::from_vec(bytes), Metadata::default());
            assert_eq!(file.name(), ".file\u{fffd}.ext\u{fffd}");
            assert_eq!(file.extension().as_deref(), Some("ext\u{fffd}"));
            assert!(file.is_hidden());
        }
    }

    #[cfg(windows)]
    #[test]
    fn remote_names_preserve_windows_lossy_surrogate_conversion() {
        use std::ffi::OsString;
        use std::os::windows::ffi::OsStringExt;

        let mut units: Vec<_> = r"C:\logs\.file".encode_utf16().collect();
        units.push(0xd800);
        units.extend(".txt".encode_utf16());
        let file = File::new(OsString::from_wide(&units), Metadata::default());
        assert_eq!(file.name(), ".file\u{fffd}.txt");
        assert_eq!(file.extension().as_deref(), Some("txt"));
        assert!(file.is_hidden());
    }
}
