//! # Rust-native client
//!
//! `RemoteFs` implementation backed by the pure-Rust [`smb`] crate.

mod convert;
mod credentials;
mod options;

use std::future::Future;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use convert::{
    dir_entry_to_file, open_info_to_file, relative_unc_path, smb_error, to_backslash_path, unc_for,
};
use credentials::ResolvedCredentials;
pub use credentials::SmbCredentials;
use futures_util::StreamExt;
pub use options::{SmbEncryptionLevel, SmbOptions};
use remotefs::fs::{File, Metadata, ReadStream, UnixPex, Welcome, WriteStream};
use remotefs::{RemoteError, RemoteErrorType, RemoteFs, RemoteResult};
use smb::{
    Client, ClientConfig, CreateDisposition, CreateOptions, DirAccessMask, Directory,
    FileAccessMask, FileAttributes, FileCreateArgs, FileDispositionInformation,
    FileIdBothDirectoryInformation, FileNetworkOpenInformation, FileRenameInformation, GetLen,
    ReadAt, Resource, ResourceHandle, UncPath, WriteAt,
};
use tokio::runtime::Runtime;

use super::{SmbDialect, AUTO_MAX_DIALECT, AUTO_MIN_DIALECT};

/// Chunk size for streaming reads and writes.
const IO_CHUNK_SIZE: usize = 64 * 1024;

/// SMB file system client built on the pure-Rust [`smb`] crate.
///
/// The client is synchronous from the caller's point of view: every
/// operation runs on the Tokio runtime supplied at construction through
/// [`Runtime::block_on`]. Consequently:
///
/// - the runtime must outlive the client;
/// - the client must not be driven from a task running on that same
///   runtime, because `block_on` would panic;
/// - readers passed to `create_file`/`append_file` are read on the thread
///   that executes the call.
///
/// Only SMB 2.0.2 through 3.1.1 are supported; [`SmbDialect::Nt1`] is
/// rejected at construction.
///
/// # Examples
///
/// ```no_run
/// use std::path::Path;
/// use std::sync::Arc;
///
/// use remotefs::RemoteFs;
/// use remotefs::fs::UnixPex;
/// use remotefs_smb::{SmbCredentials, SmbFs, SmbOptions};
/// use tokio::runtime::Runtime;
///
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let runtime = Arc::new(Runtime::new()?);
/// let mut client = SmbFs::try_new(
///     SmbCredentials::default()
///         .server("smb://localhost:3445")
///         .share("/temp")
///         .username("test")
///         .password("test")
///         .workgroup("pavao"),
///     SmbOptions::default(),
///     &runtime,
/// )?;
/// client.connect()?;
/// println!("Wrkdir: {}", client.pwd()?.display());
/// client.create_dir(Path::new("/cargo"), UnixPex::from(0o755))?;
/// client.change_dir(Path::new("/cargo"))?;
/// client.disconnect()?;
/// # Ok(())
/// # }
/// ```
#[cfg_attr(docsrs, doc(cfg(feature = "smb")))]
pub struct SmbFs {
    config: ClientConfig,
    credentials: ResolvedCredentials,
    client: Option<Client>,
    runtime: Arc<Runtime>,
    wrkdir: PathBuf,
}

impl std::fmt::Debug for SmbFs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SmbFs")
            .field("share", &self.credentials.share.to_string())
            .field("connected", &self.client.is_some())
            .field("wrkdir", &self.wrkdir)
            .finish_non_exhaustive()
    }
}

