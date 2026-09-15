//! # Rust-native client
//!
//! `AsyncRemoteFs` implementation backed by the pure-Rust [`smb`] crate.

mod convert;
mod credentials;
mod options;
mod stream;

use std::path::Path;

use convert::{dir_entry_to_file, open_info_to_file, smb_error, system_time_to_filetime, unc_for};
use credentials::ResolvedCredentials;
pub use credentials::SmbCredentials;
use futures_util::StreamExt;
pub use options::{SmbEncryptionLevel, SmbOptions};
use remotefs::fs::{
    AsyncReadStream, AsyncWriteStream, Capabilities, ExecOutput, File, ReadOptions, SetMetadata,
    UnixPex, WriteOptions,
};
use remotefs::{AsyncRemoteFs, RemoteError, RemoteErrorType, RemoteResult};
use smb::binrw_util::prelude::FileTime;
use smb::{
    Client, ClientConfig, CreateDisposition, CreateOptions, DirAccessMask, Directory,
    FileAccessMask, FileAttributes, FileBasicInformation, FileCreateArgs,
    FileDispositionInformation, FileIdBothDirectoryInformation, FileNetworkOpenInformation,
    FileRenameInformation, GetLen, ReadAt, Resource, ResourceHandle, UncPath,
};
use stream::{close_file, write_all_at, SmbReadStream, SmbWriteStream, IO_CHUNK_SIZE};
use tokio::runtime::Handle;

use super::{SmbDialect, AUTO_MAX_DIALECT, AUTO_MIN_DIALECT};
use crate::utils::path::SharePath;

/// A blocking view of [`SmbFs`] that implements [`remotefs::RemoteFs`].
///
/// Every call blocks on the Tokio handle supplied to
/// [`SmbFs::into_blocking`] and must not be made from inside an async
/// context.
#[cfg_attr(docsrs, doc(cfg(feature = "smb")))]
pub type BlockingSmbFs = remotefs::adapters::blocking::BlockOn<SmbFs>;

/// Keeps the async-only SMB client destructor from running after its runtime
/// has shut down. A client that cannot be closed asynchronously is leaked as
/// a last-resort shutdown fallback, matching the stream-handle guard.
struct RuntimeSafeClient {
    client: Option<Client>,
}

impl RuntimeSafeClient {
    fn new(client: Client) -> Self {
        Self {
            client: Some(client),
        }
    }

    fn as_client(&self) -> &Client {
        self.client
            .as_ref()
            .expect("runtime-safe client must remain present")
    }
}

impl Drop for RuntimeSafeClient {
    fn drop(&mut self) {
        let Some(client) = self.client.take() else {
            return;
        };
        if Handle::try_current().is_ok() {
            drop(client);
        } else {
            warn!("leaking an abandoned SMB client because its Tokio runtime is unavailable");
            std::mem::forget(client);
        }
    }
}

/// SMB file system client built on the pure-Rust [`smb`] crate.
///
/// The client implements [`AsyncRemoteFs`] natively and must be driven from
/// a Tokio runtime: `connect` captures the current runtime handle and uses
/// it to close abandoned handles in the background. Blocking callers wrap
/// the client with [`SmbFs::into_blocking`]. Keep that runtime alive until
/// the client and its streams have been finished or dropped; if it has
/// already shut down, the final fallback intentionally leaks the underlying
/// SMB handle rather than panicking from the `smb` destructor.
///
/// Paths are absolute and rooted at the share (`/`). Only SMB 2.0.2 through
/// 3.1.1 are supported; [`SmbDialect::Nt1`] is rejected at construction.
///
/// # Examples
///
/// ```no_run
/// use std::path::Path;
///
/// use remotefs::AsyncRemoteFs;
/// use remotefs::fs::WriteOptions;
/// use remotefs_smb::{SmbCredentials, SmbFs, SmbOptions};
///
/// # #[tokio::main]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let mut client = SmbFs::try_new(
///     SmbCredentials::default()
///         .server("smb://localhost:3445")
///         .share("/temp")
///         .username("test")
///         .password("test")
///         .workgroup("pavao"),
///     SmbOptions::default(),
/// )?;
/// client.connect().await?;
/// client.create_dir(Path::new("/cargo"), None).await?;
/// let mut source = futures::io::Cursor::new(b"hello".to_vec());
/// client
///     .write_file(Path::new("/cargo/hello.txt"), &WriteOptions::default(), &mut source)
///     .await?;
/// client.disconnect().await?;
/// # Ok(())
/// # }
/// ```
#[cfg_attr(docsrs, doc(cfg(feature = "smb")))]
pub struct SmbFs {
    config: ClientConfig,
    credentials: ResolvedCredentials,
    client: Option<Client>,
    runtime: Option<Handle>,
}

impl std::fmt::Debug for SmbFs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SmbFs")
            .field("share", &self.credentials.share.to_string())
            .field("connected", &self.client.is_some())
            .finish_non_exhaustive()
    }
}

impl Drop for SmbFs {
    fn drop(&mut self) {
        if let (Some(client), Some(runtime)) = (self.client.take(), self.runtime.take()) {
            warn!("SmbFs dropped while connected; closing the connection in the background");
            let client = RuntimeSafeClient::new(client);
            runtime.spawn(async move {
                if let Err(error) = client.as_client().close().await {
                    error!("failed to close abandoned SMB connection: {error}");
                }
                drop(client);
            });
        }
    }
}

