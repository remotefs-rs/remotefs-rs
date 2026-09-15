//! Share-rooted remote path handling shared by every client.
//!
//! Every remotefs 1 operation receives an absolute path. SMB clients address
//! entries relative to the connected share, so the accepted form is a POSIX
//! path rooted at `/` (the share root). Parsing validates the root with
//! [`remotefs::path::ensure_absolute`], drops `.` and empty components, and
//! rejects `..` and backslashes, which are not valid inside SMB names.

use std::path::{Path, PathBuf};

use remotefs::path::ensure_absolute;
use remotefs::{RemoteError, RemoteErrorType, RemoteResult};

/// A validated, normalized path inside the connected share.
///
/// # Examples
///
/// ```ignore
/// let path = SharePath::parse(Path::new("/docs/./report.txt"))?;
/// assert_eq!(path.relative(), "docs/report.txt");
/// # Ok::<(), remotefs::RemoteError>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharePath {
    components: Vec<String>,
}

impl SharePath {
    /// Parses an absolute, share-rooted remotefs path.
    ///
    /// # Errors
    ///
    /// Returns [`RemoteErrorType::InvalidPath`] when the path is not
    /// absolute, is not rooted at `/`, contains a `..` component, or
    /// contains a backslash inside a component.
    pub fn parse(path: &Path) -> RemoteResult<Self> {
        ensure_absolute(path)?;
        let text = path.to_string_lossy();
        let Some(rest) = text.strip_prefix('/') else {
            return Err(invalid("path must be rooted at the share (`/`)"));
        };
        let mut components = Vec::new();
        for component in rest.split('/') {
            match component {
                "" | "." => {}
                ".." => return Err(invalid("`..` components are not allowed")),
                name if name.contains('\\') => {
                    return Err(invalid("backslash is not a valid SMB name character"));
                }
                name => components.push(name.to_string()),
            }
        }
        Ok(Self { components })
    }

    /// Returns whether this is the share root.
    #[cfg_attr(not(feature = "smb"), allow(dead_code))]
    pub fn is_root(&self) -> bool {
        self.components.is_empty()
    }

    /// Returns the `/`-separated path relative to the share root
    /// (empty for the root).
    pub fn relative(&self) -> String {
        self.join("/")
    }

    /// Joins the components with `separator` (empty for the root).
    pub fn join(&self, separator: &str) -> String {
        self.components.join(separator)
    }

    /// Returns the normalized `/`-rooted absolute path.
    pub fn to_path_buf(&self) -> PathBuf {
        PathBuf::from(format!("/{relative}", relative = self.relative()))
    }

    /// Returns the last component, if any.
    #[cfg_attr(not(target_family = "windows"), allow(dead_code))]
    pub fn name(&self) -> Option<&str> {
        self.components.last().map(String::as_str)
    }

    /// Returns the path of a direct child.
    #[cfg_attr(not(any(feature = "smb", target_family = "windows")), allow(dead_code))]
    pub fn child(&self, name: &str) -> Self {
        let mut components = self.components.clone();
        components.push(name.to_string());
        Self { components }
    }

    /// Returns whether `other` is this path or one of its descendants.
    #[cfg_attr(not(any(feature = "smb", target_family = "windows")), allow(dead_code))]
    pub fn is_same_or_descendant(&self, other: &Self) -> bool {
        other.components.starts_with(&self.components)
    }
}

fn invalid(message: &str) -> RemoteError {
    RemoteError::with_message(RemoteErrorType::InvalidPath, message)
}

#[cfg(test)]
mod test {
    use std::path::{Path, PathBuf};

    use pretty_assertions::assert_eq;
    use remotefs::RemoteErrorType;

    use super::*;

    #[test]
    fn should_parse_root() {
        let path = SharePath::parse(Path::new("/")).unwrap();
        assert!(path.is_root());
        assert_eq!(path.relative(), "");
        assert_eq!(path.join("\\"), "");
        assert_eq!(path.to_path_buf(), PathBuf::from("/"));
        assert_eq!(path.name(), None);
    }

    #[test]
    fn should_normalize_components() {
        let path = SharePath::parse(Path::new("//cargo-test/./a.txt/")).unwrap();
        assert!(!path.is_root());
        assert_eq!(path.relative(), "cargo-test/a.txt");
        assert_eq!(path.join("\\"), r"cargo-test\a.txt");
        assert_eq!(path.to_path_buf(), PathBuf::from("/cargo-test/a.txt"));
        assert_eq!(path.name(), Some("a.txt"));
    }

    #[test]
    fn should_build_children() {
        let parent = SharePath::parse(Path::new("/cargo-test")).unwrap();
        let child = parent.child("b.txt");
        assert_eq!(child.to_path_buf(), PathBuf::from("/cargo-test/b.txt"));
        let root_child = SharePath::parse(Path::new("/")).unwrap().child("x");
        assert_eq!(root_child.to_path_buf(), PathBuf::from("/x"));
    }

    #[test]
    fn should_detect_same_or_descendant_paths() {
        let source = SharePath::parse(Path::new("/cargo-test")).unwrap();
        assert!(source.is_same_or_descendant(&source));
        assert!(source
            .is_same_or_descendant(&SharePath::parse(Path::new("/cargo-test/nested")).unwrap()));
        assert!(
            !source.is_same_or_descendant(&SharePath::parse(Path::new("/cargo-testing")).unwrap())
        );
    }

    #[test]
    fn should_reject_relative_paths() {
        for input in ["", ".", "a.txt", "cargo-test/a.txt"] {
            let error = SharePath::parse(Path::new(input)).unwrap_err();
            assert_eq!(error.kind(), RemoteErrorType::InvalidPath, "{input:?}");
        }
    }

    #[test]
    fn should_reject_non_share_roots_and_bad_components() {
        for input in [r"C:\cargo", r"\\server\share\x", "/a/../b", r"/a\b"] {
            let error = SharePath::parse(Path::new(input)).unwrap_err();
            assert_eq!(error.kind(), RemoteErrorType::InvalidPath, "{input:?}");
        }
    }
}
