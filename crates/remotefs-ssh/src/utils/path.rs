//! Remote path helpers.

use std::borrow::Cow;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use remotefs::RemoteResult;
use remotefs::path::ensure_absolute;

/// Returns `path` as the remote path to send to the server, with every `\`
/// turned into `/`.
///
/// `\` is a separator in the paths given to a client on every platform, so a
/// path built with [`Path::join`] on Windows reaches the right remote entry.
/// Validation runs first, on the path as given, so `\srv` is still rejected as
/// relative and is not turned into `/srv`. A path that is not valid Unicode is
/// kept as is.
///
/// # Errors
///
/// Returns [`remotefs::RemoteErrorType::InvalidPath`] when `path` is not
/// absolute.
pub(crate) fn remote_path(path: &Path) -> RemoteResult<Cow<'_, Path>> {
    let path = ensure_absolute(path)?;
    match path.to_str() {
        Some(text) if text.contains('\\') => Ok(Cow::Owned(PathBuf::from(text.replace('\\', "/")))),
        _ => Ok(Cow::Borrowed(path)),
    }
}

/// Builds the remote path of `name` inside `dir`, always with `/` separators.
///
/// `backslash_is_separator` tells whether the local platform treats `\` as a
/// path separator, as Windows does. When it is set, every `\` in `dir`
/// becomes `/`; otherwise `dir` is kept as is, so a `\` in a remote directory
/// name stays a literal character. A `dir`
/// that is not valid Unicode is kept as is. `name` is always appended
/// verbatim, without any lossy conversion. A `/` is inserted only when `dir`
/// does not already end with one, so `/` joined with `etc` gives `/etc`. When
/// `name` is empty or `/` (the entry of the root itself), `dir` is returned
/// (normalized) without appending anything.
pub(crate) fn join_with(dir: &OsStr, name: &OsStr, backslash_is_separator: bool) -> PathBuf {
    let mut path = match dir.to_str() {
        Some(dir) if backslash_is_separator => OsString::from(dir.replace('\\', "/")),
        _ => dir.to_os_string(),
    };
    if name.is_empty() || name == "/" {
        return PathBuf::from(path);
    }
    if path.as_encoded_bytes().last() != Some(&b'/') {
        path.push("/");
    }
    path.push(name);
    PathBuf::from(path)
}

/// Builds the remote path inside `dest` that mirrors the listed `entry`,
/// always with `/` separators.
///
/// The last component of `entry` is taken splitting only on `/`, so on
/// Windows a `\` in a remote entry name is kept, and it is appended to `dest`
/// with `/`. `dest` is kept as is, as it is sent to the server. Returns `None`
/// when `entry` has no file name: the root, an empty path or a path ending
/// with `..`.
pub(crate) fn child_path(dest: &Path, entry: &Path) -> Option<PathBuf> {
    child_path_with(dest, entry, cfg!(windows))
}

/// Implements [`child_path`].
///
/// `backslash_is_separator` tells whether the local platform treats `\` as a
/// path separator, as Windows does. When it is set, the name of `entry` is
/// taken splitting only on `/`, so a `\` in it is kept. `dest` is never
/// normalized, whatever the flag.
fn child_path_with(dest: &Path, entry: &Path, backslash_is_separator: bool) -> Option<PathBuf> {
    file_name_with(entry, backslash_is_separator)
        .map(|name| join_with(dest.as_os_str(), name, false))
}

/// Returns the last component of the remote `path`, splitting only on `/`.
///
/// When `backslash_is_separator` is set, [`Path::file_name`] is avoided since
/// that would split on `\`, which is kept as part of the name. Returns `None`
/// for the root, an empty path or a path ending with `..`. A `path` that is
/// not valid Unicode falls back to [`Path::file_name`].
fn file_name_with(path: &Path, backslash_is_separator: bool) -> Option<&OsStr> {
    match path.to_str() {
        Some(path) if backslash_is_separator => {
            last_name(path).filter(|name| *name != "..").map(OsStr::new)
        }
        _ => path.file_name(),
    }
}