impl SmbFs {
    /// Tries to create a client with secure automatic dialect bounds
    /// (SMB 2.0.2 through 3.1.1).
    ///
    /// # Errors
    ///
    /// Returns [`RemoteErrorType::BadAddress`] if the server or share in
    /// `credentials` cannot be parsed.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use remotefs_smb::{SmbCredentials, SmbFs, SmbOptions};
    ///
    /// let _client = SmbFs::try_new(
    ///     SmbCredentials::default()
    ///         .server("server.example")
    ///         .share("documents"),
    ///     SmbOptions::default(),
    /// )?;
    /// # Ok::<(), remotefs::RemoteError>(())
    /// ```
    pub fn try_new(credentials: SmbCredentials, options: SmbOptions) -> RemoteResult<Self> {
        Self::try_new_with_dialect(credentials, options, AUTO_MIN_DIALECT, AUTO_MAX_DIALECT)
    }

    /// Tries to create a client with inclusive protocol dialect bounds.
    ///
    /// # Errors
    ///
    /// Returns [`RemoteErrorType::BadAddress`] if the bounds are inverted,
    /// if either bound is [`SmbDialect::Nt1`], or if the credentials cannot
    /// be parsed.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use remotefs_smb::{SmbCredentials, SmbDialect, SmbFs, SmbOptions};
    ///
    /// let _client = SmbFs::try_new_with_dialect(
    ///     SmbCredentials::default()
    ///         .server("server.example")
    ///         .share("documents"),
    ///     SmbOptions::default(),
    ///     SmbDialect::Smb300,
    ///     SmbDialect::Smb311,
    /// )?;
    /// # Ok::<(), remotefs::RemoteError>(())
    /// ```
    pub fn try_new_with_dialect(
        credentials: SmbCredentials,
        options: SmbOptions,
        min_dialect: SmbDialect,
        max_dialect: SmbDialect,
    ) -> RemoteResult<Self> {
        let credentials = credentials.resolve()?;
        let config =
            options::build_client_config(&options, credentials.port, min_dialect, max_dialect)?;
        Ok(Self {
            config,
            credentials,
            client: None,
            runtime: None,
        })
    }

    /// Returns the inner [`smb::Client`] while connected.
    pub fn client(&self) -> Option<&Client> {
        self.client.as_ref()
    }

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
    /// ```no_run
    /// use remotefs::RemoteFs;
    /// use remotefs_smb::{SmbCredentials, SmbFs, SmbOptions};
    ///
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let runtime = tokio::runtime::Runtime::new()?;
    /// let client = SmbFs::try_new(
    ///     SmbCredentials::default().server("server.example").share("documents"),
    ///     SmbOptions::default(),
    /// )?;
    /// let mut client: Box<dyn RemoteFs> = Box::new(client.into_blocking(runtime.handle().clone()));
    /// client.connect()?;
    /// client.disconnect()?;
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn into_blocking(self, handle: Handle) -> BlockingSmbFs {
        assert_ne!(
            handle.runtime_flavor(),
            tokio::runtime::RuntimeFlavor::CurrentThread,
            "into_blocking requires a multi-thread Tokio runtime"
        );
        remotefs::adapters::blocking::BlockOn::new(self, handle)
    }

    // -- private

    fn connected(&self) -> RemoteResult<(&Client, &Handle)> {
        match (&self.client, &self.runtime) {
            (Some(client), Some(runtime)) => Ok((client, runtime)),
            _ => Err(RemoteError::new(RemoteErrorType::NotConnected)),
        }
    }

    /// Validates `path` and returns it with the matching UNC path.
    fn unc(&self, path: &Path) -> RemoteResult<(SharePath, UncPath)> {
        let share = SharePath::parse(path)?;
        let unc = unc_for(&self.credentials.share, &share);
        Ok((share, unc))
    }

    fn unsupported() -> RemoteError {
        RemoteError::new(RemoteErrorType::UnsupportedFeature)
    }

    async fn open_resource(
        client: &Client,
        unc: &UncPath,
        args: &FileCreateArgs,
        kind: RemoteErrorType,
    ) -> RemoteResult<Resource> {
        client
            .create_file(unc, args)
            .await
            .map_err(|e| smb_error(kind, e))
    }

    fn handle_of(resource: &Resource) -> &ResourceHandle {
        match resource {
            Resource::File(file) => file.handle(),
            Resource::Directory(dir) => dir.handle(),
            Resource::Pipe(pipe) => pipe,
        }
    }

    async fn close_handle(handle: &ResourceHandle) -> RemoteResult<()> {
        handle
            .close()
            .await
            .map_err(|e| smb_error(RemoteErrorType::ProtocolError, e))
    }

    /// Opens `unc` and requires a regular file, closing anything else.
    async fn open_file(
        client: &Client,
        unc: &UncPath,
        args: &FileCreateArgs,
        kind: RemoteErrorType,
    ) -> RemoteResult<smb::File> {
        match Self::open_resource(client, unc, args, kind).await? {
            Resource::File(file) => Ok(file),
            other => {
                Self::close_handle(Self::handle_of(&other)).await?;
                Err(RemoteError::with_message(kind, "not a regular file"))
            }
        }
    }

    async fn stat_unc(client: &Client, unc: &UncPath) -> RemoteResult<FileNetworkOpenInformation> {
        let args = FileCreateArgs::make_open_existing(
            FileAccessMask::new().with_file_read_attributes(true),
        );
        let resource = Self::open_resource(client, unc, &args, RemoteErrorType::StatFailed).await?;
        let handle = Self::handle_of(&resource);
        let info = handle
            .query_info::<FileNetworkOpenInformation>()
            .await
            .map_err(|e| smb_error(RemoteErrorType::StatFailed, e));
        Self::close_handle(handle).await?;
        info
    }

