//! ## Metadata
//!
//! file metadata

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
use super::UnixPex;

use std::{
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

/// File metadata
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Metadata {
    /// Last access time
    pub atime: SystemTime,
    /// Creation time
    pub ctime: SystemTime,
    /// Group id
    pub gid: Option<u32>,
    /// Unix permissions
    pub mode: Option<UnixPex>,
    /// Modify time
    pub mtime: SystemTime,
    /// File size in bytes
    pub size: usize,
    /// If file is symlink, contains the path of the file it is pointing to
    pub symlink: Option<PathBuf>,
    /// User id
    pub uid: Option<u32>,
}

impl Default for Metadata {
    fn default() -> Self {
        Self {
            atime: UNIX_EPOCH,
            ctime: UNIX_EPOCH,
            gid: None,
            mode: None,
            mtime: UNIX_EPOCH,
            size: 0,
            symlink: None,
            uid: None,
        }
    }
}

impl Metadata {
    /// Construct metadata with atime
    pub fn atime(mut self, atime: SystemTime) -> Self {
        self.atime = atime;
        self
    }

    /// Construct metadata with ctime
    pub fn ctime(mut self, ctime: SystemTime) -> Self {
        self.ctime = ctime;
        self
    }

    /// Construct metadata with group id
    pub fn gid(mut self, gid: u32) -> Self {
        self.gid = Some(gid);
        self
    }

    /// Construct metadata with UNIX permissions
    pub fn mode(mut self, mode: UnixPex) -> Self {
        self.mode = Some(mode);
        self
    }

    /// Construct metadata with modify time
    pub fn mtime(mut self, mtime: SystemTime) -> Self {
        self.mtime = mtime;
        self
    }

    /// Construct metadata with file size
    pub fn size(mut self, size: usize) -> Self {
        self.size = size;
        self
    }

    /// Construct metadata with symlink
    pub fn symlink<P: AsRef<Path>>(mut self, p: P) -> Self {
        self.symlink = Some(p.as_ref().to_path_buf());
        self
    }

    /// Construct metadata with user id
    pub fn uid(mut self, uid: u32) -> Self {
        self.uid = Some(uid);
        self
    }
}

#[cfg(test)]
mod test {

    use super::super::UnixPexClass;
    use super::*;

    use pretty_assertions::assert_eq;
    use std::time::Duration;

    #[test]
    fn should_initialize_metadata() {
        let metadata = Metadata::default();
        assert_eq!(metadata.atime, UNIX_EPOCH);
        assert_eq!(metadata.ctime, UNIX_EPOCH);
        assert!(metadata.gid.is_none());
        assert!(metadata.mode.is_none());
        assert_eq!(metadata.mtime, UNIX_EPOCH);
        assert_eq!(metadata.size, 0);
        assert!(metadata.symlink.is_none());
        assert!(metadata.uid.is_none());
    }

    #[test]
    fn should_construct_metadata() {
        let atime = UNIX_EPOCH.checked_add(Duration::from_secs(86400)).unwrap();
        let ctime = UNIX_EPOCH
            .checked_add(Duration::from_secs(4238673))
            .unwrap();
        let mtime = UNIX_EPOCH
            .checked_add(Duration::from_secs(9048045687))
            .unwrap();
        let metadata = Metadata::default()
            .atime(atime)
            .ctime(ctime)
            .gid(14)
            .mode(UnixPex::new(
                UnixPexClass::from(6),
                UnixPexClass::from(4),
                UnixPexClass::from(0),
            ))
            .mtime(mtime)
            .size(1024)
            .symlink(Path::new("/tmp/a.txt"))
            .uid(10);
        assert_eq!(metadata.atime, atime);
        assert_eq!(metadata.ctime, ctime);
        assert_eq!(metadata.gid.unwrap(), 14);
        assert!(metadata.mode.is_some());
        assert_eq!(metadata.mtime, mtime);
        assert_eq!(metadata.size, 1024);
        assert_eq!(
            metadata.symlink.as_deref().unwrap(),
            Path::new("/tmp/a.txt")
        );
        assert_eq!(metadata.uid.unwrap(), 10);
    }
}
