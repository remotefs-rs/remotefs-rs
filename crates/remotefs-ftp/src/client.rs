//! # Ftp
//!
//! ftp client for remotefs

use crate::utils::path as path_utils;

use remotefs::fs::{
    FileType, Metadata, ReadStream, RemoteError, RemoteErrorType, RemoteFs, RemoteResult, UnixPex,
    UnixPexClass, Welcome, WriteStream,
};
use remotefs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
#[cfg(feature = "native-tls")]
use suppaftp::native_tls::TlsConnector as NativeTlsConnector;
#[cfg(feature = "rustls")]
use suppaftp::rustls::ClientConfig;
#[cfg(not(any(feature = "native-tls", feature = "rustls")))]
pub use suppaftp::FtpStream;
#[cfg(feature = "native-tls")]
use suppaftp::NativeTlsConnector as TlsConnector;
#[cfg(feature = "native-tls")]
pub use suppaftp::NativeTlsFtpStream as FtpStream;
#[cfg(feature = "rustls")]
use suppaftp::RustlsConnector as TlsConnector;
#[cfg(feature = "rustls")]
pub use suppaftp::RustlsFtpStream as FtpStream;
use suppaftp::{
    list::{File as FtpFile, PosixPexQuery},
    types::{FileType as SuppaFtpFileType, Mode, Response},
    FtpError, Status,
};

/// Ftp file system client
pub struct FtpFs {
    /// Client
    stream: Option<FtpStream>,
    // -- options
    hostname: String,
    port: u16,
    /// Username to login as; default: `anonymous`
    username: String,
    password: Option<String>,
    /// Client mode; default: `Mode::Passive`
    mode: Mode,
    #[cfg(any(feature = "native-tls", feature = "rustls"))]
    /// use FTPS; default: `false`
    secure: bool,
    #[cfg(feature = "native-tls")]
    /// Accept invalid certificates when building TLS connector. (Applies only if `secure`). Default: `false`
    accept_invalid_certs: bool,
    #[cfg(feature = "native-tls")]
    /// Accept invalid hostnames when building TLS connector. (Applies only if `secure`). Default: `false`
    accept_invalid_hostnames: bool,
}

impl FtpFs {
    /// Instantiates a new `FtpFs`
    pub fn new<S: AsRef<str>>(hostname: S, port: u16) -> Self {
        Self {
            stream: None,
            hostname: hostname.as_ref().to_string(),
            port,
            username: String::from("anonymous"),
            password: None,
            mode: Mode::Passive,
            #[cfg(any(feature = "native-tls", feature = "rustls"))]
            secure: false,
            #[cfg(feature = "native-tls")]
            accept_invalid_certs: false,
            #[cfg(feature = "native-tls")]
            accept_invalid_hostnames: false,
        }
    }

    // -- constructors

    /// Set username
    pub fn username<S: AsRef<str>>(mut self, username: S) -> Self {
        self.username = username.as_ref().to_string();
        self
    }

    /// Set password
    pub fn password<S: AsRef<str>>(mut self, password: S) -> Self {
        self.password = Some(password.as_ref().to_string());
        self
    }

    /// Set active mode for client
    pub fn active_mode(mut self) -> Self {
        self.mode = Mode::Active;
        self
    }

    /// Set passive mode for client
    pub fn passive_mode(mut self) -> Self {
        self.mode = Mode::Passive;
        self
    }

    #[cfg(feature = "native-tls")]
    /// enable FTPS and configure options
    pub fn secure(mut self, accept_invalid_certs: bool, accept_invalid_hostnames: bool) -> Self {
        self.secure = true;
        self.accept_invalid_certs = accept_invalid_certs;
        self.accept_invalid_hostnames = accept_invalid_hostnames;
        self
    }

    #[cfg(feature = "rustls")]
    pub fn secure(mut self) -> Self {
        self.secure = true;
        self
    }

    // -- as_ref

    /// Get reference to inner stream
    pub fn stream(&mut self) -> Option<&mut FtpStream> {
        self.stream.as_mut()
    }

    // -- private

