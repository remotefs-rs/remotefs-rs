//! ## File type
//!
//! represents the file type

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
use std::fs::FileType as StdFileType;

/// Describes the file type (directory, regular file or symlink)
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum FileType {
    /// A directory
    Directory,
    /// Regular file
    File,
    /// Symbolic link. If the file is a symlink pointing to a directory,
    /// this will be still considered a Symlink.
    Symlink,
}

impl Default for FileType {
    fn default() -> Self {
        Self::File
    }
}

impl FileType {
    /// Returns whether file is a directory
    pub fn is_dir(&self) -> bool {
        matches!(self, Self::Directory)
    }

    /// Returns whether file is a regular file
    pub fn is_file(&self) -> bool {
        matches!(self, Self::File)
    }

    /// Returns whether file is symlink
    pub fn is_symlink(&self) -> bool {
        matches!(self, Self::Symlink)
    }
}

impl From<StdFileType> for FileType {
    fn from(t: StdFileType) -> Self {
        if t.is_symlink() {
            Self::Symlink
        } else if t.is_dir() {
            Self::Directory
        } else {
            Self::File
        }
    }
}

#[cfg(test)]
mod test {

    use super::*;

    use pretty_assertions::assert_eq;

    #[test]
    fn should_check_file_type() {
        assert_eq!(FileType::Directory.is_dir(), true);
        assert_eq!(FileType::Directory.is_file(), false);
        assert_eq!(FileType::Directory.is_symlink(), false);
        assert_eq!(FileType::File.is_dir(), false);
        assert_eq!(FileType::File.is_file(), true);
        assert_eq!(FileType::File.is_symlink(), false);
        assert_eq!(FileType::Symlink.is_dir(), false);
        assert_eq!(FileType::Symlink.is_file(), false);
        assert_eq!(FileType::Symlink.is_symlink(), true);
    }
}
