//! # Aws s3
//!
//! Aws s3 client for remotefs

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
// -- mod
mod object;
use object::S3Object;

use crate::fs::{UnixPex, Welcome};
use crate::utils::path as path_utils;
use crate::{Directory, Entry, File, RemoteError, RemoteErrorType, RemoteFs, RemoteResult};

use s3::creds::Credentials;
use s3::serde_types::Object;
use s3::{Bucket, Region};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::str::FromStr;

/// Aws s3 file system client
pub struct AwsS3Fs {
    bucket: Option<Bucket>,
    wrkdir: PathBuf,
    // -- options
    bucket_name: String,
    region: String,
    profile: Option<String>,
    access_key: Option<String>,
    secret_key: Option<String>,
    security_token: Option<String>,
    session_token: Option<String>,
}

impl AwsS3Fs {
    /// Initialize a new `AwsS3Fs`
    pub fn new<S: AsRef<str>>(bucket: S, region: S) -> Self {
        Self {
            bucket: None,
            wrkdir: PathBuf::from("/"),
            bucket_name: bucket.as_ref().to_string(),
            region: region.as_ref().to_string(),
            profile: None,
            access_key: None,
            secret_key: None,
            security_token: None,
            session_token: None,
        }
    }

    /// Set aws profile. If unset, "default" will be used
    pub fn profile<S: AsRef<str>>(mut self, profile: S) -> Self {
        self.profile = Some(profile.as_ref().to_string());
        self
    }

    /// Specify access key for aws connection.
    /// If unset, will be read from environment variable `AWS_ACCESS_KEY_ID`
    pub fn access_key<S: AsRef<str>>(mut self, key: S) -> Self {
        self.access_key = Some(key.as_ref().to_string());
        self
    }

    /// Specify secret access key for aws connection.
    /// If unset, will be read from environment variable `AWS_SECRET_ACCESS_KEY`
    pub fn secret_access_key<S: AsRef<str>>(mut self, key: S) -> Self {
        self.secret_key = Some(key.as_ref().to_string());
        self
    }

    /// Specify security token for aws connection.
    /// If unset, will be read from environment variable `AWS_SECURITY_TOKEN`
    pub fn security_token<S: AsRef<str>>(mut self, key: S) -> Self {
        self.security_token = Some(key.as_ref().to_string());
        self
    }

    /// Specify session token for aws connection.
    /// If unset, will be read from environment variable `AWS_SESSION_TOKEN`
    pub fn session_token<S: AsRef<str>>(mut self, key: S) -> Self {
        self.session_token = Some(key.as_ref().to_string());
        self
    }

    // -- private

    /// List objects contained in `p` path
    fn list_objects(&self, p: &Path, list_dir: bool) -> RemoteResult<Vec<S3Object>> {
        // Make path relative
        let key: String = Self::fmt_path(p, list_dir);
        debug!("Query list directory {}; key: {}", p.display(), key);
        self.query_objects(key, true)
    }

    /// Stat an s3 object
    fn stat_object(&self, p: &Path) -> RemoteResult<S3Object> {
        let key: String = Self::fmt_path(p, false);
        debug!("Query stat object {}; key: {}", p.display(), key);
        let objects = self.query_objects(key, false)?;
        // Absolutize path
        let absol: PathBuf = path_utils::absolutize(Path::new("/"), p);
        // Find associated object
        match objects
            .into_iter()
            .find(|x| x.path.as_path() == absol.as_path())
        {
            Some(obj) => Ok(obj),
            None => Err(RemoteError::new_ex(
                RemoteErrorType::NoSuchFileOrDirectory,
                format!("{}: No such file or directory", p.display()),
            )),
        }
    }

    /// Query objects at key
    fn query_objects(
        &self,
        key: String,
        only_direct_children: bool,
    ) -> RemoteResult<Vec<S3Object>> {
        let results = self.bucket.as_ref().unwrap().list(key.clone(), None);
        match results {
            Ok(entries) => {
                let mut objects: Vec<S3Object> = Vec::new();
                entries.iter().for_each(|x| {
                    x.contents
                        .iter()
                        .filter(|x| {
                            if only_direct_children {
                                Self::list_object_should_be_kept(x, key.as_str())
                            } else {
                                true
                            }
                        })
                        .for_each(|x| objects.push(S3Object::from(x)))
                });
                debug!("Found objects: {:?}", objects);
                Ok(objects)
            }
            Err(e) => Err(RemoteError::new_ex(RemoteErrorType::StatFailed, e)),
        }
    }

