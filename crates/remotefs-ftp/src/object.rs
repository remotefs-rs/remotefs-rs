//! ## S3 object
//!
//! This module exposes the S3Object structure, which is an intermediate structure to work with
//! S3 objects. Easy to be converted into a FsEntry.

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
use crate::utils::parser as parser_utils;
use crate::utils::path as path_utils;
use remotefs::fs::{FileType, Metadata};
use remotefs::File;

use s3::serde_types::Object;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// An intermediate struct to work with s3 `Object`.
/// Really easy to be converted into a `FsEntry`
#[derive(Debug)]
pub struct S3Object {
    pub name: String,
    pub path: PathBuf,
    pub size: u64,
    pub last_modified: SystemTime,
    /// Whether or not represents a directory. I already know directories don't exist in s3!
    pub is_dir: bool,
}

impl From<&Object> for S3Object {
    fn from(obj: &Object) -> Self {
        let is_dir: bool = obj.key.ends_with('/');
        let path: PathBuf = path_utils::absolutize(
            PathBuf::from("/").as_path(),
            PathBuf::from(obj.key.as_str()).as_path(),
        );
        let last_modified: SystemTime =
            match parser_utils::parse_datetime(obj.last_modified.as_str(), "%Y-%m-%dT%H:%M:%S%Z") {
                Ok(dt) => dt,
                Err(_) => UNIX_EPOCH,
            };
        Self {
            name: Self::object_name(obj.key.as_str()),
            path,
            size: obj.size,
            last_modified,
            is_dir,
        }
    }
}

impl From<S3Object> for File {
    fn from(obj: S3Object) -> Self {
        let path: PathBuf = path_utils::absolutize(Path::new("/"), obj.path.as_path());
        File {
            path,
            metadata: obj.into(),
        }
    }
}

impl From<S3Object> for Metadata {
    fn from(obj: S3Object) -> Self {
        Self {
            accessed: SystemTime::UNIX_EPOCH,
            created: SystemTime::UNIX_EPOCH,
            file_type: if obj.is_dir {
                FileType::Directory
            } else {
                FileType::File
            },
            gid: None,
            mode: None,
            modified: obj.last_modified,
            size: obj.size,
            symlink: None,
            uid: None,
        }
    }
}

impl S3Object {
    /// Get object name from key
    pub fn object_name(key: &str) -> String {
        let mut tokens = key.split('/');
        let count = tokens.clone().count();
        let demi_last: String = match count > 1 {
            true => tokens.nth(count - 2).unwrap().to_string(),
            false => String::new(),
        };
        if let Some(last) = tokens.last() {
            // If last is not empty, return last one
            if !last.is_empty() {
                return last.to_string();
            }
        }
        // Return demi last
        demi_last
    }
}

#[cfg(test)]
mod test {

    use super::*;

    use pretty_assertions::assert_eq;
    use std::time::Duration;

    #[test]
    fn should_make_object_into_s3object_file() {
        let obj: Object = Object {
            key: String::from("pippo/sottocartella/chiedo.gif"),
            e_tag: String::default(),
            size: 1516966,
            owner: None,
            storage_class: None,
            last_modified: String::from("2021-08-28T10:20:37.000Z"),
        };
        let s3_obj: S3Object = S3Object::from(&obj);
        assert_eq!(s3_obj.name.as_str(), "chiedo.gif");
        assert_eq!(
            s3_obj.path.as_path(),
            Path::new("/pippo/sottocartella/chiedo.gif")
        );
        assert_eq!(s3_obj.size, 1516966);
        assert_eq!(s3_obj.is_dir, false);
        assert_eq!(
            s3_obj
                .last_modified
                .duration_since(SystemTime::UNIX_EPOCH)
                .ok()
                .unwrap(),
            Duration::from_secs(1630146037)
        );
    }

    #[test]
    fn should_make_object_intoto_s3object_dir() {
        let obj: Object = Object {
            key: String::from("temp/"),
            e_tag: String::default(),
            size: 0,
            owner: None,
            storage_class: None,
            last_modified: String::from("2021-08-28T10:20:37.000Z"),
        };
        let s3_obj: S3Object = S3Object::from(&obj);
        assert_eq!(s3_obj.name.as_str(), "temp");
        assert_eq!(s3_obj.path.as_path(), Path::new("/temp"));
        assert_eq!(s3_obj.size, 0);
        assert_eq!(s3_obj.is_dir, true);
        assert_eq!(
            s3_obj
                .last_modified
                .duration_since(SystemTime::UNIX_EPOCH)
                .ok()
                .unwrap(),
            Duration::from_secs(1630146037)
        );
    }

    #[test]
    fn should_make_fsentry_from_s3obj_file() {
        let obj: S3Object = S3Object {
            name: String::from("chiedo.gif"),
            path: PathBuf::from("/pippo/sottocartella/chiedo.gif"),
            size: 1516966,
            is_dir: false,
            last_modified: UNIX_EPOCH,
        };
        let entry = File::from(obj);
        assert_eq!(entry.name().as_str(), "chiedo.gif");
        assert!(entry.is_file());
        assert_eq!(
            entry.path.as_path(),
            Path::new("/pippo/sottocartella/chiedo.gif")
        );
        assert_eq!(entry.metadata.created, UNIX_EPOCH);
        assert_eq!(entry.metadata.modified, UNIX_EPOCH);
        assert_eq!(entry.metadata.accessed, UNIX_EPOCH);
        assert_eq!(entry.metadata.size, 1516966);
        assert_eq!(entry.extension().unwrap().as_str(), "gif");
        assert_eq!(entry.metadata.uid, None);
        assert_eq!(entry.metadata.gid, None);
        assert_eq!(entry.metadata.mode, None);
    }

    #[test]
    fn should_make_fsentry_from_s3obj_directory() {
        let obj: S3Object = S3Object {
            name: String::from("temp"),
            path: PathBuf::from("/temp"),
            size: 0,
            is_dir: true,
            last_modified: UNIX_EPOCH,
        };
        let entry = File::from(obj);
        assert!(entry.is_dir());
        assert_eq!(entry.name().as_str(), "temp");
        assert_eq!(entry.path.as_path(), Path::new("/temp"));
        assert_eq!(entry.metadata.created, UNIX_EPOCH);
        assert_eq!(entry.metadata.modified, UNIX_EPOCH);
        assert_eq!(entry.metadata.accessed, UNIX_EPOCH);
        assert_eq!(entry.metadata.size, 0);
        assert_eq!(entry.metadata.uid, None);
        assert_eq!(entry.metadata.gid, None);
        assert_eq!(entry.metadata.mode, None);
    }

    #[test]
    fn should_get_object_name_from_path() {
        assert_eq!(
            S3Object::object_name("pippo/sottocartella/chiedo.gif").as_str(),
            "chiedo.gif"
        );
        assert_eq!(
            S3Object::object_name("pippo/sottocartella/").as_str(),
            "sottocartella"
        );
        assert_eq!(S3Object::object_name("pippo/").as_str(), "pippo");
    }
}
