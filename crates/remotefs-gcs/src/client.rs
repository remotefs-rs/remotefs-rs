//! Google Cloud Storage implementation of [`AsyncRemoteFs`].

use std::collections::BTreeMap;
use std::future::poll_fn;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::pin::Pin;

use futures_io::{AsyncRead, AsyncWrite};
use google_cloud_gax::backoff_policy::BackoffPolicyArg;
use google_cloud_gax::paginator::Paginator;
use google_cloud_gax::retry_policy::RetryPolicyArg;
use google_cloud_gax::retry_throttler::RetryThrottlerArg;
use google_cloud_storage::client::{Storage, StorageControl};
use google_cloud_storage::model_ext::ReadRange;
use remotefs::fs::{
    AsyncReadStream, AsyncRemoteFs, AsyncWriteStream, Capabilities, ExecOutput, ReadOptions,
    SetMetadata, UnixPex, WriteOptions,
};
use remotefs::{File, RemoteError, RemoteErrorType, RemoteResult};

use crate::credentials::GoogleCloudStorageCredentials;
use crate::error::{is_range_not_satisfiable, map_gcs_create_error, map_gcs_error};
use crate::key;
use crate::object::GcsObject;
use crate::stream::read::GcsReader;
use crate::stream::write::GcsWriter;

const BUCKET_RESOURCE_PREFIX: &str = "projects/_/buckets/";
const OBJECT_DELIMITER: &str = "/";
pub(crate) const CONTENT_TYPE: &str = "application/octet-stream";

/// An asynchronous Google Cloud Storage filesystem client.
///
/// The client keeps the storage and storage-control clients separate because
/// the Google SDK uses the former for object bytes and the latter for object
/// metadata operations. Both clients are created together by
/// [`AsyncRemoteFs::connect`]. Blocking callers can wrap the client in
/// `remotefs::adapters::blocking::BlockOn` when the `tokio` feature is enabled.
#[derive(Debug)]
pub struct GoogleCloudStorageFs {
    storage: Option<Storage>,
    control: Option<StorageControl>,
    bucket: String,
    credentials: GoogleCloudStorageCredentials,
    endpoint: Option<String>,
    control_endpoint: Option<String>,
    universe_domain: Option<String>,
    backoff_policy: Option<BackoffPolicyArg>,
    retry_policy: Option<RetryPolicyArg>,
    retry_throttler: Option<RetryThrottlerArg>,
}

impl GoogleCloudStorageFs {
    /// Creates a client using Application Default Credentials.
    #[must_use]
    pub fn new<S>(bucket: S) -> Self
    where
        S: Into<String>,
    {
        Self::with_credentials(bucket, GoogleCloudStorageCredentials::default())
    }

    /// Creates a client with an explicit credential mode.
    #[must_use]
    pub fn with_credentials<S>(bucket: S, credentials: GoogleCloudStorageCredentials) -> Self
    where
        S: Into<String>,
    {
        Self {
            storage: None,
            control: None,
            bucket: bucket.into(),
            credentials,
            endpoint: None,
            control_endpoint: None,
            universe_domain: None,
            backoff_policy: None,
            retry_policy: None,
            retry_throttler: None,
        }
    }

    /// Sets the endpoint used by both Google Cloud SDK clients.
    #[must_use]
    pub fn endpoint<S>(mut self, endpoint: S) -> Self
    where
        S: Into<String>,
    {
        self.endpoint = Some(endpoint.into());
        self
    }

    /// Sets an endpoint override for the Google Cloud SDK control client.
    ///
    /// This is useful for emulators that expose object data over HTTP and
    /// metadata operations over a separate gRPC endpoint. When unset, the
    /// endpoint configured by [`Self::endpoint`] is used for both clients.
    #[must_use]
    pub fn control_endpoint<S>(mut self, endpoint: S) -> Self
    where
        S: Into<String>,
    {
        self.control_endpoint = Some(endpoint.into());
        self
    }

    /// Sets the universe domain used by both Google Cloud SDK clients.
    #[must_use]
    pub fn universe_domain<S>(mut self, universe_domain: S) -> Self
    where
        S: Into<String>,
    {
        self.universe_domain = Some(universe_domain.into());
        self
    }

