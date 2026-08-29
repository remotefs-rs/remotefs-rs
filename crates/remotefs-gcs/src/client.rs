//! Google Cloud Storage implementation of [`remotefs::RemoteFs`].

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use google_cloud_gax::backoff_policy::BackoffPolicyArg;
use google_cloud_gax::paginator::Paginator;
use google_cloud_gax::retry_policy::RetryPolicyArg;
use google_cloud_gax::retry_throttler::RetryThrottlerArg;
use google_cloud_storage::client::{Storage, StorageControl};
use remotefs::fs::{File, Metadata, ReadStream, UnixPex, Welcome, WriteStream};
use remotefs::{RemoteError, RemoteErrorType, RemoteFs, RemoteResult};
use tokio::runtime::Runtime;

use crate::credentials::GoogleCloudStorageCredentials;
use crate::error::map_gcs_error;
use crate::object::GcsObject;
use crate::reader::BlockingReaderSource;
use crate::utils::path as path_utils;

const BUCKET_RESOURCE_PREFIX: &str = "projects/_/buckets/";
const OBJECT_DELIMITER: &str = "/";
const ROOT_PATH: &str = "/";

/// A blocking Google Cloud Storage filesystem client.
///
/// The client keeps the storage and storage-control clients separate because
/// the Google SDK uses the former for object bytes and the latter for object
/// metadata operations. Both clients are created together by
/// [`RemoteFs::connect`].
#[derive(Debug)]
pub struct GoogleCloudStorageFs {
    storage: Option<Storage>,
    control: Option<StorageControl>,
    runtime: Arc<Runtime>,
    wrkdir: PathBuf,
    bucket: String,
    credentials: GoogleCloudStorageCredentials,
    endpoint: Option<String>,
    universe_domain: Option<String>,
    backoff_policy: Option<BackoffPolicyArg>,
    retry_policy: Option<RetryPolicyArg>,
    retry_throttler: Option<RetryThrottlerArg>,
}

impl GoogleCloudStorageFs {
    /// Creates a client using Application Default Credentials.
    #[must_use]
    pub fn new<S>(bucket: S, runtime: &Arc<Runtime>) -> Self
    where
        S: Into<String>,
    {
        Self::with_credentials(bucket, GoogleCloudStorageCredentials::default(), runtime)
    }

