//! Conversion between absolute remote paths and Google Cloud Storage object
//! names.
//!
//! Google Cloud Storage has a flat namespace. A directory is either a
//! trailing-slash marker object or an implicit prefix, and the bucket root is
//! the empty prefix. Only POSIX paths rooted at `/` are accepted: Windows drive
//! and UNC roots pass [`ensure_absolute`] but have no meaning as an object name
//! and are rejected as [`RemoteErrorType::InvalidPath`].

use std::path::{Path, PathBuf};

use remotefs::path::ensure_absolute;
use remotefs::{RemoteError, RemoteErrorType, RemoteResult};

const DELIMITER: &str = "/";

/// Builds the object name for an absolute remote path.
///
/// The root maps to the empty string. `.` and empty components are dropped;
/// `..` is rejected.
pub(crate) fn object_name(path: &Path) -> RemoteResult<String> {
    ensure_absolute(path)?;
    let text = slash(path)?;
    let Some(stripped) = text.strip_prefix('/') else {
        return Err(RemoteError::with_message(
            RemoteErrorType::InvalidPath,
            "google cloud storage paths must start with '/'",
        ));
    };
    let mut components = Vec::new();
    for component in stripped.split(DELIMITER) {
        match component {
            "" | "." => {}
            ".." => {
                return Err(RemoteError::with_message(
                    RemoteErrorType::InvalidPath,
                    "parent directory components are not allowed",
                ));
            }
            other => components.push(other),
        }
    }
    Ok(components.join(DELIMITER))
}

/// Builds the listing prefix for an absolute directory path.
pub(crate) fn directory_prefix(path: &Path) -> RemoteResult<String> {
    let name = object_name(path)?;
    if name.is_empty() {
        Ok(String::new())
    } else {
        Ok(format!("{name}{DELIMITER}"))
    }
}

/// Builds the trailing-slash marker object name for an absolute directory path.
pub(crate) fn marker_name(path: &Path) -> RemoteResult<String> {
    let name = object_name(path)?;
    if name.is_empty() {
        return Err(RemoteError::with_message(
            RemoteErrorType::InvalidPath,
            "the root directory has no marker object",
        ));
    }
    Ok(format!("{name}{DELIMITER}"))
}

/// Returns whether the absolute path is the bucket root.
pub(crate) fn is_root(path: &Path) -> RemoteResult<bool> {
    Ok(object_name(path)?.is_empty())
}

/// Builds the absolute remote path for an object name or prefix.
pub(crate) fn to_path(name: &str) -> PathBuf {
    let trimmed = name.trim_matches('/');
    if trimmed.is_empty() {
        PathBuf::from("/")
    } else {
        PathBuf::from(format!("/{trimmed}"))
    }
}

fn slash(path: &Path) -> RemoteResult<String> {
    #[cfg(target_os = "windows")]
    {
        path_slash::PathExt::to_slash(path)
            .map(std::borrow::Cow::into_owned)
            .ok_or_else(|| {
                RemoteError::with_message(
                    RemoteErrorType::InvalidPath,
                    "remote paths must contain valid UTF-8",
                )
            })
    }

    #[cfg(not(target_os = "windows"))]
    {
        path.to_str().map(str::to_owned).ok_or_else(|| {
            RemoteError::with_message(
                RemoteErrorType::InvalidPath,
                "remote paths must contain valid UTF-8",
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn object_names_drop_root_and_redundant_components() {
        assert_eq!(object_name(Path::new("/")).unwrap(), "");
        assert_eq!(object_name(Path::new("//")).unwrap(), "");
        assert_eq!(
            object_name(Path::new("/docs/readme.md")).unwrap(),
            "docs/readme.md"
        );
        assert_eq!(object_name(Path::new("/docs//./a/")).unwrap(), "docs/a");
    }

    #[test]
    fn relative_and_non_posix_paths_are_invalid() {
        for input in [
            "",
            "readme.md",
            "docs/",
            r"C:\docs\readme.md",
            r"\\server\share\readme.md",
            "/docs/../secret",
            "/..",
        ] {
            let error = object_name(Path::new(input)).unwrap_err();
            assert_eq!(
                error.kind(),
                RemoteErrorType::InvalidPath,
                "input: {input:?}"
            );
        }
    }

    #[test]
    fn prefixes_and_markers_carry_a_trailing_delimiter() {
        assert_eq!(directory_prefix(Path::new("/")).unwrap(), "");
        assert_eq!(directory_prefix(Path::new("/docs")).unwrap(), "docs/");
        assert_eq!(marker_name(Path::new("/docs/img")).unwrap(), "docs/img/");
        assert_eq!(
            marker_name(Path::new("/")).unwrap_err().kind(),
            RemoteErrorType::InvalidPath
        );
        assert!(is_root(Path::new("/")).unwrap());
        assert!(!is_root(Path::new("/docs")).unwrap());
    }

    #[test]
    fn names_become_absolute_paths() {
        assert_eq!(to_path("docs/readme.md"), Path::new("/docs/readme.md"));
        assert_eq!(to_path("docs/"), Path::new("/docs"));
        assert_eq!(to_path(""), Path::new("/"));
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_paths_are_invalid() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        let path = Path::new(OsStr::from_bytes(b"/invalid/\xff"));
        assert_eq!(
            object_name(path).unwrap_err().kind(),
            RemoteErrorType::InvalidPath
        );
    }
}