    /// Sets the retry backoff policy used by both SDK clients.
    #[must_use]
    pub fn backoff_policy<P>(mut self, policy: P) -> Self
    where
        P: Into<BackoffPolicyArg>,
    {
        self.backoff_policy = Some(policy.into());
        self
    }

    /// Sets the retry policy used by both SDK clients.
    #[must_use]
    pub fn retry_policy<P>(mut self, policy: P) -> Self
    where
        P: Into<RetryPolicyArg>,
    {
        self.retry_policy = Some(policy.into());
        self
    }

    /// Sets the retry throttler used by both SDK clients.
    #[must_use]
    pub fn retry_throttler<T>(mut self, throttler: T) -> Self
    where
        T: Into<RetryThrottlerArg>,
    {
        self.retry_throttler = Some(throttler.into());
        self
    }

    /// Returns the configured bucket name.
    #[must_use]
    pub fn bucket(&self) -> &str {
        &self.bucket
    }

    /// Returns the connected object-data client, if connected.
    #[must_use]
    pub fn storage_client(&self) -> Option<&Storage> {
        self.storage.as_ref()
    }

    /// Returns the connected metadata client, if connected.
    #[must_use]
    pub fn control_client(&self) -> Option<&StorageControl> {
        self.control.as_ref()
    }

    fn credentials_for_builder(&self) -> Option<google_cloud_auth::credentials::Credentials> {
        match &self.credentials {
            GoogleCloudStorageCredentials::ApplicationDefault => None,
            GoogleCloudStorageCredentials::Anonymous => {
                Some(google_cloud_auth::credentials::anonymous::Builder::new().build())
            }
            GoogleCloudStorageCredentials::Custom(credentials) => Some(credentials.clone()),
        }
    }

    pub(crate) fn require_storage(&self) -> RemoteResult<&Storage> {
        self.storage
            .as_ref()
            .ok_or_else(|| RemoteError::new(RemoteErrorType::NotConnected))
    }

    pub(crate) fn require_control(&self) -> RemoteResult<&StorageControl> {
        self.control
            .as_ref()
            .ok_or_else(|| RemoteError::new(RemoteErrorType::NotConnected))
    }

    pub(crate) fn bucket_resource(&self) -> String {
        bucket_resource(&self.bucket)
    }

    async fn get_object(&self, name: &str) -> RemoteResult<File> {
        let control = self.require_control()?;
        control
            .get_object()
            .set_bucket(self.bucket_resource())
            .set_object(name)
            .send()
            .await
            .map_err(|error| map_gcs_error(error, RemoteErrorType::StatFailed))
            .and_then(|object| GcsObject::try_from(object).map(GcsObject::into_file))
    }

    /// Resolves an absolute path to a file, a marker directory, or an implicit
    /// prefix directory, in that order.
    pub(crate) async fn stat_path(&self, path: &Path) -> RemoteResult<File> {
        let name = key::object_name(path)?;
        if name.is_empty() {
            return Ok(GcsObject::directory("")?.into_file());
        }
        let missing = match self.get_object(&name).await {
            Ok(file) => return Ok(file),
            Err(error) if error.kind() == RemoteErrorType::NoSuchFileOrDirectory => error,
            Err(error) => return Err(error),
        };
        match self.get_object(&format!("{name}{OBJECT_DELIMITER}")).await {
            Ok(file) => return Ok(file),
            Err(error) if error.kind() == RemoteErrorType::NoSuchFileOrDirectory => {}
            Err(error) => return Err(error),
        }
        let descendants = self
            .query_objects(&format!("{name}{OBJECT_DELIMITER}"), Some(OBJECT_DELIMITER))
            .await?;
        if descendants.is_empty() {
            Err(missing)
        } else {
            Ok(GcsObject::directory(&name)?.into_file())
        }
    }

