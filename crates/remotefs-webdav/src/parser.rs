use std::path::PathBuf;

use remotefs::fs::{FileType, Metadata};
use remotefs::{File, RemoteError, RemoteErrorType, RemoteResult};
use rustydav::prelude::Response;

use super::webdav_xml::elements::{Multistatus, Response as WebDAVResponse};
use super::webdav_xml::FromXml;

pub struct ResponseParser {
    response: Response,
}

impl From<Response> for ResponseParser {
    fn from(response: Response) -> Self {
        ResponseParser { response }
    }
}

impl ResponseParser {
    pub fn status(self) -> RemoteResult<()> {
        if self.response.status().is_success() {
            Ok(())
        } else {
            match self.response.status().as_u16() {
                401 | 403 => Err(RemoteError::new(RemoteErrorType::CouldNotOpenFile)),
                400 | 404 => Err(RemoteError::new(RemoteErrorType::NoSuchFileOrDirectory)),
                _ => Err(RemoteError::new(RemoteErrorType::ProtocolError)),
            }
        }
    }

    pub fn files(self) -> RemoteResult<Vec<File>> {
        debug!("Parsing files from response");
        if !self.response.status().is_success() {
            debug!("response is not success, returning status");
            return Err(self.status().unwrap_err());
        }

        // parse body
        let bytes = self
            .response
            .bytes()
            .map_err(|e| RemoteError::new_ex(RemoteErrorType::IoError, e))?;
        trace!(
            "parsing body: {}",
            String::from_utf8(bytes.to_vec()).unwrap()
        );

        Self::parse_propfind(bytes)
    }

    fn parse_propfind(bytes: impl Into<bytes::Bytes>) -> RemoteResult<Vec<File>> {
        let multistatus = Multistatus::from_xml(bytes)
            .map_err(|e| RemoteError::new_ex(RemoteErrorType::ProtocolError, e))?;
        debug!("parsed multistatus: {:?}", multistatus);

        let mut files = Vec::new();

        // collect files
        for response in multistatus.response {
            let (path, propstats) = match response {
                WebDAVResponse::Propstat {
                    href: path,
                    propstat,
                    responsedescription: _,
                } => (path, propstat),
                _ => {
                    debug!("Skipping response, not propstat");
                    continue;
                }
            };
            debug!(
                "found {} properties for {}",
                propstats.len(),
                path.0.to_string()
            );
            for props in propstats.map(|x| x.prop) {
                let mut metadata = Metadata::default();
                if let Some(Some(Ok(date))) = props.creationdate() {
                    debug!("creation date: {:?}", date.0);
                    metadata.created = Some(date.0.into());
                }
                if let Some(Some(Ok(date))) = props.getlastmodified() {
                    debug!("last modified: {:?}", date.0);
                    metadata.modified = Some(date.0.into());
                }
                if let Some(Some(Ok(size))) = props.getcontentlength() {
                    debug!("size: {:?}", size.0);
                    metadata.size = size.0;
                }
                let file_name = path.0.to_string();
                let path = PathBuf::from(path.0.to_string());
                if file_name.ends_with('/') || path.is_dir() {
                    debug!("path {} is a directory", path.display());
                    metadata.file_type = FileType::Directory;
                } else {
                    debug!("path {} is a file", path.display());
                    metadata.file_type = FileType::File;
                }

                files.push(File { path, metadata });
            }
        }

        Ok(files)
    }
}

