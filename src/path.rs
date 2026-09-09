//! Validation helpers for paths accepted by remote filesystem operations.

use std::path::Path;

use crate::fs::{RemoteError, RemoteErrorType, RemoteResult};

/// Verifies that a path is absolute for the current platform.
///
/// Remote filesystem operations use absolute paths so that a backend does not
/// need to maintain hidden working-directory state. The original path is
/// returned unchanged on success.
///
/// # Errors
///
/// Returns [`RemoteErrorType::InvalidPath`] for relative and empty paths.
///
/// # Examples
///
/// ```
/// use std::path::Path;
///
/// assert!(remotefs::path::ensure_absolute(Path::new("/tmp/file")).is_ok());
/// assert!(remotefs::path::ensure_absolute(Path::new("file")).is_err());
/// ```
pub fn ensure_absolute(path: &Path) -> RemoteResult<&Path> {
    if path.is_absolute() {
        Ok(path)
    } else {
        Err(RemoteError::with_message(
            RemoteErrorType::InvalidPath,
            "path must be absolute",
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::fs::{RemoteErrorType, RemoteResult};

    #[test]
    fn rejects_relative_paths() {
        for input in ["", ".", "..", "readme.txt"] {
            let error = super::ensure_absolute(Path::new(input)).unwrap_err();
            assert_eq!(error.kind(), RemoteErrorType::InvalidPath);
        }
    }

    #[cfg(unix)]
    #[test]
    fn accepts_unix_absolute_paths_including_non_utf8() -> RemoteResult<()> {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        assert_eq!(
            super::ensure_absolute(Path::new("/tmp/file"))?,
            Path::new("/tmp/file")
        );
        let non_utf8 = OsString::from_vec(vec![b'/', b't', b'm', b'p', b'/', 0xff]);
        assert_eq!(
            super::ensure_absolute(Path::new(&non_utf8))?,
            Path::new(&non_utf8)
        );
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn accepts_windows_absolute_paths() -> RemoteResult<()> {
        assert!(super::ensure_absolute(Path::new(r"C:\tmp\file")).is_ok());
        assert!(super::ensure_absolute(Path::new(r"\\server\share\file")).is_ok());
        assert_eq!(
            super::ensure_absolute(Path::new(r"C:file"))
                .unwrap_err()
                .kind(),
            RemoteErrorType::InvalidPath
        );
        assert_eq!(
            super::ensure_absolute(Path::new(r"\file"))
                .unwrap_err()
                .kind(),
            RemoteErrorType::InvalidPath
        );
        Ok(())
    }
}
