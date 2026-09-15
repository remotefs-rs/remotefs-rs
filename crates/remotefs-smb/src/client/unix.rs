//! # UNIX client
//!
//! UNIX implementation of Smb fs client

// -- exports
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use libc::mode_t;
pub use pavao::{
    SmbClient, SmbCredentials as PavaoSmbCredentials,
    SmbEncryptionLevel as PavaoSmbEncryptionLevel, SmbOptions as PavaoSmbOptions,
    SmbShareMode as PavaoSmbShareMode,
};
use pavao::{SmbDirentType, SmbMode, SmbOpenOptions};
use remotefs::fs::{
    Capabilities, ExecOutput, File, ReadOptions, ReadStream, SetMetadata, UnixPex, WriteOptions,
    WriteStream,
};
use remotefs::{RemoteError, RemoteErrorType, RemoteFs, RemoteResult};

use super::{SmbDialect, AUTO_MAX_DIALECT, AUTO_MIN_DIALECT};
use crate::utils::path::SharePath;
use crate::utils::smb as smb_utils;

impl From<SmbDialect> for pavao::SmbDialect {
    fn from(value: SmbDialect) -> Self {
        match value {
            SmbDialect::Nt1 => Self::Nt1,
            SmbDialect::Smb202 => Self::Smb202,
            SmbDialect::Smb210 => Self::Smb210,
            SmbDialect::Smb300 => Self::Smb300,
            SmbDialect::Smb302 => Self::Smb302,
            SmbDialect::Smb311 => Self::Smb311,
        }
    }
}

/// Wraps a pavao error, refining the kind from the underlying I/O error.
pub(crate) fn pavao_error(kind: RemoteErrorType, error: pavao::SmbError) -> RemoteError {
    let kind = match &error {
        pavao::SmbError::Io(io) => match io.kind() {
            io::ErrorKind::NotFound | io::ErrorKind::NotADirectory => {
                RemoteErrorType::NoSuchFileOrDirectory
            }
            io::ErrorKind::PermissionDenied => RemoteErrorType::PermissionDenied,
            io::ErrorKind::AlreadyExists => RemoteErrorType::AlreadyExists,
            io::ErrorKind::DirectoryNotEmpty => RemoteErrorType::DirectoryNotEmpty,
            io::ErrorKind::ConnectionRefused
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::NotConnected => RemoteErrorType::ConnectionError,
            _ => kind,
        },
        _ => kind,
    };
    RemoteError::with_source(kind, error)
}

/// SMB file system client backed by pavao (libsmbclient).
///
/// This is a blocking [`RemoteFs`] client. Every path must be absolute and
/// rooted at the share (`/`). Streamed transfers are not offered; use the
/// one-shot `read_file`, `write_file`, and `append_file` methods, which honor
/// read offsets and lengths natively. `WriteOptions::modified` is ignored
/// because pavao does not expose a portable timestamp operation.
///
/// # Examples
///
/// ```no_run
/// use std::io::Cursor;
/// use std::path::Path;
///
/// use remotefs::RemoteFs;
/// use remotefs::fs::WriteOptions;
/// use remotefs_smb::{PavaoSmbCredentials, PavaoSmbFs, PavaoSmbOptions};
///
/// let mut client = PavaoSmbFs::try_new(
///     PavaoSmbCredentials::default()
///         .server("smb://localhost:3445")
///         .share("/temp")
///         .username("test")
///         .password("test")
///         .workgroup("pavao"),
///     PavaoSmbOptions::default(),
/// )?;
/// client.connect()?;
/// client.create_dir(Path::new("/cargo"), None)?;
/// let mut source = Cursor::new(b"hello".to_vec());
/// client.write_file(
///     Path::new("/cargo/hello.txt"),
///     &WriteOptions::default(),
///     &mut source,
/// )?;
/// client.disconnect()?;
/// # Ok::<(), remotefs::RemoteError>(())
/// ```
pub struct PavaoSmbFs {
    client: SmbClient,
    connected: bool,
}