    /// Opens `unc` with delete access and marks it for deletion on close.
    async fn delete_unc(
        client: &Client,
        unc: &UncPath,
        options: CreateOptions,
        access: FileAccessMask,
    ) -> RemoteResult<()> {
        let args = FileCreateArgs {
            disposition: CreateDisposition::Open,
            attributes: FileAttributes::new(),
            options,
            desired_access: access,
        };
        let resource =
            Self::open_resource(client, unc, &args, RemoteErrorType::CouldNotRemoveFile).await?;
        let handle = Self::handle_of(&resource);
        let result = handle
            .set_info(FileDispositionInformation::default())
            .await
            .map_err(|e| smb_error(RemoteErrorType::CouldNotRemoveFile, e));
        Self::close_handle(handle).await?;
        result
    }

    async fn copy_file(client: &Client, src: &UncPath, dest: &UncPath) -> RemoteResult<()> {
        let read_args =
            FileCreateArgs::make_open_existing(FileAccessMask::new().with_generic_read(true));
        let source =
            Self::open_file(client, src, &read_args, RemoteErrorType::CouldNotOpenFile).await?;
        let write_args = FileCreateArgs::make_overwrite(
            FileAttributes::new(),
            CreateOptions::new().with_non_directory_file(true),
        );
        let target =
            match Self::open_file(client, dest, &write_args, RemoteErrorType::CouldNotOpenFile)
                .await
            {
                Ok(target) => target,
                Err(error) => {
                    let _ = close_file(source).await;
                    return Err(error);
                }
            };
        let result = async {
            let mut buffer = vec![0u8; IO_CHUNK_SIZE];
            let mut position = 0u64;
            loop {
                let read = source
                    .read_at(&mut buffer, position)
                    .await
                    .map_err(|e| smb_error(RemoteErrorType::IoError, e))?;
                if read == 0 {
                    break;
                }
                write_all_at(&target, &buffer[..read], position)
                    .await
                    .map_err(|e| smb_error(RemoteErrorType::IoError, e))?;
                position += read as u64;
            }
            Ok::<(), RemoteError>(())
        }
        .await;
        let closed_target = close_file(target).await;
        let closed_source = close_file(source).await;
        result.and(closed_target).and(closed_source)
    }
}

#[remotefs::async_trait]
impl AsyncRemoteFs for SmbFs {
    async fn connect(&mut self) -> RemoteResult<()> {
        if self.client.is_some() {
            return Err(RemoteError::new(RemoteErrorType::AlreadyConnected));
        }
        let runtime = Handle::try_current()
            .map_err(|e| RemoteError::with_source(RemoteErrorType::ConnectionError, e))?;
        debug!("connecting to {}", self.credentials.share);
        let client = Client::new(self.config.clone());
        client
            .share_connect(
                &self.credentials.share,
                &self.credentials.logon_name,
                self.credentials.password.clone(),
            )
            .await
            .map_err(|e| smb_error(RemoteErrorType::ConnectionError, e))?;
        self.client = Some(client);
        self.runtime = Some(runtime);
        Ok(())
    }

    async fn disconnect(&mut self) -> RemoteResult<()> {
        let client = self
            .client
            .take()
            .ok_or_else(|| RemoteError::new(RemoteErrorType::NotConnected))?;
        self.runtime = None;
        debug!("disconnecting from {}", self.credentials.share);
        let result = client
            .close()
            .await
            .map_err(|e| smb_error(RemoteErrorType::ConnectionError, e));
        drop(client);
        result
    }

    fn is_connected(&self) -> bool {
        self.client.is_some()
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities::STREAM_READ
            | Capabilities::STREAM_WRITE
            | Capabilities::APPEND
            | Capabilities::RANGE_READ
            | Capabilities::SEEK_READ
            | Capabilities::SEEK_WRITE
            | Capabilities::COPY
            | Capabilities::SET_METADATA
    }

    async fn list_dir(&self, path: &Path) -> RemoteResult<Vec<File>> {
        let (share, unc) = self.unc(path)?;
        let (client, _) = self.connected()?;
        let absolute = share.to_path_buf();
        trace!("listing files at {}", absolute.display());
        let args = FileCreateArgs::make_open_existing(
            DirAccessMask::new()
                .with_list_directory(true)
                .with_synchronize(true)
                .into(),
        );
        let dir =
            match Self::open_resource(client, &unc, &args, RemoteErrorType::StatFailed).await? {
                Resource::Directory(dir) => std::sync::Arc::new(dir),
                other => {
                    Self::close_handle(Self::handle_of(&other)).await?;
                    return Err(RemoteError::with_message(
                        RemoteErrorType::StatFailed,
                        "not a directory",
                    ));
                }
            };
        let entries = async {
            let mut stream = Directory::query::<FileIdBothDirectoryInformation>(&dir, "*")
                .await
                .map_err(|e| smb_error(RemoteErrorType::StatFailed, e))?;
            let mut entries = Vec::new();
            while let Some(entry) = stream.next().await {
                let entry = entry.map_err(|e| smb_error(RemoteErrorType::StatFailed, e))?;
                let name = entry.file_name.to_string();
                if name == "." || name == ".." {
                    continue;
                }
                entries.push(dir_entry_to_file(absolute.as_path(), &entry));
            }
            Ok::<Vec<File>, RemoteError>(entries)
        }
        .await;
        let closed = Self::close_handle(dir.handle()).await;
        drop(dir);
        entries.and_then(|entries| closed.map(|()| entries))
    }

    async fn stat(&self, path: &Path) -> RemoteResult<File> {
        let (share, unc) = self.unc(path)?;
        let (client, _) = self.connected()?;
        let absolute = share.to_path_buf();
        trace!("get stat for {}", absolute.display());
        let info = Self::stat_unc(client, &unc).await?;
        Ok(open_info_to_file(absolute, &info))
    }

