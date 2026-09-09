//! Validation helpers for paths accepted by remote filesystem operations.

#[cfg(windows)]
use std::path::Component;
use std::path::{Path, PathBuf};

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

pub(crate) fn absolutize(cwd: &Path, path: &Path) -> RemoteResult<PathBuf> {
    ensure_absolute(cwd)?;
    if path.as_os_str().is_empty() {
        return Err(RemoteError::new(RemoteErrorType::InvalidPath));
    }
    #[cfg(windows)]
    if !path.is_absolute()
        && path
            .components()
            .any(|component| matches!(component, Component::Prefix(_) | Component::RootDir))
    {
        return Err(RemoteError::new(RemoteErrorType::InvalidPath));
    }
    let resolved = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    };
    ensure_absolute(&resolved)?;
    Ok(resolved)
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
    fn absolutizes_relative_paths_without_collapsing_components() -> RemoteResult<()> {
        let cwd = Path::new("/tmp/work");
        assert_eq!(
            super::absolutize(cwd, Path::new("file"))?,
            Path::new("/tmp/work/file")
        );
        assert_eq!(
            super::absolutize(cwd, Path::new("../file"))?,
            Path::new("/tmp/work/../file")
        );
        assert_eq!(
            super::absolutize(cwd, Path::new("/other/file"))?,
            Path::new("/other/file")
        );
        assert_eq!(
            super::absolutize(cwd, Path::new("")).unwrap_err().kind(),
            RemoteErrorType::InvalidPath
        );
        Ok(())
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
