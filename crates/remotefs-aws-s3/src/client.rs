//! # Aws S3
//!
//! Aws S3 client for remotefs.

use std::path::Path;

use aws_config::Region;
use aws_config::default_provider::credentials::DefaultCredentialsChain;
use aws_config::meta::region::RegionProviderChain;
pub use aws_sdk_s3::Client as S3Client;
use aws_sdk_s3::config::{Builder as S3ClientBuilder, Credentials, ProvideCredentials};
use aws_sdk_s3::error::ProvideErrorMetadata as _;
use aws_sdk_s3::types::Object;
use remotefs::fs::{
    AsyncReadStream, AsyncRemoteFs, AsyncWriteStream, Capabilities, ExecOutput, FileType, Metadata,
    ReadOptions, SetMetadata, UnixPex, WriteOptions,
};
use remotefs::{File, RemoteError, RemoteErrorType, RemoteResult};

use crate::key;
use crate::object::S3Object;
use crate::stream::read::S3Reader;
use crate::stream::write::S3Writer;

/// Aws S3 file system client.
///
/// The client implements [`AsyncRemoteFs`]. With the `tokio` feature,
/// [`AwsS3Fs::into_blocking`] returns a `BlockingAwsS3Fs` implementing the
/// blocking [`remotefs::RemoteFs`].
#[derive(Debug)]
pub struct AwsS3Fs {
    client: Option<S3Client>,
    // -- options
    bucket_name: String,
    /// Region name, if unset `Custom`.
    region: Option<String>,
    /// Custom endpoint (useful for MinIO).
    endpoint: Option<String>,
    profile: Option<String>,
    access_key: Option<String>,
    secret_key: Option<String>,
    security_token: Option<String>,
    session_token: Option<String>,
    /// New path style. Required for some backends, such as MinIO.
    new_path_style: bool,
}

/// Credentials used to configure an S3 client.
#[derive(Debug)]
pub enum RemoteFsCredentials {
    /// Credentials loaded from the default AWS provider chain.
    Default(DefaultCredentialsChain),
    /// Explicitly configured user credentials.
    User(Credentials),
}

impl ProvideCredentials for RemoteFsCredentials {
    fn fallback_on_interrupt(&self) -> Option<Credentials> {
        match self {
            Self::Default(credentials) => credentials.fallback_on_interrupt(),
            Self::User(credentials) => credentials.fallback_on_interrupt(),
        }
    }

    fn provide_credentials<'a>(
        &'a self,
    ) -> aws_credential_types::provider::future::ProvideCredentials<'a>
    where
        Self: 'a,
    {
        match self {
            Self::Default(credentials) => credentials.provide_credentials(),
            Self::User(credentials) => credentials.provide_credentials(),
        }
    }
}

impl AwsS3Fs {
    /// Initialize a new `AwsS3Fs` for `bucket`.
    pub fn new<S: AsRef<str>>(bucket: S) -> Self {
        Self {
            client: None,
            bucket_name: bucket.as_ref().to_string(),
            region: None,
            endpoint: None,
            profile: None,
            access_key: None,
            secret_key: None,
            security_token: None,
            session_token: None,
            new_path_style: false,
        }
    }

    /// Specify the AWS region to connect to.
    pub fn region<S: AsRef<str>>(mut self, region: S) -> Self {
        self.region = Some(region.as_ref().to_string());
        self
    }

    /// Specify a custom endpoint, such as a MinIO server.
    pub fn endpoint<S: AsRef<str>>(mut self, endpoint: S) -> Self {
        self.endpoint = Some(endpoint.as_ref().to_string());
        self
    }

    /// Set the AWS profile used to connect.
    pub fn profile<S: AsRef<str>>(mut self, profile: S) -> Self {
        self.profile = Some(profile.as_ref().to_string());
        self
    }

    /// Set whether to use path-style requests.
    pub fn new_path_style(mut self, new_path_style: bool) -> Self {
        self.new_path_style = new_path_style;
        self
    }

    /// Specify an AWS access key.
    pub fn access_key<S: AsRef<str>>(mut self, key: S) -> Self {
        self.access_key = Some(key.as_ref().to_string());
        self
    }

    /// Specify an AWS secret access key.
    pub fn secret_access_key<S: AsRef<str>>(mut self, key: S) -> Self {
        self.secret_key = Some(key.as_ref().to_string());
        self
    }

    /// Specify an AWS security token.
    pub fn security_token<S: AsRef<str>>(mut self, key: S) -> Self {
        self.security_token = Some(key.as_ref().to_string());
        self
    }

    /// Specify an AWS session token.
    pub fn session_token<S: AsRef<str>>(mut self, key: S) -> Self {
        self.session_token = Some(key.as_ref().to_string());
        self
    }

    /// Get a reference to the underlying AWS SDK client.
    pub fn client(&self) -> Option<&S3Client> {
        self.client.as_ref()
    }

    /// Return the SDK client or `NotConnected`.
    fn connected_client(&self) -> RemoteResult<&S3Client> {
        self.client
            .as_ref()
            .ok_or_else(|| RemoteError::new(RemoteErrorType::NotConnected))
    }

    /// Query objects under `key`, optionally keeping only its direct children.
    async fn query_objects(
        &self,
        key: &str,
        only_direct_children: bool,
    ) -> RemoteResult<Vec<S3Object>> {
        let client = self.connected_client()?;
        debug!("query objects with prefix '{key}'");
        let output = client
            .list_objects_v2()
            .bucket(self.bucket_name.as_str())
            .prefix(key)
            .send()
            .await
            .map_err(|error| RemoteError::with_source(RemoteErrorType::StatFailed, error))?;
        let objects = output
            .contents
            .unwrap_or_default()
            .into_iter()
            .filter(|object| !only_direct_children || Self::list_object_should_be_kept(object, key))
            .map(S3Object::from)
            .collect();
        debug!("found objects: {objects:?}");
        Ok(objects)
    }

