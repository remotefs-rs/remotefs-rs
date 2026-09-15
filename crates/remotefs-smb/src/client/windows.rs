//! # Windows client
//!
//! Windows implementation of Smb fs client

mod credentials;
mod file_stream;

use std::fs::OpenOptions;
use std::io::{self, Seek};
use std::path::{Path, PathBuf};

pub use credentials::WNetSmbCredentials;
use file_stream::{WNetReadStream, WNetWriteStream};
use filetime::FileTime;
use remotefs::fs::{
    Capabilities, ExecOutput, File, Metadata, ReadOptions, ReadStream, SetMetadata, UnixPex,
    WriteOptions, WriteStream,
};
use remotefs::{RemoteError, RemoteErrorType, RemoteFs, RemoteResult};
use windows_sys::Win32::Foundation::{
    ERROR_ACCESS_DENIED, ERROR_ALREADY_ASSIGNED, ERROR_ALREADY_CONNECTED, ERROR_BAD_NETPATH,
    ERROR_BAD_NET_NAME, ERROR_BAD_PROVIDER, ERROR_CONNECTION_UNAVAIL, ERROR_INVALID_ADDRESS,
    ERROR_INVALID_PASSWORD, ERROR_LOGON_FAILURE, ERROR_NETWORK_UNREACHABLE, ERROR_NO_NETWORK,
    NO_ERROR, TRUE,
};
use windows_sys::Win32::NetworkManagement::WNet;

use super::{SmbDialect, AUTO_MAX_DIALECT, AUTO_MIN_DIALECT};
use crate::utils::path::SharePath;

/// SMB file system client backed by the Windows WNet API.
///
/// This is a blocking [`RemoteFs`] client. Paths are rooted at the share:
/// `/docs/report.txt` addresses `\\server\share\docs\report.txt`. The
/// connection's own UNC root (`\\server\share\...`) is accepted as well and
/// converted to the share-rooted form; returned entries always use the
/// share-rooted form. `WriteOptions::modified` is applied when a stream
/// finishes.
///
/// # Examples
///
/// ```no_run
/// use std::path::Path;
///
/// use remotefs::RemoteFs;
/// use remotefs_smb::{WNetSmbCredentials, WNetSmbFs};
///
/// let mut client = WNetSmbFs::new(
///     WNetSmbCredentials::new("localhost", "temp")
///         .username("test")
///         .password("test"),
/// );
/// client.connect()?;
/// client.create_dir(Path::new("/cargo"), None)?;
/// client.disconnect()?;
/// # Ok::<(), remotefs::RemoteError>(())
/// ```
pub struct WNetSmbFs {
    remote_name: String,
    credentials: WNetSmbCredentials,
    connected: bool,
}

impl WNetSmbFs {
    /// Instantiates an SMB client with secure automatic dialect bounds.
    ///
    /// The portable policy represented by this constructor allows SMB2 through
    /// SMB3.1.1 and excludes the deprecated SMB1/CIFS `NT1` dialect. The
    /// Windows redirector ultimately applies the operating system's SMB
    /// negotiation policy.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use remotefs_smb::{WNetSmbCredentials, WNetSmbFs};
    ///
    /// let _client = WNetSmbFs::new(WNetSmbCredentials::new("server.example", "documents"));
    /// ```
    pub fn new(credentials: WNetSmbCredentials) -> Self {
        Self::new_with_dialect(credentials, AUTO_MIN_DIALECT, AUTO_MAX_DIALECT)
    }

