//! Wire encoding of absolute FTP paths shared by both clients.

use std::path::{Component, Path, PathBuf};

use remotefs::fs::{RemoteError, RemoteErrorType, RemoteResult};
use remotefs::path::ensure_absolute;
/// Validates that `path` is an encodable absolute FTP path and renders it for the wire.
pub(crate) fn remote_path(path: &Path) -> RemoteResult<String> {
    ensure_absolute(path)?;
    #[cfg(target_family = "unix")]
    {
        let path = path.to_str().ok_or_else(|| {
            RemoteError::with_message(
                RemoteErrorType::InvalidPath,
                "FTP paths must contain valid UTF-8",
            )
        })?;
        if !path.starts_with('/')
            || path.starts_with("//")
            || path.bytes().any(|byte| byte.is_ascii_control())
            || Path::new(path)
                .components()
                .any(|component| component == Component::ParentDir)
        {
            return Err(RemoteError::with_message(
                RemoteErrorType::InvalidPath,
                "FTP paths must use a POSIX absolute root",
            ));
        }
        Ok(path.to_owned())
    }
    #[cfg(target_os = "windows")]
    {
        use path_slash::PathExt as _;

        let path = path.to_slash().ok_or_else(|| {
            RemoteError::with_message(
                RemoteErrorType::InvalidPath,
                "FTP paths must contain valid UTF-8",
            )
        })?;
        if !path.starts_with('/')
            || path.starts_with("//")
            || path.bytes().any(|byte| byte.is_ascii_control())
            || Path::new(path.as_ref())
                .components()
                .any(|component| component == Component::ParentDir)
        {
            return Err(RemoteError::with_message(
                RemoteErrorType::InvalidPath,
                "FTP paths must use a single POSIX absolute root",
            ));
        }
        Ok(path.into_owned())
    }
}

/// Fixes a provided path; on Windows, converts backslashes to slashes.
#[cfg(target_os = "windows")]
pub(crate) fn resolve(p: &Path) -> PathBuf {
    use path_slash::PathExt as _;

    p.to_slash()
        .map(std::borrow::Cow::into_owned)
        .map(PathBuf::from)
        .unwrap_or_default()
}

/// Returns a provided path unchanged on POSIX platforms.
#[cfg(target_family = "unix")]
pub(crate) fn resolve(p: &Path) -> PathBuf {
    p.to_path_buf()
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use std::path::Path;

    use remotefs::fs::RemoteErrorType;

    use super::remote_path;

    #[test]
    fn rejects_parent_components_after_slash_conversion() {
        assert_eq!(
            remote_path(Path::new(r"/dir\..\file")).unwrap_err().kind(),
            RemoteErrorType::InvalidPath
        );
    }
}