    /// Stat the object with exactly this key.
    async fn stat_key(&self, key: &str) -> RemoteResult<S3Object> {
        let expected = key::to_path(key);
        let is_dir = key.ends_with('/');
        self.query_objects(key, false)
            .await?
            .into_iter()
            .find(|object| object.path == expected && object.is_dir == is_dir)
            .ok_or_else(|| {
                RemoteError::with_message(
                    RemoteErrorType::NoSuchFileOrDirectory,
                    format!("{key}: no such file or directory"),
                )
            })
    }

    /// Returns whether an object should be kept after a directory listing.
    fn list_object_should_be_kept(object: &Object, dir: &str) -> bool {
        Self::is_direct_child(object.key.as_deref().unwrap_or_default(), dir)
    }

    /// Check whether an S3 key is a direct child of `parent`.
    fn is_direct_child(key: &str, parent: &str) -> bool {
        key == format!("{parent}{}", S3Object::object_name(key))
            || key == format!("{parent}{}/", S3Object::object_name(key))
    }

    fn is_anonymous(&self) -> bool {
        self.access_key.is_none()
            && self.secret_key.is_none()
            && self.security_token.is_none()
            && self.session_token.is_none()
    }

    async fn load_credentials(
        &self,
        region: RegionProviderChain,
    ) -> RemoteResult<RemoteFsCredentials> {
        if self.is_anonymous() {
            Ok(RemoteFsCredentials::Default(
                DefaultCredentialsChain::builder()
                    .region(region)
                    .build()
                    .await,
            ))
        } else {
            let Some(access_key) = self.access_key.as_ref() else {
                return Err(RemoteError::with_message(
                    RemoteErrorType::AuthenticationFailed,
                    "Access key not set",
                ));
            };
            let Some(secret_key) = self.secret_key.as_ref() else {
                return Err(RemoteError::with_message(
                    RemoteErrorType::AuthenticationFailed,
                    "Secret key not set",
                ));
            };
            Ok(RemoteFsCredentials::User(Credentials::new(
                access_key,
                secret_key,
                self.session_token.clone(),
                None,
                "default",
            )))
        }
    }

    fn init_region(&self) -> RegionProviderChain {
        RegionProviderChain::first_try(self.region.as_ref().cloned().map(Region::new))
            .or_default_provider()
            .or_else(Region::new("us-west-2"))
    }

    fn make_client(
        &self,
        region: Option<Region>,
        credentials: impl ProvideCredentials + 'static,
    ) -> S3Client {
        let mut builder = S3ClientBuilder::new()
            .credentials_provider(credentials)
            .behavior_version_latest()
            .region(region);
        builder.set_force_path_style(Some(self.new_path_style));
        builder.set_endpoint_url(self.endpoint.clone());
        S3Client::from_conf(builder.build())
    }
}

/// A blocking view of [`AwsS3Fs`] that implements [`remotefs::RemoteFs`].
///
/// Every call blocks on the supplied Tokio handle and must not be made from
/// inside an async context. Build one with [`AwsS3Fs::into_blocking`].
#[cfg(feature = "tokio")]
#[cfg_attr(docsrs, doc(cfg(feature = "tokio")))]
pub type BlockingAwsS3Fs = remotefs::adapters::blocking::BlockOn<AwsS3Fs>;

#[cfg(feature = "tokio")]
#[cfg_attr(docsrs, doc(cfg(feature = "tokio")))]
impl AwsS3Fs {
    /// Wraps the client for blocking callers using the given runtime handle.
    ///
    /// The returned value implements [`remotefs::RemoteFs`] and can be stored
    /// as `Box<dyn RemoteFs>`. Calling it from an async context panics, as
    /// documented by [`tokio::runtime::Handle::block_on`].
    ///
    /// # Panics
    ///
    /// Panics if `handle` belongs to a current-thread runtime, which cannot
    /// drive the blocked operation from a non-async caller.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use remotefs::RemoteFs;
    /// use remotefs_aws_s3::AwsS3Fs;
    ///
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let runtime = tokio::runtime::Runtime::new()?;
    /// let mut client: Box<dyn RemoteFs> =
    ///     Box::new(AwsS3Fs::new("test-bucket").into_blocking(runtime.handle().clone()));
    /// client.connect()?;
    /// client.disconnect()?;
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn into_blocking(self, handle: tokio::runtime::Handle) -> BlockingAwsS3Fs {
        assert_ne!(
            handle.runtime_flavor(),
            tokio::runtime::RuntimeFlavor::CurrentThread,
            "into_blocking requires a multi-thread Tokio runtime"
        );
        remotefs::adapters::blocking::BlockOn::new(self, handle)
    }
}

#[remotefs::async_trait]
impl AsyncRemoteFs for AwsS3Fs {
    async fn connect(&mut self) -> RemoteResult<()> {
        if self.client.is_some() {
            return Err(RemoteError::new(RemoteErrorType::AlreadyConnected));
        }
        debug!("loading credentials for profile {:?}", self.profile);
        let region_provider = self.init_region();
        let region = region_provider.region().await;
        let credentials = self.load_credentials(region_provider).await?;
        trace!(
            "region: {}; endpoint: {}",
            self.region.as_deref().unwrap_or("NULL"),
            self.endpoint.as_deref().unwrap_or("NULL")
        );
        self.client = Some(self.make_client(region, credentials));
        info!(
            "connection successfully established to S3 bucket {}",
            self.bucket_name
        );
        Ok(())
    }

    async fn disconnect(&mut self) -> RemoteResult<()> {
        info!("disconnecting from S3 bucket");
        match self.client.take() {
            Some(client) => {
                drop(client);
                Ok(())
            }
            None => Err(RemoteError::new(RemoteErrorType::NotConnected)),
        }
    }

    fn is_connected(&self) -> bool {
        self.client.is_some()
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities::STREAM_READ | Capabilities::STREAM_WRITE | Capabilities::RANGE_READ
    }

    async fn list_dir(&self, path: &Path) -> RemoteResult<Vec<File>> {
        let key = key::from_path(path, true)?;
        debug!("list directory {}; key: '{key}'", path.display());
        self.query_objects(&key, true)
            .await
            .map(|objects| objects.into_iter().map(File::from).collect())
    }