/// Returns the last name of the remote `path`, splitting only on `/`.
///
/// Empty and `.` components are skipped, so the last name of `/srv/files/`
/// and of `/srv/files/.` is `files`, while a `..` component is returned as
/// is. A `\` is kept as part of the name. Returns `None` when `path` has no
/// name, such as `/` or `.`.
pub(crate) fn last_name(path: &str) -> Option<&str> {
    path.rsplit('/')
        .find(|name| !name.is_empty() && *name != ".")
}

/// Returns the parent of the remote `path`, splitting only on `/`.
///
/// Trailing `/` and `.` components are skipped, as [`Path::parent`] does, so
/// the parent of `/srv/files/.` is `/srv`. On Windows a `\` in a remote entry
/// name is kept, instead of being treated as a local separator. Returns `None`
/// for the root or an empty path.
pub(crate) fn parent(path: &Path) -> Option<&Path> {
    parent_with(path, cfg!(windows))
}

/// Implements [`parent`].
///
/// `backslash_is_separator` tells whether the local platform treats `\` as a
/// path separator, as Windows does. When it is set, [`Path::parent`] is
/// avoided, since it would split on `\`, and `path` is split only on `/`.
///
/// A `path` that is not valid Unicode falls back to [`Path::parent`].
pub(crate) fn parent_with(path: &Path, backslash_is_separator: bool) -> Option<&Path> {
    match path.to_str() {
        Some(path) if backslash_is_separator => {
            let mut rest = path.trim_end_matches('/');
            while let Some(stripped) = rest.strip_suffix("/.") {
                rest = stripped.trim_end_matches('/');
            }
            rest.rsplit_once('/')
                .map(|(parent, _)| Path::new(if parent.is_empty() { "/" } else { parent }))
        }
        _ => path.parent(),
    }
}

#[cfg(test)]
mod test {
    use std::borrow::Cow;
    use std::path::Path;

    use pretty_assertions::assert_eq;
    use remotefs::RemoteErrorType;

    use super::*;

    #[test]
    fn should_join_remote_paths_with_forward_slash() {
        for (dir, name, expected) in [
            ("/srv", "files", "/srv/files"),
            ("/", "etc", "/etc"),
            ("/srv/", "files", "/srv/files"),
            ("/srv", r"a\b", r"/srv/a\b"),
            ("/", "/", "/"),
            ("/srv", "", "/srv"),
        ] {
            assert_eq!(
                join_with(OsStr::new(dir), OsStr::new(name), false).to_str(),
                Some(expected),
                "join_with({dir:?}, {name:?}, false)"
            );
        }
    }

    #[test]
    fn should_normalize_backslash_separators_in_directory() {
        for dir in [r"\srv\data", r"/srv\data", r"\srv\data\"] {
            assert_eq!(
                join_with(OsStr::new(dir), OsStr::new(r"a\b"), true).to_str(),
                Some(r"/srv/data/a\b"),
                "join_with({dir:?}, \"a\\b\", true)"
            );
        }
    }

    #[test]
    fn should_keep_backslashes_in_directory_without_backslash_separators() {
        assert_eq!(
            join_with(OsStr::new(r"/srv/a\b"), OsStr::new("files"), false).to_str(),
            Some(r"/srv/a\b/files")
        );
    }

    #[test]
    fn should_take_file_name_after_last_forward_slash() {
        for (path, expected) in [
            ("/src/files", Some("files")),
            (r"/src/a\b", Some(r"a\b")),
            ("/src/files/", Some("files")),
            ("/src/files/.", Some("files")),
            ("/", None),
            ("/src/..", None),
        ] {
            assert_eq!(
                file_name_with(Path::new(path), true).and_then(OsStr::to_str),
                expected,
                "file_name_with({path:?}, true)"
            );
        }
    }

    #[test]
    fn should_take_last_name_skipping_empty_and_current_components() {
        for (path, expected) in [
            ("/srv/files", Some("files")),
            (r"/srv/a\b", Some(r"a\b")),
            ("/srv/files/", Some("files")),
            ("/srv/files/.", Some("files")),
            ("/srv/..", Some("..")),
            ("files", Some("files")),
            ("/", None),
            (".", None),
            ("", None),
        ] {
            assert_eq!(last_name(path), expected, "last_name({path:?})");
        }
    }

