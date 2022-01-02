//! ## File
//!
//! file system types related to file entries and directories

/**
 * MIT License
 *
 * remotefs - Copyright (c) 2021 Christian Visintin
 *
 * Permission is hereby granted, free of charge, to any person obtaining a copy
 * of this software and associated documentation files (the "Software"), to deal
 * in the Software without restriction, including without limitation the rights
 * to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
 * copies of the Software, and to permit persons to whom the Software is
 * furnished to do so, subject to the following conditions:
 *
 * The above copyright notice and this permission notice shall be included in all
 * copies or substantial portions of the Software.
 *
 * THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
 * IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
 * FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
 * AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
 * LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
 * OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
 * SOFTWARE.
 */
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

/// A file system entity represents an entity in the file system

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FsEntity {
    /// File absolute path
    pub path: PathBuf,
    /// File metadata
    pub metadata: Metadata,
    /// File type
    pub type_: FileType,
}

impl FsEntity {
    /// Get absolute path
    pub fn path(&self) -> &Path {
        self.path.as_path()
    }

    /// Get file name
    pub fn name(&self) -> String {
        self.path()
            .file_name()
            .map(|x| x.to_string_lossy().to_string())
            .unwrap_or("/".to_string())
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
        self.type_.is_dir()
    }

    /// Returns whether the file is a regular file
    pub fn is_file(&self) -> bool {
        self.type_.is_file()
    }

    /// Returns whether the file is a symbolic link
    pub fn is_symlink(&self) -> bool {
        self.type_.is_symlink()
    }

    /// Returns whether file is hidden
    pub fn is_hidden(&self) -> bool {
        self.name().starts_with('.')
    }
}

#[cfg(test)]
mod tests {

    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn should_create_fs_dir() {
        let entry: Entry = Entry::Directory(Directory {
            name: String::from("foo"),
            path: PathBuf::from("/foo"),
            metadata: Metadata::default(),
        });
        assert_eq!(entry.metadata().size, 0);
        assert_eq!(entry.is_dir(), true);
        assert_eq!(entry.is_file(), false);
        assert_eq!(entry.unwrap_dir().path, PathBuf::from("/foo"));
    }

    #[test]
    fn should_create_fs_file() {
        let entry: Entry = Entry::File(File {
            name: String::from("bar.txt"),
            path: PathBuf::from("/bar.txt"),
            extension: Some(String::from("txt")),
            metadata: Metadata::default(),
        });
        assert_eq!(entry.path(), Path::new("/bar.txt"));
        assert_eq!(entry.name(), String::from("bar.txt"));
        assert_eq!(entry.extension(), Some("txt"));
        assert_eq!(entry.is_dir(), false);
        assert_eq!(entry.is_file(), true);
        assert_eq!(entry.unwrap_file().path, PathBuf::from("/bar.txt"));
    }

    #[test]
    #[should_panic]
    fn should_fail_unwrapping_directory() {
        let entry: Entry = Entry::File(File {
            name: String::from("bar.txt"),
            path: PathBuf::from("/bar.txt"),
            metadata: Metadata::default(),
            extension: Some(String::from("txt")),
        });
        entry.unwrap_dir();
    }

    #[test]
    #[should_panic]
    fn should_fail_unwrapping_file() {
        let entry: Entry = Entry::Directory(Directory {
            name: String::from("foo"),
            path: PathBuf::from("/foo"),
            metadata: Metadata::default(),
        });
        entry.unwrap_file();
    }

    #[test]
    fn should_return_is_hidden_for_hidden_files() {
        let entry: Entry = Entry::File(File {
            name: String::from("bar.txt"),
            path: PathBuf::from("/bar.txt"),
            metadata: Metadata::default(),
            extension: Some(String::from("txt")),
        });
        assert_eq!(entry.is_hidden(), false);
        let entry: Entry = Entry::File(File {
            name: String::from(".gitignore"),
            path: PathBuf::from("/.gitignore"),
            metadata: Metadata::default(),
            extension: Some(String::from("txt")),
        });
        assert_eq!(entry.is_hidden(), true);
        let entry: Entry = Entry::Directory(Directory {
            name: String::from(".git"),
            path: PathBuf::from("/.git"),
            metadata: Metadata::default(),
        });
        assert_eq!(entry.is_hidden(), true);
    }
}