impl PavaoSmbFs {
    /// Tries to create an SMB client with secure automatic dialect bounds.
    ///
    /// Automatic negotiation is limited to SMB2 through SMB3.1.1 and excludes
    /// the deprecated SMB1/CIFS `NT1` dialect.
    ///
    /// # Errors
    ///
    /// Returns [`RemoteErrorType::BadAddress`] if Pavao cannot initialize the
    /// SMB context.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use remotefs_smb::{PavaoSmbCredentials, PavaoSmbFs, PavaoSmbOptions};
    ///
    /// let _client = PavaoSmbFs::try_new(
    ///     PavaoSmbCredentials::default()
    ///         .server("smb://server.example")
    ///         .share("/documents"),
    ///     PavaoSmbOptions::default(),
    /// )?;
    /// # Ok::<(), remotefs::RemoteError>(())
    /// ```
    pub fn try_new(
        credentials: PavaoSmbCredentials,
        options: PavaoSmbOptions,
    ) -> RemoteResult<Self> {
        Self::try_new_with_dialect(credentials, options, AUTO_MIN_DIALECT, AUTO_MAX_DIALECT)
    }

    /// Tries to create an SMB client with inclusive protocol dialect bounds.
    ///
    /// The client applies `min_dialect` and `max_dialect` to Pavao after
    /// preserving all other settings in `options`. An inverted range is
    /// rejected. Selecting [`SmbDialect::Nt1`] enables deprecated SMB1/CIFS
    /// negotiation and should be reserved for legacy devices that require it.
    ///
    /// # Errors
    ///
    /// Returns [`RemoteErrorType::BadAddress`] if the bounds are invalid or
    /// Pavao cannot initialize the SMB context.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use remotefs_smb::{PavaoSmbCredentials, PavaoSmbFs, PavaoSmbOptions, SmbDialect};
    ///
    /// let _client = PavaoSmbFs::try_new_with_dialect(
    ///     PavaoSmbCredentials::default()
    ///         .server("smb://server.example")
    ///         .share("/documents"),
    ///     PavaoSmbOptions::default(),
    ///     SmbDialect::Smb202,
    ///     SmbDialect::Smb210,
    /// )?;
    /// # Ok::<(), remotefs::RemoteError>(())
    /// ```
    pub fn try_new_with_dialect(
        credentials: PavaoSmbCredentials,
        options: PavaoSmbOptions,
        min_dialect: SmbDialect,
        max_dialect: SmbDialect,
    ) -> RemoteResult<Self> {
        let options = options
            .min_protocol(min_dialect.into())
            .max_protocol(max_dialect.into());
        Ok(Self {
            client: SmbClient::new(credentials, options)
                .map_err(|error| pavao_error(RemoteErrorType::BadAddress, error))?,
            connected: false,
        })
    }

    /// Returns a reference to the inner `pavao::SmbClient`.
    pub fn client(&self) -> &SmbClient {
        &self.client
    }

    /// Returns a mutable reference to the inner `pavao::SmbClient`.
    pub fn client_mut(&mut self) -> &mut SmbClient {
        &mut self.client
    }

    fn check_connected(&self) -> RemoteResult<()> {
        if self.connected {
            Ok(())
        } else {
            Err(RemoteError::new(RemoteErrorType::NotConnected))
        }
    }

    fn uri(&self, path: &Path) -> RemoteResult<(PathBuf, String)> {
        let share = SharePath::parse(path)?;
        self.check_connected()?;
        let absolute = share.to_path_buf();
        let uri = absolute.to_string_lossy().into_owned();
        Ok((absolute, uri))
    }

    fn uri_pair(
        &self,
        src: &Path,
        dest: &Path,
    ) -> RemoteResult<((PathBuf, String), (PathBuf, String))> {
        let src = SharePath::parse(src)?.to_path_buf();
        let dest = SharePath::parse(dest)?.to_path_buf();
        self.check_connected()?;
        Ok((
            (src.clone(), src.to_string_lossy().into_owned()),
            (dest.clone(), dest.to_string_lossy().into_owned()),
        ))
    }