    /// Returns whether object should be kept after list command.
    /// The object won't be kept if:
    ///
    /// 1. is not a direct child of provided dir
    fn list_object_should_be_kept(obj: &Object, dir: &str) -> bool {
        Self::is_direct_child(obj.key.as_str(), dir)
    }

    /// Checks whether Object's key is direct child of `parent` path.
    fn is_direct_child(key: &str, parent: &str) -> bool {
        key == format!("{}{}", parent, S3Object::object_name(key))
            || key == format!("{}{}/", parent, S3Object::object_name(key))
    }

    /// Make s3 absolute path from a given path
    fn resolve(&self, p: &Path) -> PathBuf {
        path_utils::diff_paths(
            path_utils::absolutize(self.wrkdir.as_path(), p),
            &Path::new("/"),
        )
        .unwrap_or_default()
    }

    /// fmt path for fsentry according to format expected by s3
    fn fmt_path(p: &Path, is_dir: bool) -> String {
        // prevent root as slash
        if p == Path::new("/") {
            return "".to_string();
        }
        // Remove root only if absolute
        #[cfg(target_family = "unix")]
        let is_absolute: bool = p.is_absolute();
        // NOTE: don't use is_absolute: on windows won't work
        #[cfg(target_family = "windows")]
        let is_absolute: bool = p.display().to_string().starts_with('/');
        let p: PathBuf = match is_absolute {
            true => path_utils::diff_paths(p, &Path::new("/")).unwrap_or_default(),
            false => p.to_path_buf(),
        };
        // NOTE: windows only: resolve paths
        #[cfg(target_family = "windows")]
        let p: PathBuf = PathBuf::from(path_slash::PathExt::to_slash_lossy(p.as_path()).as_str());
        // Fmt
        match is_dir {
            true => {
                let mut p: String = p.display().to_string();
                if !p.ends_with('/') {
                    p.push('/');
                }
                p
            }
            false => p.to_string_lossy().to_string(),
        }
    }

    /// Check connection status
    fn check_connection(&self) -> RemoteResult<()> {
        if self.is_connected() {
            Ok(())
        } else {
            Err(RemoteError::new(RemoteErrorType::NotConnected))
        }
    }
}

impl RemoteFs for AwsS3Fs {
    fn connect(&mut self) -> RemoteResult<Welcome> {
        // Load credentials
        debug!("Loading credentials... (profile {:?})", self.profile);
        let credentials: Credentials = Credentials::new(
            self.access_key.as_deref(),
            self.secret_key.as_deref(),
            self.security_token.as_deref(),
            self.session_token.as_deref(),
            self.profile.as_deref(),
        )
        .map_err(|e| {
            RemoteError::new_ex(
                RemoteErrorType::AuthenticationFailed,
                format!("Could not load s3 credentials: {}", e),
            )
        })?;
        // Parse region
        trace!("Parsing region {}", self.region);
        let region: Region = Region::from_str(self.region.as_str()).map_err(|e| {
            RemoteError::new_ex(
                RemoteErrorType::AuthenticationFailed,
                format!("Could not parse s3 region: {}", e),
            )
        })?;
        debug!(
            "Credentials loaded! Connecting to bucket {}...",
            self.bucket_name
        );
        self.bucket = Some(
            Bucket::new(self.bucket_name.as_str(), region, credentials).map_err(|e| {
                RemoteError::new_ex(
                    RemoteErrorType::AuthenticationFailed,
                    format!("Could not connect to bucket {}: {}", self.bucket_name, e),
                )
            })?,
        );
        info!("Connection successfully established to s3 bucket");
        Ok(Welcome::default())
    }

    fn disconnect(&mut self) -> RemoteResult<()> {
        info!("Disconnecting from S3 bucket...");
        match self.bucket.take() {
            Some(bucket) => {
                drop(bucket);
                Ok(())
            }
            None => Err(RemoteError::new(RemoteErrorType::NotConnected)),
        }
    }