    /// Parse all lines of LIST command output and instantiates a vector of `File` from it.
    /// This function also converts from `suppaftp::list::File` to `File`
    fn parse_list_lines(&mut self, path: &Path, lines: Vec<String>) -> Vec<File> {
        // Iter and collect
        lines
            .into_iter()
            .flat_map(FtpFile::try_from)
            .map(|f| {
                let mut abs_path: PathBuf = path.to_path_buf();
                abs_path.push(f.name());
                let file_type = if f.is_symlink() {
                    FileType::Symlink
                } else if f.is_directory() {
                    FileType::Directory
                } else {
                    FileType::File
                };
                let metadata = Metadata {
                    accessed: None,
                    created: None,
                    file_type,
                    gid: f.gid(),
                    mode: Some(Self::query_unix_pex(&f)),
                    modified: Some(f.modified()),
                    size: f.size() as u64,
                    symlink: f.symlink().map(|x| path_utils::absolutize(path, x)),
                    uid: None,
                };
                File {
                    path: abs_path,
                    metadata,
                }
            })
            .collect()
    }

    /// Returns unix pex from ftp file pex
    fn query_unix_pex(f: &FtpFile) -> UnixPex {
        UnixPex::new(
            UnixPexClass::new(
                f.can_read(PosixPexQuery::Owner),
                f.can_write(PosixPexQuery::Owner),
                f.can_execute(PosixPexQuery::Owner),
            ),
            UnixPexClass::new(
                f.can_read(PosixPexQuery::Group),
                f.can_write(PosixPexQuery::Group),
                f.can_execute(PosixPexQuery::Group),
            ),
            UnixPexClass::new(
                f.can_read(PosixPexQuery::Others),
                f.can_write(PosixPexQuery::Others),
                f.can_execute(PosixPexQuery::Others),
            ),
        )
    }

    /// Fix provided path; on Windows fixes the backslashes, converting them to slashes
    /// While on POSIX does nothing
    #[cfg(target_os = "windows")]
    fn resolve(p: &Path) -> PathBuf {
        PathBuf::from(path_slash::PathExt::to_slash_lossy(p).as_str())
    }

    #[cfg(target_family = "unix")]
    fn resolve(p: &Path) -> PathBuf {
        p.to_path_buf()
    }

    fn check_connection(&mut self) -> RemoteResult<()> {
        if self.is_connected() {
            Ok(())
        } else {
            Err(RemoteError::new(RemoteErrorType::NotConnected))
        }
    }

    #[cfg(feature = "native-tls")]
    fn setup_tls_connector(&self) -> RemoteResult<TlsConnector> {
        NativeTlsConnector::builder()
            .danger_accept_invalid_certs(self.accept_invalid_certs)
            .danger_accept_invalid_hostnames(self.accept_invalid_hostnames)
            .build()
            .map_err(|e| {
                error!("Failed to setup TLS stream: {}", e);
                RemoteError::new_ex(RemoteErrorType::SslError, e)
            })
            .map(|x| x.into())
    }

    #[cfg(feature = "rustls")]
    fn setup_tls_connector(&self) -> RemoteResult<TlsConnector> {
        let mut root_store = suppaftp::rustls::RootCertStore::empty();
        root_store.add_server_trust_anchors(webpki_roots::TLS_SERVER_ROOTS.0.iter().map(|ta| {
            suppaftp::rustls::OwnedTrustAnchor::from_subject_spki_name_constraints(
                ta.subject,
                ta.spki,
                ta.name_constraints,
            )
        }));
        Ok(std::sync::Arc::new(
            ClientConfig::builder()
                .with_safe_defaults()
                .with_root_certificates(root_store)
                .with_no_client_auth(),
        )
        .into())
    }
}