    async fn stat(&self, path: &Path) -> RemoteResult<File> {
        let file_key = key::from_path(path, false)?;
        self.connected_client()?;
        if file_key.is_empty() {
            return Ok(File::new(
                "/",
                Metadata::default().file_type(FileType::Directory),
            ));
        }
        match self.stat_key(&file_key).await {
            Ok(object) => return Ok(object.into()),
            Err(error) if error.kind() == RemoteErrorType::NoSuchFileOrDirectory => {}
            Err(error) => return Err(error),
        }
        trace!("failed to stat object as file; trying as a directory");
        let dir_key = key::from_path(path, true)?;
        self.stat_key(&dir_key).await.map(File::from)
    }

    async fn exists(&self, path: &Path) -> RemoteResult<bool> {
        match self.stat(path).await {
            Ok(_) => Ok(true),
            Err(error) if error.kind() == RemoteErrorType::NoSuchFileOrDirectory => Ok(false),
            Err(error) => Err(error),
        }
    }

    async fn set_metadata(&self, _path: &Path, _metadata: &SetMetadata) -> RemoteResult<()> {
        Err(RemoteError::new(RemoteErrorType::UnsupportedFeature))
    }

    async fn create_dir(&self, path: &Path, _mode: Option<UnixPex>) -> RemoteResult<()> {
        let dir = key::from_path(path, true)?;
        let client = self.connected_client()?;
        debug!("making directory '{dir}'");
        if self.stat_key(&dir).await.is_ok() {
            error!("directory '{dir}' already exists");
            return Err(RemoteError::new(RemoteErrorType::AlreadyExists));
        }
        client
            .put_object()
            .bucket(self.bucket_name.as_str())
            .key(dir.as_str())
            .send()
            .await
            .map(|_| ())
            .map_err(|error| RemoteError::with_source(RemoteErrorType::FileCreateDenied, error))
    }

    async fn remove_file(&self, path: &Path) -> RemoteResult<()> {
        let key = key::from_path(path, false)?;
        let client = self.connected_client()?;
        self.stat_key(&key).await?;
        debug!("removing object '{key}'");
        client
            .delete_object()
            .bucket(self.bucket_name.as_str())
            .key(key.as_str())
            .send()
            .await
            .map(|_| ())
            .map_err(|error| RemoteError::with_source(RemoteErrorType::CouldNotRemoveFile, error))
    }

    async fn remove_dir(&self, path: &Path) -> RemoteResult<()> {
        let key = key::from_path(path, true)?;
        let client = self.connected_client()?;
        let children = self.query_objects(&key, true).await?;
        if !children.is_empty() {
            return Err(RemoteError::new(RemoteErrorType::DirectoryNotEmpty));
        }
        self.stat_key(&key).await?;
        debug!("removing directory '{key}'");
        client
            .delete_object()
            .bucket(self.bucket_name.as_str())
            .key(key.as_str())
            .send()
            .await
            .map(|_| ())
            .map_err(|error| RemoteError::with_source(RemoteErrorType::CouldNotRemoveFile, error))
    }

    async fn remove_dir_all(&self, path: &Path) -> RemoteResult<()> {
        let key = key::from_path(path, true)?;
        match self.stat(path).await {
            Ok(entry) if entry.is_dir() => {
                for child in self.list_dir(entry.path()).await? {
                    self.remove_dir_all(child.path()).await?;
                }
                match self.remove_dir(entry.path()).await {
                    Ok(()) => Ok(()),
                    Err(error)
                        if error.kind() == RemoteErrorType::NoSuchFileOrDirectory
                            && self.query_objects(&key, false).await?.is_empty() =>
                    {
                        Ok(())
                    }
                    Err(error) => Err(error),
                }
            }
            Ok(entry) => self.remove_file(entry.path()).await,
            Err(error) if error.kind() == RemoteErrorType::NoSuchFileOrDirectory => {
                let objects = self.query_objects(&key, false).await?;
                if objects.is_empty() {
                    return Err(error);
                }
                let client = self.connected_client()?;
                for object in objects {
                    let object_key = key::from_path(object.path.as_path(), object.is_dir)?;
                    client
                        .delete_object()
                        .bucket(self.bucket_name.as_str())
                        .key(object_key.as_str())
                        .send()
                        .await
                        .map_err(|delete_error| {
                            RemoteError::with_source(
                                RemoteErrorType::CouldNotRemoveFile,
                                delete_error,
                            )
                        })?;
                }
                Ok(())
            }
            Err(error) => Err(error),
        }
    }

    async fn rename(&self, _src: &Path, _dest: &Path) -> RemoteResult<()> {
        Err(RemoteError::new(RemoteErrorType::UnsupportedFeature))
    }

    async fn copy(&self, _src: &Path, _dest: &Path) -> RemoteResult<()> {
        Err(RemoteError::new(RemoteErrorType::UnsupportedFeature))
    }

    async fn symlink(&self, _path: &Path, _target: &Path) -> RemoteResult<()> {
        Err(RemoteError::new(RemoteErrorType::UnsupportedFeature))
    }

    async fn open(&self, path: &Path, opts: &ReadOptions) -> RemoteResult<AsyncReadStream> {
        let key = key::from_path(path, false)?;
        let client = self.connected_client()?;
        if opts.length == Some(0) {
            return Ok(AsyncReadStream::new(S3Reader::empty()));
        }
        let mut request = client
            .get_object()
            .bucket(self.bucket_name.as_str())
            .key(key.as_str());
        if let Some(range) = http_range(opts) {
            debug!("GET '{key}' with range {range}");
            request = request.range(range);
        } else {
            debug!("GET '{key}'");
        }
        match request.send().await {
            Ok(output) => Ok(AsyncReadStream::new(S3Reader::new(output.body))),
            Err(error)
                if error
                    .as_service_error()
                    .is_some_and(|service_error| service_error.is_no_such_key()) =>
            {
                Err(RemoteError::with_source(
                    RemoteErrorType::NoSuchFileOrDirectory,
                    error,
                ))
            }
            Err(error) if error.code() == Some("InvalidRange") => {
                debug!("range {opts:?} is beyond EOF for '{key}'; returning empty stream");
                Ok(AsyncReadStream::new(S3Reader::empty()))
            }
            Err(error) => Err(RemoteError::with_source(
                RemoteErrorType::ProtocolError,
                error,
            )),
        }
    }