    async fn query_objects(
        &self,
        prefix: &str,
        delimiter: Option<&str>,
    ) -> RemoteResult<Vec<File>> {
        let control = self.require_control()?;
        let mut builder = control
            .list_objects()
            .set_parent(self.bucket_resource())
            .set_prefix(prefix.to_string());
        if let Some(delimiter) = delimiter {
            builder = builder
                .set_delimiter(delimiter)
                .set_include_trailing_delimiter(true);
        }
        let queried_path = key::to_path(prefix);
        let mut pages = builder.by_page();
        let mut entries = BTreeMap::new();
        while let Some(page) = pages.next().await {
            let page = page.map_err(|error| map_gcs_error(error, RemoteErrorType::StatFailed))?;
            for object in page.objects {
                let entry = GcsObject::try_from(object)?.into_file();
                if entry.path() != queried_path {
                    entries.insert(entry.path().to_path_buf(), entry);
                }
            }
            for prefix in page.prefixes {
                let entry = GcsObject::directory(&prefix)?.into_file();
                if entry.path() != queried_path {
                    entries.insert(entry.path().to_path_buf(), entry);
                }
            }
        }
        Ok(entries.into_values().collect())
    }

    async fn query_object_names(&self, prefix: &str) -> RemoteResult<Vec<String>> {
        let control = self.require_control()?;
        let builder = control
            .list_objects()
            .set_parent(self.bucket_resource())
            .set_prefix(prefix.to_string());
        let mut pages = builder.by_page();
        let mut names = Vec::new();
        while let Some(page) = pages.next().await {
            let page = page.map_err(|error| map_gcs_error(error, RemoteErrorType::StatFailed))?;
            names.extend(page.objects.into_iter().map(|object| object.name));
        }
        Ok(names)
    }

    async fn delete_object(&self, object: &str, fallback: RemoteErrorType) -> RemoteResult<()> {
        let control = self.require_control()?;
        control
            .delete_object()
            .set_bucket(self.bucket_resource())
            .set_object(object)
            .send()
            .await
            .map_err(|error| map_gcs_error(error, fallback))
    }

    async fn delete_entry(&self, file: &File, fallback: RemoteErrorType) -> RemoteResult<()> {
        let name = if file.is_dir() {
            key::marker_name(file.path())?
        } else {
            key::object_name(file.path())?
        };
        self.delete_object(&name, fallback).await
    }

    async fn ensure_file_destination(&self, path: &Path) -> RemoteResult<()> {
        self.ensure_parent_directories(path).await?;
        match self.stat_path(path).await {
            Ok(file) if file.is_dir() => {
                return Err(RemoteError::new(RemoteErrorType::BadFile));
            }
            Ok(_) => {}
            Err(error) if error.kind() == RemoteErrorType::NoSuchFileOrDirectory => {}
            Err(error) => return Err(error),
        }
        let prefix = key::directory_prefix(path)?;
        if !prefix.is_empty() && !self.query_object_names(&prefix).await?.is_empty() {
            return Err(RemoteError::new(RemoteErrorType::BadFile));
        }
        Ok(())
    }