    async fn exists(&self, path: &Path) -> RemoteResult<bool> {
        match self.stat(path).await {
            Ok(_) => Ok(true),
            Err(error) if error.kind() == RemoteErrorType::NoSuchFileOrDirectory => Ok(false),
            Err(error) => Err(error),
        }
    }

    async fn set_metadata(&self, path: &Path, metadata: &SetMetadata) -> RemoteResult<()> {
        let (_, unc) = self.unc(path)?;
        let (client, _) = self.connected()?;
        if metadata.mode.is_some() || metadata.uid.is_some() || metadata.gid.is_some() {
            return Err(Self::unsupported());
        }
        if metadata.accessed.is_none() && metadata.modified.is_none() {
            return Ok(());
        }
        let args = FileCreateArgs::make_open_existing(
            FileAccessMask::new().with_file_write_attributes(true),
        );
        let resource =
            Self::open_resource(client, &unc, &args, RemoteErrorType::PermissionDenied).await?;
        let handle = Self::handle_of(&resource);
        let info = FileBasicInformation {
            creation_time: FileTime::ZERO,
            last_access_time: metadata
                .accessed
                .map_or(FileTime::ZERO, system_time_to_filetime),
            last_write_time: metadata
                .modified
                .map_or(FileTime::ZERO, system_time_to_filetime),
            change_time: FileTime::ZERO,
            file_attributes: FileAttributes::new(),
        };
        let result = handle
            .set_info(info)
            .await
            .map_err(|e| smb_error(RemoteErrorType::PermissionDenied, e));
        Self::close_handle(handle).await?;
        result
    }

    async fn create_dir(&self, path: &Path, _mode: Option<UnixPex>) -> RemoteResult<()> {
        let (share, unc) = self.unc(path)?;
        let (client, _) = self.connected()?;
        trace!("making directory at {}", share.to_path_buf().display());
        let args = FileCreateArgs::make_create_new(
            FileAttributes::new().with_directory(true),
            CreateOptions::new().with_directory_file(true),
        );
        let resource =
            Self::open_resource(client, &unc, &args, RemoteErrorType::FileCreateDenied).await?;
        Self::close_handle(Self::handle_of(&resource)).await
    }

    async fn remove_file(&self, path: &Path) -> RemoteResult<()> {
        let (share, unc) = self.unc(path)?;
        let (client, _) = self.connected()?;
        trace!("removing file {}", share.to_path_buf().display());
        Self::delete_unc(
            client,
            &unc,
            CreateOptions::new().with_non_directory_file(true),
            FileAccessMask::new().with_delete(true),
        )
        .await
    }

    async fn remove_dir(&self, path: &Path) -> RemoteResult<()> {
        let (share, unc) = self.unc(path)?;
        let (client, _) = self.connected()?;
        trace!("removing directory at {}", share.to_path_buf().display());
        Self::delete_unc(
            client,
            &unc,
            CreateOptions::new().with_directory_file(true),
            DirAccessMask::new().with_delete(true).into(),
        )
        .await
    }

    async fn rename(&self, src: &Path, dest: &Path) -> RemoteResult<()> {
        let (src_share, src_unc) = self.unc(src)?;
        let (dest_share, _) = self.unc(dest)?;
        let (client, _) = self.connected()?;
        trace!(
            "moving {} to {}",
            src_share.to_path_buf().display(),
            dest_share.to_path_buf().display()
        );
        let args = FileCreateArgs::make_open_existing(FileAccessMask::new().with_delete(true));
        let resource =
            Self::open_resource(client, &src_unc, &args, RemoteErrorType::ProtocolError).await?;
        let handle = Self::handle_of(&resource);
        let result = handle
            .set_info(FileRenameInformation {
                replace_if_exists: false.into(),
                root_directory: 0,
                file_name: dest_share.join("\\").into(),
            })
            .await
            .map_err(|e| smb_error(RemoteErrorType::ProtocolError, e));
        Self::close_handle(handle).await?;
        result
    }

    async fn copy(&self, src: &Path, dest: &Path) -> RemoteResult<()> {
        let (src_share, src_unc) = self.unc(src)?;
        let (dest_share, dest_unc) = self.unc(dest)?;
        if src_share.is_same_or_descendant(&dest_share) {
            return Err(RemoteError::with_message(
                RemoteErrorType::InvalidPath,
                "copy destination cannot be the source or one of its descendants",
            ));
        }
        let (client, _) = self.connected()?;
        let entry = self.stat(src).await?;
        if entry.is_dir() {
            let destination_exists = self.exists(dest).await?;
            if !destination_exists {
                self.create_dir(dest, None).await?;
            }
            let result = async {
                for child in self.list_dir(src).await? {
                    let child_dest = dest_share.child(&child.name()).to_path_buf();
                    self.copy(child.path(), &child_dest).await?;
                }
                Ok::<(), RemoteError>(())
            }
            .await;
            if result.is_err() && !destination_exists {
                let _ = self.remove_dir_all(dest).await;
            }
            result
        } else {
            Self::copy_file(client, &src_unc, &dest_unc).await
        }
    }

    async fn symlink(&self, _path: &Path, _target: &Path) -> RemoteResult<()> {
        Err(Self::unsupported())
    }

    async fn open(&self, path: &Path, opts: &ReadOptions) -> RemoteResult<AsyncReadStream> {
        let (share, unc) = self.unc(path)?;
        let (client, runtime) = self.connected()?;
        trace!("opening file at {} for read", share.to_path_buf().display());
        let args =
            FileCreateArgs::make_open_existing(FileAccessMask::new().with_generic_read(true));
        let file = Self::open_file(client, &unc, &args, RemoteErrorType::CouldNotOpenFile).await?;
        Ok(AsyncReadStream::new(SmbReadStream::new(
            file,
            opts.offset.unwrap_or(0),
            opts.length,
            runtime.clone(),
            share.relative(),
        )))
    }