impl RemoteFs for FtpFs {
    fn connect(&mut self) -> RemoteResult<Welcome> {
        info!("Connecting to {}:{}", self.hostname, self.port);
        let mut stream =
            FtpStream::connect(format!("{}:{}", self.hostname, self.port)).map_err(|e| {
                error!("Failed to connect to remote server: {}", e);
                RemoteError::new_ex(RemoteErrorType::ConnectionError, e)
            })?;
        // If secure, connect TLS
        #[cfg(any(feature = "native-tls", feature = "rustls"))]
        if self.secure {
            debug!("Setting up TLS stream...");
            #[cfg(feature = "native-tls")]
            trace!("Accept invalid certs: {}", self.accept_invalid_certs);
            #[cfg(feature = "native-tls")]
            trace!(
                "Accept invalid hostnames: {}",
                self.accept_invalid_hostnames
            );
            stream = stream
                .into_secure(self.setup_tls_connector()?, self.hostname.as_str())
                .map_err(|e| {
                    error!("Failed to negotiate TLS with server: {}", e);
                    RemoteError::new_ex(RemoteErrorType::SslError, e)
                })?;
            debug!("TLS handshake OK!");
        }
        // Login
        debug!("Signin in as {}", self.username);
        stream
            .login(
                self.username.as_str(),
                self.password.as_deref().unwrap_or(""),
            )
            .map_err(|e| {
                error!("Authentication failed: {}", e);
                RemoteError::new_ex(RemoteErrorType::AuthenticationFailed, e)
            })?;
        trace!("Setting transfer type to Binary");
        stream
            .transfer_type(SuppaFtpFileType::Binary)
            .map_err(|e| {
                error!("Failed to set transfer type to Binary: {}", e);
                RemoteError::new_ex(RemoteErrorType::ProtocolError, e)
            })?;
        info!("Connection established!");
        let welcome = Welcome::default().banner(stream.get_welcome_msg().map(|x| x.to_string()));
        self.stream = Some(stream);
        Ok(welcome)
    }

    fn disconnect(&mut self) -> RemoteResult<()> {
        info!("Disconnecting from FTP server...");
        self.check_connection()?;
        let stream = self.stream.as_mut().unwrap();
        stream.quit().map_err(|e| {
            error!("Failed to disconnect from remote: {}", e);
            RemoteError::new_ex(RemoteErrorType::ConnectionError, e)
        })?;
        self.stream = None;
        Ok(())
    }

    fn is_connected(&mut self) -> bool {
        self.stream.is_some()
    }

    fn pwd(&mut self) -> RemoteResult<PathBuf> {
        debug!("Getting working directory...");
        self.check_connection()?;
        let stream = self.stream.as_mut().unwrap();
        stream.pwd().map(PathBuf::from).map_err(|e| {
            error!("Pwd failed: {}", e);
            RemoteError::new_ex(RemoteErrorType::ProtocolError, e)
        })
    }

    fn change_dir(&mut self, dir: &Path) -> RemoteResult<PathBuf> {
        debug!("Changing working directory to {}", dir.display());
        self.check_connection()?;
        let dir: PathBuf = Self::resolve(dir);
        let stream = self.stream.as_mut().unwrap();
        stream
            .cwd(dir.as_path().to_string_lossy())
            .map(|_| dir)
            .map_err(|e| {
                error!("Failed to change directory: {}", e);
                RemoteError::new_ex(RemoteErrorType::NoSuchFileOrDirectory, e)
            })
    }

    fn list_dir(&mut self, path: &Path) -> RemoteResult<Vec<File>> {
        debug!("Getting list entries for {}", path.display());
        self.check_connection()?;
        let path: PathBuf = Self::resolve(path);
        let stream = self.stream.as_mut().unwrap();
        stream
            .list(Some(&path.as_path().to_string_lossy()))
            .map(|files| self.parse_list_lines(path.as_path(), files))
            .map_err(|e| {
                error!("Failed to list directory: {}", e);
                RemoteError::new_ex(RemoteErrorType::ProtocolError, e)
            })
    }

    fn stat(&mut self, path: &Path) -> RemoteResult<File> {
        debug!("Getting file information for {}", path.display());
        self.check_connection()?;
        // Resolve and absolutize path
        let wrkdir = self.pwd()?;
        let path = Self::resolve(path);
        let path = path_utils::absolutize(wrkdir.as_path(), path.as_path());
        let parent = match path.parent() {
            Some(p) => p,
            None => {
                // Return root
                warn!("{} has no parent: returning root", path.display());
                return Ok(File {
                    path: PathBuf::from("/"),
                    metadata: Metadata::default().file_type(FileType::Directory),
                });
            }
        };
        trace!("Listing entries for stat path file: {}", parent.display());
        let entries = self.list_dir(parent)?;
        // Get target
        let target = entries.into_iter().find(|x| x.path() == path.as_path());
        match target {
            None => {
                error!("Could not find file; no such file or directory");
                Err(RemoteError::new(RemoteErrorType::NoSuchFileOrDirectory))
            }
            Some(e) => Ok(e),
        }
    }

    fn setstat(&mut self, _path: &Path, _metadata: Metadata) -> RemoteResult<()> {
        Err(RemoteError::new(RemoteErrorType::UnsupportedFeature))
    }