    #[test]
    fn should_build_child_path_with_forward_slashes() {
        for (dest, entry, expected) in [
            (r"/dst\x", r"/src/a\b", Some(r"/dst\x/a\b")),
            ("/dst/", "/src/files", Some("/dst/files")),
            ("/dst", "/src/..", None),
            ("/dst", "/", None),
        ] {
            assert_eq!(
                child_path_with(Path::new(dest), Path::new(entry), true)
                    .as_deref()
                    .and_then(Path::to_str),
                expected,
                "child_path_with({dest:?}, {entry:?}, true)"
            );
        }
    }

    #[test]
    fn should_take_parent_before_last_forward_slash() {
        for (path, expected) in [
            ("/srv/files", Some("/srv")),
            (r"/srv/a\b", Some("/srv")),
            ("/srv/files/", Some("/srv")),
            ("/srv/files/.", Some("/srv")),
            ("/srv/./", Some("/")),
            ("/etc", Some("/")),
            ("/", None),
            ("", None),
        ] {
            for backslash_is_separator in [true, false] {
                if path.contains('\\') && !backslash_is_separator {
                    continue;
                }
                assert_eq!(
                    parent_with(Path::new(path), backslash_is_separator).and_then(Path::to_str),
                    expected,
                    "parent_with({path:?}, {backslash_is_separator})"
                );
            }
        }
    }

    #[test]
    fn should_turn_backslash_separators_into_forward_slashes() {
        let joined = Path::new("/home/deck/.steam/steam").join("userdata");
        for (input, expected) in [
            (joined.as_path(), "/home/deck/.steam/steam/userdata"),
            (
                Path::new(r"/home/deck/.steam/steam\userdata"),
                "/home/deck/.steam/steam/userdata",
            ),
            (
                Path::new(r"/home/deck/.steam\steam\userdata"),
                "/home/deck/.steam/steam/userdata",
            ),
            (Path::new(r"/srv\data\"), "/srv/data/"),
            (Path::new(r"C:\Users\deck"), "C:/Users/deck"),
            (Path::new(r"\\server\share\x"), "//server/share/x"),
            (Path::new("/srv/files"), "/srv/files"),
            (Path::new("/"), "/"),
        ] {
            // Compare strings: on Windows `Path` equality treats `\` and `/` alike.
            assert_eq!(
                remote_path(input).unwrap().to_str(),
                Some(expected),
                "remote_path({input:?})"
            );
        }
    }

    #[test]
    fn should_borrow_remote_paths_without_backslashes() {
        assert!(matches!(
            remote_path(Path::new("/srv/files")).unwrap(),
            Cow::Borrowed(_)
        ));
    }

    #[test]
    fn should_reject_relative_remote_paths_before_converting() {
        for input in ["", "srv", r"srv\data", r"\srv", "C:srv"] {
            assert_eq!(
                remote_path(Path::new(input)).unwrap_err().kind(),
                RemoteErrorType::InvalidPath,
                "remote_path({input:?})"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn should_keep_non_utf8_names_verbatim() {
        use std::os::unix::ffi::OsStrExt as _;

        let name = OsStr::from_bytes(b"caf\xe9.txt");
        let path = join_with(OsStr::new("/srv"), name, false);
        assert_eq!(path.as_os_str().as_bytes(), b"/srv/caf\xe9.txt");
        let dir = OsStr::from_bytes(b"/caf\xe9");
        let path = join_with(dir, OsStr::new("files"), false);
        assert_eq!(path.as_os_str().as_bytes(), b"/caf\xe9/files");
        let dir = OsStr::from_bytes(b"/caf\xe9\\x");
        let path = remote_path(Path::new(dir)).unwrap();
        assert_eq!(path.as_os_str().as_bytes(), b"/caf\xe9\\x");
    }
}