    fn open_for_write(
        &self,
        uri: &str,
        opts: &WriteOptions,
        append: bool,
    ) -> RemoteResult<pavao::SmbFile<'_>> {
        let mode = u32::from(opts.mode.unwrap_or_else(|| UnixPex::from(0o644))) as mode_t;
        let options = SmbOpenOptions::default()
            .create(true)
            .write(true)
            .append(append)
            .truncate(!append)
            .mode(mode);
        self.client
            .open_with(uri, options)
            .map_err(|error| pavao_error(RemoteErrorType::CouldNotOpenFile, error))
    }

    fn copy_into(mut file: pavao::SmbFile<'_>, src: &mut (dyn Read + Send)) -> RemoteResult<u64> {
        let copied = io::copy(src, &mut file).map_err(RemoteError::from)?;
        file.flush().map_err(RemoteError::from)?;
        Ok(copied)
    }

    fn unsupported() -> RemoteError {
        RemoteError::new(RemoteErrorType::UnsupportedFeature)
    }
}

impl RemoteFs for PavaoSmbFs {
    fn connect(&mut self) -> RemoteResult<()> {
        if self.connected {
            return Err(RemoteError::new(RemoteErrorType::AlreadyConnected));
        }
        trace!("checking connection...");
        self.client
            .get_user()
            .map_err(|error| pavao_error(RemoteErrorType::ConnectionError, error))?;
        self.connected = true;
        debug!("connected");
        Ok(())
    }

    fn disconnect(&mut self) -> RemoteResult<()> {
        self.check_connected()?;
        self.connected = false;
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.connected
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities::APPEND
            | Capabilities::RANGE_READ
            | Capabilities::SET_METADATA
            | Capabilities::POSIX_MODE
    }

    fn list_dir(&self, path: &Path) -> RemoteResult<Vec<File>> {
        let (absolute, uri) = self.uri(path)?;
        trace!("listing files at {uri}");
        let dirents = self
            .client
            .list_dir(uri.as_str())
            .map_err(|error| pavao_error(RemoteErrorType::StatFailed, error))?;
        let mut entries = Vec::with_capacity(dirents.len());
        for dirent in dirents {
            if dirent.get_type() != SmbDirentType::File && dirent.get_type() != SmbDirentType::Dir {
                continue;
            }
            let child = absolute.join(dirent.name());
            entries.push(self.stat(&child)?);
        }
        Ok(entries)
    }

    fn stat(&self, path: &Path) -> RemoteResult<File> {
        let (absolute, uri) = self.uri(path)?;
        trace!("get stat for {uri}");
        self.client
            .stat(uri.as_str())
            .map_err(|error| pavao_error(RemoteErrorType::StatFailed, error))
            .map(|stat| smb_utils::smbstat_to_file(absolute, stat))
    }

    fn exists(&self, path: &Path) -> RemoteResult<bool> {
        match self.stat(path) {
            Ok(_) => Ok(true),
            Err(error) if error.kind() == RemoteErrorType::NoSuchFileOrDirectory => Ok(false),
            Err(error) => Err(error),
        }
    }

    fn set_metadata(&self, path: &Path, metadata: &SetMetadata) -> RemoteResult<()> {
        let (_, uri) = self.uri(path)?;
        if metadata.uid.is_some()
            || metadata.gid.is_some()
            || metadata.accessed.is_some()
            || metadata.modified.is_some()
        {
            return Err(Self::unsupported());
        }
        if let Some(mode) = metadata.mode {
            trace!("chmod {uri} to {mode:?}");
            self.client
                .chmod(uri, SmbMode::from(u32::from(mode) as mode_t))
                .map_err(|error| pavao_error(RemoteErrorType::PermissionDenied, error))?;
        }
        Ok(())
    }

    fn create_dir(&self, path: &Path, mode: Option<UnixPex>) -> RemoteResult<()> {
        let (_, uri) = self.uri(path)?;
        trace!("making directory at {uri}");
        let mode = u32::from(mode.unwrap_or_else(|| UnixPex::from(0o755))) as mode_t;
        self.client
            .mkdir(uri, SmbMode::from(mode))
            .map_err(|error| pavao_error(RemoteErrorType::FileCreateDenied, error))
    }

    fn remove_file(&self, path: &Path) -> RemoteResult<()> {
        let (_, uri) = self.uri(path)?;
        trace!("removing file {uri}");
        self.client
            .unlink(uri)
            .map_err(|error| pavao_error(RemoteErrorType::CouldNotRemoveFile, error))
    }