    fn exists(&mut self, path: &Path) -> RemoteResult<bool> {
        debug!("Checking whether {} exists", path.display());
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
        debug!("Removing file {}", path.display());
        self.check_connection()?;
        let path = Self::resolve(path);
        let stream = self.stream.as_mut().unwrap();
        stream.rm(&path.as_path().to_string_lossy()).map_err(|e| {
            error!("Failed to remove file {}", e);
            RemoteError::new_ex(RemoteErrorType::ProtocolError, e)
        })
    }

    fn remove_dir(&mut self, path: &Path) -> RemoteResult<()> {
        debug!("Removing file {}", path.display());
        self.check_connection()?;
        let path = Self::resolve(path);
        let stream = self.stream.as_mut().unwrap();
        stream
            .rmdir(&path.as_path().to_string_lossy())
            .map_err(|e| {
                error!("Failed to remove directory {}", e);
                RemoteError::new_ex(RemoteErrorType::ProtocolError, e)
            })
    }

    fn create_dir(&mut self, path: &Path, _mode: UnixPex) -> RemoteResult<()> {
        debug!("Trying to create directory {}", path.display());
        self.check_connection()?;
        let path = Self::resolve(path);
        let stream = self.stream.as_mut().unwrap();
        match stream.mkdir(&path.as_path().to_string_lossy()) {
            Ok(_) => Ok(()),
            Err(FtpError::UnexpectedResponse(Response {
                status: Status::FileUnavailable,
                ..
            })) => {
                error!("Failed to create directory: directory already exists");
                Err(RemoteError::new(RemoteErrorType::DirectoryAlreadyExists))
            }
            Err(e) => {
                error!("Failed to create directory: {}", e);
                Err(RemoteError::new_ex(RemoteErrorType::ProtocolError, e))
            }
        }
    }

    fn symlink(&mut self, _path: &Path, _target: &Path) -> RemoteResult<()> {
        Err(RemoteError::new(RemoteErrorType::UnsupportedFeature))
    }

    fn copy(&mut self, _src: &Path, _dest: &Path) -> RemoteResult<()> {
        Err(RemoteError::new(RemoteErrorType::UnsupportedFeature))
    }

    fn mov(&mut self, src: &Path, dest: &Path) -> RemoteResult<()> {
        debug!("Trying to rename {} to {}", src.display(), dest.display());
        self.check_connection()?;
        let src = Self::resolve(src);
        let dest = Self::resolve(dest);
        let stream = self.stream.as_mut().unwrap();
        stream
            .rename(
                &src.as_path().to_string_lossy(),
                &dest.as_path().to_string_lossy(),
            )
            .map_err(|e| {
                error!("Failed to rename file: {}", e);
                RemoteError::new_ex(RemoteErrorType::ProtocolError, e)
            })
    }

    fn exec(&mut self, _cmd: &str) -> RemoteResult<(u32, String)> {
        Err(RemoteError::new(RemoteErrorType::UnsupportedFeature))
    }

    fn append(&mut self, path: &Path, _metadata: &Metadata) -> RemoteResult<WriteStream> {
        debug!("Opening {} for append", path.display());
        self.check_connection()?;
        let path = Self::resolve(path);
        let stream = self.stream.as_mut().unwrap();
        stream
            .append_with_stream(&path.as_path().to_string_lossy())
            .map(|x| Box::new(x) as Box<dyn Write>)
            .map(WriteStream::from)
            .map_err(|e| {
                format!("Failed to open file: {}", e);
                RemoteError::new_ex(RemoteErrorType::ProtocolError, e)
            })
    }

    fn create(&mut self, path: &Path, _metadata: &Metadata) -> RemoteResult<WriteStream> {
        debug!("Opening {} for write", path.display());
        self.check_connection()?;
        let path = Self::resolve(path);
        let stream = self.stream.as_mut().unwrap();
        stream
            .put_with_stream(&path.as_path().to_string_lossy())
            .map(|x| Box::new(x) as Box<dyn Write>)
            .map(WriteStream::from)
            .map_err(|e| {
                format!("Failed to open file: {}", e);
                RemoteError::new_ex(RemoteErrorType::ProtocolError, e)
            })
    }