    fn is_connected(&self) -> bool {
        self.bucket.is_some()
    }

    fn pwd(&mut self) -> RemoteResult<PathBuf> {
        self.check_connection()?;
        Ok(self.wrkdir.clone())
    }

    fn change_dir(&mut self, dir: &Path) -> RemoteResult<PathBuf> {
        self.check_connection()?;
        // Always allow entering root
        if dir == Path::new("/") {
            self.wrkdir = dir.to_path_buf();
            debug!("New working directory: {}", self.wrkdir.display());
            return Ok(self.wrkdir.clone());
        }
        // Check if directory exists
        debug!("Entering directory {}...", dir.display());
        let dir_p: PathBuf = self.resolve(dir);
        let dir_s: String = Self::fmt_path(dir_p.as_path(), true);
        debug!("Searching for key {} (path: {})...", dir_s, dir_p.display());
        // Check if directory already exists
        if self
            .stat_object(PathBuf::from(dir_s.as_str()).as_path())
            .is_ok()
        {
            self.wrkdir = path_utils::absolutize(Path::new("/"), dir_p.as_path());
            debug!("New working directory: {}", self.wrkdir.display());
            Ok(self.wrkdir.clone())
        } else {
            Err(RemoteError::new(RemoteErrorType::NoSuchFileOrDirectory))
        }
    }

    fn list_dir(&mut self, path: &Path) -> RemoteResult<Vec<Entry>> {
        self.check_connection()?;
        self.list_objects(path, true)
            .map(|x| x.into_iter().map(|x| x.into()).collect())
    }

    fn stat(&mut self, path: &Path) -> RemoteResult<Entry> {
        self.check_connection()?;
        let path = self.resolve(path);
        if let Ok(obj) = self.stat_object(path.as_path()) {
            return Ok(obj.into());
        }
        // Try as a "directory"
        trace!("Failed to stat object as file; trying as a directory...");
        let path = PathBuf::from(Self::fmt_path(path.as_path(), true));
        self.stat_object(path.as_path()).map(|x| x.into())
    }

    fn setstat(&mut self, _path: &Path, _metadata: crate::fs::Metadata) -> RemoteResult<()> {
        Err(RemoteError::new(RemoteErrorType::UnsupportedFeature))
    }

    fn exists(&mut self, path: &Path) -> RemoteResult<bool> {
        match self.stat(path) {
            Ok(_) => Ok(true),
            Err(RemoteError {
                code: RemoteErrorType::NoSuchFileOrDirectory,
                ..
            }) => Ok(false),
            Err(err) => Err(err),
        }
    }

    fn remove_file(&mut self, path: &Path) -> RemoteResult<()> {
        self.check_connection()?;
        let path = Self::fmt_path(
            path_utils::diff_paths(path, &Path::new("/"))
                .unwrap_or_default()
                .as_path(),
            false,
        );
        debug!("Removing object {}...", path);
        self.bucket
            .as_ref()
            .unwrap()
            .delete_object(path)
            .map(|_| ())
            .map_err(|e| {
                RemoteError::new_ex(
                    RemoteErrorType::ProtocolError,
                    format!("Could not remove file: {}", e),
                )
            })
    }

    fn remove_dir(&mut self, path: &Path) -> RemoteResult<()> {
        self.check_connection()?;
        let path = Self::fmt_path(
            path_utils::diff_paths(path, &Path::new("/"))
                .unwrap_or_default()
                .as_path(),
            true,
        );
        debug!("Removing object {}...", path);
        self.bucket
            .as_ref()
            .unwrap()
            .delete_object(path)
            .map(|_| ())
            .map_err(|e| {
                RemoteError::new_ex(
                    RemoteErrorType::ProtocolError,
                    format!("Could not remove file: {}", e),
                )
            })
    }

    fn remove_dir_all(&mut self, path: &Path) -> RemoteResult<()> {
        debug!("Removing all content of {}", path.display());
        let is_dir = self.stat(path)?.is_dir();
        if is_dir {
            self.remove_dir(path)
        } else {
            self.remove_file(path)
        }
    }