    fn remove_dir(&self, path: &Path) -> RemoteResult<()> {
        let (_, uri) = self.uri(path)?;
        trace!("removing directory at {uri}");
        self.client
            .rmdir(uri)
            .map_err(|error| pavao_error(RemoteErrorType::CouldNotRemoveFile, error))
    }

    fn rename(&self, src: &Path, dest: &Path) -> RemoteResult<()> {
        let ((_, src), (_, dest)) = self.uri_pair(src, dest)?;
        trace!("moving {src} to {dest}");
        self.client
            .rename(src, dest)
            .map_err(|error| pavao_error(RemoteErrorType::ProtocolError, error))
    }

    fn copy(&self, _src: &Path, _dest: &Path) -> RemoteResult<()> {
        Err(Self::unsupported())
    }

    fn symlink(&self, _path: &Path, _target: &Path) -> RemoteResult<()> {
        Err(Self::unsupported())
    }

    fn open(&self, _path: &Path, _opts: &ReadOptions) -> RemoteResult<ReadStream> {
        Err(Self::unsupported())
    }

    fn create(&self, _path: &Path, _opts: &WriteOptions) -> RemoteResult<WriteStream> {
        Err(Self::unsupported())
    }

    fn append(&self, _path: &Path, _opts: &WriteOptions) -> RemoteResult<WriteStream> {
        Err(Self::unsupported())
    }

    fn read_file(
        &self,
        path: &Path,
        opts: &ReadOptions,
        dest: &mut (dyn Write + Send),
    ) -> RemoteResult<u64> {
        let (_, uri) = self.uri(path)?;
        trace!("opening file at {uri} for read");
        let mut file = self
            .client
            .open_with(uri, SmbOpenOptions::default().read(true))
            .map_err(|error| pavao_error(RemoteErrorType::CouldNotOpenFile, error))?;
        if let Some(offset) = opts.offset {
            file.seek(SeekFrom::Start(offset))
                .map_err(RemoteError::from)?;
        }
        let copied = match opts.length {
            Some(length) => io::copy(&mut (&mut file).take(length), dest),
            None => io::copy(&mut file, dest),
        }
        .map_err(RemoteError::from)?;
        dest.flush().map_err(RemoteError::from)?;
        Ok(copied)
    }

    fn write_file(
        &self,
        path: &Path,
        opts: &WriteOptions,
        src: &mut (dyn Read + Send),
    ) -> RemoteResult<u64> {
        let (_, uri) = self.uri(path)?;
        trace!("creating file at {uri}");
        let file = self.open_for_write(&uri, opts, false)?;
        Self::copy_into(file, src)
    }

    fn append_file(
        &self,
        path: &Path,
        opts: &WriteOptions,
        src: &mut (dyn Read + Send),
    ) -> RemoteResult<u64> {
        let (_, uri) = self.uri(path)?;
        trace!("opening file at {uri} for append");
        let file = self.open_for_write(&uri, opts, true)?;
        Self::copy_into(file, src)
    }

    fn exec(&self, _cmd: &str) -> RemoteResult<ExecOutput> {
        Err(Self::unsupported())
    }
}

#[cfg(test)]
mod test {
    #[cfg(feature = "with-containers")]
    use std::io::Cursor;
    #[cfg(feature = "with-containers")]
    use std::time::Duration;

    use pretty_assertions::assert_eq;
    use remotefs::fs::{Capabilities, ReadOptions};
    #[cfg(feature = "with-containers")]
    use remotefs::fs::{SetMetadata, WriteOptions};
    use remotefs::{RemoteErrorType, RemoteFs};
    use serial_test::serial;

    use super::*;

    fn test_credentials() -> PavaoSmbCredentials {
        PavaoSmbCredentials::default()
            .server("smb://localhost:3445")
            .share("/temp")
            .username("test")
            .password("test")
            .workgroup("pavao")
    }

    fn test_options() -> PavaoSmbOptions {
        PavaoSmbOptions::default()
            .case_sensitive(true)
            .one_share_per_server(true)
    }