    fn open(&mut self, path: &Path) -> RemoteResult<ReadStream> {
        debug!("Opening {} for read", path.display());
        self.check_connection()?;
        let path = Self::resolve(path);
        let stream = self.stream.as_mut().unwrap();
        stream
            .retr_as_stream(&path.as_path().to_string_lossy())
            .map(|x| Box::new(x) as Box<dyn Read>)
            .map(ReadStream::from)
            .map_err(|e| {
                format!("Failed to open file: {}", e);
                RemoteError::new_ex(RemoteErrorType::ProtocolError, e)
            })
    }

    fn on_read(&mut self, readable: ReadStream) -> RemoteResult<()> {
        debug!("Finalizing read stream");
        self.check_connection()?;
        let stream = self.stream.as_mut().unwrap();
        stream.finalize_retr_stream(readable).map_err(|e| {
            error!("Failed to finalize read stream: {}", e);
            RemoteError::new_ex(RemoteErrorType::ProtocolError, e)
        })
    }

    fn on_written(&mut self, writable: WriteStream) -> RemoteResult<()> {
        debug!("Finalizing write stream");
        self.check_connection()?;
        let stream = self.stream.as_mut().unwrap();
        stream.finalize_put_stream(writable).map_err(|e| {
            error!("Failed to finalize write stream: {}", e);
            RemoteError::new_ex(RemoteErrorType::ProtocolError, e)
        })
    }
}

#[cfg(test)]
mod test {

    use super::*;

    use pretty_assertions::assert_eq;
    #[cfg(feature = "with-containers")]
    use serial_test::serial;
    #[cfg(feature = "with-containers")]
    use std::io::Cursor;

    #[test]
    fn should_initialize_ftp_filesystem() {
        let client = FtpFs::new("127.0.0.1", 21);
        assert!(client.stream.is_none());
        assert_eq!(client.hostname.as_str(), "127.0.0.1");
        assert_eq!(client.port, 21);
        assert_eq!(client.username.as_str(), "anonymous");
        assert!(client.password.is_none());
        assert_eq!(client.mode, Mode::Passive);
        #[cfg(any(feature = "native-tls", feature = "rustls"))]
        assert_eq!(client.secure, false);
        #[cfg(feature = "native-tls")]
        assert_eq!(client.accept_invalid_certs, false);
        #[cfg(feature = "native-tls")]
        assert_eq!(client.accept_invalid_hostnames, false);
    }

    #[test]
    fn should_build_ftp_filesystem() {
        let client = FtpFs::new("127.0.0.1", 21)
            .username("test")
            .password("omar")
            .passive_mode()
            .active_mode();
        assert!(client.stream.is_none());
        assert_eq!(client.hostname.as_str(), "127.0.0.1");
        assert_eq!(client.port, 21);
        assert_eq!(client.username.as_str(), "test");
        assert_eq!(client.password.as_deref().unwrap(), "omar");
        assert_eq!(client.mode, Mode::Active);
    }

    #[test]
    #[cfg(any(feature = "native-tls", feature = "rustls"))]
    fn should_build_secure_ftp_filesystem() {
        #[cfg(feature = "native-tls")]
        let client = FtpFs::new("127.0.0.1", 21)
            .username("test")
            .password("omar")
            .secure(true, true)
            .passive_mode()
            .active_mode();
        #[cfg(feature = "rustls")]
        let client = FtpFs::new("127.0.0.1", 21)
            .username("test")
            .password("omar")
            .secure()
            .passive_mode()
            .active_mode();
        assert!(client.stream.is_none());
        assert_eq!(client.hostname.as_str(), "127.0.0.1");
        assert_eq!(client.port, 21);
        assert_eq!(client.username.as_str(), "test");
        assert_eq!(client.password.as_deref().unwrap(), "omar");
        assert_eq!(client.mode, Mode::Active);
        assert_eq!(client.secure, true);
        #[cfg(feature = "native-tls")]
        assert_eq!(client.accept_invalid_certs, true);
        #[cfg(feature = "native-tls")]
        assert_eq!(client.accept_invalid_hostnames, true);
    }

