//! Remote path syntax and validation independent of the client platform.

use std::path::Path;

use crate::fs::{RemoteError, RemoteErrorType, RemoteResult};

/// Verifies that a remote path has an absolute root, independently of client platform.
///
/// Remote filesystem operations use absolute paths so that a backend does not
/// need to maintain hidden working-directory state. Accepted roots are:
///
/// - POSIX paths beginning with `/`, including `/` itself.
/// - Windows drive paths beginning with an ASCII letter, `:`, and `/` or `\`.
/// - UNC paths beginning with `\\` and nonempty server and share components,
///   separated by `/` or `\`.
///
/// Windows device namespaces (`\\?\` and `\\.\`) are not supported. Only the
/// root syntax is checked; backends validate protocol-specific path rules. The
/// original path is returned unchanged, including non-UTF-8 content, repeated
/// separators, and `.` or `..` components. No normalization or resolution occurs.
///
/// # Errors
///
/// Returns [`RemoteErrorType::InvalidPath`] for empty paths, relative paths,
/// drive-relative paths (`C:file`), single-backslash roots (`\file`), incomplete
/// UNC roots, and Windows device namespaces.
///
/// # Examples
///
/// ```
/// use std::path::Path;
///
/// assert!(remotefs::path::ensure_absolute(Path::new("/tmp/file")).is_ok());
/// assert!(remotefs::path::ensure_absolute(Path::new(r"C:\tmp\file")).is_ok());
/// assert!(remotefs::path::ensure_absolute(Path::new(r"\\server\share\file")).is_ok());
/// assert!(remotefs::path::ensure_absolute(Path::new("file")).is_err());
/// ```
pub fn ensure_absolute(path: &Path) -> RemoteResult<&Path> {
    let bytes = path.as_os_str().as_encoded_bytes();
    let drive_root =
        matches!(bytes, [letter, b':', b'/' | b'\\', ..] if letter.is_ascii_alphabetic());
    let unc_root = bytes.strip_prefix(br"\\").is_some_and(|rest| {
        let mut components = rest.split(|byte| matches!(byte, b'/' | b'\\'));
        let server = components.next().unwrap_or_default();
        let share = components.next().unwrap_or_default();
        !server.is_empty() && server != b"?" && server != b"." && !share.is_empty()
    });
    if bytes.starts_with(b"/") || drive_root || unc_root {
        Ok(path)
    } else {
        Err(RemoteError::with_message(
            RemoteErrorType::InvalidPath,
            "path must be absolute",
        ))
    }
}

/// Verifies that a symbolic link target is usable as link content.
///
/// The target is the content of the link, not a path the client resolves. It is
/// stored verbatim by every backend: a relative target is resolved by the remote
/// host against the parent directory of the link when the link is followed, the
/// target does not need to exist, and no normalization, absolutization, or
/// separator rewriting takes place. Only an empty target is refused. The original
/// target is returned unchanged.
///
/// # Errors
///
/// Returns [`RemoteErrorType::InvalidPath`] when the target is empty.
///
/// # Examples
///
/// ```
/// use std::path::Path;
///
/// assert!(remotefs::path::ensure_symlink_target(Path::new("../file")).is_ok());
/// assert!(remotefs::path::ensure_symlink_target(Path::new("/missing")).is_ok());
/// assert!(remotefs::path::ensure_symlink_target(Path::new("")).is_err());
/// ```
pub fn ensure_symlink_target(target: &Path) -> RemoteResult<&Path> {
    if target.as_os_str().is_empty() {
        Err(RemoteError::with_message(
            RemoteErrorType::InvalidPath,
            "symlink target must not be empty",
        ))
    } else {
        Ok(target)
    }
}

pub(crate) fn file_name(path: &Path) -> Option<String> {
    let path = path.as_os_str().to_string_lossy();
    let bytes = path.as_bytes();
    let drive_root =
        matches!(bytes, [letter, b':', b'/' | b'\\', ..] if letter.is_ascii_alphabetic());
    let windows = !bytes.starts_with(b"/") && (drive_root || bytes.starts_with(br"\\"));
    let separator = |byte: &u8| *byte == b'/' || (windows && *byte == b'\\');
    let body = if drive_root {
        &bytes[2..]
    } else if windows {
        // The server and share together form a UNC root, not an entry name.
        bytes[2..].splitn(3, separator).nth(2).unwrap_or_default()
    } else {
        bytes
    };
    body.rsplit(separator)
        .find(|component| !component.is_empty() && *component != b".")
        .filter(|component| *component != b"..")
        .map(|name| String::from_utf8_lossy(name).into_owned())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::fs::{RemoteErrorType, RemoteResult};

    #[test]
    fn symlink_target_accepts_any_non_empty_path() -> RemoteResult<()> {
        for input in [
            "target",
            "../sibling/file",
            ".",
            "/abs/missing",
            r"C:\x",
            "a\\b",
        ] {
            assert_eq!(
                super::ensure_symlink_target(Path::new(input))?,
                Path::new(input)
            );
        }
        Ok(())
    }

    #[test]
    fn symlink_target_rejects_empty_path() {
        let error = super::ensure_symlink_target(Path::new("")).unwrap_err();
        assert_eq!(error.kind(), RemoteErrorType::InvalidPath);
    }

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

    #[test]
    fn preserves_absolute_remote_path_syntax_on_every_platform() -> RemoteResult<()> {
        for input in [
            "/",
            "/srv/data/file",
            "/srv/../data//file/",
            "//server/share/file",
            r"C:\",
            "z:/data/file",
            r"C:\data\..\file",
            r"\\server\share",
            r"\\server\share\file",
            r"\\server/share/file",
        ] {
            let path = Path::new(input);
            let actual = super::ensure_absolute(path)?;
            assert_eq!(actual.as_os_str(), path.as_os_str(), "input: {input:?}");
            assert!(std::ptr::eq(actual, path));
        }
        Ok(())
    }

    #[test]
    fn rejects_incomplete_remote_roots_on_every_platform() {
        for input in [
            "C:",
            "C:file",
            r"\",
            r"\file",
            r"\\",
            r"\\server",
            r"\\server\",
            r"\\\share",
            r"\\server\\share",
            r"\\?\C:\file",
            r"\\.\pipe\name",
            "1:/file",
        ] {
            let error = super::ensure_absolute(Path::new(input)).unwrap_err();
            assert_eq!(
                error.kind(),
                RemoteErrorType::InvalidPath,
                "input: {input:?}"
            );
        }
    }
}
