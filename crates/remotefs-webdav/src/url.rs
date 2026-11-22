//! Absolute remote path to WebDAV URL resolution.

use std::path::{Component, Path};

use remotefs::path::ensure_absolute;
use remotefs::{RemoteError, RemoteErrorType, RemoteResult};

/// Resolves the absolute remote `path` against the share `base` URL.
///
/// `base` must not end with a slash. When `collection` is true the returned
/// URL always ends with `/`, which WebDAV requires for collection operations.
///
/// # Errors
///
/// Returns [`RemoteErrorType::InvalidPath`] when `path` is relative or does
/// not start with `/`. Windows drive and UNC roots satisfy
/// [`ensure_absolute`] but have no HTTP representation.
pub(crate) fn resource_url(base: &str, path: &Path, collection: bool) -> RemoteResult<String> {
    ensure_absolute(path)?;
    let path_str = path.to_string_lossy();
    if !path_str.starts_with('/') {
        return Err(RemoteError::with_message(
            RemoteErrorType::InvalidPath,
            "WebDAV paths must start with '/'",
        ));
    }
    if path_str
        .split('/')
        .any(|component| matches!(component, "." | ".."))
        || path
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
        return Err(RemoteError::with_message(
            RemoteErrorType::InvalidPath,
            "WebDAV paths must not contain dot segments",
        ));
    }
    let path = encode_path(&path_str);
    let mut url = String::with_capacity(base.len() + path.len() + 1);
    url.push_str(base);
    url.push_str(&path);
    if collection && !url.ends_with('/') {
        url.push('/');
    }
    Ok(url)
}

fn encode_path(path: &str) -> String {
    let mut encoded = String::with_capacity(path.len());
    for byte in path.bytes() {
        if byte == b'/' || is_path_byte(byte) {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

fn is_path_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'-' | b'.'
                | b'_'
                | b'~'
                | b'!'
                | b'$'
                | b'&'
                | b'\''
                | b'('
                | b')'
                | b'*'
                | b'+'
                | b','
                | b';'
                | b'='
                | b':'
                | b'@'
        )
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use pretty_assertions::assert_eq;
    use remotefs::RemoteErrorType;

    use super::resource_url;

    const BASE: &str = "http://localhost:3080";

    #[test]
    fn joins_absolute_paths_to_the_base_url() {
        assert_eq!(
            resource_url(BASE, Path::new("/a.txt"), false).unwrap(),
            "http://localhost:3080/a.txt"
        );
        assert_eq!(
            resource_url(BASE, Path::new("/"), false).unwrap(),
            "http://localhost:3080/"
        );
        assert_eq!(
            resource_url(BASE, Path::new("/dir/a.txt"), false).unwrap(),
            "http://localhost:3080/dir/a.txt"
        );
    }

    #[test]
    fn collections_always_end_with_a_slash() {
        assert_eq!(
            resource_url(BASE, Path::new("/gabibbo"), true).unwrap(),
            "http://localhost:3080/gabibbo/"
        );
        assert_eq!(
            resource_url(BASE, Path::new("/gabibbo/"), true).unwrap(),
            "http://localhost:3080/gabibbo/"
        );
        assert_eq!(
            resource_url(BASE, Path::new("/"), true).unwrap(),
            "http://localhost:3080/"
        );
    }

    #[test]
    fn rejects_relative_and_non_posix_roots() {
        for input in ["", "a.txt", "dir/a.txt", r"C:\a.txt", r"\\server\share\a"] {
            let error = resource_url(BASE, Path::new(input), false).unwrap_err();
            assert_eq!(
                error.kind(),
                RemoteErrorType::InvalidPath,
                "input: {input:?}"
            );
        }
    }

    #[test]
    fn escapes_path_components_before_joining_them_to_the_url() {
        assert_eq!(
            resource_url(BASE, Path::new("/a b?c#d%e"), false).unwrap(),
            "http://localhost:3080/a%20b%3Fc%23d%25e"
        );
    }

    #[test]
    fn rejects_dot_segments_that_could_escape_the_share() {
        for input in ["/../secret", "/a/../secret", "/./secret"] {
            let error = resource_url(BASE, Path::new(input), false).unwrap_err();
            assert_eq!(
                error.kind(),
                RemoteErrorType::InvalidPath,
                "input: {input:?}"
            );
        }
    }
}