    #[test]
    #[ignore]
    #[cfg(feature = "native-tls")]
    fn should_connect_with_ftps() {
        let mut client = FtpFs::new("test.rebex.net", 21)
            .username("demo")
            .password("password")
            .secure(false, false)
            .passive_mode();
        assert!(client.connect().is_ok());
        assert!(client.disconnect().is_ok());
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_append_to_file() {
        crate::mock::logger();
        let mut client = setup_client();
        // Create file
        let p = Path::new("a.txt");
        let file_data = "test data\n";
        let reader = Cursor::new(file_data.as_bytes());
        assert_eq!(
            client
                .create_file(p, &Metadata::default(), Box::new(reader))
                .ok()
                .unwrap(),
            10
        );
        // Verify size
        assert_eq!(client.stat(p).ok().unwrap().metadata().size, 10);
        // Append to file
        let file_data = "Hello, world!\n";
        let reader = Cursor::new(file_data.as_bytes());
        assert_eq!(
            client
                .append_file(p, &Metadata::default(), Box::new(reader))
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
        let mut client = setup_client();
        // Create file
        let p = Path::new("/tmp/aaaaaaa/hbbbbb/a.txt");
        // Append to file
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
    fn should_change_directory() {
        crate::mock::logger();
        let mut client = setup_client();
        let pwd = client.pwd().ok().unwrap();
        assert!(client.change_dir(Path::new("/")).is_ok());
        assert!(client.change_dir(pwd.as_path()).is_ok());
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
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
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_not_copy_file() {
        crate::mock::logger();
        let mut client = setup_client();
        assert!(client.copy(Path::new("a.txt"), Path::new("b.txt")).is_err());
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
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
    #[cfg(feature = "with-containers")]
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
    #[cfg(feature = "with-containers")]
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
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_create_file() {
        crate::mock::logger();
        let mut client = setup_client();
        // Create file
        let p = Path::new("a.txt");
        let file_data = "test data\n";
        let reader = Cursor::new(file_data.as_bytes());
        assert_eq!(
            client
                .create_file(p, &Metadata::default(), Box::new(reader))
                .ok()
                .unwrap(),
            10
        );
        // Verify size
        assert_eq!(client.stat(p).ok().unwrap().metadata().size, 10);
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_not_create_file() {
        crate::mock::logger();
        let mut client = setup_client();
        // Create file
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
        let mut client = setup_client();
        // Create file
        assert!(client.exec("echo 5").is_err());
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_tell_whether_file_exists() {
        crate::mock::logger();
        let mut client = setup_client();
        // Create file
        let p = Path::new("a.txt");
        let file_data = "test data\n";
        let reader = Cursor::new(file_data.as_bytes());
        assert!(client
            .create_file(p, &Metadata::default(), Box::new(reader))
            .is_ok());
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
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_list_dir() {
        crate::mock::logger();
        let mut client = setup_client();
        // Create file
        let wrkdir = client.pwd().ok().unwrap();
        let p = Path::new("a.txt");
        let file_data = "test data\n";
        let reader = Cursor::new(file_data.as_bytes());
        assert!(client
            .create_file(p, &Metadata::default(), Box::new(reader))
            .is_ok());
        // Verify size
        let file = client
            .list_dir(wrkdir.as_path())
            .ok()
            .unwrap()
            .get(0)
            .unwrap()
            .clone();
        assert_eq!(file.name().as_str(), "a.txt");
        let mut expected_path = wrkdir;
        expected_path.push(p);
        assert_eq!(file.path.as_path(), expected_path.as_path());
        assert_eq!(file.extension().as_deref().unwrap(), "txt");
        assert!(file.is_file());
        assert_eq!(file.metadata.size, 10);
        assert_eq!(file.metadata.mode.unwrap(), UnixPex::from(0o644));
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_move_file() {
        crate::mock::logger();
        let mut client = setup_client();
        // Create file
        let p = Path::new("a.txt");
        let file_data = "test data\n";
        let reader = Cursor::new(file_data.as_bytes());
        assert!(client
            .create_file(p, &Metadata::default(), Box::new(reader))
            .is_ok());
        // Verify size
        let dest = Path::new("b.txt");
        assert!(client.mov(p, dest).is_ok());
        assert_eq!(client.exists(p).ok().unwrap(), false);
        assert_eq!(client.exists(dest).ok().unwrap(), true);
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_not_move_file() {
        crate::mock::logger();
        let mut client = setup_client();
        // Create file
        let p = Path::new("a.txt");
        let file_data = "test data\n";
        let reader = Cursor::new(file_data.as_bytes());
        assert!(client
            .create_file(p, &Metadata::default(), Box::new(reader))
            .is_ok());
        // Verify size
        let dest = Path::new("/tmp/wuefhiwuerfh/whjhh/b.txt");
        assert!(client.mov(p, dest).is_err());
        assert!(client
            .mov(Path::new("/tmp/wuefhiwuerfh/whjhh/b.txt"), p)
            .is_err());
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_open_file() {
        crate::mock::logger();
        let mut client = setup_client();
        // Create file
        let p = Path::new("a.txt");
        let file_data = "test data\n";
        let reader = Cursor::new(file_data.as_bytes());
        assert!(client
            .create_file(p, &Metadata::default(), Box::new(reader))
            .is_ok());
        // Verify size
        let buffer: Box<dyn std::io::Write + Send> = Box::new(Vec::with_capacity(512));
        assert!(client.open_file(p, buffer).is_ok());
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
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
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_print_working_directory() {
        crate::mock::logger();
        let mut client = setup_client();
        assert!(client.pwd().is_ok());
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
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
        assert!(client
            .create_file(file_path.as_path(), &Metadata::default(), Box::new(reader))
            .is_ok());
        // Remove dir
        assert!(client.remove_dir_all(dir_path.as_path()).is_ok());
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_not_remove_dir_all() {
        crate::mock::logger();
        let mut client = setup_client();
        // Remove dir
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
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_not_remove_dir() {
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
        assert!(client
            .create_file(file_path.as_path(), &Metadata::default(), Box::new(reader))
            .is_ok());
        // Remove dir
        assert!(client.remove_dir(dir_path.as_path()).is_err());
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_remove_file() {
        crate::mock::logger();
        let mut client = setup_client();
        // Create file
        let p = Path::new("a.txt");
        let file_data = "test data\n";
        let reader = Cursor::new(file_data.as_bytes());
        assert!(client
            .create_file(p, &Metadata::default(), Box::new(reader))
            .is_ok());
        assert!(client.remove_file(p).is_ok());
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_not_setstat_file() {
        crate::mock::logger();
        let mut client = setup_client();
        // Create file
        let p = Path::new("a.sh");
        assert!(client
            .setstat(
                p,
                Metadata {
                    accessed: None,
                    created: None,
                    gid: Some(1),
                    file_type: FileType::File,
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
        let mut client = setup_client();
        // Create file
        let p = Path::new("a.sh");
        let file_data = "echo 5\n";
        let reader = Cursor::new(file_data.as_bytes());
        assert!(client
            .create_file(p, &Metadata::default(), Box::new(reader))
            .is_ok());
        let entry = client.stat(p).ok().unwrap();
        assert_eq!(entry.name(), "a.sh");
        let mut expected_path = client.pwd().ok().unwrap();
        expected_path.push("a.sh");
        assert_eq!(entry.path(), expected_path.as_path());
        let meta = entry.metadata();
        assert_eq!(meta.mode.unwrap(), UnixPex::from(0o644));
        assert_eq!(meta.size, 7);
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_stat_root() {
        crate::mock::logger();
        let mut client = setup_client();
        // Create file
        let p = Path::new("/");
        let entry = client.stat(p).ok().unwrap();
        assert_eq!(entry.name(), "/");
        assert_eq!(entry.path(), Path::new("/"));
        assert!(entry.is_dir());
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
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
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_not_make_symlink() {
        crate::mock::logger();
        let mut client = setup_client();
        // Create file
        let p = Path::new("a.sh");
        let symlink = Path::new("b.sh");
        assert!(client.symlink(symlink, p).is_err());
        finalize_client(client);
    }

    #[test]
    fn should_return_not_connected_error() {
        crate::mock::logger();
        let mut client = FtpFs::new("127.0.0.1", 21);
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

    #[cfg(feature = "with-containers")]
    fn setup_client() -> FtpFs {
        let mut client = FtpFs::new("127.0.0.1", 10021)
            .username("test")
            .password("test");
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

    #[cfg(feature = "with-containers")]
    fn finalize_client(mut client: FtpFs) {
        // Get working directory
        let wrkdir = client.pwd().ok().unwrap();
        // Remove directory
        assert!(client.remove_dir_all(wrkdir.as_path()).is_ok());
        assert!(client.disconnect().is_ok());
    }

    #[cfg(feature = "with-containers")]
    fn generate_tempdir() -> String {
        use rand::{distributions::Alphanumeric, thread_rng, Rng};
        let mut rng = thread_rng();
        let name: String = std::iter::repeat(())
            .map(|()| rng.sample(Alphanumeric))
            .map(char::from)
            .take(8)
            .collect();
        format!("/temp_{}", name)
    }
}
