//! Parsing of `/pod/container/path` addresses.

use std::fmt;
use std::path::{Path, PathBuf};

use remotefs::RemoteResult;

use crate::utils::path as path_utils;

/// A multi-pod address split into its pod, container, and in-container path.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct KubePath {
    pub pod: Option<String>,
    pub container: Option<String>,
    /// Absolute path inside the container; `None` at the root, pod, or
    /// container level.
    pub path: Option<PathBuf>,
}

impl fmt::Display for KubePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut path = String::from("/");
        if let Some(pod) = &self.pod {
            path.push_str(pod);
            path.push('/');
        }
        if let Some(container) = &self.container {
            path.push_str(container);
            path.push('/');
        }
        if let Some(p) = &self.path {
            path.push_str(p.to_string_lossy().trim_start_matches('/'));
        }
        write!(f, "{path}")
    }
}

impl KubePath {
    /// Parse an absolute `/pod/container/path/to/file` address.
    ///
    /// Fails with `InvalidPath` when `path` is not an absolute POSIX path.
    pub fn parse(path: &Path) -> RemoteResult<Self> {
        let path = path_utils::ensure_posix_absolute(path)?;
        let mut p = KubePath::default();
        let mut parts = path.iter().skip(1).map(|part| part.to_string_lossy());
        if let Some(pod) = parts.next() {
            p.pod = Some(pod.trim_matches('/').to_string());
        }
        if let Some(container) = parts.next() {
            p.container = Some(container.trim_matches('/').to_string());
        }
        let mut inner = PathBuf::from("/");
        for part in parts {
            inner = path_utils::join(&inner, &part);
        }
        if inner != Path::new("/") {
            p.path = Some(inner);
        }
        Ok(p)
    }
}

#[cfg(test)]
mod test {

    use std::path::{Path, PathBuf};

    use pretty_assertions::assert_eq;
    use remotefs::RemoteErrorType;

    use super::*;

    #[test]
    fn test_parse_absolute_path() {
        let p = KubePath::parse(Path::new("/pod/container/path/to/file")).unwrap();
        assert_eq!(p.pod, Some("pod".to_string()));
        assert_eq!(p.container, Some("container".to_string()));
        assert_eq!(p.path, Some(PathBuf::from("/path/to/file")));
        assert_eq!(p.path.unwrap().to_string_lossy(), "/path/to/file");

        let p = KubePath::parse(Path::new("/pod/container")).unwrap();
        assert_eq!(p.pod, Some("pod".to_string()));
        assert_eq!(p.container, Some("container".to_string()));
        assert!(p.path.is_none());

        let p = KubePath::parse(Path::new("/pod")).unwrap();
        assert_eq!(p.pod, Some("pod".to_string()));
        assert!(p.container.is_none());
        assert!(p.path.is_none());

        let p = KubePath::parse(Path::new("/")).unwrap();
        assert!(p.pod.is_none());
        assert!(p.container.is_none());
        assert!(p.path.is_none());
    }

    #[test]
    fn test_reject_relative_path() {
        assert_eq!(
            KubePath::parse(Path::new("pod/container/file"))
                .unwrap_err()
                .kind(),
            RemoteErrorType::InvalidPath
        );
    }

    #[test]
    fn test_display() {
        let p = KubePath::parse(Path::new("/pod/container/path/to/file")).unwrap();
        assert_eq!(p.to_string(), "/pod/container/path/to/file");
        assert_eq!(
            KubePath::parse(Path::new("/pod")).unwrap().to_string(),
            "/pod/"
        );
    }
}