    async fn create(&self, path: &Path, opts: &WriteOptions) -> RemoteResult<AsyncWriteStream> {
        let key = key::from_path(path, false)?;
        let client = self.connected_client()?.clone();
        debug!("create '{key}' (size hint: {:?})", opts.size_hint);
        Ok(AsyncWriteStream::new(S3Writer::new(
            client,
            self.bucket_name.clone(),
            key,
        )))
    }

    async fn append(&self, _path: &Path, _opts: &WriteOptions) -> RemoteResult<AsyncWriteStream> {
        Err(RemoteError::new(RemoteErrorType::UnsupportedFeature))
    }

    async fn exec(&self, _cmd: &str) -> RemoteResult<ExecOutput> {
        Err(RemoteError::new(RemoteErrorType::UnsupportedFeature))
    }
}

/// Build the HTTP `Range` header for a ranged read.
pub(crate) fn http_range(opts: &ReadOptions) -> Option<String> {
    let offset = opts.offset.unwrap_or(0);
    match opts.length {
        None if offset == 0 => None,
        None => Some(format!("bytes={offset}-")),
        Some(length) => {
            let end = offset.saturating_add(length.saturating_sub(1));
            Some(format!("bytes={offset}-{end}"))
        }
    }
}

#[cfg(test)]
mod test {
    use std::path::Path;
    #[cfg(any(feature = "with-s3-ci", feature = "with-containers"))]
    use std::path::PathBuf;

    #[cfg(any(feature = "with-s3-ci", feature = "with-containers"))]
    use futures::io::{AsyncReadExt as _, AsyncWriteExt as _, Cursor};
    use pretty_assertions::assert_eq;

    use super::*;
    #[cfg(feature = "with-containers")]
    use crate::mock::container::Minio;
    #[cfg(any(feature = "with-s3-ci", feature = "with-containers"))]
    use crate::stream::write::PART_SIZE;

    #[test]
    fn should_init_s3() {
        let s3 = AwsS3Fs::new("aws-s3-test");
        assert_eq!(s3.bucket_name.as_str(), "aws-s3-test");
        assert!(s3.region.is_none());
        assert!(s3.endpoint.is_none());
        assert!(s3.is_anonymous());
        assert!(!s3.new_path_style);
        assert!(s3.client.is_none());
        assert!(s3.access_key.is_none());
        assert!(s3.profile.is_none());
        assert!(s3.secret_key.is_none());
        assert!(s3.security_token.is_none());
        assert!(s3.session_token.is_none());
        assert!(!s3.is_connected());
    }

    #[test]
    fn should_init_s3_with_options() {
        let s3 = AwsS3Fs::new("aws-s3-test")
            .region("eu-central-1")
            .access_key("AKIA0000")
            .profile("default")
            .secret_access_key("PASSWORD")
            .security_token("secret")
            .session_token("token")
            .new_path_style(true)
            .endpoint("omar");
        assert_eq!(s3.bucket_name.as_str(), "aws-s3-test");
        assert_eq!(s3.region.as_deref(), Some("eu-central-1"));
        assert_eq!(s3.access_key.as_deref(), Some("AKIA0000"));
        assert_eq!(s3.secret_key.as_deref(), Some("PASSWORD"));
        assert_eq!(s3.security_token.as_deref(), Some("secret"));
        assert_eq!(s3.session_token.as_deref(), Some("token"));
        assert_eq!(s3.endpoint.as_deref(), Some("omar"));
        assert!(!s3.is_anonymous());
        assert!(s3.new_path_style);
    }

    #[test]
    fn should_advertise_capabilities() {
        let caps = AwsS3Fs::new("bucket").capabilities();
        assert!(caps.contains(Capabilities::STREAM_READ));
        assert!(caps.contains(Capabilities::STREAM_WRITE));
        assert!(caps.contains(Capabilities::RANGE_READ));
        assert!(!caps.contains(Capabilities::APPEND));
        assert!(!caps.contains(Capabilities::COPY));
        assert!(!caps.contains(Capabilities::SYMLINK));
        assert!(!caps.contains(Capabilities::SET_METADATA));
        assert!(!caps.contains(Capabilities::SEEK_READ));
        assert!(!caps.contains(Capabilities::EXEC));
    }

    #[test]
    fn should_build_http_range_header() {
        assert_eq!(http_range(&ReadOptions::default()), None);
        assert_eq!(http_range(&ReadOptions::default().offset(0)), None);
        assert_eq!(
            http_range(&ReadOptions::default().offset(5)).as_deref(),
            Some("bytes=5-")
        );
        assert_eq!(
            http_range(&ReadOptions::default().offset(2).length(3)).as_deref(),
            Some("bytes=2-4")
        );
        assert_eq!(
            http_range(&ReadOptions::default().length(10)).as_deref(),
            Some("bytes=0-9")
        );
        let expected = format!("bytes={}-{}", u64::MAX, u64::MAX);
        assert_eq!(
            http_range(&ReadOptions::default().offset(u64::MAX).length(2)).as_deref(),
            Some(expected.as_str())
        );
    }

    #[test]
    fn s3_is_direct_child() {
        assert!(AwsS3Fs::is_direct_child("pippo/", ""));
        assert!(!AwsS3Fs::is_direct_child("pippo/sottocartella/", ""));
        assert!(AwsS3Fs::is_direct_child("pippo/sottocartella/", "pippo/"));
        assert!(!AwsS3Fs::is_direct_child("pippo/sottocartella/", "pippo"));
        assert!(AwsS3Fs::is_direct_child(
            "pippo/sottocartella/readme.md",
            "pippo/sottocartella/"
        ));
    }

