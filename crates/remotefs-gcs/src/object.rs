use std::path::PathBuf;
use std::time::SystemTime;

use google_cloud_storage::model::Object;
use remotefs::fs::{FileType, Metadata};
use remotefs::{File, RemoteError, RemoteErrorType};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct GcsObject {
    path: PathBuf,
    metadata: Metadata,
}

impl GcsObject {
    pub(crate) fn directory(prefix: &str) -> Self {
        let name = prefix.trim_matches('/');
        let path = if name.is_empty() {
            PathBuf::from("/")
        } else {
            PathBuf::from(format!("/{name}"))
        };

        Self {
            path,
            metadata: Metadata {
                file_type: FileType::Directory,
                ..Metadata::default()
            },
        }
    }

    pub(crate) fn into_file(self) -> File {
        File {
            path: self.path,
            metadata: self.metadata,
        }
    }
}

impl TryFrom<Object> for GcsObject {
    type Error = RemoteError;

    fn try_from(object: Object) -> Result<Self, Self::Error> {
        if object.size < 0 {
            return Err(RemoteError::new_ex(
                RemoteErrorType::ProtocolError,
                format!("object {} has a negative size", object.name),
            ));
        }

        let is_directory = object.name.ends_with('/');
        let name = object.name.trim_matches('/');
        if name.is_empty() {
            return Err(RemoteError::new_ex(
                RemoteErrorType::ProtocolError,
                "object name is empty",
            ));
        }

        let metadata = Metadata {
            created: object
                .create_time
                .and_then(|value| SystemTime::try_from(value).ok()),
            file_type: if is_directory {
                FileType::Directory
            } else {
                FileType::File
            },
            modified: object
                .update_time
                .and_then(|value| SystemTime::try_from(value).ok()),
            size: if is_directory {
                0
            } else {
                object.size.cast_unsigned()
            },
            ..Metadata::default()
        };

        Ok(Self {
            path: PathBuf::from(format!("/{name}")),
            metadata,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use google_cloud_storage::model::Object;

    use super::GcsObject;

    #[test]
    fn object_metadata_becomes_a_remote_file() {
        let object = Object::new().set_name("docs/readme.md").set_size(12_i64);
        let file = GcsObject::try_from(object).unwrap().into_file();
        assert_eq!(file.path(), Path::new("/docs/readme.md"));
        assert_eq!(file.metadata().size, 12);
        assert!(file.is_file());
    }

    #[test]
    fn prefix_becomes_a_directory() {
        let file = GcsObject::directory("docs/").into_file();
        assert_eq!(file.path(), Path::new("/docs"));
        assert!(file.is_dir());
    }

    #[test]
    fn negative_sizes_are_rejected() {
        let object = Object::new().set_name("broken").set_size(-1_i64);
        let error = GcsObject::try_from(object).unwrap_err();
        assert_eq!(error.kind, remotefs::RemoteErrorType::ProtocolError);
    }
}
