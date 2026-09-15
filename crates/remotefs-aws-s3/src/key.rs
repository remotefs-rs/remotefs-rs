//! ## Key
//!
//! Conversion between absolute remote paths and S3 object keys.
//!
//! S3 has no directories. A "directory" is an object whose key ends with `/`,
//! and the bucket root is the empty prefix. Only POSIX paths rooted at `/` are
//! accepted: Windows drive and UNC roots pass `ensure_absolute` but have no
//! meaning as an S3 key and are rejected as `InvalidPath`.

use std::path::{Path, PathBuf};

#[cfg(target_os = "windows")]
use path_slash::PathExt as _;
use remotefs::path::ensure_absolute;
use remotefs::{RemoteError, RemoteErrorType, RemoteResult};

/// Build the S3 object key for an absolute remote path.
///
/// Directory keys carry a trailing `/`; the root maps to the empty prefix.
pub(crate) fn from_path(path: &Path, is_dir: bool) -> RemoteResult<String> {
    ensure_absolute(path)?;
    #[cfg(target_os = "windows")]
    let text = path.to_slash_lossy().to_string();
    #[cfg(not(target_os = "windows"))]
    let text = path.to_string_lossy().to_string();
    let Some(stripped) = text.strip_prefix('/') else {
        return Err(RemoteError::with_message(
            RemoteErrorType::InvalidPath,
            "s3 paths must start with '/'",
        ));
    };
    let key = stripped.trim_matches('/');
    if key.is_empty() {
        return Ok(String::new());
    }
    Ok(if is_dir {
        format!("{key}/")
    } else {
        key.to_string()
    })
}

/// Build the absolute remote path for an S3 object key.
pub(crate) fn to_path(key: &str) -> PathBuf {
    PathBuf::from(format!("/{key}", key = key.trim_end_matches('/')))
}

#[cfg(test)]
mod test {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn should_build_file_key() {
        assert_eq!(
            from_path(Path::new("/tmp/omar.txt"), false).unwrap(),
            "tmp/omar.txt"
        );
        assert_eq!(from_path(Path::new("/tmp/"), false).unwrap(), "tmp");
        assert_eq!(from_path(Path::new("//tmp"), false).unwrap(), "tmp");
    }

    #[test]
    fn should_build_dir_key() {
        assert_eq!(
            from_path(Path::new("/tmp/subfolder"), true).unwrap(),
            "tmp/subfolder/"
        );
        assert_eq!(from_path(Path::new("/tmp/"), true).unwrap(), "tmp/");
        assert_eq!(from_path(Path::new("/"), true).unwrap(), "");
        assert_eq!(from_path(Path::new("/"), false).unwrap(), "");
    }

    #[test]
    fn should_reject_relative_and_non_posix_paths() {
        for input in [
            "",
            "omar.txt",
            "tmp/",
            r"C:\tmp\file",
            r"\\server\share\file",
        ] {
            let error = from_path(Path::new(input), false).unwrap_err();
            assert_eq!(
                error.kind(),
                RemoteErrorType::InvalidPath,
                "input: {input:?}"
            );
        }
    }

    #[test]
    fn should_build_path_from_key() {
        assert_eq!(to_path("tmp/omar.txt"), PathBuf::from("/tmp/omar.txt"));
        assert_eq!(to_path("tmp/subfolder/"), PathBuf::from("/tmp/subfolder"));
        assert_eq!(to_path(""), PathBuf::from("/"));
    }
}