    async fn create(&self, path: &Path, opts: &WriteOptions) -> RemoteResult<AsyncWriteStream> {
        let (share, unc) = self.unc(path)?;
        let (client, runtime) = self.connected()?;
        trace!("creating file at {}", share.to_path_buf().display());
        let args = FileCreateArgs::make_overwrite(
            FileAttributes::new(),
            CreateOptions::new().with_non_directory_file(true),
        );
        let file = Self::open_file(client, &unc, &args, RemoteErrorType::CouldNotOpenFile).await?;
        Ok(AsyncWriteStream::new(SmbWriteStream::new(
            file,
            0,
            opts.modified,
            runtime.clone(),
            share.relative(),
        )))
    }

    async fn append(&self, path: &Path, opts: &WriteOptions) -> RemoteResult<AsyncWriteStream> {
        let (share, unc) = self.unc(path)?;
        let (client, runtime) = self.connected()?;
        trace!(
            "opening file at {} for append",
            share.to_path_buf().display()
        );
        let args = FileCreateArgs {
            disposition: CreateDisposition::OpenIf,
            attributes: FileAttributes::new(),
            options: CreateOptions::new().with_non_directory_file(true),
            desired_access: FileAccessMask::new()
                .with_generic_read(true)
                .with_generic_write(true),
        };
        let file = Self::open_file(client, &unc, &args, RemoteErrorType::CouldNotOpenFile).await?;
        let position = match file.get_len().await {
            Ok(len) => len,
            Err(error) => {
                let _ = close_file(file).await;
                return Err(smb_error(RemoteErrorType::IoError, error));
            }
        };
        Ok(AsyncWriteStream::new(SmbWriteStream::new(
            file,
            position,
            opts.modified,
            runtime.clone(),
            share.relative(),
        )))
    }

    async fn exec(&self, _cmd: &str) -> RemoteResult<ExecOutput> {
        Err(Self::unsupported())
    }
}

#[cfg(test)]
mod test {
    #[cfg(feature = "with-containers")]
    use std::io::SeekFrom;
    #[cfg(feature = "with-containers")]
    use std::time::{Duration, UNIX_EPOCH};