    #[test]
    fn should_convert_all_dialects_to_pavao() {
        for (ours, theirs) in [
            (SmbDialect::Nt1, pavao::SmbDialect::Nt1),
            (SmbDialect::Smb202, pavao::SmbDialect::Smb202),
            (SmbDialect::Smb210, pavao::SmbDialect::Smb210),
            (SmbDialect::Smb300, pavao::SmbDialect::Smb300),
            (SmbDialect::Smb302, pavao::SmbDialect::Smb302),
            (SmbDialect::Smb311, pavao::SmbDialect::Smb311),
        ] {
            assert_eq!(pavao::SmbDialect::from(ours), theirs);
        }
    }

    #[test]
    #[serial]
    fn should_reject_inverted_dialect_bounds() {
        let result = PavaoSmbFs::try_new_with_dialect(
            test_credentials(),
            PavaoSmbOptions::default(),
            SmbDialect::Smb311,
            SmbDialect::Nt1,
        );
        assert_eq!(
            result.err().expect("inverted bounds must fail").kind(),
            RemoteErrorType::BadAddress,
        );
    }

    #[test]
    #[serial]
    fn should_default_to_secure_auto_dialect_bounds() {
        let default_client =
            PavaoSmbFs::try_new(test_credentials(), PavaoSmbOptions::default()).unwrap();
        let explicit = PavaoSmbFs::try_new_with_dialect(
            test_credentials(),
            PavaoSmbOptions::default(),
            SmbDialect::Smb202,
            SmbDialect::Smb311,
        );
        assert!(explicit.is_ok());
        drop(default_client);
    }

    #[test]
    #[serial]
    fn should_advertise_capabilities() {
        let client = PavaoSmbFs::try_new(test_credentials(), test_options()).unwrap();
        let capabilities = client.capabilities();
        assert!(capabilities.contains(Capabilities::APPEND));
        assert!(capabilities.contains(Capabilities::RANGE_READ));
        assert!(capabilities.contains(Capabilities::SET_METADATA));
        assert!(capabilities.contains(Capabilities::POSIX_MODE));
        assert!(!capabilities.contains(Capabilities::STREAM_READ));
        assert!(!capabilities.contains(Capabilities::STREAM_WRITE));
        assert!(!capabilities.contains(Capabilities::COPY));
        assert!(!capabilities.contains(Capabilities::SYMLINK));
        assert!(!capabilities.contains(Capabilities::EXEC));
    }

    #[test]
    #[serial]
    fn should_fail_when_not_connected_and_on_relative_paths() {
        let client = PavaoSmbFs::try_new(test_credentials(), test_options()).unwrap();
        assert!(!client.is_connected());
        assert_eq!(
            client.stat(Path::new("/")).unwrap_err().kind(),
            RemoteErrorType::NotConnected
        );
        assert_eq!(
            client.stat(Path::new("a.txt")).unwrap_err().kind(),
            RemoteErrorType::InvalidPath
        );
        assert_eq!(
            client
                .open(Path::new("/a.txt"), &ReadOptions::default())
                .unwrap_err()
                .kind(),
            RemoteErrorType::UnsupportedFeature
        );
    }

    #[test]
    #[serial]
    fn should_map_pavao_io_errors() {
        let not_found = pavao::SmbError::Io(io::Error::from(io::ErrorKind::NotFound));
        assert_eq!(
            pavao_error(RemoteErrorType::StatFailed, not_found).kind(),
            RemoteErrorType::NoSuchFileOrDirectory
        );
        let denied = pavao::SmbError::Io(io::Error::from(io::ErrorKind::PermissionDenied));
        assert_eq!(
            pavao_error(RemoteErrorType::IoError, denied).kind(),
            RemoteErrorType::PermissionDenied
        );
        let other = pavao::SmbError::BadValue;
        let mapped = pavao_error(RemoteErrorType::ProtocolError, other);
        assert_eq!(mapped.kind(), RemoteErrorType::ProtocolError);
        assert!(std::error::Error::source(&mapped).is_some());
    }

    fn is_send<T: Send>(_send: T) {}

    fn is_sync<T: Sync>(_sync: T) {}