    /// Instantiates an SMB client with cross-platform dialect bounds.
    ///
    /// Windows' native `WNetAddConnection2W` redirector API does not expose
    /// per-connection SMB dialect bounds. The `min_dialect` and `max_dialect`
    /// arguments are therefore accepted for API parity but are not enforced;
    /// the operating system manages protocol negotiation. In particular,
    /// selecting [`SmbDialect::Nt1`] here does not enable SMB1.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use remotefs_smb::{SmbDialect, WNetSmbCredentials, WNetSmbFs};
    ///
    /// let _client = WNetSmbFs::new_with_dialect(
    ///     WNetSmbCredentials::new("server.example", "documents"),
    ///     SmbDialect::Smb202,
    ///     SmbDialect::Smb210,
    /// );
    /// ```
    pub fn new_with_dialect(
        credentials: WNetSmbCredentials,
        _min_dialect: SmbDialect,
        _max_dialect: SmbDialect,
    ) -> Self {
        let remote_name = format!(
            "\\\\{server}\\{share}",
            server = credentials.server,
            share = credentials.share,
        );
        Self {
            remote_name,
            credentials,
            connected: false,
        }
    }

    /// Parses a share-rooted path, also accepting this connection's UNC root.
    fn share_path(&self, path: &Path) -> RemoteResult<SharePath> {
        let text = path.to_string_lossy();
        let prefix_len = self.remote_name.len();
        let rooted = match text.get(..prefix_len) {
            Some(prefix) if prefix.eq_ignore_ascii_case(&self.remote_name) => {
                let rest = &text[prefix_len..];
                if !rest.is_empty() && !rest.starts_with(['\\', '/']) {
                    return Err(RemoteError::with_message(
                        RemoteErrorType::InvalidPath,
                        "UNC path does not belong to the connected share",
                    ));
                }
                format!("/{rest}", rest = rest.replace('\\', "/"))
            }
            _ => text.into_owned(),
        };
        if rooted.starts_with("\\\\") {
            return Err(RemoteError::with_message(
                RemoteErrorType::InvalidPath,
                "UNC path does not belong to the connected share",
            ));
        }
        SharePath::parse(Path::new(&rooted))
    }

    /// Builds the local UNC path the redirector understands.
    fn local_path(&self, share: &SharePath) -> PathBuf {
        PathBuf::from(format!(
            "{root}\\{relative}",
            root = self.remote_name,
            relative = share.join("\\")
        ))
    }

    fn resolve(&self, path: &Path) -> RemoteResult<(SharePath, PathBuf)> {
        let share = self.share_path(path)?;
        self.check_connection()?;
        let local = self.local_path(&share);
        Ok((share, local))
    }

    fn resolve_pair(
        &self,
        src: &Path,
        dest: &Path,
    ) -> RemoteResult<((SharePath, PathBuf), (SharePath, PathBuf))> {
        let src_share = self.share_path(src)?;
        let dest_share = self.share_path(dest)?;
        self.check_connection()?;
        Ok((
            (src_share.clone(), self.local_path(&src_share)),
            (dest_share.clone(), self.local_path(&dest_share)),
        ))
    }

    fn check_connection(&self) -> RemoteResult<()> {
        if self.connected {
            Ok(())
        } else {
            Err(RemoteError::new(RemoteErrorType::NotConnected))
        }
    }

    fn to_wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn wnet_error(code: u32) -> RemoteError {
        let kind = match code {
            ERROR_LOGON_FAILURE | ERROR_INVALID_PASSWORD => RemoteErrorType::AuthenticationFailed,
            ERROR_ACCESS_DENIED => RemoteErrorType::PermissionDenied,
            ERROR_BAD_NETPATH | ERROR_BAD_NET_NAME | ERROR_INVALID_ADDRESS => {
                RemoteErrorType::BadAddress
            }
            ERROR_ALREADY_CONNECTED | ERROR_ALREADY_ASSIGNED => RemoteErrorType::AlreadyConnected,
            ERROR_CONNECTION_UNAVAIL
            | ERROR_NO_NETWORK
            | ERROR_NETWORK_UNREACHABLE
            | ERROR_BAD_PROVIDER => RemoteErrorType::ConnectionError,
            _ => RemoteErrorType::ConnectionError,
        };
        RemoteError::with_source(kind, io::Error::from_raw_os_error(code as i32))
    }