#[cfg(test)]
mod test {

    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn test_should_parse_dir_content() {
        let response = r#"
        <?xml version="1.0" encoding="utf-8"?>
        <D:multistatus xmlns:D="DAV:" xmlns:ns0="DAV:">
        <D:response xmlns:lp2="http://apache.org/dav/props/" xmlns:lp1="DAV:">
        <D:href>/ciao/</D:href>
        <D:propstat>
        <D:prop>
        <lp1:resourcetype><D:collection/></lp1:resourcetype>
        <lp1:creationdate>2024-03-02T15:44:46Z</lp1:creationdate>
        <lp1:getlastmodified>Sat, 02 Mar 2024 15:44:46 GMT</lp1:getlastmodified>
        <lp1:getetag>"1a-612af5f3d72b2"</lp1:getetag>
        <D:supportedlock>
        <D:lockentry>
        <D:lockscope><D:exclusive/></D:lockscope>
        <D:locktype><D:write/></D:locktype>
        </D:lockentry>
        <D:lockentry>
        <D:lockscope><D:shared/></D:lockscope>
        <D:locktype><D:write/></D:locktype>
        </D:lockentry>
        </D:supportedlock>
        <D:lockdiscovery/>
        <D:getcontenttype>httpd/unix-directory</D:getcontenttype>
        </D:prop>
        <D:status>HTTP/1.1 200 OK</D:status>
        </D:propstat>
        </D:response>
        <D:response xmlns:lp2="http://apache.org/dav/props/" xmlns:lp1="DAV:">
        <D:href>/ciao/pippo/</D:href>
        <D:propstat>
        <D:prop>
        <lp1:resourcetype><D:collection/></lp1:resourcetype>
        <lp1:creationdate>2024-03-02T15:40:53Z</lp1:creationdate>
        <lp1:getlastmodified>Sat, 02 Mar 2024 15:40:53 GMT</lp1:getlastmodified>
        <lp1:getetag>"0-612af5150498f"</lp1:getetag>
        <D:supportedlock>
        <D:lockentry>
        <D:lockscope><D:exclusive/></D:lockscope>
        <D:locktype><D:write/></D:locktype>
        </D:lockentry>
        <D:lockentry>
        <D:lockscope><D:shared/></D:lockscope>
        <D:locktype><D:write/></D:locktype>
        </D:lockentry>
        </D:supportedlock>
        <D:lockdiscovery/>
        <D:getcontenttype>httpd/unix-directory</D:getcontenttype>
        </D:prop>
        <D:status>HTTP/1.1 200 OK</D:status>
        </D:propstat>
        </D:response>
        <D:response xmlns:lp2="http://apache.org/dav/props/" xmlns:lp1="DAV:">
        <D:href>/ciao/build.rs</D:href>
        <D:propstat>
        <D:prop>
        <lp1:resourcetype/>
        <lp1:creationdate>2024-03-02T15:44:46Z</lp1:creationdate>
        <lp1:getcontentlength>486</lp1:getcontentlength>
        <lp1:getlastmodified>Sat, 02 Mar 2024 15:44:46 GMT</lp1:getlastmodified>
        <lp1:getetag>"1e6-612af5f3d72b2"</lp1:getetag>
        <lp2:executable>F</lp2:executable>
        <D:supportedlock>
        <D:lockentry>
        <D:lockscope><D:exclusive/></D:lockscope>
        <D:locktype><D:write/></D:locktype>
        </D:lockentry>
        <D:lockentry>
        <D:lockscope><D:shared/></D:lockscope>
        <D:locktype><D:write/></D:locktype>
        </D:lockentry>
        </D:supportedlock>
        <D:lockdiscovery/>
        <D:getcontenttype>application/rls-services+xml</D:getcontenttype>
        </D:prop>
        <D:status>HTTP/1.1 200 OK</D:status>
        </D:propstat>
        </D:response>
        </D:multistatus>
        
"#;

        let files = ResponseParser::parse_propfind(response.as_bytes()).unwrap();
        assert_eq!(files.len(), 3);
        let ciao_dir = &files[0];
        assert!(ciao_dir.is_dir());
        assert_eq!(ciao_dir.path, PathBuf::from("/ciao/"));

        let pippo_dir = &files[1];
        assert!(pippo_dir.is_dir());
        assert_eq!(pippo_dir.path, PathBuf::from("/ciao/pippo/"));

        let build_rs = &files[2];
        assert!(build_rs.is_file());
        assert_eq!(build_rs.path, PathBuf::from("/ciao/build.rs"));
        assert_eq!(build_rs.metadata.size, 486);
    }
}