    #[test]
    #[serial]
    fn test_should_be_send_and_sync() {
        let client = PavaoSmbFs::try_new(test_credentials(), test_options()).unwrap();
        is_sync(&client);
        is_send(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_connect_and_disconnect() {
        crate::mock::logger();
        let mut client = init_client();
        assert!(client.is_connected());
        assert_eq!(
            client.connect().unwrap_err().kind(),
            RemoteErrorType::AlreadyConnected
        );
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_write_stat_and_read_file() {
        crate::mock::logger();
        let client = init_client();
        let p = Path::new("/cargo-test/a.txt");
        let mut reader = Cursor::new(b"test data\n".to_vec());
        assert_eq!(
            client
                .write_file(p, &WriteOptions::default().size_hint(10), &mut reader)
                .unwrap(),
            10
        );
        let entry = client.stat(p).unwrap();
        assert_eq!(entry.path(), p);
        assert_eq!(entry.name(), "a.txt");
        assert_eq!(entry.metadata().size, Some(10));
        assert!(entry.is_file());
        let mut buffer: Vec<u8> = Vec::new();
        assert_eq!(
            client
                .read_file(p, &ReadOptions::default(), &mut buffer)
                .unwrap(),
            10
        );
        assert_eq!(buffer, b"test data\n");
        let mut reader = Cursor::new(b"xy".to_vec());
        assert_eq!(
            client
                .write_file(p, &WriteOptions::default(), &mut reader)
                .unwrap(),
            2
        );
        assert_eq!(client.stat(p).unwrap().metadata().size, Some(2));
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_read_ranges() {
        crate::mock::logger();
        let client = init_client();
        let p = Path::new("/cargo-test/range.txt");
        let mut reader = Cursor::new(b"abcdef".to_vec());
        client
            .write_file(p, &WriteOptions::default(), &mut reader)
            .unwrap();
        let mut out = Vec::new();
        client
            .read_file(p, &ReadOptions::default().offset(2).length(2), &mut out)
            .unwrap();
        assert_eq!(out, b"cd");
        let mut out = Vec::new();
        client
            .read_file(p, &ReadOptions::default().offset(2).length(0), &mut out)
            .unwrap();
        assert!(out.is_empty());
        let mut out = Vec::new();
        client
            .read_file(p, &ReadOptions::default().offset(100), &mut out)
            .unwrap();
        assert!(out.is_empty());
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_append_to_file() {
        crate::mock::logger();
        let client = init_client();
        let p = Path::new("/cargo-test/a.txt");
        let mut reader = Cursor::new(b"test data\n".to_vec());
        assert_eq!(
            client
                .write_file(p, &WriteOptions::default(), &mut reader)
                .unwrap(),
            10
        );
        let mut reader = Cursor::new(b"Hello, world!\n".to_vec());
        assert_eq!(
            client
                .append_file(p, &WriteOptions::default(), &mut reader)
                .unwrap(),
            14
        );
        assert_eq!(client.stat(p).unwrap().metadata().size, Some(24));
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_not_write_into_missing_directory() {
        crate::mock::logger();
        let client = init_client();
        let p = Path::new("/tmp/ahsufhauiefhuiashf/hfhfhfhf");
        let mut reader = Cursor::new(b"x".to_vec());
        assert!(client
            .write_file(p, &WriteOptions::default(), &mut reader)
            .is_err());
        assert!(client
            .append_file(p, &WriteOptions::default(), &mut reader)
            .is_err());
        let mut out = Vec::new();
        assert!(client
            .read_file(p, &ReadOptions::default(), &mut out)
            .is_err());
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_create_list_and_remove_directories() {
        crate::mock::logger();
        let client = init_client();
        let dir = Path::new("/cargo-test/mydir");
        client.create_dir(dir, Some(UnixPex::from(0o755))).unwrap();
        assert_eq!(
            client.create_dir(dir, None).unwrap_err().kind(),
            RemoteErrorType::AlreadyExists
        );
        assert!(client
            .create_dir(Path::new("/tmp/werfgjwerughjwurih/iwerjghiwgui"), None)
            .is_err());
        let mut reader = Cursor::new(b"x".to_vec());
        client
            .write_file(
                Path::new("/cargo-test/mydir/a.txt"),
                &WriteOptions::default(),
                &mut reader,
            )
            .unwrap();
        let entries = client.list_dir(Path::new("/cargo-test/mydir")).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path(), Path::new("/cargo-test/mydir/a.txt"));
        assert_eq!(entries[0].extension().as_deref(), Some("txt"));
        assert!(client.list_dir(Path::new("/tmp/auhhfh/hfhjfhf/")).is_err());
        assert!(client.remove_dir(dir).is_err());
        client.remove_dir_all(dir).unwrap();
        assert!(!client.exists(dir).unwrap());
        assert!(client
            .remove_dir_all(Path::new("/tmp/aaaaaa/asuhi"))
            .is_err());
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_tell_whether_entries_exist() {
        crate::mock::logger();
        let client = init_client();
        let p = Path::new("/cargo-test/a.txt");
        let mut reader = Cursor::new(b"x".to_vec());
        client
            .write_file(p, &WriteOptions::default(), &mut reader)
            .unwrap();
        assert!(client.exists(p).unwrap());
        assert!(client.exists(Path::new("/cargo-test/")).unwrap());
        assert!(!client.exists(Path::new("/cargo-test/b.txt")).unwrap());
        assert!(!client.exists(Path::new("/tmp/ppppp/bhhrhu")).unwrap());
        assert_eq!(
            client.exists(Path::new("a.txt")).unwrap_err().kind(),
            RemoteErrorType::InvalidPath
        );
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_rename_and_remove_file() {
        crate::mock::logger();
        let client = init_client();
        let p = Path::new("/cargo-test/a.txt");
        let dest = Path::new("/cargo-test/b.txt");
        let mut reader = Cursor::new(b"x".to_vec());
        client
            .write_file(p, &WriteOptions::default(), &mut reader)
            .unwrap();
        client.rename(p, dest).unwrap();
        assert!(!client.exists(p).unwrap());
        assert!(client.exists(dest).unwrap());
        assert!(client
            .rename(dest, Path::new("/tmp/wuefhiwuerfh/whjhh/b.txt"))
            .is_err());
        client.remove_file(dest).unwrap();
        assert!(!client.exists(dest).unwrap());
        assert_eq!(
            client.remove_file(dest).unwrap_err().kind(),
            RemoteErrorType::NoSuchFileOrDirectory
        );
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_set_mode_only() {
        crate::mock::logger();
        let client = init_client();
        let p = Path::new("/cargo-test/a.sh");
        let mut reader = Cursor::new(b"echo 5\n".to_vec());
        client
            .write_file(p, &WriteOptions::default(), &mut reader)
            .unwrap();
        client
            .set_metadata(p, &SetMetadata::default().mode(UnixPex::from(0o755)))
            .unwrap();
        assert_eq!(
            client
                .set_metadata(p, &SetMetadata::default().uid(1))
                .unwrap_err()
                .kind(),
            RemoteErrorType::UnsupportedFeature
        );
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_reject_unsupported_operations() {
        crate::mock::logger();
        let client = init_client();
        let p = Path::new("/cargo-test/a.sh");
        assert_eq!(
            client.exec("echo 5").unwrap_err().kind(),
            RemoteErrorType::UnsupportedFeature
        );
        assert_eq!(
            client
                .symlink(Path::new("/cargo-test/b.sh"), p)
                .unwrap_err()
                .kind(),
            RemoteErrorType::UnsupportedFeature
        );
        assert_eq!(
            client
                .copy(p, Path::new("/cargo-test/c.sh"))
                .unwrap_err()
                .kind(),
            RemoteErrorType::UnsupportedFeature
        );
        assert_eq!(
            client
                .create(p, &WriteOptions::default())
                .unwrap_err()
                .kind(),
            RemoteErrorType::UnsupportedFeature
        );
        finalize_client(client);
    }

    #[cfg(feature = "with-containers")]
    fn init_client() -> PavaoSmbFs {
        let mut client = PavaoSmbFs::try_new(test_credentials(), test_options()).unwrap();
        client.connect().unwrap();
        let _ = client.remove_dir_all(Path::new("/cargo-test"));
        client
            .create_dir(Path::new("/cargo-test"), Some(UnixPex::from(0o755)))
            .unwrap();
        client
    }

    #[cfg(feature = "with-containers")]
    fn finalize_client(mut client: PavaoSmbFs) {
        let _ = client.remove_dir_all(Path::new("/cargo-test"));
        client.disconnect().unwrap();
        std::thread::sleep(Duration::from_secs(1));
        drop(client);
    }
}