    fn stat_local(share: &SharePath, local: &Path) -> RemoteResult<File> {
        let attr = std::fs::symlink_metadata(local).map_err(RemoteError::from)?;
        Ok(File::new(share.to_path_buf(), Metadata::from(attr)))
    }

    fn unsupported() -> RemoteError {
        RemoteError::new(RemoteErrorType::UnsupportedFeature)
    }
}

impl RemoteFs for WNetSmbFs {
    fn connect(&mut self) -> RemoteResult<()> {
        if self.connected {
            return Err(RemoteError::new(RemoteErrorType::AlreadyConnected));
        }
        trace!("connecting to {}", self.remote_name);
        let mut remote_name = Self::to_wide(&self.remote_name);
        let resource = WNet::NETRESOURCEW {
            dwDisplayType: WNet::RESOURCEDISPLAYTYPE_SHAREADMIN,
            dwScope: WNet::RESOURCE_GLOBALNET,
            dwType: WNet::RESOURCETYPE_DISK,
            dwUsage: WNet::RESOURCEUSAGE_ALL,
            lpComment: std::ptr::null_mut(),
            lpLocalName: std::ptr::null_mut(),
            lpProvider: std::ptr::null_mut(),
            lpRemoteName: remote_name.as_mut_ptr(),
        };
        let username = self.credentials.username.as_deref().map(Self::to_wide);
        let password = self.credentials.password.as_deref().map(Self::to_wide);
        // SAFETY: every pointer handed to `WNetAddConnection2W` points at a
        // UTF-16 buffer or `NETRESOURCEW` that outlives the call, and null is the
        // documented value for an absent user name or password.
        let result = unsafe {
            let username_ptr = username
                .as_ref()
                .map_or(std::ptr::null(), |username| username.as_ptr());
            let password_ptr = password
                .as_ref()
                .map_or(std::ptr::null(), |password| password.as_ptr());
            WNet::WNetAddConnection2W(
                &resource,
                password_ptr,
                username_ptr,
                WNet::CONNECT_INTERACTIVE,
            )
        };
        if result == NO_ERROR {
            self.connected = true;
            debug!("connected to {}", self.remote_name);
            Ok(())
        } else {
            Err(Self::wnet_error(result))
        }
    }

    fn disconnect(&mut self) -> RemoteResult<()> {
        self.check_connection()?;
        let remote_name = Self::to_wide(&self.remote_name);
        // SAFETY: `remote_name` is a valid NUL-terminated UTF-16 string that
        // outlives the call.
        let result = unsafe { WNet::WNetCancelConnection2W(remote_name.as_ptr(), 0, TRUE) };
        if result == NO_ERROR {
            self.connected = false;
            debug!("disconnected from {}", self.remote_name);
            Ok(())
        } else {
            Err(Self::wnet_error(result))
        }
    }