impl Drop for SmbFs {
    fn drop(&mut self) {
        let Some(client) = self.client.take() else {
            return;
        };
        let close = async move {
            let _ = client.close().await;
            drop(client);
        };
        if tokio::runtime::Handle::try_current().is_ok() {
            self.runtime.spawn(close);
        } else {
            self.runtime.block_on(close);
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
    /// use std::sync::Arc;
    ///
    /// use remotefs_smb::{SmbCredentials, SmbFs, SmbOptions};
    /// use tokio::runtime::Runtime;
    ///
    /// let runtime = Arc::new(Runtime::new()?);
    /// let _client = SmbFs::try_new(
    ///     SmbCredentials::default()
    ///         .server("server.example")
    ///         .share("documents"),
    ///     SmbOptions::default(),
    ///     &runtime,
    /// )?;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn try_new(
        credentials: SmbCredentials,
        options: SmbOptions,
        runtime: &Arc<Runtime>,
    ) -> RemoteResult<Self> {
        Self::try_new_with_dialect(
            credentials,
            options,
            AUTO_MIN_DIALECT,
            AUTO_MAX_DIALECT,
            runtime,
        )
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
    /// use std::sync::Arc;
    ///
    /// use remotefs_smb::{SmbCredentials, SmbDialect, SmbFs, SmbOptions};
    /// use tokio::runtime::Runtime;
    ///
    /// let runtime = Arc::new(Runtime::new()?);
    /// let _client = SmbFs::try_new_with_dialect(
    ///     SmbCredentials::default()
    ///         .server("server.example")
    ///         .share("documents"),
    ///     SmbOptions::default(),
    ///     SmbDialect::Smb300,
    ///     SmbDialect::Smb311,
    ///     &runtime,
    /// )?;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn try_new_with_dialect(
        credentials: SmbCredentials,
        options: SmbOptions,
        min_dialect: SmbDialect,
        max_dialect: SmbDialect,
        runtime: &Arc<Runtime>,
    ) -> RemoteResult<Self> {
        let credentials = credentials.resolve()?;
        let config =
            options::build_client_config(&options, credentials.port, min_dialect, max_dialect)?;
        Ok(Self {
            config,
            credentials,
            client: None,
            runtime: Arc::clone(runtime),
            wrkdir: PathBuf::from("/"),
        })
    }

    /// Returns the inner [`smb::Client`] while connected.
    pub fn client(&self) -> Option<&Client> {
        self.client.as_ref()
    }

    // -- private

    fn is_connected_flag(&self) -> bool {
        self.client.is_some()
    }

    fn connected_client(&self) -> RemoteResult<&Client> {
        self.client
            .as_ref()
            .ok_or_else(|| RemoteError::new(RemoteErrorType::NotConnected))
    }

    fn block_on<F: Future>(&self, future: F) -> F::Output {
        self.runtime.block_on(future)
    }

    /// Absolutizes `path` and returns it with the matching UNC path.
    fn unc(&self, path: &Path) -> (PathBuf, UncPath) {
        let (absolute, relative) = relative_unc_path(self.wrkdir.as_path(), path);
        (absolute, unc_for(&self.credentials.share, &relative))
    }

    async fn open(
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

    fn handle_of(resource: &Resource) -> Option<&ResourceHandle> {
        match resource {
            Resource::File(file) => Some(file.handle()),
            Resource::Directory(dir) => Some(dir.handle()),
            Resource::Pipe(pipe) => Some(pipe),
        }
    }

    async fn close(handle: &ResourceHandle) -> RemoteResult<()> {
        handle
            .close()
            .await
            .map_err(|e| smb_error(RemoteErrorType::ProtocolError, e))
    }

    async fn stat_unc(client: &Client, unc: &UncPath) -> RemoteResult<FileNetworkOpenInformation> {
        let args = FileCreateArgs::make_open_existing(
            FileAccessMask::new().with_file_read_attributes(true),
        );
        let resource = Self::open(client, unc, &args, RemoteErrorType::StatFailed).await?;
        let handle = match Self::handle_of(&resource) {
            Some(handle) => handle,
            None => {
                drop(resource);
                return Err(RemoteError::new_ex(
                    RemoteErrorType::StatFailed,
                    "not a file",
                ));
            }
        };
        let info = handle
            .query_info::<FileNetworkOpenInformation>()
            .await
            .map_err(|e| smb_error(RemoteErrorType::StatFailed, e));
        Self::close(handle).await.map_err(|e| {
            RemoteError::new_ex(RemoteErrorType::StatFailed, e.msg.unwrap_or_default())
        })?;
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
        let resource = Self::open(client, unc, &args, RemoteErrorType::CouldNotRemoveFile).await?;
        let handle = Self::handle_of(&resource).ok_or_else(|| {
            RemoteError::new_ex(RemoteErrorType::CouldNotRemoveFile, "not a file")
        })?;
        let result = handle
            .set_info(FileDispositionInformation::default())
            .await
            .map_err(|e| smb_error(RemoteErrorType::CouldNotRemoveFile, e));
        handle
            .close()
            .await
            .map_err(|e| smb_error(RemoteErrorType::CouldNotRemoveFile, e))?;
        result
    }

    /// Copies `reader` into `file` starting at `offset`; returns bytes written.
    async fn write_from(
        file: &smb::File,
        mut reader: Box<dyn Read + Send>,
        mut offset: u64,
    ) -> RemoteResult<u64> {
        let mut buffer = vec![0u8; IO_CHUNK_SIZE];
        let mut written = 0u64;
        loop {
            let read = reader
                .read(&mut buffer)
                .map_err(|e| RemoteError::new_ex(RemoteErrorType::IoError, e))?;
            if read == 0 {
                break;
            }
            let mut chunk = &buffer[..read];
            while !chunk.is_empty() {
                let n = file
                    .write_at(chunk, offset)
                    .await
                    .map_err(|e| smb_error(RemoteErrorType::IoError, e))?;
                if n == 0 {
                    return Err(RemoteError::new_ex(
                        RemoteErrorType::IoError,
                        "server accepted zero bytes",
                    ));
                }
                chunk = &chunk[n..];
                offset += n as u64;
                written += n as u64;
            }
        }
        Ok(written)
    }

    /// Copies `file` into `dest`; returns bytes read.
    async fn read_into(file: &smb::File, mut dest: Box<dyn Write + Send>) -> RemoteResult<u64> {
        let mut buffer = vec![0u8; IO_CHUNK_SIZE];
        let mut offset = 0u64;
        loop {
            let n = file
                .read_at(&mut buffer, offset)
                .await
                .map_err(|e| smb_error(RemoteErrorType::IoError, e))?;
            if n == 0 {
                break;
            }
            dest.write_all(&buffer[..n])
                .map_err(|e| RemoteError::new_ex(RemoteErrorType::IoError, e))?;
            offset += n as u64;
        }
        dest.flush()
            .map_err(|e| RemoteError::new_ex(RemoteErrorType::IoError, e))?;
        Ok(offset)
    }
}

impl RemoteFs for SmbFs {
    fn connect(&mut self) -> RemoteResult<Welcome> {
        if self.client.is_some() {
            return Err(RemoteError::new(RemoteErrorType::AlreadyConnected));
        }
        let config = self.config.clone();
        let credentials = self.credentials.clone();
        debug!("connecting to {}", credentials.share);
        let client = self.block_on(async move {
            let client = Client::new(config);
            client
                .share_connect(
                    &credentials.share,
                    &credentials.logon_name,
                    credentials.password,
                )
                .await
                .map_err(|e| smb_error(RemoteErrorType::ConnectionError, e))?;
            Ok::<Client, RemoteError>(client)
        })?;
        self.client = Some(client);
        self.wrkdir = PathBuf::from("/");
        Ok(Welcome::default())
    }

    fn disconnect(&mut self) -> RemoteResult<()> {
        let client = self
            .client
            .take()
            .ok_or_else(|| RemoteError::new(RemoteErrorType::NotConnected))?;
        debug!("disconnecting from {}", self.credentials.share);
        self.block_on(async move {
            let result = client
                .close()
                .await
                .map_err(|e| smb_error(RemoteErrorType::ConnectionError, e));
            drop(client);
            result
        })
    }

    fn is_connected(&mut self) -> bool {
        self.is_connected_flag()
    }

    fn pwd(&mut self) -> RemoteResult<PathBuf> {
        self.connected_client().map(|_| self.wrkdir.clone())
    }

    fn change_dir(&mut self, dir: &Path) -> RemoteResult<PathBuf> {
        self.connected_client()?;
        let (dir, _) = self.unc(dir);
        trace!("changing directory to {}", dir.display());
        if self.stat(dir.as_path())?.is_dir() {
            self.wrkdir = dir;
            debug!("new working directory: {}", self.wrkdir.display());
            Ok(self.wrkdir.clone())
        } else {
            error!("cannot enter directory {}. Not a directory", dir.display());
            Err(RemoteError::new_ex(
                RemoteErrorType::BadFile,
                "not a directory",
            ))
        }
    }

    fn list_dir(&mut self, path: &Path) -> RemoteResult<Vec<File>> {
        let client = self.connected_client()?;
        let (absolute, unc) = self.unc(path);
        trace!("listing files at {}", absolute.display());
        self.block_on(async {
            let args = FileCreateArgs::make_open_existing(
                DirAccessMask::new()
                    .with_list_directory(true)
                    .with_synchronize(true)
                    .into(),
            );
            let resource = Self::open(client, &unc, &args, RemoteErrorType::StatFailed).await?;
            let dir = match resource {
                Resource::Directory(dir) => Arc::new(dir),
                other => {
                    if let Some(handle) = Self::handle_of(&other) {
                        Self::close(handle).await?;
                    }
                    return Err(RemoteError::new_ex(
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
            let close_result = Self::close(dir.handle()).await.map_err(|e| {
                RemoteError::new_ex(RemoteErrorType::StatFailed, e.msg.unwrap_or_default())
            });
            drop(dir);
            match (entries, close_result) {
                (Err(err), _) => Err(err),
                (Ok(_), Err(err)) => Err(err),
                (Ok(entries), Ok(())) => Ok(entries),
            }
        })
    }

    fn stat(&mut self, path: &Path) -> RemoteResult<File> {
        let client = self.connected_client()?;
        let (absolute, unc) = self.unc(path);
        trace!("get stat for {}", absolute.display());
        let info = self.block_on(Self::stat_unc(client, &unc))?;
        Ok(open_info_to_file(absolute, &info))
    }

    fn setstat(&mut self, _path: &Path, _metadata: Metadata) -> RemoteResult<()> {
        Err(RemoteError::new(RemoteErrorType::UnsupportedFeature))
    }

    fn exists(&mut self, path: &Path) -> RemoteResult<bool> {
        trace!("checking if {} exists...", path.display());
        match self.stat(path) {
            Ok(_) => Ok(true),
            Err(RemoteError {
                kind: RemoteErrorType::StatFailed,
                ..
            }) => Ok(false),
            Err(err) => Err(err),
        }
    }

    fn remove_file(&mut self, path: &Path) -> RemoteResult<()> {
        let client = self.connected_client()?;
        let (absolute, unc) = self.unc(path);
        trace!("removing file {}", absolute.display());
        self.block_on(Self::delete_unc(
            client,
            &unc,
            CreateOptions::new().with_non_directory_file(true),
            FileAccessMask::new().with_delete(true),
        ))
    }

    fn remove_dir(&mut self, path: &Path) -> RemoteResult<()> {
        let client = self.connected_client()?;
        let (absolute, unc) = self.unc(path);
        trace!("removing directory at {}", absolute.display());
        self.block_on(Self::delete_unc(
            client,
            &unc,
            CreateOptions::new().with_directory_file(true),
            DirAccessMask::new().with_delete(true).into(),
        ))
    }

    fn create_dir(&mut self, path: &Path, _mode: UnixPex) -> RemoteResult<()> {
        self.connected_client()?;
        if self.exists(path)? {
            return Err(RemoteError::new(RemoteErrorType::DirectoryAlreadyExists));
        }
        let client = self.connected_client()?;
        let (absolute, unc) = self.unc(path);
        trace!("making directory at {}", absolute.display());
        self.block_on(async {
            let args = FileCreateArgs::make_create_new(
                FileAttributes::new().with_directory(true),
                CreateOptions::new().with_directory_file(true),
            );
            let resource =
                Self::open(client, &unc, &args, RemoteErrorType::FileCreateDenied).await?;
            match Self::handle_of(&resource) {
                Some(handle) => Self::close(handle).await,
                None => Ok(()),
            }
        })
    }

    fn symlink(&mut self, _path: &Path, _target: &Path) -> RemoteResult<()> {
        Err(RemoteError::new(RemoteErrorType::UnsupportedFeature))
    }

    fn copy(&mut self, _src: &Path, _dest: &Path) -> RemoteResult<()> {
        Err(RemoteError::new(RemoteErrorType::UnsupportedFeature))
    }

    fn mov(&mut self, src: &Path, dest: &Path) -> RemoteResult<()> {
        let client = self.connected_client()?;
        let (src_abs, src_unc) = self.unc(src);
        let (dest_abs, dest_rel) = relative_unc_path(self.wrkdir.as_path(), dest);
        trace!("moving {} to {}", src_abs.display(), dest_abs.display());
        self.block_on(async {
            let args = FileCreateArgs::make_open_existing(FileAccessMask::new().with_delete(true));
            let resource =
                Self::open(client, &src_unc, &args, RemoteErrorType::ProtocolError).await?;
            let handle = Self::handle_of(&resource)
                .ok_or_else(|| RemoteError::new_ex(RemoteErrorType::ProtocolError, "not a file"))?;
            let result = handle
                .set_info(FileRenameInformation {
                    replace_if_exists: false.into(),
                    root_directory: 0,
                    file_name: to_backslash_path(dest_rel.trim_matches(['/', '\\'])).into(),
                })
                .await
                .map_err(|e| smb_error(RemoteErrorType::ProtocolError, e));
            Self::close(handle).await?;
            result
        })
    }

    fn exec(&mut self, _cmd: &str) -> RemoteResult<(u32, String)> {
        Err(RemoteError::new(RemoteErrorType::UnsupportedFeature))
    }

    fn append_file(
        &mut self,
        path: &Path,
        _metadata: &Metadata,
        reader: Box<dyn Read + Send>,
    ) -> RemoteResult<u64> {
        let client = self.connected_client()?;
        let (absolute, unc) = self.unc(path);
        trace!("opening file at {} for append", absolute.display());
        let args = FileCreateArgs {
            disposition: CreateDisposition::OpenIf,
            attributes: FileAttributes::new(),
            options: CreateOptions::new().with_non_directory_file(true),
            desired_access: FileAccessMask::new()
                .with_generic_read(true)
                .with_generic_write(true),
        };
        self.block_on(async {
            let resource =
                Self::open(client, &unc, &args, RemoteErrorType::CouldNotOpenFile).await?;
            let file = match resource {
                Resource::File(file) => file,
                other => {
                    if let Some(handle) = Self::handle_of(&other) {
                        Self::close(handle).await?;
                    }
                    return Err(RemoteError::new_ex(
                        RemoteErrorType::CouldNotOpenFile,
                        "not a regular file",
                    ));
                }
            };
            let result = async {
                let offset = file
                    .get_len()
                    .await
                    .map_err(|e| smb_error(RemoteErrorType::IoError, e))?;
                Self::write_from(&file, reader, offset).await
            }
            .await;
            Self::close(&file).await?;
            result
        })
    }

    fn create_file(
        &mut self,
        path: &Path,
        _metadata: &Metadata,
        reader: Box<dyn Read + Send>,
    ) -> RemoteResult<u64> {
        let client = self.connected_client()?;
        let (absolute, unc) = self.unc(path);
        trace!("creating file at {}", absolute.display());
        let args = FileCreateArgs::make_overwrite(
            FileAttributes::new(),
            CreateOptions::new().with_non_directory_file(true),
        );
        self.block_on(async {
            let resource =
                Self::open(client, &unc, &args, RemoteErrorType::CouldNotOpenFile).await?;
            let file = match resource {
                Resource::File(file) => file,
                other => {
                    if let Some(handle) = Self::handle_of(&other) {
                        Self::close(handle).await?;
                    }
                    return Err(RemoteError::new_ex(
                        RemoteErrorType::CouldNotOpenFile,
                        "not a regular file",
                    ));
                }
            };
            let result = Self::write_from(&file, reader, 0).await;
            Self::close(&file).await?;
            result
        })
    }

    fn open_file(&mut self, path: &Path, dest: Box<dyn Write + Send>) -> RemoteResult<u64> {
        let client = self.connected_client()?;
        let (absolute, unc) = self.unc(path);
        trace!("opening file at {} for read", absolute.display());
        let args =
            FileCreateArgs::make_open_existing(FileAccessMask::new().with_generic_read(true));
        self.block_on(async {
            let resource =
                Self::open(client, &unc, &args, RemoteErrorType::CouldNotOpenFile).await?;
            let file = match resource {
                Resource::File(file) => file,
                other => {
                    if let Some(handle) = Self::handle_of(&other) {
                        Self::close(handle).await?;
                    }
                    return Err(RemoteError::new_ex(
                        RemoteErrorType::CouldNotOpenFile,
                        "not a regular file",
                    ));
                }
            };
            let result = Self::read_into(&file, dest).await;
            Self::close(&file).await?;
            result
        })
    }

    fn append(&mut self, _path: &Path, _metadata: &Metadata) -> RemoteResult<WriteStream> {
        Err(RemoteError::new(RemoteErrorType::UnsupportedFeature))
    }

    fn create(&mut self, _path: &Path, _metadata: &Metadata) -> RemoteResult<WriteStream> {
        Err(RemoteError::new(RemoteErrorType::UnsupportedFeature))
    }

    fn open(&mut self, _path: &Path) -> RemoteResult<ReadStream> {
        Err(RemoteError::new(RemoteErrorType::UnsupportedFeature))
    }
}

#[cfg(test)]
mod test {
    #[cfg(feature = "with-containers")]
    use std::io::Cursor;
    use std::sync::Arc;
    #[cfg(feature = "with-containers")]
    use std::time::Duration;

    use pretty_assertions::assert_eq;
    #[cfg(feature = "with-containers")]
    use serial_test::serial;
    use tokio::runtime::Runtime;

    use super::*;

    #[test]
    fn should_reject_inverted_dialect_bounds() {
        let runtime = Arc::new(Runtime::new().unwrap());
        let result = SmbFs::try_new_with_dialect(
            test_credentials(),
            SmbOptions::default(),
            SmbDialect::Smb311,
            SmbDialect::Smb202,
            &runtime,
        );
        assert_eq!(
            result.expect_err("inverted bounds must fail").kind,
            RemoteErrorType::BadAddress,
        );
    }

    #[test]
    fn should_reject_nt1_dialect() {
        let runtime = Arc::new(Runtime::new().unwrap());
        let result = SmbFs::try_new_with_dialect(
            test_credentials(),
            SmbOptions::default(),
            SmbDialect::Nt1,
            SmbDialect::Smb311,
            &runtime,
        );
        assert_eq!(result.err().unwrap().kind, RemoteErrorType::BadAddress);
    }

    #[test]
    fn should_default_to_secure_auto_dialect_bounds() {
        let runtime = Arc::new(Runtime::new().unwrap());
        let client = SmbFs::try_new(test_credentials(), SmbOptions::default(), &runtime).unwrap();
        assert_eq!(
            client.config.connection.min_dialect,
            Some(smb::Dialect::Smb0202)
        );
        assert_eq!(
            client.config.connection.max_dialect,
            Some(smb::Dialect::Smb0311)
        );
        assert!(!client.is_connected_flag());
        assert!(client.client().is_none());
    }

    #[test]
    fn should_reject_bad_credentials_at_construction() {
        let runtime = Arc::new(Runtime::new().unwrap());
        let result = SmbFs::try_new(
            SmbCredentials::default().server("smb://"),
            SmbOptions::default(),
            &runtime,
        );
        assert_eq!(result.err().unwrap().kind, RemoteErrorType::BadAddress);
    }

    #[test]
    fn should_fail_when_not_connected() {
        let runtime = Arc::new(Runtime::new().unwrap());
        let mut client =
            SmbFs::try_new(test_credentials(), SmbOptions::default(), &runtime).unwrap();
        assert_eq!(
            client.pwd().unwrap_err().kind,
            RemoteErrorType::NotConnected
        );
        assert_eq!(
            client.stat(Path::new("/")).unwrap_err().kind,
            RemoteErrorType::NotConnected
        );
        assert!(!client.is_connected());
    }

    fn is_send<T: Send>(_send: T) {}

    fn is_sync<T: Sync>(_sync: T) {}

    #[test]
    fn test_should_be_sync() {
        let runtime = Arc::new(Runtime::new().unwrap());
        let client = SmbFs::try_new(test_credentials(), SmbOptions::default(), &runtime).unwrap();
        is_sync(client);
    }

    #[test]
    fn test_should_be_send() {
        let runtime = Arc::new(Runtime::new().unwrap());
        let client = SmbFs::try_new(test_credentials(), SmbOptions::default(), &runtime).unwrap();
        is_send(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_connect_and_disconnect() {
        crate::mock::logger();
        let mut client = init_client();
        assert!(client.is_connected());
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_change_directory() {
        crate::mock::logger();
        let mut client = init_client();
        let pwd = client.pwd().ok().unwrap();
        assert!(client.change_dir(Path::new("/cargo-test")).is_ok());
        assert!(client.change_dir(pwd.as_path()).is_ok());
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_not_change_directory() {
        crate::mock::logger();
        let mut client = init_client();
        assert!(client
            .change_dir(Path::new("/tmp/sdfghjuireghiuergh/useghiyuwegh"))
            .is_err());
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_print_working_directory() {
        crate::mock::logger();
        let mut client = init_client();
        assert!(client.pwd().is_ok());
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_stat_directory() {
        crate::mock::logger();
        let mut client = init_client();
        let entry = client.stat(Path::new("/cargo-test")).ok().unwrap();
        assert!(entry.is_dir());
        assert_eq!(entry.path(), Path::new("/cargo-test"));
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_not_stat_file() {
        crate::mock::logger();
        let mut client = init_client();
        let p = Path::new("a.sh");
        assert!(client.stat(p).is_err());
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_tell_whether_directory_exists() {
        crate::mock::logger();
        let mut client = init_client();
        assert!(client.exists(Path::new("/cargo-test/")).ok().unwrap());
        assert!(!client.exists(Path::new("/cargo-test/b.txt")).ok().unwrap());
        assert!(!client.exists(Path::new("/tmp/ppppp/bhhrhu")).ok().unwrap());
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_list_empty_dir() {
        crate::mock::logger();
        let mut client = init_client();
        let files = client.list_dir(Path::new("/cargo-test/")).ok().unwrap();
        assert!(files.is_empty());
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_not_list_dir() {
        crate::mock::logger();
        let mut client = init_client();
        assert!(client.list_dir(Path::new("/tmp/auhhfh/hfhjfhf/")).is_err());
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_append_to_file() {
        crate::mock::logger();
        let mut client = init_client();
        let p = Path::new("/cargo-test/a.txt");
        let file_data = "test data\n";
        let reader = Cursor::new(file_data.as_bytes());
        assert_eq!(
            client
                .create_file(p, &Metadata::default().size(10), Box::new(reader))
                .ok()
                .unwrap(),
            10
        );
        assert_eq!(client.stat(p).ok().unwrap().metadata().size, 10);
        let file_data = "Hello, world!\n";
        let reader = Cursor::new(file_data.as_bytes());
        assert_eq!(
            client
                .append_file(p, &Metadata::default().size(14), Box::new(reader))
                .ok()
                .unwrap(),
            14
        );
        assert_eq!(client.stat(p).ok().unwrap().metadata().size, 24);
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_not_append_to_file() {
        crate::mock::logger();
        let mut client = init_client();
        let p = Path::new("/tmp/aaaaaaa/hbbbbb/a.txt");
        let file_data = "Hello, world!\n";
        let reader = Cursor::new(file_data.as_bytes());
        assert!(client
            .append_file(p, &Metadata::default(), Box::new(reader))
            .is_err());
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_not_copy_file() {
        crate::mock::logger();
        let mut client = init_client();
        let p = Path::new("a.txt");
        let file_data = "test data\n";
        let reader = Cursor::new(file_data.as_bytes());
        assert!(client
            .create_file(p, &Metadata::default(), Box::new(reader))
            .is_ok());
        assert!(client.copy(p, Path::new("aaa/bbbb/ccc/b.txt")).is_err());
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_create_directory() {
        crate::mock::logger();
        let mut client = init_client();
        assert!(client
            .create_dir(Path::new("/cargo-test/mydir"), UnixPex::from(0o755))
            .is_ok());
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_not_create_directory_cause_already_exists() {
        crate::mock::logger();
        let mut client = init_client();
        assert!(client
            .create_dir(Path::new("/cargo-test/mydir"), UnixPex::from(0o755))
            .is_ok());
        assert_eq!(
            client
                .create_dir(Path::new("/cargo-test/mydir"), UnixPex::from(0o755))
                .err()
                .unwrap()
                .kind,
            RemoteErrorType::DirectoryAlreadyExists
        );
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_not_create_directory() {
        crate::mock::logger();
        let mut client = init_client();
        assert!(client
            .create_dir(
                Path::new("/tmp/werfgjwerughjwurih/iwerjghiwgui"),
                UnixPex::from(0o755)
            )
            .is_err());
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_create_file() {
        crate::mock::logger();
        let mut client = init_client();
        let p = Path::new("/cargo-test/a.txt");
        let file_data = "test data\n";
        let reader = Cursor::new(file_data.as_bytes());
        assert_eq!(
            client
                .create_file(p, &Metadata::default().size(10), Box::new(reader))
                .ok()
                .unwrap(),
            10
        );
        assert_eq!(client.stat(p).ok().unwrap().metadata().size, 10);
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_not_create_file() {
        crate::mock::logger();
        let mut client = init_client();
        let p = Path::new("/tmp/ahsufhauiefhuiashf/hfhfhfhf");
        let file_data = "test data\n";
        let reader = Cursor::new(file_data.as_bytes());
        assert!(client
            .create_file(p, &Metadata::default(), Box::new(reader))
            .is_err());
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_not_exec_command() {
        crate::mock::logger();
        let mut client = init_client();
        assert!(client.exec("echo 5").is_err());
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_tell_whether_file_exists() {
        crate::mock::logger();
        let mut client = init_client();
        let p = Path::new("/cargo-test/a.txt");
        let file_data = "test data\n";
        let reader = Cursor::new(file_data.as_bytes());
        assert!(client
            .create_file(p, &Metadata::default(), Box::new(reader))
            .is_ok());
        assert!(client.exists(p).ok().unwrap());
        assert!(!client.exists(Path::new("/cargo-test/b.txt")).ok().unwrap());
        assert!(!client.exists(Path::new("/tmp/ppppp/bhhrhu")).ok().unwrap());
        assert!(client.exists(Path::new("/cargo-test/")).ok().unwrap());
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_list_dir() {
        crate::mock::logger();
        let mut client = init_client();
        let wrkdir = client.pwd().ok().unwrap();
        let p = Path::new("/cargo-test/a.txt");
        let file_data = "test data\n";
        let reader = Cursor::new(file_data.as_bytes());
        assert_eq!(
            client
                .append_file(p, &Metadata::default().size(10), Box::new(reader))
                .unwrap(),
            10
        );
        let file = client
            .list_dir(Path::new("/cargo-test/"))
            .ok()
            .unwrap()
            .first()
            .unwrap()
            .clone();
        assert_eq!(file.name().as_str(), "a.txt");
        let mut expected_path = wrkdir;
        expected_path.push(p);
        assert_eq!(file.path.as_path(), expected_path.as_path());
        assert_eq!(file.extension().as_deref().unwrap(), "txt");
        assert_eq!(file.metadata.size, 10);
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_move_file() {
        crate::mock::logger();
        let mut client = init_client();
        let p = Path::new("/cargo-test/a.txt");
        let file_data = "test data\n";
        let reader = Cursor::new(file_data.as_bytes());
        assert!(client
            .create_file(p, &Metadata::default(), Box::new(reader))
            .is_ok());
        let dest = Path::new("/cargo-test/b.txt");
        assert!(client.mov(p, dest).is_ok());
        assert!(!client.exists(p).ok().unwrap());
        assert!(client.exists(dest).ok().unwrap());
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_not_move_file() {
        crate::mock::logger();
        let mut client = init_client();
        let p = Path::new("a.txt");
        let file_data = "test data\n";
        let reader = Cursor::new(file_data.as_bytes());
        assert!(client
            .create_file(p, &Metadata::default(), Box::new(reader))
            .is_ok());
        let dest = Path::new("/tmp/wuefhiwuerfh/whjhh/b.txt");
        assert!(client.mov(p, dest).is_err());
        assert!(client.mov(dest, p).is_err());
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_open_file() {
        crate::mock::logger();
        let mut client = init_client();
        let p = Path::new("/cargo-test/a.txt");
        let file_data = "test data\n";
        let reader = Cursor::new(file_data.as_bytes());
        assert!(client
            .create_file(p, &Metadata::default().size(10), Box::new(reader))
            .is_ok());
        let buffer: Box<dyn Write + Send> = Box::new(Vec::with_capacity(512));
        assert_eq!(client.open_file(p, buffer).ok().unwrap(), 10);
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_not_open_file() {
        crate::mock::logger();
        let mut client = init_client();
        let buffer: Box<dyn Write + Send> = Box::new(Vec::with_capacity(512));
        assert!(client
            .open_file(Path::new("/tmp/aashafb/hhh"), buffer)
            .is_err());
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_remove_dir_all() {
        crate::mock::logger();
        let mut client = init_client();
        let dir_path = Path::new("/cargo-test/test");
        assert!(client.create_dir(dir_path, UnixPex::from(0o775)).is_ok());
        let file_path = Path::new("/cargo-test/test/a.txt");
        let file_data = "test data\n";
        let reader = Cursor::new(file_data.as_bytes());
        assert!(client
            .create_file(file_path, &Metadata::default(), Box::new(reader))
            .is_ok());
        assert!(client.remove_dir_all(dir_path).is_ok());
        assert!(!client.exists(dir_path).ok().unwrap());
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_not_remove_dir_all() {
        crate::mock::logger();
        let mut client = init_client();
        assert!(client
            .remove_dir_all(Path::new("/tmp/aaaaaa/asuhi"))
            .is_err());
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_remove_dir() {
        crate::mock::logger();
        let mut client = init_client();
        let dir_path = Path::new("/cargo-test/test");
        assert!(client.create_dir(dir_path, UnixPex::from(0o775)).is_ok());
        assert!(client.remove_dir(dir_path).is_ok());
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_not_remove_dir() {
        crate::mock::logger();
        let mut client = init_client();
        let dir_path = Path::new("/cargo-test/test");
        assert!(client.create_dir(dir_path, UnixPex::from(0o775)).is_ok());
        let file_path = Path::new("/cargo-test/test/a.txt");
        let file_data = "test data\n";
        let reader = Cursor::new(file_data.as_bytes());
        assert!(client
            .create_file(file_path, &Metadata::default(), Box::new(reader))
            .is_ok());
        assert!(client.remove_dir(dir_path).is_err());
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_remove_file() {
        crate::mock::logger();
        let mut client = init_client();
        let p = Path::new("/cargo-test/a.txt");
        let file_data = "test data\n";
        let reader = Cursor::new(file_data.as_bytes());
        assert!(client
            .create_file(p, &Metadata::default(), Box::new(reader))
            .is_ok());
        assert!(client.remove_file(p).is_ok());
        assert!(!client.exists(p).ok().unwrap());
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_not_setstat_file() {
        crate::mock::logger();
        let mut client = init_client();
        let p = Path::new("bbbbb/cccc/a.sh");
        assert!(client
            .setstat(
                p,
                Metadata {
                    accessed: None,
                    created: None,
                    file_type: remotefs::fs::FileType::File,
                    gid: Some(1),
                    mode: Some(UnixPex::from(0o755)),
                    modified: None,
                    size: 7,
                    symlink: None,
                    uid: Some(1),
                }
            )
            .is_err());
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_stat_file() {
        crate::mock::logger();
        let mut client = init_client();
        let p = Path::new("/cargo-test/a.sh");
        let file_data = "echo 5\n";
        let reader = Cursor::new(file_data.as_bytes());
        assert!(client
            .create_file(p, &Metadata::default().size(7), Box::new(reader))
            .is_ok());
        let entry = client.stat(p).ok().unwrap();
        assert_eq!(entry.name(), "a.sh");
        let mut expected_path = client.pwd().ok().unwrap();
        expected_path.push("/cargo-test/a.sh");
        assert_eq!(entry.path(), expected_path.as_path());
        let meta = entry.metadata();
        assert_eq!(meta.size, 7);
        assert!(meta.modified.is_some());
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_not_make_symlink() {
        crate::mock::logger();
        let mut client = init_client();
        let p = Path::new("/cargo-test/a.sh");
        let symlink = Path::new("/cargo-test/b.sh");
        assert!(client.symlink(symlink, p).is_err());
        finalize_client(client);
    }

    fn test_credentials() -> SmbCredentials {
        SmbCredentials::default()
            .server("smb://localhost:3445")
            .share("/temp")
            .username("test")
            .password("test")
            .workgroup("pavao")
    }

    #[cfg(feature = "with-containers")]
    fn init_client() -> SmbFs {
        let runtime = Arc::new(Runtime::new().unwrap());
        let mut client =
            SmbFs::try_new(test_credentials(), SmbOptions::default(), &runtime).unwrap();
        assert!(client.connect().is_ok());
        // Make the test directory over SMB instead of on the host filesystem.
        let _ = client.remove_dir_all(Path::new("/cargo-test"));
        client
            .create_dir(Path::new("/cargo-test"), UnixPex::from(0o755))
            .unwrap();
        client
    }

    #[cfg(feature = "with-containers")]
    fn finalize_client(mut client: SmbFs) {
        let _ = client.remove_dir_all(Path::new("/cargo-test"));
        assert!(client.disconnect().is_ok());
        std::thread::sleep(Duration::from_secs(1));
        drop(client);
    }
}
