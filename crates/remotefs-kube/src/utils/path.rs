//! ## Path
//!
//! Helpers for the POSIX paths used inside containers.

use std::path::{Path, PathBuf};

use remotefs::{RemoteError, RemoteErrorType, RemoteResult};

/// Validate that `path` is an absolute POSIX path (`/...`).
///
/// [`remotefs::path::ensure_absolute`] also accepts Windows drive and UNC
/// roots; a container only understands `/`-rooted paths, so those are
/// rejected here with [`RemoteErrorType::InvalidPath`].
pub fn ensure_posix_absolute(path: &Path) -> RemoteResult<&Path> {
    let path = remotefs::path::ensure_absolute(path)?;
    if path.as_os_str().as_encoded_bytes().starts_with(b"/") {
        Ok(path)
    } else {
        Err(RemoteError::with_message(
            RemoteErrorType::InvalidPath,
            "path must be an absolute POSIX path",
        ))
    }
}

/// Join `name` onto `parent` with a `/` separator regardless of the host
/// platform, so that paths built on Windows never contain backslashes.
pub fn join(parent: &Path, name: &str) -> PathBuf {
    let parent = parent.to_string_lossy();
    let parent = parent.trim_end_matches('/');
    PathBuf::from(format!("{parent}/{name}"))
}

/// Quote `path` for `/bin/sh` with single quotes.
pub fn shell_quote(path: &Path) -> String {
    let raw = path.to_string_lossy();
    format!("'{raw}'", raw = raw.replace('\'', r"'\''"))
}

#[cfg(test)]
mod test {

    use std::path::PathBuf;

    use pretty_assertions::assert_eq;
    use remotefs::RemoteErrorType;

    use super::*;

    #[test]
    fn should_accept_posix_absolute_paths() {
        assert_eq!(
            ensure_posix_absolute(Path::new("/tmp/a.txt")).unwrap(),
            Path::new("/tmp/a.txt")
        );
        assert_eq!(
            ensure_posix_absolute(Path::new("/")).unwrap(),
            Path::new("/")
        );
    }

    #[test]
    fn should_reject_relative_and_windows_paths() {
        for input in ["", "a.txt", "tmp/a.txt", r"C:\tmp", r"\\server\share"] {
            assert_eq!(
                ensure_posix_absolute(Path::new(input)).unwrap_err().kind(),
                RemoteErrorType::InvalidPath,
                "input: {input:?}"
            );
        }
    }

    #[test]
    fn should_join_with_forward_slash() {
        assert_eq!(join(Path::new("/"), "a"), PathBuf::from("/a"));
        assert_eq!(join(Path::new("/tmp/"), "a"), PathBuf::from("/tmp/a"));
        assert_eq!(
            join(Path::new("/tmp"), "a.txt"),
            PathBuf::from("/tmp/a.txt")
        );
        assert_eq!(join(Path::new("/tmp"), "a").to_string_lossy(), "/tmp/a");
    }

    #[test]
    fn should_quote_for_shell() {
        assert_eq!(shell_quote(Path::new("/tmp/a b")), "'/tmp/a b'");
        assert_eq!(shell_quote(Path::new("/tmp/it's")), r"'/tmp/it'\''s'");
    }
}