    /// Creates a client with an explicit credential mode.
    #[must_use]
    pub fn with_credentials<S>(
        bucket: S,
        credentials: GoogleCloudStorageCredentials,
        runtime: &Arc<Runtime>,
    ) -> Self
    where
        S: Into<String>,
    {
        Self {
            storage: None,
            control: None,
            runtime: Arc::clone(runtime),
            wrkdir: PathBuf::from(ROOT_PATH),
            bucket: bucket.into(),
            credentials,
            endpoint: None,
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

    fn require_storage(&self) -> RemoteResult<&Storage> {
        self.storage
            .as_ref()
            .ok_or_else(|| RemoteError::new(RemoteErrorType::NotConnected))
    }

    fn require_control(&self) -> RemoteResult<&StorageControl> {
        self.control
            .as_ref()
            .ok_or_else(|| RemoteError::new(RemoteErrorType::NotConnected))
    }

    fn resolve(&self, path: &Path) -> RemoteResult<PathBuf> {
        let absolute = path_utils::absolutize(&self.wrkdir, path);
        let normalized = path_utils::normalize(&absolute).ok_or_else(|| {
            RemoteError::new_ex(
                RemoteErrorType::BadFile,
                format!("path escapes root: {}", path.display()),
            )
        })?;
        path_utils::diff_paths(normalized, Path::new(ROOT_PATH)).ok_or_else(|| {
            RemoteError::new_ex(
                RemoteErrorType::BadFile,
                format!("path is not rooted: {}", path.display()),
            )
        })
    }

    fn absolute_path(&self, path: &Path) -> RemoteResult<PathBuf> {
        let relative = self.resolve(path)?;
        Ok(Path::new(ROOT_PATH).join(relative))
    }

    fn stat_path(&self, path: &Path) -> RemoteResult<File> {
        if path == Path::new(ROOT_PATH) {
            return Ok(GcsObject::directory("").into_file());
        }

        let name = object_name(path)?;
        let control = self.require_control()?;
        match self.runtime.block_on(
            control
                .get_object()
                .set_bucket(bucket_resource(&self.bucket))
                .set_object(name.clone())
                .send(),
        ) {
            Ok(object) => GcsObject::try_from(object).map(GcsObject::into_file),
            Err(error) => {
                let mapped = map_gcs_error(error, RemoteErrorType::StatFailed);
                if mapped.kind != RemoteErrorType::NoSuchFileOrDirectory {
                    return Err(mapped);
                }

                let marker_name = format!("{name}{OBJECT_DELIMITER}");
                match self.runtime.block_on(
                    control
                        .get_object()
                        .set_bucket(bucket_resource(&self.bucket))
                        .set_object(marker_name)
                        .send(),
                ) {
                    Ok(object) => GcsObject::try_from(object).map(GcsObject::into_file),
                    Err(marker_error) => {
                        let marker_mapped =
                            map_gcs_error(marker_error, RemoteErrorType::StatFailed);
                        if marker_mapped.kind != RemoteErrorType::NoSuchFileOrDirectory {
                            return Err(marker_mapped);
                        }

                        let descendants = self.query_objects(
                            &format!("{name}{OBJECT_DELIMITER}"),
                            Some(OBJECT_DELIMITER),
                        )?;
                        if descendants.is_empty() {
                            Err(mapped)
                        } else {
                            Ok(GcsObject::directory(&name).into_file())
                        }
                    }
                }
            }
        }
    }

    fn query_objects(&self, prefix: &str, delimiter: Option<&str>) -> RemoteResult<Vec<File>> {
        let control = self.require_control()?;
        let mut builder = control
            .list_objects()
            .set_parent(bucket_resource(&self.bucket))
            .set_prefix(prefix.to_string());
        if let Some(delimiter) = delimiter {
            builder = builder
                .set_delimiter(delimiter)
                .set_include_trailing_delimiter(true);
        }

        let queried_path = Path::new(ROOT_PATH).join(prefix.trim_matches('/'));
        let mut pages = builder.by_page();
        let mut entries = BTreeMap::new();
        while let Some(page) = self.runtime.block_on(pages.next()) {
            let page = page.map_err(|error| map_gcs_error(error, RemoteErrorType::StatFailed))?;
            for object in page.objects {
                let entry = GcsObject::try_from(object)?.into_file();
                if entry.path() != queried_path {
                    entries.insert(entry.path().to_path_buf(), entry);
                }
            }
            for prefix in page.prefixes {
                let entry = GcsObject::directory(&prefix).into_file();
                if entry.path() != queried_path {
                    entries.insert(entry.path().to_path_buf(), entry);
                }
            }
        }
        Ok(entries.into_values().collect())
    }

    fn delete_object(&self, object: &str, fallback: RemoteErrorType) -> RemoteResult<()> {
        let control = self.require_control()?;
        self.runtime
            .block_on(
                control
                    .delete_object()
                    .set_bucket(bucket_resource(&self.bucket))
                    .set_object(object)
                    .send(),
            )
            .map_err(|error| map_gcs_error(error, fallback))
    }

    fn delete_file_object(&self, file: &File, fallback: RemoteErrorType) -> RemoteResult<()> {
        let mut name = object_name(file.path())?;
        if file.is_dir() {
            name.push_str(OBJECT_DELIMITER);
        }
        self.delete_object(&name, fallback)
    }

    fn unsupported() -> RemoteError {
        RemoteError::new(RemoteErrorType::UnsupportedFeature)
    }
}

impl RemoteFs for GoogleCloudStorageFs {
    fn connect(&mut self) -> RemoteResult<Welcome> {
        if self.storage.is_some() || self.control.is_some() {
            return Err(RemoteError::new(RemoteErrorType::AlreadyConnected));
        }

        let mut storage_builder = Storage::builder();
        let mut control_builder = StorageControl::builder();
        if let Some(endpoint) = &self.endpoint {
            storage_builder = storage_builder.with_endpoint(endpoint.clone());
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
        let storage = self
            .runtime
            .block_on(storage_builder.build())
            .map_err(|error| RemoteError::new_ex(RemoteErrorType::ConnectionError, error))?;
        let control = self
            .runtime
            .block_on(control_builder.build())
            .map_err(|error| RemoteError::new_ex(RemoteErrorType::ConnectionError, error))?;

        self.storage = Some(storage);
        self.control = Some(control);
        debug!("connected to Google Cloud Storage bucket {}", self.bucket);
        Ok(Welcome::default())
    }

    fn disconnect(&mut self) -> RemoteResult<()> {
        if self.storage.is_none() && self.control.is_none() {
            return Err(RemoteError::new(RemoteErrorType::NotConnected));
        }
        self.storage = None;
        self.control = None;
        Ok(())
    }

    fn is_connected(&mut self) -> bool {
        self.storage.is_some() && self.control.is_some()
    }

    fn pwd(&mut self) -> RemoteResult<PathBuf> {
        if !self.is_connected() {
            return Err(RemoteError::new(RemoteErrorType::NotConnected));
        }
        Ok(self.wrkdir.clone())
    }

    fn change_dir(&mut self, dir: &Path) -> RemoteResult<PathBuf> {
        if !self.is_connected() {
            return Err(RemoteError::new(RemoteErrorType::NotConnected));
        }
        let absolute = self.absolute_path(dir)?;
        let entry = self.stat_path(&absolute)?;
        if !entry.is_dir() {
            return Err(RemoteError::new(RemoteErrorType::BadFile));
        }
        self.wrkdir.clone_from(&absolute);
        Ok(absolute)
    }

    fn list_dir(&mut self, path: &Path) -> RemoteResult<Vec<File>> {
        if !self.is_connected() {
            return Err(RemoteError::new(RemoteErrorType::NotConnected));
        }
        let absolute = self.absolute_path(path)?;
        if !self.stat_path(&absolute)?.is_dir() {
            return Err(RemoteError::new(RemoteErrorType::BadFile));
        }
        let prefix = directory_prefix(&absolute)?;
        self.query_objects(&prefix, Some(OBJECT_DELIMITER))
    }

    fn stat(&mut self, path: &Path) -> RemoteResult<File> {
        if !self.is_connected() {
            return Err(RemoteError::new(RemoteErrorType::NotConnected));
        }
        let absolute = self.absolute_path(path)?;
        self.stat_path(&absolute)
    }

    fn setstat(&mut self, _path: &Path, _metadata: Metadata) -> RemoteResult<()> {
        Err(Self::unsupported())
    }

    fn exists(&mut self, path: &Path) -> RemoteResult<bool> {
        if !self.is_connected() {
            return Err(RemoteError::new(RemoteErrorType::NotConnected));
        }
        match self.stat(path) {
            Ok(_) => Ok(true),
            Err(error) if error.kind == RemoteErrorType::NoSuchFileOrDirectory => Ok(false),
            Err(error) => Err(error),
        }
    }

    fn remove_file(&mut self, path: &Path) -> RemoteResult<()> {
        if !self.is_connected() {
            return Err(RemoteError::new(RemoteErrorType::NotConnected));
        }
        let absolute = self.absolute_path(path)?;
        let entry = self.stat_path(&absolute)?;
        if !entry.is_file() {
            return Err(RemoteError::new(RemoteErrorType::BadFile));
        }
        self.delete_file_object(&entry, RemoteErrorType::CouldNotRemoveFile)
    }

    fn remove_dir(&mut self, path: &Path) -> RemoteResult<()> {
        if !self.is_connected() {
            return Err(RemoteError::new(RemoteErrorType::NotConnected));
        }
        let absolute = self.absolute_path(path)?;
        if absolute == Path::new(ROOT_PATH) {
            return Err(RemoteError::new(RemoteErrorType::BadFile));
        }
        let entry = self.stat_path(&absolute)?;
        if !entry.is_dir() {
            return Err(RemoteError::new(RemoteErrorType::BadFile));
        }
        let prefix = directory_prefix(&absolute)?;
        let entries = self.query_objects(&prefix, None)?;
        let marker_path = entry.path().to_path_buf();
        let mut marker = None;
        for child in entries {
            if child.path() == marker_path && child.is_dir() {
                marker = Some(child);
            } else {
                return Err(RemoteError::new(RemoteErrorType::DirectoryNotEmpty));
            }
        }
        let marker =
            marker.ok_or_else(|| RemoteError::new(RemoteErrorType::NoSuchFileOrDirectory))?;
        self.delete_file_object(&marker, RemoteErrorType::CouldNotRemoveFile)
    }

    fn remove_dir_all(&mut self, path: &Path) -> RemoteResult<()> {
        if !self.is_connected() {
            return Err(RemoteError::new(RemoteErrorType::NotConnected));
        }
        let absolute = self.absolute_path(path)?;
        if absolute == Path::new(ROOT_PATH) {
            return Err(RemoteError::new(RemoteErrorType::BadFile));
        }
        let entry = self.stat_path(&absolute)?;
        if entry.is_file() {
            return self.delete_file_object(&entry, RemoteErrorType::CouldNotRemoveFile);
        }
        let prefix = directory_prefix(&absolute)?;
        let entries = self.query_objects(&prefix, None)?;
        if entries.is_empty() {
            return Err(RemoteError::new(RemoteErrorType::NoSuchFileOrDirectory));
        }
        for child in entries {
            self.delete_file_object(&child, RemoteErrorType::CouldNotRemoveFile)?;
        }
        Ok(())
    }

    /// Creates a zero-byte trailing-slash marker object.
    ///
    /// The Unix permission mode is ignored because Google Cloud Storage
    /// authorizes requests through IAM rather than per-object Unix bits.
    fn create_dir(&mut self, path: &Path, _mode: UnixPex) -> RemoteResult<()> {
        if !self.is_connected() {
            return Err(RemoteError::new(RemoteErrorType::NotConnected));
        }
        let absolute = self.absolute_path(path)?;
        match self.stat_path(&absolute) {
            Ok(_) => return Err(RemoteError::new(RemoteErrorType::DirectoryAlreadyExists)),
            Err(error) if error.kind == RemoteErrorType::NoSuchFileOrDirectory => {}
            Err(error) => return Err(error),
        }
        let name = object_name(&absolute)?;
        let marker = format!("{name}{OBJECT_DELIMITER}");
        let storage = self.require_storage()?;
        self.runtime
            .block_on(
                storage
                    .write_object(bucket_resource(&self.bucket), marker, bytes::Bytes::new())
                    .set_if_generation_match(0_i64)
                    .send_buffered(),
            )
            .map(|_| ())
            .map_err(|error| map_gcs_error(error, RemoteErrorType::FileCreateDenied))
    }

    fn symlink(&mut self, _path: &Path, _target: &Path) -> RemoteResult<()> {
        Err(Self::unsupported())
    }

    /// Copies an object with GCS rewrite requests.
    ///
    /// Rewrite is completed synchronously, including every continuation page.
    fn copy(&mut self, src: &Path, dest: &Path) -> RemoteResult<()> {
        if !self.is_connected() {
            return Err(RemoteError::new(RemoteErrorType::NotConnected));
        }
        let source = self.stat(src)?;
        if !source.is_file() {
            return Err(Self::unsupported());
        }
        let destination = self.absolute_path(dest)?;
        let source_name = object_name(source.path())?;
        let destination_name = object_name(&destination)?;
        let control = self.require_control()?;
        let mut rewrite_token = String::new();
        loop {
            let mut request = control
                .rewrite_object()
                .set_source_bucket(bucket_resource(&self.bucket))
                .set_source_object(source_name.clone())
                .set_destination_bucket(bucket_resource(&self.bucket))
                .set_destination_name(destination_name.clone());
            if !rewrite_token.is_empty() {
                request = request.set_rewrite_token(rewrite_token.clone());
            }
            let response = self
                .runtime
                .block_on(request.send())
                .map_err(|error| map_gcs_error(error, RemoteErrorType::ProtocolError))?;
            if response.done {
                return Ok(());
            }
            if response.rewrite_token.is_empty() {
                return Err(RemoteError::new_ex(
                    RemoteErrorType::ProtocolError,
                    "rewrite response was not done and had no continuation token",
                ));
            }
            rewrite_token = response.rewrite_token;
        }
    }

    /// Moves an object by copying it and then deleting the source.
    ///
    /// The operation is not atomic.
    fn mov(&mut self, src: &Path, dest: &Path) -> RemoteResult<()> {
        self.copy(src, dest)?;
        self.remove_file(src)
    }

    fn exec(&mut self, _cmd: &str) -> RemoteResult<(u32, String)> {
        Err(Self::unsupported())
    }

    fn append(&mut self, _path: &Path, _metadata: &Metadata) -> RemoteResult<WriteStream> {
        Err(Self::unsupported())
    }

    fn create(&mut self, _path: &Path, _metadata: &Metadata) -> RemoteResult<WriteStream> {
        Err(Self::unsupported())
    }

    fn open(&mut self, _path: &Path) -> RemoteResult<ReadStream> {
        Err(Self::unsupported())
    }

    fn create_file(
        &mut self,
        path: &Path,
        metadata: &Metadata,
        reader: Box<dyn Read + Send>,
    ) -> RemoteResult<u64> {
        if !self.is_connected() {
            return Err(RemoteError::new(RemoteErrorType::NotConnected));
        }
        let absolute = self.absolute_path(path)?;
        let object = object_name(&absolute)?;
        let storage = self.require_storage()?;
        let response = self
            .runtime
            .block_on(
                storage
                    .write_object(
                        bucket_resource(&self.bucket),
                        object,
                        BlockingReaderSource::new(reader),
                    )
                    .send_buffered(),
            )
            .map_err(|error| map_gcs_error(error, RemoteErrorType::FileCreateDenied))?;
        if response.size < 0 {
            return Err(RemoteError::new_ex(
                RemoteErrorType::ProtocolError,
                "storage returned a negative object size",
            ));
        }
        let size = response.size.cast_unsigned();
        if metadata.size != 0 && metadata.size != size {
            return Err(RemoteError::new_ex(
                RemoteErrorType::ProtocolError,
                format!(
                    "uploaded size {size} differs from metadata size {}",
                    metadata.size
                ),
            ));
        }
        Ok(size)
    }

    fn open_file(&mut self, src: &Path, mut dest: Box<dyn Write + Send>) -> RemoteResult<u64> {
        if !self.is_connected() {
            return Err(RemoteError::new(RemoteErrorType::NotConnected));
        }
        let absolute = self.absolute_path(src)?;
        let object = object_name(&absolute)?;
        let storage = self.require_storage()?;
        let mut response = self
            .runtime
            .block_on(
                storage
                    .read_object(bucket_resource(&self.bucket), object)
                    .send(),
            )
            .map_err(|error| map_gcs_error(error, RemoteErrorType::CouldNotOpenFile))?;
        let mut total = 0_u64;
        while let Some(chunk) = self.runtime.block_on(response.next()) {
            let chunk =
                chunk.map_err(|error| map_gcs_error(error, RemoteErrorType::CouldNotOpenFile))?;
            let chunk_size = u64::try_from(chunk.len())
                .map_err(|error| RemoteError::new_ex(RemoteErrorType::ProtocolError, error))?;
            total = total
                .checked_add(chunk_size)
                .ok_or_else(|| RemoteError::new(RemoteErrorType::ProtocolError))?;
            dest.write_all(&chunk)
                .map_err(|error| RemoteError::new_ex(RemoteErrorType::IoError, error))?;
        }
        Ok(total)
    }
}

fn bucket_resource(bucket: &str) -> String {
    format!("{BUCKET_RESOURCE_PREFIX}{bucket}")
}

fn object_name(path: &Path) -> RemoteResult<String> {
    let path = path_utils::slash(path);
    let mut components = Vec::new();
    for component in path.split(OBJECT_DELIMITER) {
        if component.is_empty() || component == "." {
            continue;
        }
        if component == ".." {
            return Err(RemoteError::new(RemoteErrorType::BadFile));
        }
        components.push(component);
    }
    Ok(components.join(OBJECT_DELIMITER))
}

fn directory_prefix(path: &Path) -> RemoteResult<String> {
    let name = object_name(path)?;
    if name.is_empty() {
        Ok(String::new())
    } else {
        Ok(format!("{name}{OBJECT_DELIMITER}"))
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use google_cloud_auth::credentials::anonymous::Builder as AnonymousBuilder;
    use google_cloud_gax::exponential_backoff::ExponentialBackoff;
    use google_cloud_gax::retry_policy::{AlwaysRetry, RetryPolicyExt};
    use google_cloud_gax::retry_throttler::AdaptiveThrottler;
    use google_cloud_storage::model::Object;

    use super::*;

    #[test]
    fn new_uses_application_default_credentials() {
        let runtime = Arc::new(Runtime::new().unwrap());
        let client = GoogleCloudStorageFs::new("bucket", &runtime);
        assert_eq!(client.bucket, "bucket");
        assert_eq!(client.wrkdir, Path::new(ROOT_PATH));
        assert!(matches!(
            client.credentials,
            GoogleCloudStorageCredentials::ApplicationDefault
        ));
        assert!(client.storage.is_none());
        assert!(client.control.is_none());
    }

    #[test]
    fn construction_accepts_credentials_and_configuration() {
        let runtime = Arc::new(Runtime::new().unwrap());
        let client = GoogleCloudStorageFs::with_credentials(
            "bucket",
            GoogleCloudStorageCredentials::custom(AnonymousBuilder::new().build()),
            &runtime,
        )
        .endpoint("http://localhost:4443")
        .universe_domain("example.test")
        .backoff_policy(ExponentialBackoff::default())
        .retry_policy(AlwaysRetry.with_attempt_limit(2))
        .retry_throttler(AdaptiveThrottler::default());

        assert_eq!(client.endpoint.as_deref(), Some("http://localhost:4443"));
        assert_eq!(client.universe_domain.as_deref(), Some("example.test"));
        assert!(client.backoff_policy.is_some());
        assert!(client.retry_policy.is_some());
        assert!(client.retry_throttler.is_some());
        assert!(matches!(
            client.credentials,
            GoogleCloudStorageCredentials::Custom(_)
        ));
    }

    #[test]
    fn disconnecting_an_unconnected_client_fails() {
        let runtime = Arc::new(Runtime::new().unwrap());
        let mut client = GoogleCloudStorageFs::new("bucket", &runtime);
        assert_eq!(
            client.disconnect().unwrap_err().kind,
            RemoteErrorType::NotConnected
        );
    }

    #[test]
    fn disconnected_implemented_operations_fail_as_not_connected() {
        let runtime = Arc::new(Runtime::new().unwrap());
        let mut client = GoogleCloudStorageFs::new("bucket", &runtime);
        assert_eq!(
            client.pwd().unwrap_err().kind,
            RemoteErrorType::NotConnected
        );
        assert_eq!(
            client.stat(Path::new("file")).unwrap_err().kind,
            RemoteErrorType::NotConnected
        );
        assert_eq!(
            client.exists(Path::new("file")).unwrap_err().kind,
            RemoteErrorType::NotConnected
        );
        assert_eq!(
            client
                .create_file(
                    Path::new("file"),
                    &Metadata::default(),
                    Box::new(Cursor::new(Vec::new())),
                )
                .unwrap_err()
                .kind,
            RemoteErrorType::NotConnected
        );
    }

    #[test]
    fn unsupported_operations_are_explicit() {
        let runtime = Arc::new(Runtime::new().unwrap());
        let mut client = GoogleCloudStorageFs::new("bucket", &runtime);
        let metadata = Metadata::default();
        let operations = [
            client
                .setstat(Path::new("file"), metadata.clone())
                .unwrap_err(),
            client
                .symlink(Path::new("link"), Path::new("file"))
                .unwrap_err(),
            client.exec("echo test").unwrap_err(),
            client.append(Path::new("file"), &metadata).err().unwrap(),
            client.create(Path::new("file"), &metadata).err().unwrap(),
            client.open(Path::new("file")).err().unwrap(),
        ];
        assert!(
            operations
                .iter()
                .all(|error| error.kind == RemoteErrorType::UnsupportedFeature)
        );
    }

    #[test]
    fn formats_gcs_resource_names() {
        assert_eq!(bucket_resource("my-bucket"), "projects/_/buckets/my-bucket");
        assert_eq!(
            object_name(Path::new("/docs/readme.md")).unwrap(),
            "docs/readme.md"
        );
        assert_eq!(directory_prefix(Path::new("/docs")).unwrap(), "docs/");
        assert_eq!(directory_prefix(Path::new(ROOT_PATH)).unwrap(), "");
    }

    #[test]
    fn resolves_relative_paths_from_the_working_directory() {
        let runtime = Arc::new(Runtime::new().unwrap());
        let mut client = GoogleCloudStorageFs::new("bucket", &runtime);
        client.wrkdir = PathBuf::from("/docs");
        assert_eq!(
            client.resolve(Path::new("api.md")).unwrap(),
            Path::new("docs/api.md")
        );
    }

    #[test]
    fn path_traversal_cannot_escape_root() {
        let runtime = Arc::new(Runtime::new().unwrap());
        let mut client = GoogleCloudStorageFs::new("bucket", &runtime);
        client.wrkdir = PathBuf::from("/docs");
        assert_eq!(
            client.resolve(Path::new("../../secret")).unwrap_err().kind,
            RemoteErrorType::BadFile
        );
    }

    #[test]
    fn list_shaping_merges_objects_and_prefixes() {
        let entries = [
            GcsObject::try_from(Object::new().set_name("docs/readme.md").set_size(4)).unwrap(),
            GcsObject::directory("docs/images/"),
            GcsObject::directory("docs/"),
        ];
        let mut shaped = BTreeMap::new();
        for entry in entries {
            let file = entry.into_file();
            if file.path() != Path::new("/docs") {
                shaped.insert(file.path().to_path_buf(), file);
            }
        }
        let files: Vec<_> = shaped.into_values().collect();
        assert_eq!(files.len(), 2);
        assert!(files[0].is_dir());
        assert_eq!(files[0].path(), Path::new("/docs/images"));
        assert!(files[1].is_file());
    }

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn client_is_send_and_sync() {
        assert_send_sync::<GoogleCloudStorageFs>();
    }

    #[test]
    fn unix_permissions_are_accepted_for_directory_creation() {
        let permissions = UnixPex::from(0o755);
        assert_eq!(permissions, UnixPex::from(0o755));
    }
}
