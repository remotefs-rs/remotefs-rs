//! ## S3 object
//!
//! This module exposes the S3Object structure, which is an intermediate structure to work with
//! S3 objects. Easy to be converted into a FsEntry.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use aws_sdk_s3::types::Object;
use remotefs::File;
use remotefs::fs::{FileType, Metadata};

use crate::utils::path as path_utils;

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

impl From<Object> for S3Object {
    fn from(obj: Object) -> Self {
        let key = obj.key.clone().unwrap_or_default();

        let is_dir: bool = key.ends_with('/');
        let path: PathBuf = path_utils::absolutize(
            PathBuf::from("/").as_path(),
            PathBuf::from(key.as_str()).as_path(),
        );
        let last_modified = obj
            .last_modified()
            .map(|dt| dt.to_millis().unwrap_or_default())
            .map_or(UNIX_EPOCH, |ms| {
                UNIX_EPOCH + std::time::Duration::from_millis(ms as u64)
            });

        Self {
            name: Self::object_name(key.as_str()),
            path,
            size: obj.size().unwrap_or_default() as u64,
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
            accessed: None,
            created: None,
            file_type: if obj.is_dir {
                FileType::Directory
            } else {
                FileType::File
            },
            gid: None,
            mode: None,
            modified: Some(obj.last_modified),
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

    use pretty_assertions::assert_eq;

    use super::*;

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
        assert_eq!(entry.metadata.accessed, None);
        assert_eq!(entry.metadata.created, None);
        assert_eq!(entry.metadata.modified, Some(UNIX_EPOCH));
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
        assert_eq!(entry.metadata.accessed, None);
        assert_eq!(entry.metadata.created, None);
        assert_eq!(entry.metadata.modified, Some(UNIX_EPOCH));
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