    #[tokio::test]
    async fn should_reject_relative_paths_before_connection_check() {
        let client = AwsS3Fs::new("bucket");
        let path = Path::new("relative/file.txt");
        assert_eq!(
            client.stat(path).await.unwrap_err().kind(),
            RemoteErrorType::InvalidPath
        );
        assert_eq!(
            client.list_dir(path).await.unwrap_err().kind(),
            RemoteErrorType::InvalidPath
        );
        assert_eq!(
            client.create_dir(path, None).await.unwrap_err().kind(),
            RemoteErrorType::InvalidPath
        );
        assert_eq!(
            client.remove_file(path).await.unwrap_err().kind(),
            RemoteErrorType::InvalidPath
        );
        assert_eq!(
            client.remove_dir(path).await.unwrap_err().kind(),
            RemoteErrorType::InvalidPath
        );
        assert_eq!(
            client.exists(path).await.unwrap_err().kind(),
            RemoteErrorType::InvalidPath
        );
    }

    #[tokio::test]
    async fn should_return_errors_on_uninitialized_client() {
        let mut client = AwsS3Fs::new("aws-s3-test").region("eu-central-1");
        let path = Path::new("/tmp");
        assert_eq!(
            client.stat(path).await.unwrap_err().kind(),
            RemoteErrorType::NotConnected
        );
        assert_eq!(
            client.list_dir(path).await.unwrap_err().kind(),
            RemoteErrorType::NotConnected
        );
        assert_eq!(
            client
                .create_dir(path, Some(UnixPex::from(0o755)))
                .await
                .unwrap_err()
                .kind(),
            RemoteErrorType::NotConnected
        );
        assert_eq!(
            client.remove_file(path).await.unwrap_err().kind(),
            RemoteErrorType::NotConnected
        );
        assert_eq!(
            client.remove_dir(path).await.unwrap_err().kind(),
            RemoteErrorType::NotConnected
        );
        assert_eq!(
            client.remove_dir_all(path).await.unwrap_err().kind(),
            RemoteErrorType::NotConnected
        );
        assert_eq!(
            client
                .append(path, &WriteOptions::default())
                .await
                .unwrap_err()
                .kind(),
            RemoteErrorType::UnsupportedFeature
        );
        assert_eq!(
            client.disconnect().await.unwrap_err().kind(),
            RemoteErrorType::NotConnected
        );
        assert!(client.copy(path, Path::new("/culonia")).await.is_err());
        assert!(client.rename(path, Path::new("/culonia")).await.is_err());
        assert!(
            client
                .symlink(Path::new("/a"), Path::new("/b"))
                .await
                .is_err()
        );
        assert!(client.exec("echo 5").await.is_err());
        assert!(
            client
                .set_metadata(path, &SetMetadata::default())
                .await
                .is_err()
        );
    }

    fn is_send<T: Send>(_send: T) {}

    fn is_sync<T: Sync>(_sync: T) {}

    #[test]
    fn test_should_be_sync() {
        is_sync(AwsS3Fs::new("bucket"));
    }

    #[test]
    fn test_should_be_send() {
        is_send(AwsS3Fs::new("bucket"));
    }

    #[test]
    fn should_be_object_safe() {
        let _: Box<dyn AsyncRemoteFs> = Box::new(AwsS3Fs::new("bucket"));
    }

    #[test]
    fn should_initialize_test_logger() {
        crate::mock::logger();
    }

    #[cfg(any(feature = "with-s3-ci", feature = "with-containers"))]
    #[tokio::test]
    async fn should_not_append_to_file() {
        crate::mock::logger();
        let ctx = setup_client().await;
        let path = ctx.wrkdir.join("a.txt");
        let mut source = Cursor::new(b"x".to_vec());
        let error = ctx
            .client
            .append_file(&path, &WriteOptions::default(), &mut source)
            .await
            .unwrap_err();
        assert_eq!(error.kind(), RemoteErrorType::UnsupportedFeature);
        finalize_client(ctx).await;
    }

    #[cfg(any(feature = "with-s3-ci", feature = "with-containers"))]
    #[tokio::test]
    async fn should_not_copy_file() {
        let ctx = setup_client().await;
        let source = ctx.wrkdir.join("a.txt");
        upload(&ctx.client, &source, b"test data\n").await.unwrap();
        let error = ctx
            .client
            .copy(&source, &ctx.wrkdir.join("b.txt"))
            .await
            .unwrap_err();
        assert_eq!(error.kind(), RemoteErrorType::UnsupportedFeature);
        finalize_client(ctx).await;
    }

    #[cfg(any(feature = "with-s3-ci", feature = "with-containers"))]
    #[tokio::test]
    async fn should_not_rename_file() {
        let ctx = setup_client().await;
        let source = ctx.wrkdir.join("a.txt");
        upload(&ctx.client, &source, b"test data\n").await.unwrap();
        let error = ctx
            .client
            .rename(&source, &ctx.wrkdir.join("b.txt"))
            .await
            .unwrap_err();
        assert_eq!(error.kind(), RemoteErrorType::UnsupportedFeature);
        finalize_client(ctx).await;
    }

    #[cfg(any(feature = "with-s3-ci", feature = "with-containers"))]
    #[tokio::test]
    async fn should_not_exec_command() {
        let ctx = setup_client().await;
        let error = ctx.client.exec("echo 5").await.unwrap_err();
        assert_eq!(error.kind(), RemoteErrorType::UnsupportedFeature);
        finalize_client(ctx).await;
    }

    #[cfg(any(feature = "with-s3-ci", feature = "with-containers"))]
    #[tokio::test]
    async fn should_not_set_metadata() {
        let ctx = setup_client().await;
        let path = ctx.wrkdir.join("a.txt");
        upload(&ctx.client, &path, b"test data\n").await.unwrap();
        let error = ctx
            .client
            .set_metadata(&path, &SetMetadata::default().mode(UnixPex::from(0o755)))
            .await
            .unwrap_err();
        assert_eq!(error.kind(), RemoteErrorType::UnsupportedFeature);
        finalize_client(ctx).await;
    }