    fn is_connected(&self) -> bool {
        self.connected
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

    fn list_dir(&self, path: &Path) -> RemoteResult<Vec<File>> {
        let (share, local) = self.resolve(path)?;
        debug!("listing dir {}", local.display());
        let mut entries = Vec::new();
        for entry in std::fs::read_dir(&local).map_err(RemoteError::from)? {
            let entry = entry.map_err(RemoteError::from)?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let child = share.child(&name);
            entries.push(Self::stat_local(&child, &entry.path())?);
        }
        Ok(entries)
    }

    fn stat(&self, path: &Path) -> RemoteResult<File> {
        let (share, local) = self.resolve(path)?;
        debug!("stat {}", local.display());
        Self::stat_local(&share, &local)
    }

    fn exists(&self, path: &Path) -> RemoteResult<bool> {
        match self.stat(path) {
            Ok(_) => Ok(true),
            Err(error) if error.kind() == RemoteErrorType::NoSuchFileOrDirectory => Ok(false),
            Err(error) => Err(error),
        }
    }

    fn set_metadata(&self, path: &Path, metadata: &SetMetadata) -> RemoteResult<()> {
        let (_, local) = self.resolve(path)?;
        if metadata.mode.is_some() || metadata.uid.is_some() || metadata.gid.is_some() {
            return Err(Self::unsupported());
        }
        if let Some(modified) = metadata.modified {
            filetime::set_file_mtime(&local, FileTime::from_system_time(modified))
                .map_err(RemoteError::from)?;
        }
        if let Some(accessed) = metadata.accessed {
            filetime::set_file_atime(&local, FileTime::from_system_time(accessed))
                .map_err(RemoteError::from)?;
        }
        Ok(())
    }

    fn create_dir(&self, path: &Path, _mode: Option<UnixPex>) -> RemoteResult<()> {
        let (_, local) = self.resolve(path)?;
        debug!("creating dir at {}", local.display());
        std::fs::create_dir(&local).map_err(RemoteError::from)
    }

    fn remove_file(&self, path: &Path) -> RemoteResult<()> {
        let (_, local) = self.resolve(path)?;
        debug!("removing file {}", local.display());
        std::fs::remove_file(local).map_err(RemoteError::from)
    }

    fn remove_dir(&self, path: &Path) -> RemoteResult<()> {
        let (_, local) = self.resolve(path)?;
        debug!("removing dir {}", local.display());
        std::fs::remove_dir(local).map_err(RemoteError::from)
    }

    fn rename(&self, src: &Path, dest: &Path) -> RemoteResult<()> {
        let ((_, src), (_, dest)) = self.resolve_pair(src, dest)?;
        debug!("moving {} to {}", src.display(), dest.display());
        std::fs::rename(src, dest).map_err(RemoteError::from)
    }

    fn copy(&self, src: &Path, dest: &Path) -> RemoteResult<()> {
        let ((src_share, src_local), (dest_share, dest_local)) = self.resolve_pair(src, dest)?;
        if src_share.is_same_or_descendant(&dest_share) {
            return Err(RemoteError::with_message(
                RemoteErrorType::InvalidPath,
                "copy destination cannot be the source or one of its descendants",
            ));
        }
        debug!(
            "copying {} to {}",
            src_local.display(),
            dest_local.display()
        );
        if src_local.is_dir() {
            let destination_exists = dest_local.exists();
            if !destination_exists {
                std::fs::create_dir(&dest_local).map_err(RemoteError::from)?;
            }
            let result = (|| {
                for entry in self.list_dir(&src_share.to_path_buf())? {
                    let child_dest = dest_share.child(&entry.name()).to_path_buf();
                    self.copy(entry.path(), &child_dest)?;
                }
                Ok(())
            })();
            if result.is_err() && !destination_exists {
                let _ = std::fs::remove_dir_all(&dest_local);
            }
            result
        } else {
            let dest_local = if dest_local.is_dir() {
                match src_share.name() {
                    Some(name) => dest_local.join(name),
                    None => dest_local,
                }
            } else {
                dest_local
            };
            if dest_local == src_local {
                return Err(RemoteError::with_message(
                    RemoteErrorType::InvalidPath,
                    "copy destination cannot be the source or one of its descendants",
                ));
            }
            std::fs::copy(src_local, dest_local)
                .map(|_| ())
                .map_err(RemoteError::from)
        }
    }

    fn symlink(&self, _path: &Path, _target: &Path) -> RemoteResult<()> {
        Err(Self::unsupported())
    }

    fn open(&self, path: &Path, opts: &ReadOptions) -> RemoteResult<ReadStream> {
        let (_, local) = self.resolve(path)?;
        debug!("opening file {} for reading", local.display());
        let mut file = std::fs::File::open(local).map_err(RemoteError::from)?;
        if let Some(offset) = opts.offset {
            file.seek(std::io::SeekFrom::Start(offset))
                .map_err(RemoteError::from)?;
        }
        Ok(ReadStream::new(WNetReadStream::new(file, opts.length)))
    }

    fn create(&self, path: &Path, opts: &WriteOptions) -> RemoteResult<WriteStream> {
        let (_, local) = self.resolve(path)?;
        debug!("creating {} for writing", local.display());
        let file = std::fs::File::create(&local).map_err(RemoteError::from)?;
        Ok(WriteStream::new(WNetWriteStream::new(file, opts.modified)))
    }

    fn append(&self, path: &Path, opts: &WriteOptions) -> RemoteResult<WriteStream> {
        let (_, local) = self.resolve(path)?;
        debug!("opening {} for append", local.display());
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .truncate(false)
            .write(true)
            .open(&local)
            .map_err(RemoteError::from)?;
        let mut file = file;
        file.seek(std::io::SeekFrom::End(0))
            .map_err(RemoteError::from)?;
        Ok(WriteStream::new(WNetWriteStream::new(file, opts.modified)))
    }

    fn exec(&self, _cmd: &str) -> RemoteResult<ExecOutput> {
        Err(Self::unsupported())
    }
}

#[cfg(test)]
mod test {
    use pretty_assertions::assert_eq;
    use remotefs::fs::Capabilities;
    use remotefs::{RemoteErrorType, RemoteFs};

