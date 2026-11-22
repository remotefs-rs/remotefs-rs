//! Conversion of WebDAV resources into remotefs entries.

use std::path::Path;
use std::time::{Duration, SystemTime};

use dav_xml_client::Resource;
use remotefs::File;
use remotefs::fs::{FileType, Metadata};

/// Converts a `PROPFIND` resource into a [`File`].
///
/// Collections drop their trailing slash so that `File::name` and path
/// comparisons behave like every other backend; the root stays `/`. WebDAV
/// carries no POSIX ownership or mode, so those fields stay `None`.
pub(crate) fn resource_to_file(resource: &Resource) -> File {
    let raw = resource.path();
    let path = if raw.len() > 1 {
        raw.trim_end_matches('/')
    } else {
        raw
    };
    let file_type = if resource.is_collection {
        FileType::Directory
    } else {
        FileType::File
    };
    let mut metadata = Metadata::default().file_type(file_type);
    if let Some(size) = resource.content_length {
        metadata = metadata.size(size);
    }
    if let Some(modified) = resource.last_modified {
        metadata = metadata.modified(SystemTime::from(modified));
    }
    if let Some(created) = resource
        .creation_date
        .and_then(|date| u64::try_from(date.unix_timestamp()).ok())
    {
        metadata = metadata.created(SystemTime::UNIX_EPOCH + Duration::from_secs(created));
    }
    debug!("converted resource {raw} into {path} ({file_type:?})");
    File::new(path, metadata)
}

/// Converts a resource while assigning the path from the request namespace.
///
/// WebDAV servers may return an absolute href containing the share prefix or a
/// relative href. The client already knows the requested remote path, so it is
/// the authoritative path for a depth-zero response.
pub(crate) fn resource_to_file_at(resource: &Resource, path: &Path) -> File {
    let file = resource_to_file(resource);
    File::new(path.to_path_buf(), file.metadata().clone())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use dav_xml_client::Resource;
    use dav_xml_client::dav_xml::FromXml;
    use dav_xml_client::dav_xml::elements::Multistatus;
    use pretty_assertions::assert_eq;

    use super::resource_to_file;

    const LISTING: &str = r#"<D:multistatus xmlns:D="DAV:">
  <D:response>
    <D:href>/dir/</D:href>
    <D:propstat>
      <D:prop><D:resourcetype><D:collection/></D:resourcetype></D:prop>
      <D:status>HTTP/1.1 200 OK</D:status>
    </D:propstat>
  </D:response>
  <D:response>
    <D:href>/dir/file.txt</D:href>
    <D:propstat>
      <D:prop>
        <D:resourcetype/>
        <D:getcontentlength>12</D:getcontentlength>
        <D:getlastmodified>Mon, 01 Jan 2024 00:00:00 GMT</D:getlastmodified>
        <D:creationdate>2024-01-01T00:00:00Z</D:creationdate>
      </D:prop>
      <D:status>HTTP/1.1 200 OK</D:status>
    </D:propstat>
  </D:response>
  <D:response>
    <D:href>/</D:href>
    <D:propstat>
      <D:prop><D:resourcetype><D:collection/></D:resourcetype></D:prop>
      <D:status>HTTP/1.1 200 OK</D:status>
    </D:propstat>
  </D:response>
</D:multistatus>"#;

    fn resources() -> Vec<Resource> {
        let multistatus = Multistatus::from_xml(LISTING.as_bytes().to_vec()).unwrap();
        Resource::from_multistatus(&multistatus)
    }

    #[test]
    fn collections_drop_the_trailing_slash_and_are_directories() {
        let dir = resource_to_file(&resources()[0]);
        assert_eq!(dir.path(), Path::new("/dir"));
        assert_eq!(dir.name(), "dir");
        assert!(dir.is_dir());
        assert_eq!(dir.metadata().size, None);
    }

    #[test]
    fn files_copy_size_and_timestamps() {
        let file = resource_to_file(&resources()[1]);
        assert_eq!(file.path(), Path::new("/dir/file.txt"));
        assert!(file.is_file());
        assert_eq!(file.metadata().size, Some(12));
        assert!(file.metadata().modified.is_some());
        assert!(file.metadata().created.is_some());
        assert_eq!(file.metadata().mode, None);
    }

    #[test]
    fn root_stays_root() {
        let root = resource_to_file(&resources()[2]);
        assert_eq!(root.path(), Path::new("/"));
        assert!(root.is_dir());
    }
}