    #[cfg(any(feature = "with-s3-ci", feature = "with-containers"))]
    #[tokio::test]
    async fn should_not_make_symlink() {
        let ctx = setup_client().await;
        let path = ctx.wrkdir.join("a.txt");
        upload(&ctx.client, &path, b"test data\n").await.unwrap();
        let error = ctx
            .client
            .symlink(&ctx.wrkdir.join("b.sh"), &path)
            .await
            .unwrap_err();
        assert_eq!(error.kind(), RemoteErrorType::UnsupportedFeature);
        finalize_client(ctx).await;
    }

    #[cfg(any(feature = "with-s3-ci", feature = "with-containers"))]
    #[tokio::test]
    async fn should_create_directory() {
        let ctx = setup_client().await;
        let path = ctx.wrkdir.join("mydir");
        ctx.client
            .create_dir(&path, Some(UnixPex::from(0o755)))
            .await
            .unwrap();
        assert!(ctx.client.stat(&path).await.unwrap().is_dir());
        finalize_client(ctx).await;
    }

    #[cfg(any(feature = "with-s3-ci", feature = "with-containers"))]
    #[tokio::test]
    async fn should_not_create_directory_cause_already_exists() {
        let ctx = setup_client().await;
        let path = ctx.wrkdir.join("mydir");
        ctx.client.create_dir(&path, None).await.unwrap();
        let error = ctx.client.create_dir(&path, None).await.unwrap_err();
        assert_eq!(error.kind(), RemoteErrorType::AlreadyExists);
        finalize_client(ctx).await;
    }

    #[cfg(any(feature = "with-s3-ci", feature = "with-containers"))]
    #[tokio::test]
    async fn should_create_file() {
        let ctx = setup_client().await;
        let path = ctx.wrkdir.join("a.txt");
        assert_eq!(
            upload(&ctx.client, &path, b"test data\n").await.unwrap(),
            10
        );
        assert_eq!(
            ctx.client.stat(&path).await.unwrap().metadata().size,
            Some(10)
        );
        finalize_client(ctx).await;
    }

    #[cfg(any(feature = "with-s3-ci", feature = "with-containers"))]
    #[tokio::test]
    async fn should_write_empty_file() {
        let ctx = setup_client().await;
        let path = ctx.wrkdir.join("empty.txt");
        assert_eq!(upload(&ctx.client, &path, b"").await.unwrap(), 0);
        assert_eq!(
            ctx.client.stat(&path).await.unwrap().metadata().size,
            Some(0)
        );
        assert!(
            download(&ctx.client, &path, &ReadOptions::default())
                .await
                .unwrap()
                .is_empty()
        );
        finalize_client(ctx).await;
    }

    #[cfg(any(feature = "with-s3-ci", feature = "with-containers"))]
    #[tokio::test]
    async fn should_tell_whether_file_exists() {
        let ctx = setup_client().await;
        let path = ctx.wrkdir.join("a.txt");
        upload(&ctx.client, &path, b"test data\n").await.unwrap();
        assert!(ctx.client.exists(&path).await.unwrap());
        assert!(!ctx.client.exists(&ctx.wrkdir.join("b.txt")).await.unwrap());
        assert!(
            !ctx.client
                .exists(Path::new("/tmp/ppppp/bhhrhu"))
                .await
                .unwrap()
        );
        assert!(ctx.client.exists(Path::new("/")).await.unwrap());
        finalize_client(ctx).await;
    }

