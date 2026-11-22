//! Intermediate representation that maps storage objects onto [`File`].

use std::path::PathBuf;
use std::time::SystemTime;

use google_cloud_storage::model::Object;
use remotefs::fs::{FileType, Metadata};
use remotefs::{File, RemoteError, RemoteErrorType};

use crate::key;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct GcsObject {
    path: PathBuf,
    metadata: Metadata,
}

impl GcsObject {
    /// Creates a directory entry from an object prefix or marker name.
    pub(crate) fn directory(prefix: &str) -> Result<Self, RemoteError> {
        if !prefix.is_empty() {
            validate_name(prefix, prefix.ends_with('/'))?;
        }
        Ok(Self {
            path: key::to_path(prefix),
            metadata: Metadata::default().file_type(FileType::Directory),
        })
    }

    pub(crate) fn into_file(self) -> File {
        File::new(self.path, self.metadata)
    }
}

impl TryFrom<Object> for GcsObject {
    type Error = RemoteError;

    fn try_from(object: Object) -> Result<Self, Self::Error> {
        if object.size < 0 {
            return Err(RemoteError::with_message(
                RemoteErrorType::ProtocolError,
                format!("object {name} has a negative size", name = object.name),
            ));
        }
        let is_directory = object.name.ends_with('/');
        validate_name(&object.name, is_directory)?;
        let name = object.name.trim_matches('/');
        if name.is_empty() {
            return Err(RemoteError::with_message(
                RemoteErrorType::ProtocolError,
                "object name is empty",
            ));
        }

        let mut metadata = Metadata::default().file_type(if is_directory {
            FileType::Directory
        } else {
            FileType::File
        });
        if !is_directory {
            metadata = metadata.size(object.size.cast_unsigned());
        }
        if let Some(created) = object
            .create_time
            .and_then(|value| SystemTime::try_from(value).ok())
        {
            metadata = metadata.created(created);
        }
        if let Some(modified) = object
            .update_time
            .and_then(|value| SystemTime::try_from(value).ok())
        {
            metadata = metadata.modified(modified);
        }

        Ok(Self {
            path: key::to_path(name),
            metadata,
        })
    }
}

fn validate_name(name: &str, is_directory: bool) -> Result<(), RemoteError> {
    let path = key::to_path(name);
    let canonical = if is_directory {
        key::marker_name(&path)
    } else {
        key::object_name(&path)
    }
    .map_err(|error| RemoteError::with_source(RemoteErrorType::ProtocolError, error))?;
    if canonical == name {
        Ok(())
    } else {
        Err(RemoteError::with_message(
            RemoteErrorType::ProtocolError,
            format!("storage returned non-canonical object name {name:?}"),
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use google_cloud_storage::model::Object;
    use pretty_assertions::assert_eq;

    use super::GcsObject;

    #[test]
    fn object_metadata_becomes_a_remote_file() {
        let object = Object::new().set_name("docs/readme.md").set_size(12_i64);
        let file = GcsObject::try_from(object).unwrap().into_file();
        assert_eq!(file.path(), Path::new("/docs/readme.md"));
        assert_eq!(file.metadata().size, Some(12));
        assert!(file.is_file());
    }

    #[test]
    fn marker_objects_and_prefixes_become_directories_without_a_size() {
        let marker = Object::new().set_name("docs/").set_size(0_i64);
        let file = GcsObject::try_from(marker).unwrap().into_file();
        assert_eq!(file.path(), Path::new("/docs"));
        assert!(file.is_dir());
        assert_eq!(file.metadata().size, None);

        let file = GcsObject::directory("docs/images/").unwrap().into_file();
        assert_eq!(file.path(), Path::new("/docs/images"));
        assert!(file.is_dir());
        assert_eq!(file.metadata().size, None);
    }

    #[test]
    fn negative_sizes_are_rejected() {
        let object = Object::new().set_name("broken").set_size(-1_i64);
        let error = GcsObject::try_from(object).unwrap_err();
        assert_eq!(error.kind(), remotefs::RemoteErrorType::ProtocolError);
    }

    #[test]
    fn non_canonical_names_are_rejected() {
        for name in ["/docs", "docs//readme", "docs/./readme", "docs/../readme"] {
            let object = Object::new().set_name(name).set_size(1_i64);
            let error = GcsObject::try_from(object).unwrap_err();
            assert_eq!(error.kind(), remotefs::RemoteErrorType::ProtocolError);
        }
    }
}