    async fn ensure_parent_directories(&self, path: &Path) -> RemoteResult<()> {
        let name = key::object_name(path)?;
        let Some((parents, _)) = name.rsplit_once('/') else {
            return Ok(());
        };
        let mut ancestor = String::new();
        for component in parents.split('/') {
            if !ancestor.is_empty() {
                ancestor.push('/');
            }
            ancestor.push_str(component);
            let ancestor_path = PathBuf::from(format!("/{ancestor}"));
            match self.stat_path(&ancestor_path).await {
                Ok(file) if file.is_file() => {
                    return Err(RemoteError::new(RemoteErrorType::BadFile));
                }
                Ok(_) => {}
                Err(error) if error.kind() == RemoteErrorType::NoSuchFileOrDirectory => {}
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    fn unsupported() -> RemoteError {
        RemoteError::new(RemoteErrorType::UnsupportedFeature)
    }
}

/// A blocking view of [`GoogleCloudStorageFs`] that implements
/// [`remotefs::RemoteFs`].
///
/// Every call blocks on the supplied Tokio handle and must not be made from
/// inside an async context. Build one with
/// [`GoogleCloudStorageFs::into_blocking`].
#[cfg(feature = "tokio")]
pub type BlockingGoogleCloudStorageFs = remotefs::adapters::blocking::BlockOn<GoogleCloudStorageFs>;

#[cfg(feature = "tokio")]
impl GoogleCloudStorageFs {
    /// Wraps the client for blocking callers using the given runtime handle.
    ///
    /// The returned value implements [`remotefs::RemoteFs`] and can be stored
    /// as `Box<dyn RemoteFs>`. Calling it from an async context panics, as
    /// documented by `tokio::runtime::Handle::block_on`.
    ///
    /// # Panics
    ///
    /// Panics if `handle` belongs to a current-thread runtime. Such a runtime
    /// cannot drive the blocked operation from a non-async caller.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use remotefs::RemoteFs;
    /// use remotefs_gcs::GoogleCloudStorageFs;
    ///
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let runtime = tokio::runtime::Runtime::new()?;
    /// let mut client: Box<dyn RemoteFs> =
    ///     Box::new(GoogleCloudStorageFs::new("my-bucket").into_blocking(runtime.handle().clone()));
    /// client.connect()?;
    /// client.disconnect()?;
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn into_blocking(self, handle: tokio::runtime::Handle) -> BlockingGoogleCloudStorageFs {
        assert_ne!(
            handle.runtime_flavor(),
            tokio::runtime::RuntimeFlavor::CurrentThread,
            "into_blocking requires a multi-thread Tokio runtime"
        );
        remotefs::adapters::blocking::BlockOn::new(self, handle)
    }
}

#[remotefs::async_trait]
impl AsyncRemoteFs for GoogleCloudStorageFs {
    async fn connect(&mut self) -> RemoteResult<()> {
        if self.storage.is_some() || self.control.is_some() {
            return Err(RemoteError::new(RemoteErrorType::AlreadyConnected));
        }

        let mut storage_builder = Storage::builder();
        let mut control_builder = StorageControl::builder();
        if let Some(endpoint) = &self.endpoint {
            storage_builder = storage_builder.with_endpoint(endpoint.clone());
            control_builder = control_builder.with_endpoint(endpoint.clone());
        }
        if let Some(endpoint) = &self.control_endpoint {
            control_builder = control_builder.with_endpoint(endpoint.clone());
        }
        if let Some(universe_domain) = &self.universe_domain {
            storage_builder = storage_builder.with_universe_domain(universe_domain.clone());
            control_builder = control_builder.with_universe_domain(universe_domain.clone());
        }
        if let Some(credentials) = self.credentials_for_builder() {
            storage_builder = storage_builder.with_credentials(credentials.clone());
            control_builder = control_builder.with_credentials(credentials);
        }
        if let Some(policy) = &self.retry_policy {
            storage_builder = storage_builder.with_retry_policy(policy.clone());
            control_builder = control_builder.with_retry_policy(policy.clone());
        }
        if let Some(policy) = &self.backoff_policy {
            storage_builder = storage_builder.with_backoff_policy(policy.clone());
            control_builder = control_builder.with_backoff_policy(policy.clone());
        }
        if let Some(throttler) = &self.retry_throttler {
            storage_builder = storage_builder.with_retry_throttler(throttler.clone());
            control_builder = control_builder.with_retry_throttler(throttler.clone());
        }
        let storage = storage_builder
            .build()
            .await
            .map_err(|error| RemoteError::with_source(RemoteErrorType::ConnectionError, error))?;
        let control = control_builder
            .build()
            .await
            .map_err(|error| RemoteError::with_source(RemoteErrorType::ConnectionError, error))?;

        self.storage = Some(storage);
        self.control = Some(control);
        debug!(
            "connected to Google Cloud Storage bucket {bucket}",
            bucket = self.bucket
        );
        Ok(())
    }

    async fn disconnect(&mut self) -> RemoteResult<()> {
        if self.storage.is_none() && self.control.is_none() {
            return Err(RemoteError::new(RemoteErrorType::NotConnected));
        }
        self.storage = None;
        self.control = None;
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.storage.is_some() && self.control.is_some()
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities::STREAM_READ
            | Capabilities::STREAM_WRITE
            | Capabilities::RANGE_READ
            | Capabilities::COPY
    }

    async fn list_dir(&self, path: &Path) -> RemoteResult<Vec<File>> {
        let prefix = key::directory_prefix(path)?;
        if !self.stat_path(path).await?.is_dir() {
            return Err(RemoteError::new(RemoteErrorType::BadFile));
        }
        self.query_objects(&prefix, Some(OBJECT_DELIMITER)).await
    }

    async fn stat(&self, path: &Path) -> RemoteResult<File> {
        self.stat_path(path).await
    }

    async fn exists(&self, path: &Path) -> RemoteResult<bool> {
        match self.stat_path(path).await {
            Ok(_) => Ok(true),
            Err(error) if error.kind() == RemoteErrorType::NoSuchFileOrDirectory => Ok(false),
            Err(error) => Err(error),
        }
    }

    async fn set_metadata(&self, path: &Path, _metadata: &SetMetadata) -> RemoteResult<()> {
        key::object_name(path)?;
        Err(Self::unsupported())
    }

    /// Creates a zero-byte trailing-slash marker object.
    ///
    /// The Unix permission mode is ignored because Google Cloud Storage
    /// authorizes requests through IAM rather than per-object Unix bits.
    async fn create_dir(&self, path: &Path, _mode: Option<UnixPex>) -> RemoteResult<()> {
        let marker = key::marker_name(path)?;
        match self.stat_path(path).await {
            Ok(_) => return Err(RemoteError::new(RemoteErrorType::AlreadyExists)),
            Err(error) if error.kind() == RemoteErrorType::NoSuchFileOrDirectory => {}
            Err(error) => return Err(error),
        }
        self.ensure_parent_directories(path).await?;
        let storage = self.require_storage()?;
        Box::pin(
            storage
                .write_object(self.bucket_resource(), marker, bytes::Bytes::new())
                .set_content_type(CONTENT_TYPE)
                .set_if_generation_match(0_i64)
                .send_buffered(),
        )
        .await
        .map(|_| ())
        .map_err(|error| map_gcs_create_error(error, RemoteErrorType::FileCreateDenied))
    }

    async fn remove_file(&self, path: &Path) -> RemoteResult<()> {
        let entry = self.stat_path(path).await?;
        if !entry.is_file() {
            return Err(RemoteError::new(RemoteErrorType::BadFile));
        }
        self.delete_entry(&entry, RemoteErrorType::CouldNotRemoveFile)
            .await
    }

    async fn remove_dir(&self, path: &Path) -> RemoteResult<()> {
        if key::is_root(path)? {
            return Err(RemoteError::new(RemoteErrorType::InvalidPath));
        }
        let marker = key::marker_name(path)?;
        let entry = self.stat_path(path).await?;
        if !entry.is_dir() {
            return Err(RemoteError::new(RemoteErrorType::BadFile));
        }
        let prefix = key::directory_prefix(path)?;
        if !self.query_objects(&prefix, None).await?.is_empty() {
            return Err(RemoteError::new(RemoteErrorType::DirectoryNotEmpty));
        }
        self.delete_object(&marker, RemoteErrorType::CouldNotRemoveFile)
            .await
    }

    /// Removes every object below the path with one flat listing, then the
    /// marker object if one exists.
    async fn remove_dir_all(&self, path: &Path) -> RemoteResult<()> {
        if key::is_root(path)? {
            return Err(RemoteError::new(RemoteErrorType::InvalidPath));
        }
        let entry = self.stat_path(path).await?;
        if entry.is_file() {
            let name = key::object_name(path)?;
            self.delete_object(&name, RemoteErrorType::CouldNotRemoveFile)
                .await?;
            return Ok(());
        }
        let prefix = key::directory_prefix(path)?;
        for name in self.query_object_names(&prefix).await? {
            self.delete_object(&name, RemoteErrorType::CouldNotRemoveFile)
                .await?;
        }
        Ok(())
    }

    /// Renames an object by copying it and then deleting the source.
    ///
    /// The operation is not atomic.
    async fn rename(&self, src: &Path, dest: &Path) -> RemoteResult<()> {
        let source_name = key::object_name(src)?;
        let destination_name = key::object_name(dest)?;
        if source_name == destination_name {
            let source = self.stat_path(src).await?;
            return if source.is_file() {
                Ok(())
            } else {
                Err(Self::unsupported())
            };
        }
        self.copy(src, dest).await?;
        self.remove_file(src).await
    }

    /// Copies an object with rewrite requests until the rewrite is done.
    async fn copy(&self, src: &Path, dest: &Path) -> RemoteResult<()> {
        let destination_name = key::object_name(dest)?;
        let source = self.stat_path(src).await?;
        if !source.is_file() {
            return Err(Self::unsupported());
        }
        self.ensure_file_destination(dest).await?;
        let source_name = key::object_name(source.path())?;
        let control = self.require_control()?;
        let mut rewrite_token = String::new();
        loop {
            let mut request = control
                .rewrite_object()
                .set_source_bucket(self.bucket_resource())
                .set_source_object(source_name.clone())
                .set_destination_bucket(self.bucket_resource())
                .set_destination_name(destination_name.clone());
            if !rewrite_token.is_empty() {
                request = request.set_rewrite_token(rewrite_token.clone());
            }
            let response = request
                .send()
                .await
                .map_err(|error| map_gcs_error(error, RemoteErrorType::ProtocolError))?;
            if response.done {
                return Ok(());
            }
            if response.rewrite_token.is_empty() {
                return Err(RemoteError::with_message(
                    RemoteErrorType::ProtocolError,
                    "rewrite response was not done and had no continuation token",
                ));
            }
            rewrite_token = response.rewrite_token;
        }
    }

    async fn symlink(&self, path: &Path, target: &Path) -> RemoteResult<()> {
        key::object_name(path)?;
        key::object_name(target)?;
        Err(Self::unsupported())
    }

    /// Opens an object for a ranged read.
    ///
    /// `length == Some(0)` verifies that the object exists and returns an
    /// empty reader. A range that starts beyond the end of the object returns
    /// an empty reader as well.
    async fn open(&self, path: &Path, opts: &ReadOptions) -> RemoteResult<AsyncReadStream> {
        let name = key::object_name(path)?;
        let storage = self.require_storage()?;
        if opts.length == Some(0) {
            let entry = self.stat_path(path).await?;
            if !entry.is_file() {
                return Err(RemoteError::new(RemoteErrorType::BadFile));
            }
            return Ok(AsyncReadStream::new(GcsReader::empty()));
        }
        let range = match (opts.offset, opts.length) {
            (None | Some(0), None) => ReadRange::all(),
            (Some(offset), None) => ReadRange::offset(offset),
            (offset, Some(length)) => ReadRange::segment(offset.unwrap_or(0), length),
        };
        debug!("open '{name}' with range {range:?}");
        match storage
            .read_object(self.bucket_resource(), name)
            .set_read_range(range)
            .send()
            .await
        {
            Ok(response) => Ok(AsyncReadStream::new(GcsReader::new(response, opts.length))),
            Err(error) if is_range_not_satisfiable(&error) => {
                debug!("range {opts:?} is beyond the end of the object; returning an empty reader");
                Ok(AsyncReadStream::new(GcsReader::empty()))
            }
            Err(error) => Err(map_gcs_error(error, RemoteErrorType::CouldNotOpenFile)),
        }
    }

    /// Creates or replaces an object through a resumable upload.
    ///
    /// The object is committed only when the returned stream is finished. A
    /// size hint, when given, is sent to the service and verified after the
    /// upload.
    async fn create(&self, path: &Path, opts: &WriteOptions) -> RemoteResult<AsyncWriteStream> {
        let name = key::object_name(path)?;
        if name.is_empty() {
            return Err(RemoteError::new(RemoteErrorType::BadFile));
        }
        self.ensure_file_destination(path).await?;
        let storage = self.require_storage()?.clone();
        debug!(
            "create '{name}' (size hint: {hint:?})",
            hint = opts.size_hint
        );
        Ok(AsyncWriteStream::new(GcsWriter::new(
            storage,
            self.bucket_resource(),
            name,
            opts.size_hint,
        )?))
    }

    async fn write_file(
        &self,
        path: &Path,
        opts: &WriteOptions,
        src: &mut (dyn AsyncRead + Send + Unpin),
    ) -> RemoteResult<u64> {
        let mut stream = self.create(path, opts).await?;
        let mut buffer = [0_u8; 8192];
        let mut total = 0_u64;
        loop {
            let read = poll_fn(|context| Pin::new(&mut *src).poll_read(context, &mut buffer)).await;
            let count = match read {
                Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                result => result.map_err(RemoteError::from)?,
            };
            if count == 0 {
                break;
            }
            let mut written = 0;
            while written < count {
                let size = poll_fn(|context| {
                    Pin::new(&mut stream).poll_write(context, &buffer[written..count])
                })
                .await
                .map_err(RemoteError::from)?;
                if size == 0 {
                    return Err(RemoteError::with_message(
                        RemoteErrorType::IoError,
                        "remote writer returned zero bytes",
                    ));
                }
                written += size;
            }
            total = total.checked_add(count as u64).ok_or_else(|| {
                RemoteError::with_message(RemoteErrorType::IoError, "transfer byte count overflow")
            })?;
        }
        poll_fn(|context| Pin::new(&mut stream).poll_flush(context))
            .await
            .map_err(RemoteError::from)?;
        stream.finish().await?;
        Ok(total)
    }

    async fn append(&self, path: &Path, _opts: &WriteOptions) -> RemoteResult<AsyncWriteStream> {
        key::object_name(path)?;
        Err(Self::unsupported())
    }

    async fn exec(&self, _cmd: &str) -> RemoteResult<ExecOutput> {
        Err(Self::unsupported())
    }
}

pub(crate) fn bucket_resource(bucket: &str) -> String {
    format!("{BUCKET_RESOURCE_PREFIX}{bucket}")
}

#[cfg(test)]
mod tests {
    use google_cloud_auth::credentials::anonymous::Builder as AnonymousBuilder;
    use google_cloud_gax::exponential_backoff::ExponentialBackoff;
    use google_cloud_gax::retry_policy::{AlwaysRetry, RetryPolicyExt};
    use google_cloud_gax::retry_throttler::AdaptiveThrottler;
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn new_uses_application_default_credentials() {
        let client = GoogleCloudStorageFs::new("bucket");
        assert_eq!(client.bucket(), "bucket");
        assert!(matches!(
            client.credentials,
            GoogleCloudStorageCredentials::ApplicationDefault
        ));
        assert!(!client.is_connected());
        assert_eq!(client.bucket_resource(), "projects/_/buckets/bucket");
    }

    #[test]
    fn construction_accepts_credentials_and_configuration() {
        let client = GoogleCloudStorageFs::with_credentials(
            "bucket",
            GoogleCloudStorageCredentials::custom(AnonymousBuilder::new().build()),
        )
        .endpoint("http://localhost:4443")
        .control_endpoint("http://localhost:8888")
        .universe_domain("example.test")
        .backoff_policy(ExponentialBackoff::default())
        .retry_policy(AlwaysRetry.with_attempt_limit(2))
        .retry_throttler(AdaptiveThrottler::default());
        assert_eq!(client.endpoint.as_deref(), Some("http://localhost:4443"));
        assert_eq!(
            client.control_endpoint.as_deref(),
            Some("http://localhost:8888")
        );
        assert_eq!(client.universe_domain.as_deref(), Some("example.test"));
        assert!(client.backoff_policy.is_some());
        assert!(client.retry_policy.is_some());
        assert!(client.retry_throttler.is_some());
    }

    #[test]
    fn capabilities_advertise_native_operations() {
        let capabilities = GoogleCloudStorageFs::new("bucket").capabilities();
        assert!(capabilities.contains(Capabilities::STREAM_READ));
        assert!(capabilities.contains(Capabilities::STREAM_WRITE));
        assert!(capabilities.contains(Capabilities::RANGE_READ));
        assert!(capabilities.contains(Capabilities::COPY));
        assert!(!capabilities.contains(Capabilities::APPEND));
        assert!(!capabilities.contains(Capabilities::SEEK_READ));
        assert!(!capabilities.contains(Capabilities::SYMLINK));
        assert!(!capabilities.contains(Capabilities::SET_METADATA));
        assert!(!capabilities.contains(Capabilities::POSIX_MODE));
        assert!(!capabilities.contains(Capabilities::EXEC));
    }

    #[tokio::test]
    async fn disconnecting_an_unconnected_client_fails() {
        let mut client = GoogleCloudStorageFs::new("bucket");
        assert_eq!(
            client.disconnect().await.unwrap_err().kind(),
            RemoteErrorType::NotConnected
        );
    }

    #[tokio::test]
    async fn relative_paths_are_rejected_before_the_connection_check() {
        let client = GoogleCloudStorageFs::new("bucket");
        let relative = Path::new("file");
        assert_eq!(
            client.stat(relative).await.unwrap_err().kind(),
            RemoteErrorType::InvalidPath
        );
        assert_eq!(
            client.list_dir(relative).await.unwrap_err().kind(),
            RemoteErrorType::InvalidPath
        );
        assert_eq!(
            client.create_dir(relative, None).await.unwrap_err().kind(),
            RemoteErrorType::InvalidPath
        );
        assert_eq!(
            client
                .open(relative, &ReadOptions::default())
                .await
                .unwrap_err()
                .kind(),
            RemoteErrorType::InvalidPath
        );
        assert_eq!(
            client
                .create(relative, &WriteOptions::default())
                .await
                .unwrap_err()
                .kind(),
            RemoteErrorType::InvalidPath
        );
    }

    #[tokio::test]
    async fn disconnected_operations_fail_as_not_connected() {
        let client = GoogleCloudStorageFs::new("bucket");
        let absolute = Path::new("/file");
        assert_eq!(
            client.stat(absolute).await.unwrap_err().kind(),
            RemoteErrorType::NotConnected
        );
        assert_eq!(
            client.exists(absolute).await.unwrap_err().kind(),
            RemoteErrorType::NotConnected
        );
        assert_eq!(
            client.list_dir(Path::new("/")).await.unwrap_err().kind(),
            RemoteErrorType::NotConnected
        );
        assert_eq!(
            client.remove_file(absolute).await.unwrap_err().kind(),
            RemoteErrorType::NotConnected
        );
    }

    #[tokio::test]
    async fn unsupported_operations_are_explicit() {
        let client = GoogleCloudStorageFs::new("bucket");
        let path = Path::new("/file");
        assert_eq!(
            client
                .set_metadata(path, &SetMetadata::default())
                .await
                .unwrap_err()
                .kind(),
            RemoteErrorType::UnsupportedFeature
        );
        assert_eq!(
            client
                .symlink(Path::new("/link"), path)
                .await
                .unwrap_err()
                .kind(),
            RemoteErrorType::UnsupportedFeature
        );
        assert_eq!(
            client.exec("echo test").await.unwrap_err().kind(),
            RemoteErrorType::UnsupportedFeature
        );
        assert_eq!(
            client
                .append(path, &WriteOptions::default())
                .await
                .unwrap_err()
                .kind(),
            RemoteErrorType::UnsupportedFeature
        );
    }

    #[test]
    fn client_is_send_sync_and_object_safe() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<GoogleCloudStorageFs>();
        let _: Box<dyn AsyncRemoteFs> = Box::new(GoogleCloudStorageFs::new("bucket"));
    }

    #[cfg(feature = "tokio")]
    #[test]
    fn blocking_wrapper_is_a_remote_fs_trait_object() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let client: Box<dyn remotefs::RemoteFs> =
            Box::new(GoogleCloudStorageFs::new("bucket").into_blocking(runtime.handle().clone()));
        assert!(!client.is_connected());
        assert!(client.capabilities().contains(Capabilities::RANGE_READ));
    }

    #[cfg(feature = "tokio")]
    #[test]
    #[should_panic(expected = "into_blocking requires a multi-thread Tokio runtime")]
    fn blocking_wrapper_rejects_current_thread_runtime() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _ = GoogleCloudStorageFs::new("bucket").into_blocking(runtime.handle().clone());
    }
}
