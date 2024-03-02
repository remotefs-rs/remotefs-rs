use std::path::PathBuf;

use remotefs::fs::{FileType, Metadata};
use remotefs::{File, RemoteError, RemoteErrorType, RemoteResult};
use rustydav::prelude::Response;
use webdav_xml::elements::{Multistatus, Response as WebDAVResponse};
use webdav_xml::FromXml;

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
                let path = PathBuf::from(path.0.to_string());
                if path.is_dir() {
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