    #[cfg(feature = "with-containers")]
    use futures::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt, Cursor};
    use pretty_assertions::assert_eq;
    use remotefs::fs::{Capabilities, ReadOptions};
    #[cfg(feature = "with-containers")]
    use remotefs::fs::{SetMetadata, WriteOptions};
    #[cfg(feature = "with-containers")]
    use remotefs::RemoteFs;
    #[cfg(feature = "with-containers")]
    use serial_test::serial;
    use tokio::runtime::{Builder, Runtime};

    use super::*;

    fn test_credentials() -> SmbCredentials {
        SmbCredentials::default()
            .server("smb://localhost:3445")
            .share("/temp")
            .username("test")
            .password("test")
            .workgroup("pavao")
    }

    fn client() -> SmbFs {
        SmbFs::try_new(test_credentials(), SmbOptions::default()).unwrap()
    }

    #[test]
    fn should_reject_inverted_dialect_bounds() {
        let result = SmbFs::try_new_with_dialect(
            test_credentials(),
            SmbOptions::default(),
            SmbDialect::Smb311,
            SmbDialect::Smb202,
        );
        assert_eq!(
            result.expect_err("inverted bounds must fail").kind(),
            RemoteErrorType::BadAddress,
        );
    }

    #[test]
    fn should_reject_nt1_dialect() {
        let result = SmbFs::try_new_with_dialect(
            test_credentials(),
            SmbOptions::default(),
            SmbDialect::Nt1,
            SmbDialect::Smb311,
        );
        assert_eq!(result.unwrap_err().kind(), RemoteErrorType::BadAddress);
    }

    #[test]
    fn should_default_to_secure_auto_dialect_bounds() {
        let client = client();
        assert_eq!(
            client.config.connection.min_dialect,
            Some(smb::Dialect::Smb0202)
        );
        assert_eq!(
            client.config.connection.max_dialect,
            Some(smb::Dialect::Smb0311)
        );
        assert!(!client.is_connected());
        assert!(client.client().is_none());
    }

    #[test]
    fn should_reject_bad_credentials_at_construction() {
        let result = SmbFs::try_new(
            SmbCredentials::default().server("smb://"),
            SmbOptions::default(),
        );
        assert_eq!(result.unwrap_err().kind(), RemoteErrorType::BadAddress);
    }

    #[test]
    fn should_advertise_capabilities() {
        let capabilities = client().capabilities();
        assert!(capabilities.contains(Capabilities::STREAM_READ));
        assert!(capabilities.contains(Capabilities::STREAM_WRITE));
        assert!(capabilities.contains(Capabilities::APPEND));
        assert!(capabilities.contains(Capabilities::RANGE_READ));
        assert!(capabilities.contains(Capabilities::SEEK_READ));
        assert!(capabilities.contains(Capabilities::SEEK_WRITE));
        assert!(capabilities.contains(Capabilities::COPY));
        assert!(capabilities.contains(Capabilities::SET_METADATA));
        assert!(!capabilities.contains(Capabilities::SYMLINK));
        assert!(!capabilities.contains(Capabilities::POSIX_MODE));
        assert!(!capabilities.contains(Capabilities::EXEC));
    }

    #[tokio::test]
    async fn should_fail_when_not_connected_and_on_relative_paths() {
        let client = client();
        assert_eq!(
            client.stat(Path::new("/")).await.unwrap_err().kind(),
            RemoteErrorType::NotConnected
        );
        assert_eq!(
            client.stat(Path::new("a.txt")).await.unwrap_err().kind(),
            RemoteErrorType::InvalidPath
        );
        assert_eq!(
            client
                .open(Path::new("/a.txt"), &ReadOptions::default())
                .await
                .unwrap_err()
                .kind(),
            RemoteErrorType::NotConnected
        );
        assert_eq!(
            client.exec("echo 5").await.unwrap_err().kind(),
            RemoteErrorType::UnsupportedFeature
        );
    }

    #[test]
    fn should_wrap_into_blocking_on_multi_thread_runtime() {
        let runtime = Runtime::new().unwrap();
        let blocking: Box<dyn remotefs::RemoteFs> =
            Box::new(client().into_blocking(runtime.handle().clone()));
        assert!(!blocking.is_connected());
        assert!(blocking.capabilities().contains(Capabilities::RANGE_READ));
    }

    #[test]
    #[should_panic(expected = "into_blocking requires a multi-thread Tokio runtime")]
    fn should_reject_current_thread_runtime_for_blocking() {
        let runtime = Builder::new_current_thread().build().unwrap();
        let _ = client().into_blocking(runtime.handle().clone());
    }

    fn is_send<T: Send>(_send: T) {}

    fn is_sync<T: Sync>(_sync: T) {}

    #[test]
    fn test_should_be_send_and_sync() {
        let client = client();
        is_sync(&client);
        is_send(client);
    }

    // -- container tests (async)

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[cfg(feature = "with-containers")]
    #[serial]
    async fn should_connect_and_disconnect() {
        crate::mock::logger();
        let mut client = init_client().await;
        assert!(client.is_connected());
        assert_eq!(
            client.connect().await.unwrap_err().kind(),
            RemoteErrorType::AlreadyConnected
        );
        finalize_client(client).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[cfg(feature = "with-containers")]
    #[serial]
    async fn should_write_stat_and_read_file() {
        crate::mock::logger();
        let client = init_client().await;
        let p = Path::new("/cargo-test/a.txt");
        let mut source = Cursor::new(b"test data\n".to_vec());
        assert_eq!(
            client
                .write_file(p, &WriteOptions::default().size_hint(10), &mut source)
                .await
                .unwrap(),
            10
        );
        let entry = client.stat(p).await.unwrap();
        assert_eq!(entry.path(), p);
        assert_eq!(entry.name(), "a.txt");
        assert_eq!(entry.metadata().size, Some(10));
        assert!(entry.metadata().modified.is_some());
        assert!(entry.is_file());
        let mut buffer: Vec<u8> = Vec::new();
        assert_eq!(
            client
                .read_file(p, &ReadOptions::default(), &mut buffer)
                .await
                .unwrap(),
            10
        );
        assert_eq!(buffer, b"test data\n");
        let mut source = Cursor::new(b"xy".to_vec());
        client
            .write_file(p, &WriteOptions::default(), &mut source)
            .await
            .unwrap();
        assert_eq!(client.stat(p).await.unwrap().metadata().size, Some(2));
        finalize_client(client).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[cfg(feature = "with-containers")]
    #[serial]
    async fn should_read_ranges() {
        crate::mock::logger();
        let client = init_client().await;
        let p = Path::new("/cargo-test/range.txt");
        let mut source = Cursor::new(b"abcdef".to_vec());
        client
            .write_file(p, &WriteOptions::default(), &mut source)
            .await
            .unwrap();
        let mut out = Vec::new();
        client
            .read_file(p, &ReadOptions::default().offset(2).length(2), &mut out)
            .await
            .unwrap();
        assert_eq!(out, b"cd");
        let mut out = Vec::new();
        client
            .read_file(p, &ReadOptions::default().offset(2).length(0), &mut out)
            .await
            .unwrap();
        assert!(out.is_empty());
        let mut reader = client
            .open(p, &ReadOptions::default().length(4))
            .await
            .unwrap();
        let mut first = [0; 1];
        reader.read_exact(&mut first).await.unwrap();
        assert_eq!(&first, b"a");
        assert_eq!(reader.seek(SeekFrom::Current(0)).await.unwrap(), 1);
        let mut rest = Vec::new();
        reader.read_to_end(&mut rest).await.unwrap();
        assert_eq!(rest, b"bcd");
        reader.finish().await.unwrap();
        let mut out = Vec::new();
        client
            .read_file(p, &ReadOptions::default().offset(100), &mut out)
            .await
            .unwrap();
        assert!(out.is_empty());
        let mut out = Vec::new();
        assert!(client
            .read_file(
                Path::new("/cargo-test/missing.txt"),
                &ReadOptions::default().length(0),
                &mut out
            )
            .await
            .is_err());
        finalize_client(client).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[cfg(feature = "with-containers")]
    #[serial]
    async fn should_stream_write_seek_and_read() {
        crate::mock::logger();
        let client = init_client().await;
        let p = Path::new("/cargo-test/stream.txt");
        let mut writer = client.create(p, &WriteOptions::default()).await.unwrap();
        assert!(writer.seekable());
        writer.write_all(b"hello world").await.unwrap();
        writer.flush().await.unwrap();
        writer.seek(SeekFrom::Start(6)).await.unwrap();
        writer.write_all(b"there").await.unwrap();
        writer.finish().await.unwrap();
        assert_eq!(client.stat(p).await.unwrap().metadata().size, Some(11));

        let mut reader = client.open(p, &ReadOptions::default()).await.unwrap();
        assert!(reader.seekable());
        let mut first = [0u8; 5];
        reader.read_exact(&mut first).await.unwrap();
        assert_eq!(&first, b"hello");
        assert_eq!(reader.seek(SeekFrom::Current(1)).await.unwrap(), 6);
        let mut rest = Vec::new();
        reader.read_to_end(&mut rest).await.unwrap();
        assert_eq!(rest, b"there");
        assert_eq!(reader.seek(SeekFrom::End(-5)).await.unwrap(), 6);
        let mut again = String::new();
        reader.read_to_string(&mut again).await.unwrap();
        assert_eq!(again, "there");
        reader.finish().await.unwrap();

        let eof_path = Path::new("/cargo-test/eof.txt");
        let mut eof_writer = client
            .create(eof_path, &WriteOptions::default())
            .await
            .unwrap();
        eof_writer.write_all(b"abc").await.unwrap();
        eof_writer.flush().await.unwrap();
        assert_eq!(eof_writer.seek(SeekFrom::End(0)).await.unwrap(), 3);
        eof_writer.write_all(b"d").await.unwrap();
        eof_writer.finish().await.unwrap();
        let mut eof_contents = Vec::new();
        client
            .read_file(eof_path, &ReadOptions::default(), &mut eof_contents)
            .await
            .unwrap();
        assert_eq!(eof_contents, b"abcd");
        finalize_client(client).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[cfg(feature = "with-containers")]
    #[serial]
    async fn should_append_with_stream_and_one_shot() {
        crate::mock::logger();
        let client = init_client().await;
        let p = Path::new("/cargo-test/a.txt");
        let mut source = Cursor::new(b"test data\n".to_vec());
        client
            .write_file(p, &WriteOptions::default(), &mut source)
            .await
            .unwrap();
        let mut writer = client.append(p, &WriteOptions::default()).await.unwrap();
        writer.write_all(b"Hello, world!\n").await.unwrap();
        writer.finish().await.unwrap();
        assert_eq!(client.stat(p).await.unwrap().metadata().size, Some(24));
        let mut source = Cursor::new(b"!".to_vec());
        assert_eq!(
            client
                .append_file(p, &WriteOptions::default(), &mut source)
                .await
                .unwrap(),
            1
        );
        assert_eq!(client.stat(p).await.unwrap().metadata().size, Some(25));
        // append creates a missing file but not a missing directory
        let mut source = Cursor::new(b"x".to_vec());
        assert!(client
            .append_file(
                Path::new("/tmp/aaaaaaa/hbbbbb/a.txt"),
                &WriteOptions::default(),
                &mut source
            )
            .await
            .is_err());
        finalize_client(client).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[cfg(feature = "with-containers")]
    #[serial]
    async fn should_close_dropped_streams_without_panicking() {
        crate::mock::logger();
        let client = init_client().await;
        let p = Path::new("/cargo-test/dropped.txt");
        let mut writer = client.create(p, &WriteOptions::default()).await.unwrap();
        writer.write_all(b"partial").await.unwrap();
        drop(writer);
        let reader = client.open(p, &ReadOptions::default()).await.unwrap();
        drop(reader);
        tokio::time::sleep(Duration::from_millis(200)).await;
        // the handle was closed in the background, so the file can be removed
        client.remove_file(p).await.unwrap();
        finalize_client(client).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[cfg(feature = "with-containers")]
    #[serial]
    async fn should_create_list_and_remove_directories() {
        crate::mock::logger();
        let client = init_client().await;
        let dir = Path::new("/cargo-test/mydir");
        client
            .create_dir(dir, Some(UnixPex::from(0o755)))
            .await
            .unwrap();
        assert_eq!(
            client.create_dir(dir, None).await.unwrap_err().kind(),
            RemoteErrorType::AlreadyExists
        );
        assert!(client
            .create_dir(Path::new("/tmp/werfgjwerughjwurih/iwerjghiwgui"), None)
            .await
            .is_err());
        let mut source = Cursor::new(b"x".to_vec());
        client
            .write_file(
                Path::new("/cargo-test/mydir/a.txt"),
                &WriteOptions::default(),
                &mut source,
            )
            .await
            .unwrap();
        let entries = client.list_dir(dir).await.unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path(), Path::new("/cargo-test/mydir/a.txt"));
        assert_eq!(entries[0].extension().as_deref(), Some("txt"));
        assert_eq!(entries[0].metadata().size, Some(1));
        assert!(client
            .list_dir(Path::new("/tmp/auhhfh/hfhjfhf/"))
            .await
            .is_err());
        assert_eq!(
            client.remove_dir(dir).await.unwrap_err().kind(),
            RemoteErrorType::DirectoryNotEmpty
        );
        client.remove_dir_all(dir).await.unwrap();
        assert!(!client.exists(dir).await.unwrap());
        assert!(client
            .remove_dir_all(Path::new("/tmp/aaaaaa/asuhi"))
            .await
            .is_err());
        finalize_client(client).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[cfg(feature = "with-containers")]
    #[serial]
    async fn should_tell_whether_entries_exist() {
        crate::mock::logger();
        let client = init_client().await;
        let p = Path::new("/cargo-test/a.txt");
        let mut source = Cursor::new(b"x".to_vec());
        client
            .write_file(p, &WriteOptions::default(), &mut source)
            .await
            .unwrap();
        assert!(client.exists(p).await.unwrap());
        assert!(client.exists(Path::new("/cargo-test/")).await.unwrap());
        assert!(!client.exists(Path::new("/cargo-test/b.txt")).await.unwrap());
        assert!(!client.exists(Path::new("/tmp/ppppp/bhhrhu")).await.unwrap());
        assert_eq!(
            client.exists(Path::new("a.txt")).await.unwrap_err().kind(),
            RemoteErrorType::InvalidPath
        );
        finalize_client(client).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[cfg(feature = "with-containers")]
    #[serial]
    async fn should_rename_copy_and_remove_files() {
        crate::mock::logger();
        let client = init_client().await;
        let p = Path::new("/cargo-test/a.txt");
        let dest = Path::new("/cargo-test/b.txt");
        let mut source = Cursor::new(b"test data\n".to_vec());
        client
            .write_file(p, &WriteOptions::default(), &mut source)
            .await
            .unwrap();
        client.rename(p, dest).await.unwrap();
        assert!(!client.exists(p).await.unwrap());
        assert!(client.exists(dest).await.unwrap());
        assert!(client
            .rename(dest, Path::new("/tmp/wuefhiwuerfh/whjhh/b.txt"))
            .await
            .is_err());
        client.copy(dest, p).await.unwrap();
        let mut out = Vec::new();
        client
            .read_file(p, &ReadOptions::default(), &mut out)
            .await
            .unwrap();
        assert_eq!(out, b"test data\n");
        // directory copy
        let dir = Path::new("/cargo-test/dir");
        client.create_dir(dir, None).await.unwrap();
        client
            .copy(p, Path::new("/cargo-test/dir/c.txt"))
            .await
            .unwrap();
        client
            .copy(dir, Path::new("/cargo-test/dir2"))
            .await
            .unwrap();
        assert!(client
            .exists(Path::new("/cargo-test/dir2/c.txt"))
            .await
            .unwrap());
        assert!(client
            .copy(p, Path::new("/tmp/aaa/bbbb/ccc/b.txt"))
            .await
            .is_err());
        client.remove_file(dest).await.unwrap();
        assert_eq!(
            client.remove_file(dest).await.unwrap_err().kind(),
            RemoteErrorType::NoSuchFileOrDirectory
        );
        finalize_client(client).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[cfg(feature = "with-containers")]
    #[serial]
    async fn should_set_times_but_not_mode() {
        crate::mock::logger();
        let client = init_client().await;
        let p = Path::new("/cargo-test/a.sh");
        let modified = UNIX_EPOCH + Duration::from_secs(1_600_000_000);
        let mut source = Cursor::new(b"echo 5\n".to_vec());
        client
            .write_file(p, &WriteOptions::default().modified(modified), &mut source)
            .await
            .unwrap();
        assert_eq!(
            client.stat(p).await.unwrap().metadata().modified,
            Some(modified)
        );
        let later = modified + Duration::from_secs(60);
        client
            .set_metadata(p, &SetMetadata::default().modified(later).accessed(later))
            .await
            .unwrap();
        let metadata = client.stat(p).await.unwrap().metadata().clone();
        assert_eq!(metadata.modified, Some(later));
        assert_eq!(metadata.accessed, Some(later));
        assert_eq!(
            client
                .set_metadata(p, &SetMetadata::default().mode(UnixPex::from(0o755)))
                .await
                .unwrap_err()
                .kind(),
            RemoteErrorType::UnsupportedFeature
        );
        assert_eq!(
            client
                .symlink(Path::new("/cargo-test/b.sh"), p)
                .await
                .unwrap_err()
                .kind(),
            RemoteErrorType::UnsupportedFeature
        );
        finalize_client(client).await;
    }

    // -- container tests (blocking wrapper)

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_round_trip_through_blocking_wrapper() {
        crate::mock::logger();
        let runtime = Runtime::new().unwrap();
        let mut client: Box<dyn RemoteFs> =
            Box::new(client().into_blocking(runtime.handle().clone()));
        client.connect().unwrap();
        assert!(client.is_connected());
        let _ = client.remove_dir_all(Path::new("/cargo-test"));
        client.create_dir(Path::new("/cargo-test"), None).unwrap();
        let p = Path::new("/cargo-test/blocking.txt");
        let mut source = std::io::Cursor::new(b"hello".to_vec());
        assert_eq!(
            client
                .write_file(p, &WriteOptions::default().size_hint(5), &mut source)
                .unwrap(),
            5
        );
        let names: Vec<String> = client
            .list_dir(Path::new("/cargo-test"))
            .unwrap()
            .iter()
            .map(|entry| entry.name())
            .collect();
        assert_eq!(names, vec!["blocking.txt".to_string()]);
        let mut reader = client.open(p, &ReadOptions::default().offset(1)).unwrap();
        let mut out = String::new();
        std::io::Read::read_to_string(&mut reader, &mut out).unwrap();
        assert_eq!(out, "ello");
        reader.finish().unwrap();
        // an abandoned stream must not panic when dropped on a plain thread
        let abandoned = client.open(p, &ReadOptions::default()).unwrap();
        drop(abandoned);
        client.remove_dir_all(Path::new("/cargo-test")).unwrap();
        client.disconnect().unwrap();
        drop(client);
        runtime.shutdown_timeout(Duration::from_secs(5));
    }

    #[cfg(feature = "with-containers")]
    async fn init_client() -> SmbFs {
        let mut client = client();
        client.connect().await.unwrap();
        // Make the test directory over SMB instead of on the host filesystem.
        let _ = client.remove_dir_all(Path::new("/cargo-test")).await;
        client
            .create_dir(Path::new("/cargo-test"), Some(UnixPex::from(0o755)))
            .await
            .unwrap();
        client
    }

    #[cfg(feature = "with-containers")]
    async fn finalize_client(mut client: SmbFs) {
        let _ = client.remove_dir_all(Path::new("/cargo-test")).await;
        client.disconnect().await.unwrap();
        tokio::time::sleep(Duration::from_secs(1)).await;
        drop(client);
    }
}