    use super::*;

    fn client() -> WNetSmbFs {
        WNetSmbFs::new(WNetSmbCredentials::new("pippo", "pippo"))
    }

    #[test]
    fn should_construct_with_explicit_dialect_bounds() {
        let client = WNetSmbFs::new_with_dialect(
            WNetSmbCredentials::new("pippo", "pippo"),
            SmbDialect::Nt1,
            SmbDialect::Nt1,
        );
        assert_eq!(client.remote_name, r"\\pippo\pippo");
        assert!(!client.is_connected());
    }

    #[test]
    fn should_construct_with_secure_auto_defaults() {
        let client = client();
        assert_eq!(AUTO_MIN_DIALECT, SmbDialect::Smb202);
        assert_eq!(AUTO_MAX_DIALECT, SmbDialect::Smb311);
        assert!(!client.is_connected());
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

    #[test]
    fn should_map_share_and_unc_paths() {
        let client = client();
        assert_eq!(
            client
                .share_path(Path::new("/cargo/a.txt"))
                .unwrap()
                .to_path_buf(),
            PathBuf::from("/cargo/a.txt")
        );
        assert_eq!(
            client
                .share_path(Path::new(r"\\PIPPO\pippo\cargo\a.txt"))
                .unwrap()
                .to_path_buf(),
            PathBuf::from("/cargo/a.txt")
        );
        assert_eq!(
            client
                .share_path(Path::new(r"\\pippo\pippo"))
                .unwrap()
                .to_path_buf(),
            PathBuf::from("/")
        );
        assert_eq!(
            client
                .share_path(Path::new(r"\\other\share\x"))
                .unwrap_err()
                .kind(),
            RemoteErrorType::InvalidPath
        );
        assert_eq!(
            client.share_path(Path::new(r"\cargo")).unwrap_err().kind(),
            RemoteErrorType::InvalidPath
        );
        assert_eq!(
            client.share_path(Path::new("cargo")).unwrap_err().kind(),
            RemoteErrorType::InvalidPath
        );
    }

    #[test]
    fn should_build_local_paths() {
        let client = client();
        let share = client.share_path(Path::new("/cargo/a.txt")).unwrap();
        assert_eq!(
            client.local_path(&share),
            PathBuf::from(r"\\pippo\pippo\cargo\a.txt")
        );
        let root = client.share_path(Path::new("/")).unwrap();
        assert_eq!(client.local_path(&root), PathBuf::from(r"\\pippo\pippo\"));
    }

    #[test]
    fn should_fail_when_not_connected() {
        let client = client();
        assert_eq!(
            client.stat(Path::new("/")).unwrap_err().kind(),
            RemoteErrorType::NotConnected
        );
    }

    fn is_send<T: Send>(_send: T) {}

    fn is_sync<T: Sync>(_sync: T) {}

    #[test]
    fn test_should_be_send_and_sync() {
        let client = client();
        is_sync(&client);
        is_send(client);
    }
}
