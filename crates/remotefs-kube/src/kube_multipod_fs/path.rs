use std::fmt;
use std::path::{Path, PathBuf};

#[derive(Default, Clone)]
pub struct KubePath {
    pub pod: Option<String>,
    pub container: Option<String>,
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
            path.push_str(p.to_string_lossy().as_ref());
        }
        write!(f, "{}", path)
    }
}

impl KubePath {
    /// Get Kube Path from a path, using the current pod and container.
    pub fn from_path(pod: Option<&str>, container: Option<&str>, path: &Path) -> Self {
        if path.is_absolute() {
            Self::from_absolute_path(path)
        } else {
            Self::from_relative_path(pod, container, path)
        }
    }

    /// Get Kube Path from an absolute resource path.
    ///
    /// The syntax is `/pod/container/path/to/file`
    fn from_absolute_path(path: &Path) -> Self {
        let mut p = KubePath::default();

        let mut parts = path.iter();
        parts.next(); // skip the root

        if let Some(pod) = parts.next() {
            p.pod = Some(pod.to_string_lossy().trim_matches('/').to_string());
        }
        if let Some(container) = parts.next() {
            p.container = Some(container.to_string_lossy().trim_matches('/').to_string());
        }

        // path must be absolute in this case
        let mut path = PathBuf::from("/");
        for part in parts {
            path.push(part);
        }
        // if the path is not empty, set it
        if path != Path::new("/") {
            p.path = Some(path);
        }
        p
    }

    /// Get Kube Path from a relative path, using the current pod and container.
    fn from_relative_path(pod: Option<&str>, container: Option<&str>, path: &Path) -> Self {
        let mut p = KubePath::default();

        if pod.is_none() && container.is_some() {
            panic!("Cannot specify a container without a pod");
        }

        let mut parts = path.iter();
        if let Some(pod) = pod {
            p.pod = Some(pod.to_string());
        } else if let Some(pod) = parts.next() {
            p.pod = Some(pod.to_string_lossy().trim_matches('/').to_string());
        }

        if let Some(container) = container {
            p.container = Some(container.to_string());
        } else if let Some(container) = parts.next() {
            p.container = Some(container.to_string_lossy().trim_matches('/').to_string());
        }

        // if pod and container are not specified, the path must be trated as absolute
        let path = if container.is_none() {
            let mut p = PathBuf::from("/");
            for part in parts {
                p.push(part);
            }

            p
        } else {
            parts.collect::<PathBuf>()
        };
        // if the path is not empty, set it
        if path != Path::new("/") {
            p.path = Some(path);
        }

        p
    }
}

#[cfg(test)]
mod test {

    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn test_from_absolute_path() {
        let path = Path::new("/pod/container/path/to/file");
        let p = KubePath::from_path(None, None, path);
        assert_eq!(p.pod, Some("pod".to_string()));
        assert_eq!(p.container, Some("container".to_string()));
        assert_eq!(p.path, Some(PathBuf::from("/path/to/file")));

        let path = Path::new("/pod/container");

        let p = KubePath::from_path(None, None, path);
        assert_eq!(p.pod, Some("pod".to_string()));
        assert_eq!(p.container, Some("container".to_string()));

        let path = Path::new("/pod");

        let p = KubePath::from_path(None, None, path);
        assert_eq!(p.pod, Some("pod".to_string()));
        assert!(p.container.is_none());
        assert!(p.path.is_none());

        let path = Path::new("/");

        let p = KubePath::from_path(None, None, path);
        assert!(p.pod.is_none());
        assert!(p.container.is_none());
        assert!(p.path.is_none());
    }

    #[test]
    fn test_relative_path() {
        let path = Path::new("path/to/file");
        let p = KubePath::from_path(Some("pod"), Some("container"), path);
        assert_eq!(p.pod, Some("pod".to_string()));
        assert_eq!(p.container, Some("container".to_string()));
        assert_eq!(p.path, Some(PathBuf::from("path/to/file")));

        let path = Path::new("container/path/to/file");
        let p = KubePath::from_path(Some("pod"), None, path);
        assert_eq!(p.pod, Some("pod".to_string()));
        assert_eq!(p.container, Some("container".to_string()));
        assert_eq!(p.path, Some(PathBuf::from("/path/to/file")));

        let path = Path::new("pod/container/path/to/file");
        let p = KubePath::from_path(None, None, path);
        assert_eq!(p.pod, Some("pod".to_string()));
        assert_eq!(p.container, Some("container".to_string()));
        assert_eq!(p.path, Some(PathBuf::from("/path/to/file")));
    }

    #[test]
    #[should_panic]
    fn test_relative_path_panic() {
        let path = Path::new("path/to/file");
        KubePath::from_path(None, Some("container"), path);
    }
}
