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
use super::object::S3Object;

use crate::utils::path as path_utils;
use remotefs::fs::{Metadata, UnixPex, Welcome};
use remotefs::{Entry, RemoteError, RemoteErrorType, RemoteFs, RemoteResult};

use s3::creds::Credentials;
use s3::serde_types::Object;
use s3::Region;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::str::FromStr;

pub use s3::Bucket;

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

    // -- get ref

    /// Get a reference to the `Bucket` struct
    pub fn bucket(&self) -> Option<&Bucket> {
        self.bucket.as_ref()
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
    fn check_connection(&mut self) -> RemoteResult<()> {
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

    fn is_connected(&mut self) -> bool {
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

    fn setstat(&mut self, _path: &Path, _metadata: Metadata) -> RemoteResult<()> {
        Err(RemoteError::new(RemoteErrorType::UnsupportedFeature))
    }

    fn exists(&mut self, path: &Path) -> RemoteResult<bool> {
        match self.stat(path) {
            Ok(_) => Ok(true),
            Err(RemoteError {
                kind: RemoteErrorType::NoSuchFileOrDirectory,
                ..
            }) => Ok(false),
            Err(err) => Err(err),
        }
    }

    fn remove_file(&mut self, path: &Path) -> RemoteResult<()> {
        self.check_connection()?;
        let path = Self::fmt_path(self.resolve(path).as_path(), true);
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
        if !self.exists(path).ok().unwrap_or(false) {
            return Err(RemoteError::new(RemoteErrorType::NoSuchFileOrDirectory));
        }
        println!("{}", self.resolve(path).as_path().display());
        let path = Self::fmt_path(self.resolve(path).as_path(), true);
        debug!("Removing object {}...", path);
        self.bucket
            .as_ref()
            .unwrap()
            .delete_object(path)
            .map(|_| ())
            .map_err(|e| {
                RemoteError::new_ex(
                    RemoteErrorType::ProtocolError,
                    format!("Could not remove directory: {}", e),
                )
            })
    }

    fn remove_dir_all(&mut self, path: &Path) -> RemoteResult<()> {
        debug!("Removing all content of {}", path.display());
        if self.remove_dir(path).is_err() {
            self.remove_file(path)
        } else {
            Ok(())
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
        _metadata: &Metadata,
    ) -> RemoteResult<Box<dyn std::io::Write>> {
        Err(RemoteError::new(RemoteErrorType::UnsupportedFeature))
    }

    fn create(
        &mut self,
        _path: &Path,
        _metadata: &Metadata,
    ) -> RemoteResult<Box<dyn std::io::Write>> {
        Err(RemoteError::new(RemoteErrorType::UnsupportedFeature))
    }

    fn open(&mut self, _path: &Path) -> RemoteResult<Box<dyn Read>> {
        Err(RemoteError::new(RemoteErrorType::UnsupportedFeature))
    }

    fn create_file(
        &mut self,
        path: &Path,
        _metadata: &Metadata,
        mut reader: Box<dyn Read>,
    ) -> RemoteResult<()> {
        self.check_connection()?;
        let src = self.resolve(path);
        let key = Self::fmt_path(src.as_path(), false);
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

    fn open_file(
        &mut self,
        src: &Path,
        mut dest: Box<dyn std::io::Write + Send>,
    ) -> RemoteResult<()> {
        self.check_connection()?;
        if !self.exists(src).ok().unwrap_or(false) {
            return Err(RemoteError::new(RemoteErrorType::NoSuchFileOrDirectory));
        }
        let src = self.resolve(src);
        let key = Self::fmt_path(src.as_path(), false);
        info!("Query GET for key '{}'", key);
        self.bucket
            .as_ref()
            .unwrap()
            .get_object_stream(key.as_str(), &mut dest)
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

    use pretty_assertions::assert_eq;
    #[cfg(feature = "with-s3-ci")]
    use serial_test::serial;
    #[cfg(feature = "with-s3-ci")]
    use std::env;
    #[cfg(feature = "with-s3-ci")]
    use std::io::Cursor;
    #[cfg(feature = "with-s3-ci")]
    use std::time::SystemTime;

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
        assert!(s3.bucket.is_none());
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

    #[test]
    #[cfg(feature = "with-s3-ci")]
    #[serial]
    fn should_not_append_to_file() {
        crate::mock::logger();
        let mut client = setup_client();
        // Create file
        let p = Path::new("a.txt");
        // Append to file
        let file_data = "Hello, world!\n";
        let reader = Cursor::new(file_data.as_bytes());
        assert!(client
            .append_file(p, &Metadata::default(), Box::new(reader))
            .is_err());
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-s3-ci")]
    #[serial]
    fn should_change_directory() {
        crate::mock::logger();
        let mut client = setup_client();
        let pwd = client.pwd().ok().unwrap();
        assert!(client.change_dir(Path::new("/")).is_ok());
        assert!(client.change_dir(pwd.as_path()).is_ok());
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-s3-ci")]
    #[serial]
    fn should_not_change_directory() {
        crate::mock::logger();
        let mut client = setup_client();
        assert!(client
            .change_dir(Path::new("/tmp/sdfghjuireghiuergh/useghiyuwegh"))
            .is_err());
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-s3-ci")]
    #[serial]
    fn should_not_copy_file() {
        crate::mock::logger();
        let mut client = setup_client();
        // Create file
        let p = Path::new("a.txt");
        let file_data = "test data\n";
        let reader = Cursor::new(file_data.as_bytes());
        let mut metadata = Metadata::default();
        metadata.size = file_data.len() as u64;
        assert!(client.create_file(p, &metadata, Box::new(reader)).is_ok());
        assert!(client.copy(p, Path::new("aaa/bbbb/ccc/b.txt")).is_err());
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-s3-ci")]
    #[serial]
    fn should_create_directory() {
        crate::mock::logger();
        let mut client = setup_client();
        // create directory
        assert!(client
            .create_dir(Path::new("mydir"), UnixPex::from(0o755))
            .is_ok());
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-s3-ci")]
    #[serial]
    fn should_not_create_directory_cause_already_exists() {
        crate::mock::logger();
        let mut client = setup_client();
        // create directory
        assert!(client
            .create_dir(Path::new("mydir"), UnixPex::from(0o755))
            .is_ok());
        assert_eq!(
            client
                .create_dir(Path::new("mydir"), UnixPex::from(0o755))
                .err()
                .unwrap()
                .kind,
            RemoteErrorType::DirectoryAlreadyExists
        );
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-s3-ci")]
    #[serial]
    fn should_not_create_directory() {
        crate::mock::logger();
        let mut client = setup_client();
        // create directory
        assert!(client
            .create_dir(
                Path::new("/tmp/werfgjwerughjwurih/iwerjghiwgui"),
                UnixPex::from(0o755)
            )
            .is_err());
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-s3-ci")]
    #[serial]
    fn should_create_file() {
        crate::mock::logger();
        let mut client = setup_client();
        // Create file
        let p = Path::new("a.txt");
        let file_data = "test data\n";
        let reader = Cursor::new(file_data.as_bytes());
        let mut metadata = Metadata::default();
        metadata.size = file_data.len() as u64;
        assert!(client.create_file(p, &metadata, Box::new(reader)).is_ok());
        // Verify size
        assert_eq!(client.stat(p).ok().unwrap().metadata().size, 10);
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-s3-ci")]
    #[serial]
    fn should_not_exec_command() {
        crate::mock::logger();
        let mut client = setup_client();
        assert!(client.exec("echo 5").is_err());
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-s3-ci")]
    #[serial]
    fn should_tell_whether_file_exists() {
        crate::mock::logger();
        let mut client = setup_client();
        // Create file
        let p = Path::new("a.txt");
        let file_data = "test data\n";
        let reader = Cursor::new(file_data.as_bytes());
        let mut metadata = Metadata::default();
        metadata.size = file_data.len() as u64;
        assert!(client.create_file(p, &metadata, Box::new(reader)).is_ok());
        // Verify size
        assert_eq!(client.exists(p).ok().unwrap(), true);
        assert_eq!(client.exists(Path::new("b.txt")).ok().unwrap(), false);
        assert_eq!(
            client.exists(Path::new("/tmp/ppppp/bhhrhu")).ok().unwrap(),
            false
        );
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-s3-ci")]
    #[serial]
    fn should_list_dir() {
        crate::mock::logger();
        let mut client = setup_client();
        // Create file
        let wrkdir = client.pwd().ok().unwrap();
        let p = Path::new("a.txt");
        let file_data = "test data\n";
        let reader = Cursor::new(file_data.as_bytes());
        let mut metadata = Metadata::default();
        metadata.size = file_data.len() as u64;
        assert!(client.create_file(p, &metadata, Box::new(reader)).is_ok());
        // Verify size
        let file = client
            .list_dir(wrkdir.as_path())
            .ok()
            .unwrap()
            .get(0)
            .unwrap()
            .clone()
            .unwrap_file();
        assert_eq!(file.name.as_str(), "a.txt");
        let mut expected_path = wrkdir;
        expected_path.push(p);
        assert_eq!(file.path.as_path(), expected_path.as_path());
        assert_eq!(file.extension.as_deref().unwrap(), "txt");
        assert_eq!(file.metadata.size, 10);
        assert_eq!(file.metadata.mode, None);
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-s3-ci")]
    #[serial]
    fn should_not_move_file() {
        crate::mock::logger();
        let mut client = setup_client();
        // Create file
        let p = Path::new("a.txt");
        let file_data = "test data\n";
        let reader = Cursor::new(file_data.as_bytes());
        let mut metadata = Metadata::default();
        metadata.size = file_data.len() as u64;
        assert!(client.create_file(p, &metadata, Box::new(reader)).is_ok());
        let dest = Path::new("b.txt");
        assert!(client.mov(p, dest).is_err());
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-s3-ci")]
    #[serial]
    fn should_open_file() {
        crate::mock::logger();
        let mut client = setup_client();
        // Create file
        let p = Path::new("a.txt");
        let file_data = "test data\n";
        let reader = Cursor::new(file_data.as_bytes());
        let mut metadata = Metadata::default();
        metadata.size = file_data.len() as u64;
        assert!(client.create_file(p, &metadata, Box::new(reader)).is_ok());
        // Verify size
        let buffer: Box<dyn std::io::Write + Send> = Box::new(Vec::with_capacity(512));
        assert!(client.open_file(p, buffer).is_ok());
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-s3-ci")]
    #[serial]
    fn should_not_open_file() {
        crate::mock::logger();
        let mut client = setup_client();
        // Verify size
        let buffer: Box<dyn std::io::Write + Send> = Box::new(Vec::with_capacity(512));
        assert!(client
            .open_file(Path::new("/tmp/aashafb/hhh"), buffer)
            .is_err());
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-s3-ci")]
    #[serial]
    fn should_print_working_directory() {
        crate::mock::logger();
        let mut client = setup_client();
        assert!(client.pwd().is_ok());
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-s3-ci")]
    #[serial]
    fn should_remove_dir_all() {
        crate::mock::logger();
        let mut client = setup_client();
        // Create dir
        let mut dir_path = client.pwd().ok().unwrap();
        dir_path.push(Path::new("test/"));
        assert!(client
            .create_dir(dir_path.as_path(), UnixPex::from(0o775))
            .is_ok());
        // Create file
        let mut file_path = dir_path.clone();
        file_path.push(Path::new("a.txt"));
        let file_data = "test data\n";
        let reader = Cursor::new(file_data.as_bytes());
        let mut metadata = Metadata::default();
        metadata.size = file_data.len() as u64;
        assert!(client
            .create_file(file_path.as_path(), &metadata, Box::new(reader))
            .is_ok());
        // Remove dir
        assert!(client.remove_dir_all(dir_path.as_path()).is_ok());
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-s3-ci")]
    #[serial]
    fn should_remove_dir() {
        crate::mock::logger();
        let mut client = setup_client();
        // Create dir
        let mut dir_path = client.pwd().ok().unwrap();
        dir_path.push(Path::new("test/"));
        assert!(client
            .create_dir(dir_path.as_path(), UnixPex::from(0o775))
            .is_ok());
        assert!(client.remove_dir(dir_path.as_path()).is_ok());
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-s3-ci")]
    #[serial]
    fn should_not_remove_dir() {
        crate::mock::logger();
        let mut client = setup_client();
        // Remove dir
        assert!(client.remove_dir(Path::new("test/")).is_err());
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-s3-ci")]
    #[serial]
    fn should_remove_file() {
        crate::mock::logger();
        let mut client = setup_client();
        // Create file
        let p = Path::new("a.txt");
        let file_data = "test data\n";
        let reader = Cursor::new(file_data.as_bytes());
        let mut metadata = Metadata::default();
        metadata.size = file_data.len() as u64;
        assert!(client.create_file(p, &metadata, Box::new(reader)).is_ok());
        assert!(client.remove_file(p).is_ok());
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-s3-ci")]
    #[serial]
    fn should_not_setstat_file() {
        crate::mock::logger();
        let mut client = setup_client();
        // Create file
        let p = Path::new("a.sh");
        let file_data = "echo 5\n";
        let reader = Cursor::new(file_data.as_bytes());
        let mut metadata = Metadata::default();
        metadata.size = file_data.len() as u64;
        assert!(client.create_file(p, &metadata, Box::new(reader)).is_ok());
        assert!(client
            .setstat(
                p,
                Metadata {
                    atime: SystemTime::UNIX_EPOCH,
                    ctime: SystemTime::UNIX_EPOCH,
                    gid: Some(1000),
                    mode: Some(UnixPex::from(0o755)),
                    mtime: SystemTime::UNIX_EPOCH,
                    size: 7,
                    symlink: None,
                    uid: Some(1000),
                }
            )
            .is_err());
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-s3-ci")]
    #[serial]
    fn should_stat_file() {
        crate::mock::logger();
        let mut client = setup_client();
        // Create file
        let p = Path::new("a.sh");
        let file_data = "echo 5\n";
        let reader = Cursor::new(file_data.as_bytes());
        let mut metadata = Metadata::default();
        metadata.size = file_data.len() as u64;
        assert!(client.create_file(p, &metadata, Box::new(reader)).is_ok());
        let entry = client.stat(p).ok().unwrap();
        assert_eq!(entry.name(), "a.sh");
        let mut expected_path = client.pwd().ok().unwrap();
        expected_path.push("a.sh");
        assert_eq!(entry.path(), expected_path.as_path());
        let meta = entry.metadata();
        assert_eq!(meta.size, 7);
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-s3-ci")]
    #[serial]
    fn should_not_stat_file() {
        crate::mock::logger();
        let mut client = setup_client();
        // Create file
        let p = Path::new("a.sh");
        assert!(client.stat(p).is_err());
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-s3-ci")]
    #[serial]
    fn should_make_symlink() {
        crate::mock::logger();
        let mut client = setup_client();
        // Create file
        let p = Path::new("a.sh");
        let file_data = "echo 5\n";
        let reader = Cursor::new(file_data.as_bytes());
        let mut metadata = Metadata::default();
        metadata.size = file_data.len() as u64;
        assert!(client.create_file(p, &metadata, Box::new(reader)).is_ok());
        let symlink = Path::new("b.sh");
        assert!(client.symlink(symlink, p).is_err());
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-s3-ci")]
    #[serial]
    fn should_not_make_symlink() {
        crate::mock::logger();
        let mut client = setup_client();
        // Create file
        let p = Path::new("a.sh");
        let file_data = "echo 5\n";
        let reader = Cursor::new(file_data.as_bytes());
        let mut metadata = Metadata::default();
        metadata.size = file_data.len() as u64;
        assert!(client.create_file(p, &metadata, Box::new(reader)).is_ok());
        let symlink = Path::new("b.sh");
        let file_data = "echo 5\n";
        let reader = Cursor::new(file_data.as_bytes());
        assert!(client
            .create_file(symlink, &metadata, Box::new(reader))
            .is_ok());
        assert!(client.symlink(symlink, p).is_err());
        assert!(client.remove_file(symlink).is_ok());
        assert!(client.symlink(symlink, Path::new("c.sh")).is_err());
        finalize_client(client);
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
            .setstat(Path::new("/tmp"), Metadata::default())
            .is_err());
        assert!(client.open(Path::new("/tmp/pippo.txt")).is_err());
        assert!(client
            .create(Path::new("/tmp/pippo.txt"), &Metadata::default())
            .is_err());
        assert!(client
            .append(Path::new("/tmp/pippo.txt"), &Metadata::default())
            .is_err());
    }

    // -- test utils

    #[cfg(feature = "with-s3-ci")]
    fn setup_client() -> AwsS3Fs {
        // Gather s3 environment args
        let bucket = env!("AWS_S3_BUCKET");
        let region = env!("AWS_S3_REGION");
        // Get transfer
        let mut client = AwsS3Fs::new(bucket, region);
        assert!(client.connect().is_ok());
        // Create wrkdir
        let tempdir = PathBuf::from(generate_tempdir());
        assert!(client
            .create_dir(tempdir.as_path(), UnixPex::from(0o775))
            .is_ok());
        // Change directory
        assert!(client.change_dir(tempdir.as_path()).is_ok());
        client
    }

    #[cfg(feature = "with-s3-ci")]
    fn finalize_client(mut client: AwsS3Fs) {
        // Get working directory
        let wrkdir = client.pwd().ok().unwrap();
        // Remove directory
        assert!(client.remove_dir_all(wrkdir.as_path()).is_ok());
        assert!(client.disconnect().is_ok());
    }

    #[cfg(feature = "with-s3-ci")]
    fn generate_tempdir() -> String {
        use rand::{distributions::Alphanumeric, thread_rng, Rng};
        let mut rng = thread_rng();
        let name: String = std::iter::repeat(())
            .map(|()| rng.sample(Alphanumeric))
            .map(char::from)
            .take(8)
            .collect();
        format!("/github-ci/temp_{}/", name)
    }
}