    #[cfg(any(feature = "with-s3-ci", feature = "with-containers"))]
    #[tokio::test]
    async fn should_list_dir() {
        let ctx = setup_client().await;
        let path = ctx.wrkdir.join("a.txt");
        upload(&ctx.client, &path, b"test data\n").await.unwrap();
        let entries = ctx.client.list_dir(&ctx.wrkdir).await.unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name(), "a.txt");
        assert_eq!(entries[0].path, path);
        assert_eq!(entries[0].extension().as_deref(), Some("txt"));
        assert_eq!(entries[0].metadata().size, Some(10));
        assert_eq!(entries[0].metadata().mode, None);
        finalize_client(ctx).await;
    }

    #[cfg(any(feature = "with-s3-ci", feature = "with-containers"))]
    #[tokio::test]
    async fn should_open_file() {
        let ctx = setup_client().await;
        let path = ctx.wrkdir.join("a.txt");
        upload(&ctx.client, &path, b"test data\n").await.unwrap();
        assert_eq!(
            download(&ctx.client, &path, &ReadOptions::default())
                .await
                .unwrap(),
            b"test data\n"
        );
        finalize_client(ctx).await;
    }

    #[cfg(any(feature = "with-s3-ci", feature = "with-containers"))]
    #[tokio::test]
    async fn should_read_ranges() {
        let ctx = setup_client().await;
        let path = ctx.wrkdir.join("ranges.txt");
        upload(&ctx.client, &path, b"0123456789").await.unwrap();
        assert_eq!(
            download(
                &ctx.client,
                &path,
                &ReadOptions::default().offset(2).length(3)
            )
            .await
            .unwrap(),
            b"234"
        );
        assert_eq!(
            download(&ctx.client, &path, &ReadOptions::default().offset(7))
                .await
                .unwrap(),
            b"789"
        );
        assert!(
            download(&ctx.client, &path, &ReadOptions::default().length(0))
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            download(
                &ctx.client,
                &path,
                &ReadOptions::default().offset(2).length(0)
            )
            .await
            .unwrap()
            .is_empty()
        );
        assert!(
            download(&ctx.client, &path, &ReadOptions::default().offset(10))
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            download(&ctx.client, &path, &ReadOptions::default().offset(100))
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            download(
                &ctx.client,
                &path,
                &ReadOptions::default().offset(8).length(50)
            )
            .await
            .unwrap(),
            b"89"
        );
        finalize_client(ctx).await;
    }

    #[cfg(any(feature = "with-s3-ci", feature = "with-containers"))]
    #[tokio::test]
    async fn should_write_big_data() {
        let ctx = setup_client().await;
        let path = ctx.wrkdir.join("large.bin");
        let data = vec![1_u8; PART_SIZE * 2 + 1];
        assert_eq!(
            upload(&ctx.client, &path, &data).await.unwrap(),
            data.len() as u64
        );
        assert_eq!(
            download(&ctx.client, &path, &ReadOptions::default())
                .await
                .unwrap()
                .len(),
            data.len()
        );
        assert_eq!(
            ctx.client.stat(&path).await.unwrap().metadata().size,
            Some(data.len() as u64)
        );
        finalize_client(ctx).await;
    }

    #[cfg(any(feature = "with-s3-ci", feature = "with-containers"))]
    #[tokio::test]
    async fn should_stream_write_and_finish() {
        let ctx = setup_client().await;
        let path = ctx.wrkdir.join("stream.txt");
        let mut stream = ctx
            .client
            .create(&path, &WriteOptions::default())
            .await
            .unwrap();
        stream.write_all(b"hello ").await.unwrap();
        stream.write_all(b"world").await.unwrap();
        stream.flush().await.unwrap();
        stream.finish().await.unwrap();
        assert_eq!(
            download(&ctx.client, &path, &ReadOptions::default())
                .await
                .unwrap(),
            b"hello world"
        );
        finalize_client(ctx).await;
    }

    #[cfg(any(feature = "with-s3-ci", feature = "with-containers"))]
    #[tokio::test]
    async fn should_abandon_dropped_write_stream() {
        let ctx = setup_client().await;
        let path = ctx.wrkdir.join("abandoned.txt");
        let mut stream = ctx
            .client
            .create(&path, &WriteOptions::default())
            .await
            .unwrap();
        stream.write_all(b"never").await.unwrap();
        drop(stream);
        assert!(!ctx.client.exists(&path).await.unwrap());
        finalize_client(ctx).await;
    }

    #[cfg(any(feature = "with-s3-ci", feature = "with-containers"))]
    #[tokio::test]
    async fn should_stream_read_and_finish() {
        let ctx = setup_client().await;
        let path = ctx.wrkdir.join("stream.txt");
        upload(&ctx.client, &path, b"test data\n").await.unwrap();
        let mut stream = ctx
            .client
            .open(&path, &ReadOptions::default())
            .await
            .unwrap();
        assert!(!stream.seekable());
        let mut output = Vec::new();
        stream.read_to_end(&mut output).await.unwrap();
        stream.finish().await.unwrap();
        assert_eq!(output, b"test data\n");
        finalize_client(ctx).await;
    }

    #[cfg(any(feature = "with-s3-ci", feature = "with-containers"))]
    #[tokio::test]
    async fn should_not_open_file() {
        let ctx = setup_client().await;
        let error = ctx
            .client
            .open(&ctx.wrkdir.join("missing"), &ReadOptions::default())
            .await
            .unwrap_err();
        assert_eq!(error.kind(), RemoteErrorType::NoSuchFileOrDirectory);
        finalize_client(ctx).await;
    }

    #[cfg(any(feature = "with-s3-ci", feature = "with-containers"))]
    #[tokio::test]
    async fn should_remove_dir_all() {
        let ctx = setup_client().await;
        let directory = ctx.wrkdir.join("test");
        let nested = directory.join("nested");
        ctx.client.create_dir(&directory, None).await.unwrap();
        upload(&ctx.client, &directory.join("a.txt"), b"a")
            .await
            .unwrap();
        ctx.client.create_dir(&nested, None).await.unwrap();
        upload(&ctx.client, &nested.join("b.txt"), b"b")
            .await
            .unwrap();
        ctx.client.remove_dir_all(&directory).await.unwrap();
        assert!(!ctx.client.exists(&directory).await.unwrap());
        assert!(!ctx.client.exists(&nested.join("b.txt")).await.unwrap());
        finalize_client(ctx).await;
    }

    #[cfg(any(feature = "with-s3-ci", feature = "with-containers"))]
    #[tokio::test]
    async fn should_remove_dir() {
        let ctx = setup_client().await;
        let path = ctx.wrkdir.join("test");
        ctx.client.create_dir(&path, None).await.unwrap();
        ctx.client.remove_dir(&path).await.unwrap();
        assert!(!ctx.client.exists(&path).await.unwrap());
        finalize_client(ctx).await;
    }

    #[cfg(any(feature = "with-s3-ci", feature = "with-containers"))]
    #[tokio::test]
    async fn should_not_remove_dir() {
        let ctx = setup_client().await;
        let error = ctx
            .client
            .remove_dir(&ctx.wrkdir.join("missing"))
            .await
            .unwrap_err();
        assert_eq!(error.kind(), RemoteErrorType::NoSuchFileOrDirectory);
        finalize_client(ctx).await;
    }

    #[cfg(any(feature = "with-s3-ci", feature = "with-containers"))]
    #[tokio::test]
    async fn should_not_remove_non_empty_dir() {
        let ctx = setup_client().await;
        let directory = ctx.wrkdir.join("test");
        ctx.client.create_dir(&directory, None).await.unwrap();
        upload(&ctx.client, &directory.join("a.txt"), b"a")
            .await
            .unwrap();
        let error = ctx.client.remove_dir(&directory).await.unwrap_err();
        assert_eq!(error.kind(), RemoteErrorType::DirectoryNotEmpty);
        finalize_client(ctx).await;
    }

    #[cfg(any(feature = "with-s3-ci", feature = "with-containers"))]
    #[tokio::test]
    async fn should_remove_file() {
        let ctx = setup_client().await;
        let path = ctx.wrkdir.join("a.txt");
        upload(&ctx.client, &path, b"test data\n").await.unwrap();
        ctx.client.remove_file(&path).await.unwrap();
        assert_eq!(
            ctx.client.stat(&path).await.unwrap_err().kind(),
            RemoteErrorType::NoSuchFileOrDirectory
        );
        finalize_client(ctx).await;
    }

    #[cfg(any(feature = "with-s3-ci", feature = "with-containers"))]
    #[tokio::test]
    async fn should_not_remove_missing_file() {
        let ctx = setup_client().await;
        let error = ctx
            .client
            .remove_file(&ctx.wrkdir.join("missing"))
            .await
            .unwrap_err();
        assert_eq!(error.kind(), RemoteErrorType::NoSuchFileOrDirectory);
        finalize_client(ctx).await;
    }

    #[cfg(any(feature = "with-s3-ci", feature = "with-containers"))]
    #[tokio::test]
    async fn should_stat_file() {
        let ctx = setup_client().await;
        let path = ctx.wrkdir.join("a.sh");
        upload(&ctx.client, &path, b"#!/bin\n").await.unwrap();
        let file = ctx.client.stat(&path).await.unwrap();
        assert_eq!(file.name(), "a.sh");
        assert_eq!(file.path(), path.as_path());
        assert_eq!(file.metadata().size, Some(7));
        assert!(file.is_file());
        finalize_client(ctx).await;
    }

    #[cfg(any(feature = "with-s3-ci", feature = "with-containers"))]
    #[tokio::test]
    async fn should_not_stat_file() {
        let ctx = setup_client().await;
        let error = ctx.client.stat(&ctx.wrkdir.join("a.sh")).await.unwrap_err();
        assert_eq!(error.kind(), RemoteErrorType::NoSuchFileOrDirectory);
        finalize_client(ctx).await;
    }

    #[cfg(any(feature = "with-s3-ci", feature = "with-containers"))]
    #[tokio::test]
    async fn should_not_connect_twice() {
        let mut ctx = setup_client().await;
        assert_eq!(
            ctx.client.connect().await.unwrap_err().kind(),
            RemoteErrorType::AlreadyConnected
        );
        finalize_client(ctx).await;
    }

    #[cfg(all(feature = "with-containers", feature = "tokio"))]
    #[test]
    fn should_work_through_block_on_adapter() {
        use std::io::Cursor as StdCursor;

        use remotefs::RemoteFs as _;

        crate::mock::logger();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let ctx = runtime.block_on(setup_client());
        let Ctx {
            client,
            wrkdir,
            container: _container,
        } = ctx;
        let blocking = client.into_blocking(runtime.handle().clone());
        let path = wrkdir.join("blocking.txt");
        let mut source = StdCursor::new(b"blocking".to_vec());
        assert_eq!(
            blocking
                .write_file(&path, &WriteOptions::default().size_hint(8), &mut source)
                .unwrap(),
            8
        );
        let mut destination = StdCursor::new(Vec::new());
        assert_eq!(
            blocking
                .read_file(&path, &ReadOptions::default(), &mut destination)
                .unwrap(),
            8
        );
        assert_eq!(destination.into_inner(), b"blocking");
        assert!(blocking.remove_dir_all(&wrkdir).is_ok());
        let mut client = blocking.into_inner();
        assert!(runtime.block_on(client.disconnect()).is_ok());
        runtime.block_on(async {
            drop(_container);
        });
    }

    #[cfg(any(feature = "with-s3-ci", feature = "with-containers"))]
    struct Ctx {
        client: AwsS3Fs,
        /// Absolute working directory created for the test.
        wrkdir: PathBuf,
        #[cfg(feature = "with-containers")]
        container: Minio,
        #[cfg(all(feature = "with-s3-ci", not(feature = "with-containers")))]
        container: (),
    }

    #[cfg(all(feature = "with-s3-ci", not(feature = "with-containers")))]
    async fn setup_client() -> Ctx {
        let bucket = env!("AWS_S3_BUCKET");
        let mut client = AwsS3Fs::new(bucket);
        assert!(client.connect().await.is_ok());
        let wrkdir = PathBuf::from(generate_tempdir());
        client
            .create_dir(wrkdir.as_path(), None)
            .await
            .expect("could not create test directory");
        Ctx {
            client,
            wrkdir,
            container: (),
        }
    }

    #[cfg(feature = "with-containers")]
    async fn setup_client() -> Ctx {
        let minio = Minio::start().await;
        let port = minio.port().await;
        let mut client = AwsS3Fs::new("github-ci")
            .endpoint(format!("http://localhost:{port}"))
            .access_key("minioadmin")
            .secret_access_key("minioadmin")
            .new_path_style(true);
        assert!(client.connect().await.is_ok());
        client
            .client()
            .unwrap()
            .create_bucket()
            .bucket("github-ci")
            .send()
            .await
            .expect("could not create bucket");
        let wrkdir = PathBuf::from(generate_tempdir());
        client
            .create_dir(wrkdir.as_path(), None)
            .await
            .expect("could not create test directory");
        Ctx {
            client,
            wrkdir,
            container: minio,
        }
    }

    #[cfg(any(feature = "with-s3-ci", feature = "with-containers"))]
    async fn finalize_client(ctx: Ctx) {
        let Ctx {
            mut client,
            wrkdir,
            container: _container,
        } = ctx;
        match client.remove_dir_all(wrkdir.as_path()).await {
            Ok(()) => {}
            Err(error) if error.kind() == RemoteErrorType::NoSuchFileOrDirectory => {}
            Err(error) => panic!("could not finalize test directory: {error:?}"),
        }
        assert!(client.disconnect().await.is_ok());
    }

    #[cfg(any(feature = "with-s3-ci", feature = "with-containers"))]
    fn generate_tempdir() -> String {
        use rand::distr::Alphanumeric;
        use rand::{RngExt as _, rng};

        let mut rng = rng();
        let name: String = std::iter::repeat(())
            .map(|()| rng.sample(Alphanumeric))
            .map(char::from)
            .take(8)
            .collect();
        format!("/github-ci/temp_{name}/")
    }

    #[cfg(any(feature = "with-s3-ci", feature = "with-containers"))]
    async fn upload(client: &AwsS3Fs, path: &Path, data: &[u8]) -> RemoteResult<u64> {
        let opts = WriteOptions::default().size_hint(data.len() as u64);
        let mut source = Cursor::new(data.to_vec());
        client.write_file(path, &opts, &mut source).await
    }

    #[cfg(any(feature = "with-s3-ci", feature = "with-containers"))]
    async fn download(client: &AwsS3Fs, path: &Path, opts: &ReadOptions) -> RemoteResult<Vec<u8>> {
        let mut destination = Cursor::new(Vec::new());
        client.read_file(path, opts, &mut destination).await?;
        Ok(destination.into_inner())
    }
}