    fn create_dir(&mut self, path: &Path, _mode: UnixPex) -> RemoteResult<()> {
        self.check_connection()?;
        let dir: String = Self::fmt_path(self.resolve(path).as_path(), true);
        debug!("Making directory {}...", dir);
        // Check if directory already exists
        if self
            .stat_object(PathBuf::from(dir.as_str()).as_path())
            .is_ok()
        {
            error!("Directory {} already exists", dir);
            return Err(RemoteError::new(RemoteErrorType::DirectoryAlreadyExists));
        }
        self.bucket
            .as_ref()
            .unwrap()
            .put_object(dir.as_str(), &[])
            .map(|_| ())
            .map_err(|e| {
                RemoteError::new_ex(
                    RemoteErrorType::FileCreateDenied,
                    format!("Could not make directory: {}", e),
                )
            })
    }

    fn symlink(&mut self, _path: &Path, _target: &Path) -> RemoteResult<()> {
        Err(RemoteError::new(RemoteErrorType::UnsupportedFeature))
    }

    fn copy(&mut self, _src: &Path, _dest: &Path) -> RemoteResult<()> {
        Err(RemoteError::new(RemoteErrorType::UnsupportedFeature))
    }

    fn mov(&mut self, _src: &Path, _dest: &Path) -> RemoteResult<()> {
        Err(RemoteError::new(RemoteErrorType::UnsupportedFeature))
    }

    fn exec(&mut self, _cmd: &str) -> RemoteResult<(u32, String)> {
        Err(RemoteError::new(RemoteErrorType::UnsupportedFeature))
    }

    fn append(
        &mut self,
        _path: &Path,
        _metadata: &crate::fs::Metadata,
    ) -> RemoteResult<Box<dyn std::io::Write>> {
        Err(RemoteError::new(RemoteErrorType::UnsupportedFeature))
    }

    fn create(
        &mut self,
        _path: &Path,
        _metadata: &crate::fs::Metadata,
    ) -> RemoteResult<Box<dyn std::io::Write>> {
        Err(RemoteError::new(RemoteErrorType::UnsupportedFeature))
    }

    fn open(&mut self, _path: &Path) -> RemoteResult<Box<dyn Read>> {
        Err(RemoteError::new(RemoteErrorType::UnsupportedFeature))
    }

    fn create_file(
        &mut self,
        path: &Path,
        _metadata: &crate::fs::Metadata,
        mut reader: Box<dyn Read>,
    ) -> RemoteResult<()> {
        self.check_connection()?;
        let key = Self::fmt_path(path, false);
        debug!("Query PUT for key '{}'", key);
        self.bucket
            .as_ref()
            .unwrap()
            .put_object_stream(&mut reader, key.as_str())
            .map(|_| ())
            .map_err(|e| {
                RemoteError::new_ex(
                    RemoteErrorType::ProtocolError,
                    format!("Could not put file: {}", e),
                )
            })
    }

    fn open_file<W>(&mut self, src: &Path, dest: &mut W) -> RemoteResult<()>
    where
        W: std::io::Write + Send,
    {
        self.check_connection()?;
        let key = Self::fmt_path(src, false);
        info!("Query GET for key '{}'", key);
        self.bucket
            .as_ref()
            .unwrap()
            .get_object_stream(key.as_str(), dest)
            .map(|_| ())
            .map_err(|e| {
                RemoteError::new_ex(
                    RemoteErrorType::ProtocolError,
                    format!("Could not get file: {}", e),
                )
            })
    }
}

#[cfg(test)]
mod test {

    use super::*;
    #[cfg(feature = "with-s3-ci")]
    use crate::mock::fs as mock_fs;

    use pretty_assertions::assert_eq;
    #[cfg(feature = "with-s3-ci")]
    use std::env;

    #[test]
    fn should_init_s3() {
        let s3 = AwsS3Fs::new("aws-s3-test", "eu-central-1");
        assert_eq!(s3.wrkdir.as_path(), Path::new("/"));
        assert_eq!(s3.bucket_name.as_str(), "aws-s3-test");
        assert_eq!(s3.region.as_str(), "eu-central-1");
        assert!(s3.bucket.is_none());
        assert!(s3.access_key.is_none());
        assert!(s3.profile.is_none());
        assert!(s3.secret_key.is_none());
        assert!(s3.security_token.is_none());
        assert!(s3.session_token.is_none());
        assert!(s3.secret_key.is_none());
    }

