//! ## File
//!
//! file system types related to file entries and directories

// -- ext
use std::path::{Path, PathBuf};

// -- mod
mod file_type;
mod metadata;
mod permissions;

// -- export
pub use file_type::FileType;
pub use metadata::Metadata;
pub use permissions::{UnixPex, UnixPexClass};

/// A file represents an entity in the file system

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct File {
    /// File absolute path
    pub path: PathBuf,
    /// File metadata
    pub metadata: Metadata,
}

impl File {
    /// Get absolute path
    pub fn path(&self) -> &Path {
        self.path.as_path()
    }

    /// Get file name
    pub fn name(&self) -> String {
        self.path()
            .file_name()
            .map(|x| x.to_string_lossy().to_string())
            .unwrap_or_else(|| "/".to_string())
    }

    /// Get metadata
    pub fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    /// Get file type, if defined
    pub fn extension(&self) -> Option<String> {
        self.path()
            .extension()
            .map(|x| x.to_string_lossy().to_string())
    }

    /// Returns whether the file is a directory
    pub fn is_dir(&self) -> bool {
        self.metadata().is_dir()
    }

    /// Returns whether the file is a regular file
    pub fn is_file(&self) -> bool {
        self.metadata().is_file()
    }

    /// Returns whether the file is a symbolic link
    pub fn is_symlink(&self) -> bool {
        self.metadata().is_symlink()
    }

    /// Returns whether file is hidden
    pub fn is_hidden(&self) -> bool {
        self.name().starts_with('.')
    }
}

#[cfg(test)]
mod tests {

    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn should_create_file() {
        let entry = File {
            path: PathBuf::from("/bar.txt"),
            metadata: Metadata::default(),
        };
        assert_eq!(entry.path(), Path::new("/bar.txt"));
        assert_eq!(entry.name(), String::from("bar.txt"));
        assert_eq!(entry.extension().as_deref(), Some("txt"));
        assert_eq!(entry.metadata(), &Metadata::default());
        assert_eq!(entry.is_dir(), false);
        assert_eq!(entry.is_file(), true);
        assert_eq!(entry.is_hidden(), false);
    }

    #[test]
    fn should_return_is_hidden_for_hidden_files() {
        let entry = File {
            path: PathBuf::from("/.bar.txt"),
            metadata: Metadata::default(),
        };
        assert_eq!(entry.is_hidden(), true);
    }
}
