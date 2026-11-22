//! ## S3 object
//!
//! This module exposes the intermediate representation used to map S3 objects
//! into [`remotefs::File`] values.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use aws_sdk_s3::types::Object;
use remotefs::File;
use remotefs::fs::{FileType, Metadata};

use crate::key;

/// An intermediate representation of an S3 object.
#[derive(Debug)]
pub struct S3Object {
    /// The final component of the S3 key.
    pub name: String,
    /// The absolute remote path.
    pub path: PathBuf,
    /// The object size in bytes.
    pub size: u64,
    /// The last modification time reported by S3.
    pub last_modified: SystemTime,
    /// Whether the object represents a directory marker.
    pub is_dir: bool,
}

impl From<Object> for S3Object {
    fn from(obj: Object) -> Self {
        let key = obj.key.clone().unwrap_or_default();
        let is_dir = key.ends_with('/');
        let path = key::to_path(&key);
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
        File::new(obj.path.clone(), obj.into())
    }
}

impl From<S3Object> for Metadata {
    fn from(obj: S3Object) -> Self {
        let file_type = if obj.is_dir {
            FileType::Directory
        } else {
            FileType::File
        };
        Metadata::default()
            .file_type(file_type)
            .modified(obj.last_modified)
            .size(obj.size)
    }
}

impl S3Object {
    /// Get the object name from an S3 key.
    pub fn object_name(key: &str) -> String {
        let mut tokens = key.split('/');
        let count = tokens.clone().count();
        let demi_last = match count > 1 {
            true => tokens.nth(count - 2).unwrap_or_default().to_string(),
            false => String::new(),
        };
        if let Some(last) = tokens.next_back()
            && !last.is_empty()
        {
            return last.to_string();
        }
        demi_last
    }
}

#[cfg(test)]
mod test {
    use std::path::Path;

    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn should_make_file_from_s3_object() {
        let obj = S3Object {
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
        assert_eq!(entry.metadata.size, Some(1516966));
        assert_eq!(entry.extension().unwrap().as_str(), "gif");
        assert_eq!(entry.metadata.uid, None);
        assert_eq!(entry.metadata.gid, None);
        assert_eq!(entry.metadata.mode, None);
    }

    #[test]
    fn should_make_directory_from_s3_object() {
        let obj = S3Object {
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
        assert_eq!(entry.metadata.size, Some(0));
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