    #[test]
    fn should_init_s3_with_options() {
        let s3 = AwsS3Fs::new("aws-s3-test", "eu-central-1")
            .access_key("AKIA0000")
            .profile("default")
            .secret_access_key("PASSWORD")
            .security_token("secret")
            .session_token("token");
        assert_eq!(s3.bucket_name.as_str(), "aws-s3-test");
        assert_eq!(s3.region.as_str(), "eu-central-1");
        assert_eq!(s3.access_key.as_deref().unwrap(), "AKIA0000");
        assert_eq!(s3.secret_key.as_deref().unwrap(), "PASSWORD");
        assert_eq!(s3.security_token.as_deref().unwrap(), "secret");
        assert_eq!(s3.session_token.as_deref().unwrap(), "token");
    }

    #[test]
    fn s3_is_direct_child() {
        assert_eq!(AwsS3Fs::is_direct_child("pippo/", ""), true);
        assert_eq!(AwsS3Fs::is_direct_child("pippo/sottocartella/", ""), false);
        assert_eq!(
            AwsS3Fs::is_direct_child("pippo/sottocartella/", "pippo/"),
            true
        );
        assert_eq!(
            AwsS3Fs::is_direct_child("pippo/sottocartella/", "pippo"), // This case must be handled indeed
            false
        );
        assert_eq!(
            AwsS3Fs::is_direct_child("pippo/sottocartella/readme.md", "pippo/sottocartella/"),
            true
        );
        assert_eq!(
            AwsS3Fs::is_direct_child("pippo/sottocartella/readme.md", "pippo/sottocartella/"),
            true
        );
    }

    #[test]
    fn s3_resolve() {
        let mut s3 = AwsS3Fs::new("aws-s3-test", "eu-central-1");
        s3.wrkdir = PathBuf::from("/tmp");
        // Absolute
        assert_eq!(
            s3.resolve(&Path::new("/tmp/sottocartella/")).as_path(),
            Path::new("tmp/sottocartella")
        );
        // Relative
        assert_eq!(
            s3.resolve(&Path::new("subfolder/")).as_path(),
            Path::new("tmp/subfolder")
        );
    }

    #[test]
    fn s3_fmt_path() {
        assert_eq!(
            AwsS3Fs::fmt_path(&Path::new("/tmp/omar.txt"), false).as_str(),
            "tmp/omar.txt"
        );
        assert_eq!(
            AwsS3Fs::fmt_path(&Path::new("omar.txt"), false).as_str(),
            "omar.txt"
        );
        assert_eq!(
            AwsS3Fs::fmt_path(&Path::new("/tmp/subfolder"), true).as_str(),
            "tmp/subfolder/"
        );
        assert_eq!(
            AwsS3Fs::fmt_path(&Path::new("tmp/subfolder"), true).as_str(),
            "tmp/subfolder/"
        );
        assert_eq!(AwsS3Fs::fmt_path(&Path::new("tmp"), true).as_str(), "tmp/");
        assert_eq!(AwsS3Fs::fmt_path(&Path::new("tmp/"), true).as_str(), "tmp/");
        assert_eq!(AwsS3Fs::fmt_path(&Path::new("/"), true).as_str(), "");
    }

    #[cfg(feature = "with-s3-ci")]
    #[test]
    fn s3_filetransfer() {
        crate::mock::logger();
        // Gather s3 environment args
        let bucket = env!("AWS_S3_BUCKET");
        let region = env!("AWS_S3_REGION");
        // Get transfer
        let mut s3 = AwsS3Fs::new(bucket, region);
        // Connect
        assert!(s3.connect().is_ok());
        // Check is connected
        assert_eq!(s3.is_connected(), true);
        // Remove temp dir (cleanup)
        let dir_path = PathBuf::from("/github-ci/playground/test-dir");
        let _ = s3.remove_dir_all(&dir_path.as_path());
        // Pwd
        assert_eq!(s3.pwd().ok().unwrap(), PathBuf::from("/"));
        // Go to github-ci directory
        assert!(s3.change_dir(&Path::new("/github-ci")).is_ok());
        assert_eq!(s3.pwd().ok().unwrap(), PathBuf::from("/github-ci"));
        // Find
        assert_eq!(s3.find("*.jpg").ok().unwrap().len(), 1);
        // List directory (3 entries)
        assert_eq!(s3.list_dir(&Path::new("/github-ci")).ok().unwrap().len(), 3);
        // Go to playground
        assert!(s3.change_dir(&Path::new("/github-ci/playground")).is_ok());
        assert_eq!(
            s3.pwd().ok().unwrap(),
            PathBuf::from("/github-ci/playground")
        );
        // Create directory
        assert!(s3
            .create_dir(dir_path.as_path(), UnixPex::from(0o755))
            .is_ok());
        assert!(s3.change_dir(dir_path.as_path()).is_ok());
        // Copy/rename file is unsupported
        assert!(s3.copy(dir_path.as_path(), &Path::new("/copia")).is_err());
        assert!(s3.mov(dir_path.as_path(), &Path::new("/copia")).is_err());
        // Exec is unsupported
        assert!(s3.exec("omar!").is_err());
        // Stat file
        let entry = s3
            .stat(&Path::new("/github-ci/avril_lavigne.jpg"))
            .ok()
            .unwrap()
            .unwrap_file();
        assert_eq!(entry.name.as_str(), "avril_lavigne.jpg");
        assert_eq!(
            entry.abs_path.as_path(),
            Path::new("/github-ci/avril_lavigne.jpg")
        );
        assert_eq!(entry.extension.as_deref().unwrap(), "jpg");
        assert_eq!(entry.metadata.size, 101738);
        assert_eq!(entry.metadata.uid, None);
        assert_eq!(entry.metadata.gid, None);
        assert_eq!(entry.metadata.mode, None);
        // Download file
        let mut local_file = mock_fs::create_sample_file();
        assert!(s3
            .open_file(&Path::new("/github-ci/avril_lavigne.jpg"), &mut local_file)
            .is_ok());
        // Upload file
        let mut dest_path = dir_path.clone();
        dest_path.push("aurellia_lavagna.jpg");
        assert!(s3
            .create_file(
                dest_path.as_path(),
                &crate::fs::Metadata::default(),
                Box::new(local_file)
            )
            .is_ok());
        // Remove temp dir
        assert!(s3.remove_dir_all(&dir_path.as_path()).is_ok());
        // Disconnect
        assert!(s3.disconnect().is_ok());
    }

    #[test]
    fn should_return_errors_on_uninitialized_client() {
        let mut client = AwsS3Fs::new("aws-s3-test", "eu-central-1");
        assert!(client.change_dir(Path::new("/tmp")).is_err());
        assert!(client
            .copy(Path::new("/nowhere"), PathBuf::from("/culonia").as_path())
            .is_err());
        assert!(client.exec("echo 5").is_err());
        assert!(client.disconnect().is_err());
        assert!(client.symlink(Path::new("/a"), Path::new("/b")).is_err());
        assert!(client.list_dir(Path::new("/tmp")).is_err());
        assert!(client
            .create_dir(Path::new("/tmp"), UnixPex::from(0o755))
            .is_err());
        assert!(client.pwd().is_err());
        assert!(client.remove_dir_all(Path::new("/nowhere")).is_err());
        assert!(client
            .mov(Path::new("/nowhere"), Path::new("/culonia"))
            .is_err());
        assert!(client.stat(Path::new("/tmp")).is_err());
        assert!(client
            .setstat(Path::new("/tmp"), crate::fs::Metadata::default())
            .is_err());
        assert!(client.open(Path::new("/tmp/pippo.txt")).is_err());
        assert!(client
            .create(Path::new("/tmp/pippo.txt"), &crate::fs::Metadata::default())
            .is_err());
        assert!(client
            .append(Path::new("/tmp/pippo.txt"), &crate::fs::Metadata::default())
            .is_err());
    }
}