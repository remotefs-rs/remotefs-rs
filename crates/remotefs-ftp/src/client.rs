//! # Ftp
//!
//! ftp client for remotefs

mod error;
mod stream;

use std::error::Error as _;
use std::fmt;
use std::io::Read as _;
use std::net::{SocketAddr, TcpStream};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use remotefs::File;
use remotefs::fs::{
    Capabilities, ExecOutput, FileType, Metadata, ReadOptions, ReadStream, RemoteError,
    RemoteErrorType, RemoteFs, RemoteResult, SetMetadata, UnixPex, UnixPexClass, WriteOptions,
    WriteStream,
};
use remotefs::path::ensure_absolute;
#[cfg(not(any(
    feature = "native-tls",
    feature = "rustls-aws-lc-rs",
    feature = "rustls-ring"
)))]
pub use suppaftp::FtpStream;
#[cfg(feature = "native-tls")]
use suppaftp::NativeTlsConnector as TlsConnector;
#[cfg(feature = "native-tls")]
pub use suppaftp::NativeTlsFtpStream as FtpStream;
#[cfg(any(feature = "rustls-aws-lc-rs", feature = "rustls-ring"))]
use suppaftp::RustlsConnector as TlsConnector;
#[cfg(any(feature = "rustls-aws-lc-rs", feature = "rustls-ring"))]
pub use suppaftp::RustlsFtpStream as FtpStream;
use suppaftp::list::{File as FtpFile, ListParser, PosixPexQuery};
#[cfg(feature = "native-tls")]
use suppaftp::native_tls::TlsConnector as NativeTlsConnector;
#[cfg(any(feature = "rustls-aws-lc-rs", feature = "rustls-ring"))]
use suppaftp::rustls::ClientConfig;
use suppaftp::types::{FileType as SuppaFtpFileType, Mode};
use suppaftp::{FtpError, FtpResult, Status};

use self::error::ftp_error;
use self::stream::{FtpReadStream, FtpWriteStream, TransferGuard};
use crate::utils::path as path_utils;

/// A function that creates a new stream for the data connection in passive mode.
///
/// It takes a [`SocketAddr`] and returns a [`TcpStream`].
pub type PassiveStreamBuilder = dyn Fn(SocketAddr) -> FtpResult<TcpStream> + Send + Sync;

/// FTP and FTPS client implementing [`RemoteFs`].
///
/// Every path passed to the filesystem operations must be absolute; the client
/// keeps no working directory. Operations take `&self` and serialise access to
/// the control connection with a mutex. FTP allows a single data connection:
/// while a stream returned by [`RemoteFs::open`], [`RemoteFs::create`] or
/// [`RemoteFs::append`] is alive, other control-connection operations return
/// [`RemoteErrorType::ProtocolError`] until the stream is finished or dropped.
/// Finishing an early or length-limited read drains the remaining file data
/// before closing the transfer, so it can wait for the rest of the download.
/// A cleanup failure marks the control connection unusable until reconnect,
/// because `suppaftp` cannot safely drain every possible pair of completion
/// replies through an owned transfer stream.
///
/// # Examples
///
/// ```rust,no_run
/// use std::path::Path;
///
/// use remotefs::RemoteFs;
/// use remotefs::fs::WriteOptions;
/// use remotefs_ftp::FtpFs;
///
/// # fn run() -> remotefs::RemoteResult<()> {
/// let mut client = FtpFs::new("127.0.0.1", 21)
///     .username("test")
///     .password("password");
/// client.connect()?;
/// let mut source = std::io::Cursor::new(b"hello".to_vec());
/// client.write_file(
///     Path::new("/upload/hello.txt"),
///     &WriteOptions::default().size_hint(5),
///     &mut source,
/// )?;
/// client.disconnect()?;
/// # Ok(())
/// # }
/// ```
pub struct FtpFs {
    /// Control connection; `None` until [`RemoteFs::connect`] succeeds.
    stream: Mutex<Option<FtpStream>>,
    /// Raised while a transfer stream is alive.
    transfer_active: Arc<AtomicBool>,
    /// Cleared when transfer cleanup may have left unread FTP replies.
    connection_usable: Arc<AtomicBool>,
    // -- options
    hostname: String,
    port: u16,
    /// Username to login as; default: `anonymous`
    username: String,
    password: Option<String>,
    /// passive stream builder
    passive_stream_builder: Option<Arc<PassiveStreamBuilder>>,
    /// Client mode; default: `Mode::Passive`
    mode: Mode,
    #[cfg(any(
        feature = "native-tls",
        feature = "rustls-aws-lc-rs",
        feature = "rustls-ring"
    ))]
    /// use FTPS; default: `false`
    secure: bool,
    #[cfg(feature = "native-tls")]
    /// Accept invalid certificates when building TLS connector. (Applies only if `secure`). Default: `false`
    accept_invalid_certs: bool,
    #[cfg(feature = "native-tls")]
    /// Accept invalid hostnames when building TLS connector. (Applies only if `secure`). Default: `false`
    accept_invalid_hostnames: bool,
}

/// Temporarily owns the control stream while an internal data transfer runs.
struct ExclusiveStream<'a> {
    client: &'a FtpFs,
    stream: Option<FtpStream>,
}

impl Drop for ExclusiveStream<'_> {
    fn drop(&mut self) {
        if let Some(stream) = self.stream.take() {
            self.client.lock().replace(stream);
        }
        self.client.transfer_active.store(false, Ordering::SeqCst);
    }
}

impl FtpFs {
    /// Instantiates a new `FtpFs` for `hostname:port`; connect with [`RemoteFs::connect`].
    ///
    /// # Examples
    ///
    /// ```
    /// use remotefs_ftp::FtpFs;
    ///
    /// let client = FtpFs::new("127.0.0.1", 21).username("test").password("secret");
    /// ```
    pub fn new<S: AsRef<str>>(hostname: S, port: u16) -> Self {
        Self {
            stream: Mutex::new(None),
            transfer_active: Arc::new(AtomicBool::new(false)),
            connection_usable: Arc::new(AtomicBool::new(true)),
            hostname: hostname.as_ref().to_string(),
            port,
            username: String::from("anonymous"),
            password: None,
            mode: Mode::Passive,
            passive_stream_builder: None,
            #[cfg(any(
                feature = "native-tls",
                feature = "rustls-aws-lc-rs",
                feature = "rustls-ring"
            ))]
            secure: false,
            #[cfg(feature = "native-tls")]
            accept_invalid_certs: false,
            #[cfg(feature = "native-tls")]
            accept_invalid_hostnames: false,
        }
    }

    // -- constructors

    /// Sets the username used when connecting.
    ///
    /// # Examples
    ///
    /// ```
    /// use remotefs_ftp::FtpFs;
    /// let client = FtpFs::new("localhost", 21).username("test");
    /// ```
    pub fn username<S: AsRef<str>>(mut self, username: S) -> Self {
        self.username = username.as_ref().to_string();
        self
    }

    /// Sets the password used when connecting.
    ///
    /// # Examples
    ///
    /// ```
    /// use remotefs_ftp::FtpFs;
    /// let client = FtpFs::new("localhost", 21).password("secret");
    /// ```
    pub fn password<S: AsRef<str>>(mut self, password: S) -> Self {
        self.password = Some(password.as_ref().to_string());
        self
    }

    /// Selects active mode for data connections.
    ///
    /// # Examples
    ///
    /// ```
    /// use remotefs_ftp::FtpFs;
    /// let client = FtpFs::new("localhost", 21).active_mode();
    /// ```
    pub fn active_mode(mut self) -> Self {
        self.mode = Mode::Active;
        self
    }

    /// Selects passive mode for data connections, the default.
    ///
    /// # Examples
    ///
    /// ```
    /// use remotefs_ftp::FtpFs;
    /// let client = FtpFs::new("localhost", 21).passive_mode();
    /// ```
    pub fn passive_mode(mut self) -> Self {
        self.mode = Mode::Passive;
        self
    }

    #[cfg(feature = "native-tls")]
    /// Enables FTPS with the specified certificate and hostname validation options.
    ///
    /// Passing `true` disables the corresponding validation when connecting.
    ///
    /// # Examples
    ///
    /// ```
    /// use remotefs_ftp::FtpFs;
    /// let client = FtpFs::new("localhost", 21).secure(false, false);
    /// ```
    pub fn secure(mut self, accept_invalid_certs: bool, accept_invalid_hostnames: bool) -> Self {
        self.secure = true;
        self.accept_invalid_certs = accept_invalid_certs;
        self.accept_invalid_hostnames = accept_invalid_hostnames;
        self
    }

    #[cfg(any(feature = "rustls-aws-lc-rs", feature = "rustls-ring"))]
    /// Enables FTPS using rustls and the webpki root store.
    ///
    /// # Examples
    ///
    /// ```
    /// use remotefs_ftp::FtpFs;
    /// let client = FtpFs::new("localhost", 21).secure();
    /// ```
    pub fn secure(mut self) -> Self {
        self.secure = true;
        self
    }

    /// Sets a custom [`PassiveStreamBuilder`] for passive mode.
    ///
    /// The stream builder is a function that takes a `SocketAddr` and returns a `TcpStream` and it's used
    /// to create the [`TcpStream`] for the data connection in passive mode.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::net::TcpStream;
    /// use remotefs_ftp::FtpFs;
    /// use suppaftp::FtpError;
    /// let client = FtpFs::new("localhost", 21)
    ///     .passive_stream_builder(|address| {
    ///         TcpStream::connect(address).map_err(FtpError::ConnectionError)
    ///     });
    /// ```
    pub fn passive_stream_builder<F>(mut self, builder: F) -> Self
    where
        F: Fn(SocketAddr) -> FtpResult<TcpStream> + Send + Sync + 'static,
    {
        self.passive_stream_builder = Some(Arc::new(builder));
        self
    }

    // -- accessors

    /// Returns the control stream when connected and no managed transfer is active.
    ///
    /// Returns `None` until a managed transfer is finished or dropped. This
    /// accessor bypasses the filesystem's command coordination: callers must
    /// finish any raw transfer and consume its completion reply before using
    /// filesystem operations again. Leave the connection authenticated and in
    /// binary transfer mode.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use remotefs::RemoteFs;
    /// use remotefs_ftp::FtpFs;
    ///
    /// let mut client = FtpFs::new("127.0.0.1", 21);
    /// client.connect().unwrap();
    /// let cwd = client.stream().unwrap().pwd().unwrap();
    /// ```
    pub fn stream(&mut self) -> Option<&mut FtpStream> {
        if self.transfer_active.load(Ordering::SeqCst)
            || !self.connection_usable.load(Ordering::SeqCst)
        {
            return None;
        }
        self.stream
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner)
            .as_mut()
    }

    /// Returns the banner the server sent on connection, if connected.
    ///
    /// This replaces the `Welcome` value returned by `connect` in remotefs 0.3.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use remotefs::RemoteFs;
    /// use remotefs_ftp::FtpFs;
    ///
    /// let mut client = FtpFs::new("127.0.0.1", 21);
    /// client.connect().unwrap();
    /// if let Some(banner) = client.welcome_message() {
    ///     println!("{banner}");
    /// }
    /// ```
    pub fn welcome_message(&self) -> Option<String> {
        self.lock()
            .as_ref()
            .and_then(|stream| stream.get_welcome_msg().map(str::to_string))
    }

    // -- private

    fn lock(&self) -> MutexGuard<'_, Option<FtpStream>> {
        self.stream.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Runs `f` on the connected control stream while holding the client lock.
    ///
    /// Fails with `NotConnected` when there is no connection and with
    /// `ProtocolError` while a transfer stream is alive.
    fn with_stream<R>(&self, f: impl FnOnce(&mut FtpStream) -> RemoteResult<R>) -> RemoteResult<R> {
        let mut guard = self.lock();
        self.check_operation_state()?;
        let Some(stream) = guard.as_mut() else {
            return Err(RemoteError::new(RemoteErrorType::NotConnected));
        };
        let result = f(stream);
        if let Err(error) = &result {
            self.record_remote_error(error);
        }
        result
    }

    /// Runs `f` without holding the outer client mutex while `suppaftp` finishes a LIST transfer.
    fn with_exclusive_stream<R>(
        &self,
        f: impl FnOnce(&mut FtpStream) -> RemoteResult<R>,
    ) -> RemoteResult<R> {
        let mut guard = self.lock();
        self.check_operation_state()?;
        let stream = guard
            .take()
            .ok_or_else(|| RemoteError::new(RemoteErrorType::NotConnected))?;
        self.transfer_active.store(true, Ordering::SeqCst);
        drop(guard);
        let mut owned = ExclusiveStream {
            client: self,
            stream: Some(stream),
        };
        let result = f(owned.stream.as_mut().expect("exclusive stream is present"));
        if let Err(error) = &result {
            self.record_remote_error(error);
        }
        result
    }

    /// Checks state after the caller has acquired the control-stream mutex.
    fn check_operation_state(&self) -> RemoteResult<()> {
        if self.transfer_active.load(Ordering::SeqCst) {
            return Err(RemoteError::with_message(
                RemoteErrorType::ProtocolError,
                "a data transfer is in progress; finish or drop its stream first",
            ));
        }
        if !self.connection_usable.load(Ordering::SeqCst) {
            return Err(RemoteError::with_message(
                RemoteErrorType::ConnectionError,
                "the control connection is unusable; reconnect before retrying",
            ));
        }
        Ok(())
    }

    /// Marks the current control channel unusable until a fresh connection is established.
    fn mark_connection_unusable(&self) {
        self.connection_usable.store(false, Ordering::SeqCst);
    }

    /// Records an FTP failure while the control-stream lock is still held.
    fn record_ftp_error(&self, error: &FtpError) {
        if transfer_setup_requires_reconnect(error) {
            self.mark_connection_unusable();
        }
    }

    /// Records a mapped failure while the control-stream lock is still held.
    fn record_remote_error(&self, error: &RemoteError) {
        if remote_error_requires_reconnect(error) {
            self.mark_connection_unusable();
        }
    }

    /// Classifies a file operation refusal after probing for positive evidence.
    fn classify_file_refusal(&self, path: &Path, err: FtpError) -> RemoteError {
        if !is_file_unavailable(&err) {
            return ftp_error(err);
        }
        match self.stat(path) {
            Ok(file) if file.is_dir() => RemoteError::with_source(RemoteErrorType::BadFile, err),
            Err(probe) if probe.kind() == RemoteErrorType::NoSuchFileOrDirectory => {
                RemoteError::with_source(RemoteErrorType::NoSuchFileOrDirectory, err)
            }
            _ => ftp_error(err),
        }
    }

    /// Classifies a failed STOR or APPE while preserving ambiguous refusals.
    fn classify_create_refusal(&self, path: &Path, err: FtpError) -> RemoteError {
        if !is_creation_refusal(&err) {
            return ftp_error(err);
        }
        match self.stat(path) {
            Ok(file) if file.is_dir() => {
                RemoteError::with_source(RemoteErrorType::AlreadyExists, err)
            }
            Err(error)
                if error.kind() == RemoteErrorType::NoSuchFileOrDirectory
                    && self.destination_parent_is_missing(path) =>
            {
                RemoteError::with_source(RemoteErrorType::NoSuchFileOrDirectory, err)
            }
            _ => RemoteError::with_source(RemoteErrorType::PermissionDenied, err),
        }
    }

    /// Classifies a failed RMD while preserving ambiguous refusals.
    fn classify_remove_dir_refusal(&self, path: &Path, err: FtpError) -> RemoteError {
        if !is_file_unavailable(&err) {
            return ftp_error(err);
        }
        match self.stat(path) {
            Ok(file) if !file.is_dir() => RemoteError::with_source(RemoteErrorType::BadFile, err),
            Ok(_) => match self.list_dir(path) {
                Ok(entries) if !entries.is_empty() => {
                    RemoteError::with_source(RemoteErrorType::DirectoryNotEmpty, err)
                }
                _ => RemoteError::with_source(RemoteErrorType::PermissionDenied, err),
            },
            Err(probe) if probe.kind() == RemoteErrorType::NoSuchFileOrDirectory => {
                RemoteError::with_source(RemoteErrorType::NoSuchFileOrDirectory, err)
            }
            _ => RemoteError::with_source(RemoteErrorType::PermissionDenied, err),
        }
    }

    /// Probes ancestors until a successful listing can positively prove absence.
    fn ancestor_proves_absent(&self, path: &Path) -> RemoteResult<bool> {
        let mut candidate = path.to_path_buf();
        for _ in 0..8 {
            let Some(parent) = candidate.parent() else {
                return Ok(false);
            };
            let remote_parent = Self::remote_path(parent)?;
            match self.list_dir_raw(parent, &remote_parent) {
                Ok(entries) => return Ok(!entries.iter().any(|entry| entry.path() == candidate)),
                Err(error) if is_ambiguous_path_refusal(&error) => {
                    candidate = parent.to_path_buf();
                }
                Err(_) => return Ok(false),
            }
        }
        Ok(false)
    }

    /// Returns true only when metadata lookup positively proves a missing parent.
    fn destination_parent_is_missing(&self, path: &Path) -> bool {
        let Some(parent) = path.parent() else {
            return false;
        };
        matches!(
            self.stat(parent),
            Err(error) if error.kind() == RemoteErrorType::NoSuchFileOrDirectory
        )
    }

    /// Classifies a failed `MKD` while preserving ambiguous 550 refusals.
    fn classify_create_dir_refusal(&self, path: &Path, err: FtpError) -> RemoteError {
        if !is_creation_refusal(&err) {
            return ftp_error(err);
        }
        match self.stat(path) {
            Ok(_) => RemoteError::with_source(RemoteErrorType::AlreadyExists, err),
            Err(error)
                if error.kind() == RemoteErrorType::NoSuchFileOrDirectory
                    && self.destination_parent_is_missing(path) =>
            {
                RemoteError::with_source(RemoteErrorType::NoSuchFileOrDirectory, err)
            }
            _ => RemoteError::with_source(RemoteErrorType::PermissionDenied, err),
        }
    }

    /// Classifies a failed rename while distinguishing source and destination lookup.
    fn classify_rename_refusal(&self, src: &Path, dest: &Path, err: FtpError) -> RemoteError {
        if !is_creation_refusal(&err) {
            return ftp_error(err);
        }
        match self.stat(src) {
            Err(error) if error.kind() == RemoteErrorType::NoSuchFileOrDirectory => {
                RemoteError::with_source(RemoteErrorType::NoSuchFileOrDirectory, err)
            }
            Err(_) => RemoteError::with_source(RemoteErrorType::PermissionDenied, err),
            Ok(_) if self.destination_parent_is_missing(dest) => {
                RemoteError::with_source(RemoteErrorType::NoSuchFileOrDirectory, err)
            }
            Ok(_) => RemoteError::with_source(RemoteErrorType::PermissionDenied, err),
        }
    }

    /// Raises the transfer flag and returns the guard that clears it.
    ///
    /// Must be called while the client lock is held.
    fn start_transfer(&self) -> TransferGuard {
        self.transfer_active.store(true, Ordering::SeqCst);
        TransferGuard::new(
            Arc::clone(&self.transfer_active),
            Arc::clone(&self.connection_usable),
        )
    }

    /// Validates that `path` is an encodable absolute FTP path and renders it for the wire.
    fn remote_path(path: &Path) -> RemoteResult<String> {
        ensure_absolute(path)?;
        #[cfg(target_family = "unix")]
        {
            let path = path.to_str().ok_or_else(|| {
                RemoteError::with_message(
                    RemoteErrorType::InvalidPath,
                    "FTP paths must contain valid UTF-8",
                )
            })?;
            if !path.starts_with('/')
                || path.starts_with("//")
                || path.bytes().any(|byte| byte.is_ascii_control())
                || Path::new(path)
                    .components()
                    .any(|component| component == Component::ParentDir)
            {
                return Err(RemoteError::with_message(
                    RemoteErrorType::InvalidPath,
                    "FTP paths must use a POSIX absolute root",
                ));
            }
            Ok(path.to_owned())
        }
        #[cfg(target_os = "windows")]
        {
            use path_slash::PathExt as _;

            let path = path.to_slash().ok_or_else(|| {
                RemoteError::with_message(
                    RemoteErrorType::InvalidPath,
                    "FTP paths must contain valid UTF-8",
                )
            })?;
            if !path.starts_with('/')
                || path.starts_with("//")
                || path.bytes().any(|byte| byte.is_ascii_control())
                || Path::new(&path)
                    .components()
                    .any(|component| component == Component::ParentDir)
            {
                return Err(RemoteError::with_message(
                    RemoteErrorType::InvalidPath,
                    "FTP paths must use a single POSIX absolute root",
                ));
            }
            Ok(path.into_owned())
        }
    }

    /// Requests a native FTP restart marker, falling back to a local skip when refused.
    fn request_offset(stream: &mut FtpStream, offset: u64) -> RemoteResult<bool> {
        let Ok(offset) = usize::try_from(offset) else {
            log::warn!("offset {offset} does not fit the REST command; skipping locally");
            return Ok(false);
        };
        match stream.resume_transfer(offset) {
            Ok(()) => Ok(true),
            Err(FtpError::UnexpectedResponse(response))
                if response.status != Status::NotAvailable =>
            {
                log::warn!(
                    "server refused REST {offset} ({}); skipping locally",
                    response.status
                );
                Ok(false)
            }
            Err(error) => {
                log::error!("Failed to request offset {offset}: {error}");
                Err(ftp_error(error))
            }
        }
    }

    /// Fix provided path; on Windows fixes the backslashes, converting them to slashes
    /// While on POSIX does nothing
    #[cfg(target_os = "windows")]
    fn resolve(p: &Path) -> PathBuf {
        use path_slash::PathExt as _;
        p.to_slash()
            .map(std::borrow::Cow::into_owned)
            .map(PathBuf::from)
            .unwrap_or_default()
    }

    #[cfg(target_family = "unix")]
    fn resolve(p: &Path) -> PathBuf {
        p.to_path_buf()
    }

    #[cfg(feature = "native-tls")]
    fn setup_tls_connector(&self) -> RemoteResult<TlsConnector> {
        NativeTlsConnector::builder()
            .danger_accept_invalid_certs(self.accept_invalid_certs)
            .danger_accept_invalid_hostnames(self.accept_invalid_hostnames)
            .build()
            .map_err(|e| {
                error!("Failed to setup TLS stream: {}", e);
                RemoteError::with_source(RemoteErrorType::ConnectionError, e)
            })
            .map(|x| x.into())
    }

    #[cfg(any(feature = "rustls-aws-lc-rs", feature = "rustls-ring"))]
    fn setup_tls_connector(&self) -> RemoteResult<TlsConnector> {
        let mut root_store = suppaftp::rustls::RootCertStore::empty();
        root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().map(|ta| {
            rustls_pki_types::TrustAnchor {
                subject: ta.subject.clone(),
                subject_public_key_info: ta.subject_public_key_info.clone(),
                name_constraints: ta.name_constraints.clone(),
            }
        }));
        Ok(std::sync::Arc::new(
            ClientConfig::builder()
                .with_root_certificates(root_store)
                .with_no_client_auth(),
        )
        .into())
    }

    /// Lists a path without first asserting that the path is a directory.
    ///
    /// This is used by metadata lookup so `list_dir` can validate a regular-file
    /// argument without recursing through `stat` and back into `list_dir`.
    fn list_dir_raw(&self, path: &Path, remote: &str) -> RemoteResult<Vec<File>> {
        let bytes = self.with_exclusive_stream(|stream| {
            let (result, requires_reconnect) = read_list_bytes(stream, remote);
            if requires_reconnect {
                self.mark_connection_unusable();
            }
            if let Err(error) = &result {
                error!("Failed to list directory: {}", error);
            }
            result
        })?;
        let lines = decode_list_bytes(bytes).map_err(ftp_error)?;
        parse_list_lines(path, lines)
    }
}

/// Returns whether a failed data-command setup may have left replies unsynchronized.
fn transfer_setup_requires_reconnect(err: &FtpError) -> bool {
    match err {
        FtpError::ConnectionError(_) | FtpError::BadResponse => true,
        #[cfg(any(
            feature = "native-tls",
            feature = "rustls-aws-lc-rs",
            feature = "rustls-ring"
        ))]
        FtpError::SecureError(_) => true,
        // A 421 closes the service. Other server replies have been consumed,
        // including the preliminary refusal path in `data_command_with_response`.
        FtpError::UnexpectedResponse(response) => response.status == Status::NotAvailable,
        FtpError::InvalidAddress(_) | FtpError::DataConnectionAlreadyOpen => true,
    }
}

/// Returns whether a mapped error means the control channel may be out of sync.
fn remote_error_requires_reconnect(err: &RemoteError) -> bool {
    let mut source = err.source();
    while let Some(error) = source {
        if let Some(ftp_error) = error.downcast_ref::<FtpError>() {
            return transfer_setup_requires_reconnect(ftp_error);
        }
        source = error.source();
    }
    err.kind() == RemoteErrorType::ConnectionError
}

/// Returns whether an FTP reply used the ambiguous 550 refusal code.
fn is_file_unavailable(err: &FtpError) -> bool {
    matches!(
        err,
        FtpError::UnexpectedResponse(response) if response.status == Status::FileUnavailable
    )
}

/// Returns whether a create or rename refusal can indicate a missing parent.
fn is_creation_refusal(err: &FtpError) -> bool {
    matches!(
        err,
        FtpError::UnexpectedResponse(response)
            if matches!(response.status, Status::FileUnavailable | Status::BadFilename)
    )
}

/// Returns whether an error preserves an ambiguous FTP 550 refusal.
fn is_ambiguous_path_refusal(err: &RemoteError) -> bool {
    let mut source = err.source();
    while let Some(error) = source {
        if let Some(FtpError::UnexpectedResponse(response)) = error.downcast_ref::<FtpError>() {
            return response.status == Status::FileUnavailable;
        }
        source = error.source();
    }
    false
}

/// Preserves an ambiguous 550 refusal when probing cannot prove absence.
fn ambiguous_path_permission(err: RemoteError) -> RemoteError {
    RemoteError::with_source(RemoteErrorType::PermissionDenied, err)
}

/// Returns whether a failed LIST setup can leave the control channel unusable.
fn list_setup_requires_reconnect(err: &FtpError) -> bool {
    transfer_setup_requires_reconnect(err)
}

/// Reads raw `LIST` bytes and reports whether cleanup requires reconnecting.
fn read_list_bytes(stream: &mut FtpStream, remote: &str) -> (RemoteResult<Vec<u8>>, bool) {
    let (_, mut transfer) = match stream.custom_data_command(
        format!("LIST {remote}"),
        &[Status::AboutToSend, Status::AlreadyOpen],
    ) {
        Ok(transfer) => transfer,
        Err(err) => {
            let requires_reconnect = list_setup_requires_reconnect(&err);
            return (Err(ftp_error(err)), requires_reconnect);
        }
    };
    let mut bytes = Vec::new();
    let read_result = transfer
        .read_to_end(&mut bytes)
        .map(|_| ())
        .map_err(FtpError::ConnectionError);
    let finish_result = transfer.finish();
    match (read_result, finish_result) {
        (Ok(()), Ok(())) => (Ok(bytes), false),
        (Err(read), Ok(())) => (Err(ftp_error(read)), true),
        (Ok(()), Err(finish)) => (Err(ftp_error(finish)), true),
        (Err(read), Err(finish)) => {
            let read = ftp_error(read);
            let finish = ftp_error(finish);
            (
                Err(RemoteError::with_source(
                    read.kind(),
                    ListCleanupFailure { read, finish },
                )),
                true,
            )
        }
    }
}

/// Retains both failures from a raw LIST data read and its completion reply.
#[derive(Debug)]
struct ListCleanupFailure {
    read: RemoteError,
    finish: RemoteError,
}

impl fmt::Display for ListCleanupFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "LIST read failed: {}; completion failed: {}",
            self.read, self.finish
        )
    }
}

impl std::error::Error for ListCleanupFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.finish)
    }
}

/// Decodes raw `LIST` bytes without replacing invalid UTF-8.
fn decode_list_bytes(bytes: Vec<u8>) -> FtpResult<Vec<String>> {
    String::from_utf8(bytes)
        .map_err(|_| FtpError::BadResponse)
        .map(|listing| {
            listing
                .lines()
                .filter(|line| !line.is_empty())
                .map(ToOwned::to_owned)
                .collect()
        })
}

fn parse_list_lines(path: &Path, lines: Vec<String>) -> RemoteResult<Vec<File>> {
    let entries = lines
        .into_iter()
        .enumerate()
        // Some servers prepend the POSIX block count; it is not a file entry.
        .filter(|(index, line)| {
            !(*index == 0
                && line.strip_prefix("total ").is_some_and(|count| {
                    !count.is_empty() && count.bytes().all(|byte| byte.is_ascii_digit())
                }))
        })
        .map(|(_, line)| {
            // The generic parser also tries MLSD and accepts arbitrary text as
            // a filename. LIST uses POSIX or DOS entries, not MLSD facts.
            reject_ambiguous_list_name(&line)?;
            let f = parse_list_entry(&line)
                .map_err(|err| RemoteError::with_source(RemoteErrorType::ProtocolError, err))?;
            if matches!(f.name(), "." | "..") {
                return Ok(None);
            }
            validate_list_child_name(f.name())?;
            let file_type = if f.is_symlink() {
                FileType::Symlink
            } else if f.is_directory() {
                FileType::Directory
            } else {
                FileType::File
            };
            let mut metadata = Metadata::default()
                .file_type(file_type)
                .size(f.size() as u64)
                .modified(f.modified())
                .mode(query_unix_pex(&f));
            if let Some(gid) = f.gid() {
                metadata = metadata.gid(gid);
            }
            if let Some(uid) = f.uid() {
                metadata = metadata.uid(uid);
            }
            if let Some(target) = f.symlink() {
                metadata = metadata.symlink(path_utils::absolutize(path, target));
            }
            Ok(Some(File::new(path.join(f.name()), metadata)))
        })
        .collect::<RemoteResult<Vec<_>>>()?;
    Ok(entries.into_iter().flatten().collect())
}

/// Rejects listing rows whose format cannot preserve the exact server name.
fn reject_ambiguous_list_name(line: &str) -> RemoteResult<()> {
    reject_ambiguous_posix_name_boundary(line)?;
    if line.starts_with('l') && line.matches(" -> ").count() > 1 {
        return Err(RemoteError::with_message(
            RemoteErrorType::ProtocolError,
            "LIST returned a symlink name or target containing multiple ` -> ` separators",
        ));
    }
    if !line.starts_with('-')
        && !line.starts_with('l')
        && !line.starts_with('d')
        && looks_like_dos_listing(line)
    {
        reject_ambiguous_dos_name_boundary(line)?;
    }
    Ok(())
}

/// Rejects POSIX rows whose whitespace boundary could hide a leading-space name.
fn reject_ambiguous_posix_name_boundary(line: &str) -> RemoteResult<()> {
    let is_special = matches!(
        line.as_bytes().first(),
        Some(b'b' | b'c' | b'p' | b's' | b'D')
    );
    let normalized = if is_special {
        normalize_special_posix_line(line)
    } else {
        line.to_owned()
    };
    let spans = field_spans(&normalized);
    let is_posix = matches!(
        line.as_bytes().first(),
        Some(b'-' | b'l' | b'd' | b'b' | b'c' | b'p' | b's' | b'D')
    );
    let has_ambiguous_boundary =
        is_posix && spans.len() >= 9 && spans[7].1.saturating_add(1) < spans[8].0;
    if has_ambiguous_boundary {
        return Err(RemoteError::with_message(
            RemoteErrorType::ProtocolError,
            "LIST returned a filename with an ambiguous leading-space boundary",
        ));
    }
    if is_posix
        && spans
            .get(7)
            .is_some_and(|(_, end)| contains_unicode_whitespace(&normalized[..*end]))
    {
        return Err(RemoteError::with_message(
            RemoteErrorType::ProtocolError,
            "LIST returned Unicode whitespace in its POSIX metadata",
        ));
    }
    let name_suffix = spans.get(7).map_or("", |(_, end)| &normalized[*end..]);
    if is_posix && !name_suffix.is_empty() && name_suffix.chars().all(char::is_whitespace) {
        return Err(RemoteError::with_message(
            RemoteErrorType::ProtocolError,
            "LIST returned a filename made only of whitespace",
        ));
    }
    if is_posix
        && spans.len() >= 8
        && has_unicode_whitespace_after_ascii_padding(&normalized[spans[7].1..])
    {
        return Err(RemoteError::with_message(
            RemoteErrorType::ProtocolError,
            "LIST returned a filename with an ambiguous Unicode-whitespace boundary",
        ));
    }
    Ok(())
}

/// Returns whether a row starts with the date/time shape used by DOS `LIST`.
fn looks_like_dos_listing(line: &str) -> bool {
    let bytes = line.as_bytes();
    bytes.len() >= 8
        && bytes[2] == b'-'
        && bytes[5] == b'-'
        && bytes[0].is_ascii_digit()
        && bytes[1].is_ascii_digit()
        && bytes[3].is_ascii_digit()
        && bytes[4].is_ascii_digit()
        && bytes[6].is_ascii_digit()
        && bytes[7].is_ascii_digit()
}

/// Rejects DOS rows whose metadata separator could hide a leading-space name.
fn reject_ambiguous_dos_name_boundary(line: &str) -> RemoteResult<()> {
    // Windows FTP listings conventionally align names at byte column 39.
    const PADDED_METADATA_COLUMN: usize = 24;
    const PADDED_NAME_COLUMN: usize = 39;

    let Some(time_start) = line[8..]
        .find(|character: char| !character.is_ascii_whitespace())
        .map(|index| index + 8)
    else {
        return Ok(());
    };
    let timestamp = &line[time_start..];
    let timestamp_prefix_end = timestamp
        .char_indices()
        .nth(8)
        .map_or(timestamp.len(), |(index, _)| index);
    let timestamp_prefix = &timestamp[..timestamp_prefix_end];
    let Some(time_end) = ["AM", "PM"].iter().find_map(|marker| {
        timestamp_prefix
            .find(marker)
            .map(|index| time_start + index + marker.len())
    }) else {
        return Err(RemoteError::with_message(
            RemoteErrorType::ProtocolError,
            "LIST returned an unsupported DOS timestamp",
        ));
    };
    let rest = &line[time_end..];
    if rest
        .chars()
        .next()
        .is_some_and(|character| character.is_whitespace() && !character.is_ascii_whitespace())
    {
        return Err(RemoteError::with_message(
            RemoteErrorType::ProtocolError,
            "LIST returned a DOS filename with an ambiguous Unicode-whitespace boundary",
        ));
    }
    let Some(metadata_start) = rest.find(|character: char| !character.is_ascii_whitespace()) else {
        return Ok(());
    };
    let metadata = &rest[metadata_start..];
    let metadata_end = metadata
        .find(|character: char| character.is_ascii_whitespace())
        .unwrap_or(metadata.len());
    let metadata_token = &metadata[..metadata_end];
    let after_metadata = &metadata[metadata_end..];
    if after_metadata.chars().any(char::is_whitespace)
        && after_metadata.chars().all(char::is_whitespace)
    {
        return Err(RemoteError::with_message(
            RemoteErrorType::ProtocolError,
            "LIST returned a filename made only of whitespace",
        ));
    }
    if has_unicode_whitespace_after_ascii_padding(after_metadata) {
        return Err(RemoteError::with_message(
            RemoteErrorType::ProtocolError,
            "LIST returned a filename with an ambiguous Unicode-whitespace boundary",
        ));
    }
    let separator = after_metadata
        .bytes()
        .take_while(|byte| byte.is_ascii_whitespace())
        .count();
    let metadata_column = time_end + metadata_start;
    let name_start = time_end + metadata_start + metadata_end + separator;
    let is_supported_padded_layout = metadata_token == "<DIR>"
        && metadata_column == PADDED_METADATA_COLUMN
        && name_start == PADDED_NAME_COLUMN;
    if separator > 1 && !is_supported_padded_layout {
        return Err(RemoteError::with_message(
            RemoteErrorType::ProtocolError,
            "LIST returned a DOS filename with an ambiguous leading-space boundary",
        ));
    }
    Ok(())
}

/// Detects a non-ASCII whitespace character after the format's ASCII padding.
fn has_unicode_whitespace_after_ascii_padding(text: &str) -> bool {
    text.trim_start_matches(|character: char| character.is_ascii_whitespace())
        .chars()
        .next()
        .is_some_and(|character| character.is_whitespace())
}

/// Returns whether text contains Unicode whitespace rather than ASCII padding.
fn contains_unicode_whitespace(text: &str) -> bool {
    text.chars()
        .any(|character| character.is_whitespace() && !character.is_ascii_whitespace())
}

/// Parses a POSIX or DOS `LIST` entry, accepting POSIX special files as files.
fn parse_list_entry(line: &str) -> Result<FtpFile, suppaftp::list::ParseError> {
    let is_special = matches!(
        line.as_bytes().first(),
        Some(b'b' | b'c' | b'p' | b's' | b'D')
    );
    if is_special {
        let mut normalized = normalize_special_posix_line(line);
        normalized.replace_range(..1, "-");
        ListParser::parse_posix(&normalized).or_else(|_| ListParser::parse_dos(line))
    } else {
        ListParser::parse_posix(line).or_else(|_| ListParser::parse_dos(line))
    }
}

/// Normalizes device-node major/minor fields to the regular-file size format.
fn normalize_special_posix_line(line: &str) -> String {
    let spans = field_spans(line);
    let is_device = spans.len() > 5
        && line
            .as_bytes()
            .first()
            .is_some_and(|byte| matches!(byte, b'b' | b'c'))
        && line[spans[4].0..spans[4].1].ends_with(',')
        && line[spans[4].0..spans[4].1]
            .trim_end_matches(',')
            .bytes()
            .all(|byte| byte.is_ascii_digit())
        && line[spans[5].0..spans[5].1]
            .bytes()
            .all(|byte| byte.is_ascii_digit());
    if !is_device {
        return line.to_owned();
    }

    let mut normalized = line.to_owned();
    normalized.replace_range(spans[4].0..spans[5].1, "0");
    normalized
}

/// Returns byte ranges for the whitespace-separated fields of a LIST line.
fn field_spans(line: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut start = None;
    for (index, byte) in line.bytes().enumerate() {
        if byte.is_ascii_whitespace() {
            if let Some(start) = start.take() {
                spans.push((start, index));
            }
        } else if start.is_none() {
            start = Some(index);
        }
    }
    if let Some(start) = start {
        spans.push((start, line.len()));
    }
    spans
}

/// Ensures a server-supplied listing name cannot escape its listed directory.
fn validate_list_child_name(name: &str) -> RemoteResult<()> {
    let path = Path::new(name);
    let has_path_separator = name
        .bytes()
        .any(|byte| byte == b'/' || (cfg!(target_os = "windows") && byte == b'\\'));
    let is_direct_child = !name.is_empty()
        && name != "."
        && name != ".."
        && !name.bytes().any(|byte| byte.is_ascii_control())
        && !has_path_separator
        && path.components().count() == 1
        && matches!(path.components().next(), Some(Component::Normal(_)));
    if is_direct_child {
        Ok(())
    } else {
        Err(RemoteError::with_message(
            RemoteErrorType::ProtocolError,
            format!("LIST returned an unsafe child name: {name:?}"),
        ))
    }
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

impl RemoteFs for FtpFs {
    fn connect(&mut self) -> RemoteResult<()> {
        if self.transfer_active.load(Ordering::SeqCst) {
            return Err(RemoteError::with_message(
                RemoteErrorType::ProtocolError,
                "a data transfer is in progress; finish or drop its stream first",
            ));
        }
        if self.connection_usable.load(Ordering::SeqCst) && self.lock().is_some() {
            return Err(RemoteError::new(RemoteErrorType::AlreadyConnected));
        }
        if !self.connection_usable.load(Ordering::SeqCst) {
            // The old stream may contain unread replies. Dropping it is the
            // only safe recovery; the next connection starts with fresh state.
            self.lock().take();
            self.connection_usable.store(true, Ordering::SeqCst);
        }
        info!("Connecting to {}:{}", self.hostname, self.port);
        let mut stream =
            FtpStream::connect(format!("{}:{}", self.hostname, self.port)).map_err(|e| {
                error!("Failed to connect to remote server: {}", e);
                ftp_error(e)
            })?;

        // if provided, set passive stream builder
        if let Some(builder) = &self.passive_stream_builder {
            debug!("Setting up a custom passive stream builder");
            let builder = Arc::clone(builder);
            let connection_usable = Arc::clone(&self.connection_usable);
            stream = stream.passive_stream_builder(move |address| {
                let result = builder(address);
                if result.is_err() {
                    connection_usable.store(false, Ordering::SeqCst);
                }
                result
            });
        };
        stream.set_mode(self.mode);

        // If secure, connect TLS
        #[cfg(any(
            feature = "native-tls",
            feature = "rustls-aws-lc-rs",
            feature = "rustls-ring"
        ))]
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
                    RemoteError::with_source(RemoteErrorType::ConnectionError, e)
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
                error!("Login failed: {e}");
                ftp_error(e)
            })?;
        trace!("Setting transfer type to Binary");
        stream
            .transfer_type(SuppaFtpFileType::Binary)
            .map_err(|e| {
                error!("Failed to set transfer type to Binary: {}", e);
                ftp_error(e)
            })?;
        info!("Connection established!");
        *self.lock() = Some(stream);
        Ok(())
    }

    fn disconnect(&mut self) -> RemoteResult<()> {
        info!("Disconnecting from FTP server...");
        if self.transfer_active.load(Ordering::SeqCst) {
            return Err(RemoteError::with_message(
                RemoteErrorType::ProtocolError,
                "a data transfer is in progress; finish or drop its stream first",
            ));
        }
        let usable = self.connection_usable.load(Ordering::SeqCst);
        let Some(mut stream) = self.lock().take() else {
            return Err(RemoteError::new(RemoteErrorType::NotConnected));
        };
        self.connection_usable.store(true, Ordering::SeqCst);
        if !usable {
            drop(stream);
            return Err(RemoteError::with_message(
                RemoteErrorType::ConnectionError,
                "the control connection was unusable and has been closed",
            ));
        }
        let result = stream.quit().map_err(|e| {
            error!("Failed to disconnect from remote: {}", e);
            ftp_error(e)
        });
        if let Err(error) = &result {
            self.record_remote_error(error);
        }
        result
    }

    fn is_connected(&self) -> bool {
        self.connection_usable.load(Ordering::SeqCst)
            && (self.lock().is_some() || self.transfer_active.load(Ordering::SeqCst))
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities::STREAM_READ
            | Capabilities::STREAM_WRITE
            | Capabilities::APPEND
            | Capabilities::RANGE_READ
            | Capabilities::EXEC
    }

    fn list_dir(&self, path: &Path) -> RemoteResult<Vec<File>> {
        debug!("Getting list entries for {}", path.display());
        let remote = Self::remote_path(path)?;
        let path = Self::resolve(path);
        let entries = match self.list_dir_raw(&path, &remote) {
            Ok(entries) => entries,
            Err(error) if is_ambiguous_path_refusal(&error) => {
                if self.ancestor_proves_absent(&path)? {
                    return Err(RemoteError::with_source(
                        RemoteErrorType::NoSuchFileOrDirectory,
                        error,
                    ));
                }
                return Err(ambiguous_path_permission(error));
            }
            Err(error) => return Err(error),
        };
        if path != Path::new("/") {
            let target_name = path.file_name().and_then(|name| name.to_str());
            let target_is_file = entries.len() == 1
                && target_name.is_some_and(|name| entries[0].name() == name)
                && entries[0].is_file();
            if target_is_file && matches!(self.stat(&path), Ok(entry) if entry.is_file()) {
                return Err(RemoteError::with_message(
                    RemoteErrorType::BadFile,
                    "LIST requires a directory path",
                ));
            }
            if entries.is_empty() && matches!(self.stat(&path), Ok(entry) if entry.is_file()) {
                return Err(RemoteError::with_message(
                    RemoteErrorType::BadFile,
                    "LIST requires a directory path",
                ));
            }
        }
        Ok(entries)
    }

    fn stat(&self, path: &Path) -> RemoteResult<File> {
        debug!("Getting file information for {}", path.display());
        let _remote = Self::remote_path(path)?;
        let path = Self::resolve(path);
        if path == Path::new("/") {
            trace!("{} has no parent: returning root", path.display());
            return self.with_stream(|_| {
                Ok(File::new(
                    path,
                    Metadata::default().file_type(FileType::Directory),
                ))
            });
        }
        let parent = path
            .parent()
            .expect("a non-root absolute path must have a parent");
        trace!("Listing entries for stat path file: {}", parent.display());
        let remote_parent = Self::remote_path(parent)?;
        let entries = match self.list_dir_raw(parent, &remote_parent) {
            Ok(entries) => entries,
            Err(error) if is_ambiguous_path_refusal(&error) => {
                if self.ancestor_proves_absent(parent)? {
                    return Err(RemoteError::with_source(
                        RemoteErrorType::NoSuchFileOrDirectory,
                        error,
                    ));
                }
                return Err(ambiguous_path_permission(error));
            }
            Err(error) => return Err(error),
        };
        let parent_name = parent.file_name().and_then(|name| name.to_str());
        let parent_is_ambiguous_file_listing = entries.len() == 1
            && parent_name.is_some_and(|name| entries[0].name() == name)
            && entries[0].is_file();
        if parent_is_ambiguous_file_listing {
            match self.stat(parent) {
                Ok(parent_entry) if parent_entry.is_file() => {
                    return Err(RemoteError::with_message(
                        RemoteErrorType::BadFile,
                        "the parent path is a regular file",
                    ));
                }
                Ok(_) => {}
                Err(error) if error.kind() == RemoteErrorType::PermissionDenied => {}
                Err(error) => return Err(error),
            }
        }
        entries
            .into_iter()
            .find(|entry| entry.path() == path.as_path())
            .ok_or_else(|| {
                error!("Could not find file; no such file or directory");
                RemoteError::new(RemoteErrorType::NoSuchFileOrDirectory)
            })
    }

    fn exists(&self, path: &Path) -> RemoteResult<bool> {
        debug!("Checking whether {} exists", path.display());
        match self.stat(path) {
            Ok(_) => Ok(true),
            Err(err) if err.kind() == RemoteErrorType::NoSuchFileOrDirectory => Ok(false),
            Err(err) => Err(err),
        }
    }

    fn set_metadata(&self, path: &Path, _metadata: &SetMetadata) -> RemoteResult<()> {
        Self::remote_path(path)?;
        Err(RemoteError::new(RemoteErrorType::UnsupportedFeature))
    }

    fn create_dir(&self, path: &Path, _mode: Option<UnixPex>) -> RemoteResult<()> {
        debug!("Trying to create directory {}", path.display());
        let remote = Self::remote_path(path)?;
        let result = self.with_stream(|stream| {
            let result = stream.mkdir(&remote);
            if let Err(error) = &result {
                self.record_ftp_error(error);
            }
            Ok(result)
        })?;
        match result {
            Ok(()) => Ok(()),
            Err(FtpError::UnexpectedResponse(response))
                if matches!(
                    response.status,
                    Status::FileUnavailable | Status::BadFilename
                ) =>
            {
                let err = FtpError::UnexpectedResponse(response);
                let error = self.classify_create_dir_refusal(path, err);
                error!("Failed to create directory: {error}");
                Err(error)
            }
            Err(e) => {
                error!("Failed to create directory: {}", e);
                Err(ftp_error(e))
            }
        }
    }

    fn remove_file(&self, path: &Path) -> RemoteResult<()> {
        debug!("Removing file {}", path.display());
        let remote = Self::remote_path(path)?;
        let result = self.with_stream(|stream| {
            let result = stream.rm(&remote);
            if let Err(error) = &result {
                self.record_ftp_error(error);
            }
            Ok(result)
        })?;
        result.map_err(|err| {
            let error = self.classify_file_refusal(path, err);
            error!("Failed to remove file: {error}");
            error
        })
    }

    fn remove_dir(&self, path: &Path) -> RemoteResult<()> {
        debug!("Removing directory {}", path.display());
        let remote = Self::remote_path(path)?;
        let result = self.with_stream(|stream| {
            let result = stream.rmdir(&remote);
            if let Err(error) = &result {
                self.record_ftp_error(error);
            }
            Ok(result)
        })?;
        result.map_err(|err| {
            self.record_ftp_error(&err);
            let error = self.classify_remove_dir_refusal(path, err);
            error!("Failed to remove directory: {error}");
            error
        })
    }

    fn rename(&self, src: &Path, dest: &Path) -> RemoteResult<()> {
        debug!("Trying to rename {} to {}", src.display(), dest.display());
        let remote_src = Self::remote_path(src)?;
        let remote_dest = Self::remote_path(dest)?;
        let result = self.with_stream(|stream| {
            let result = stream.rename(&remote_src, &remote_dest);
            if let Err(error) = &result {
                self.record_ftp_error(error);
            }
            Ok(result)
        })?;
        result.map_err(|err| {
            let error = self.classify_rename_refusal(src, dest, err);
            error!("Failed to rename file: {error}");
            error
        })
    }

    fn copy(&self, src: &Path, dest: &Path) -> RemoteResult<()> {
        Self::remote_path(src)?;
        Self::remote_path(dest)?;
        Err(RemoteError::new(RemoteErrorType::UnsupportedFeature))
    }

    fn symlink(&self, path: &Path, target: &Path) -> RemoteResult<()> {
        Self::remote_path(path)?;
        Self::remote_path(target)?;
        Err(RemoteError::new(RemoteErrorType::UnsupportedFeature))
    }

    fn open(&self, path: &Path, opts: &ReadOptions) -> RemoteResult<ReadStream> {
        debug!("Opening {} for read ({opts:?})", path.display());
        let remote = Self::remote_path(path)?;
        let offset = opts.offset.unwrap_or(0);
        let transfer_result = self.with_stream(|stream| {
            let mut resumed = offset > 0 && Self::request_offset(stream, offset)?;
            let mut result = stream.retr_as_stream(&remote);
            if resumed && result.is_err() {
                // Reset the marker after every failed setup so it cannot affect the
                // fallback transfer or a later managed operation.
                match stream.resume_transfer(0) {
                    Ok(()) => {
                        resumed = false;
                        result = stream.retr_as_stream(&remote);
                    }
                    Err(error) => {
                        self.mark_connection_unusable();
                        error!("Failed to reset REST marker after RETR refusal: {error}");
                        return Err(ftp_error(error));
                    }
                }
            }
            if let Err(err) = &result {
                self.record_ftp_error(err);
            }
            Ok(result.map(|transfer| (transfer, self.start_transfer(), resumed)))
        })?;
        let (transfer, guard, resumed) = transfer_result.map_err(|err| {
            error!("Failed to open file: {err}");
            self.classify_file_refusal(path, err)
        })?;
        let mut reader = FtpReadStream::new(transfer, opts.length, guard);
        if offset > 0 && !resumed {
            debug!("Skipping {offset} bytes locally");
            if let Err(skip) = reader.skip_prefix(offset) {
                return Err(reader.finish_after_skip(skip));
            }
        }
        Ok(ReadStream::new(reader))
    }

    fn create(&self, path: &Path, opts: &WriteOptions) -> RemoteResult<WriteStream> {
        debug!("Opening {} for write ({opts:?})", path.display());
        let remote = Self::remote_path(path)?;
        let transfer_result = self.with_stream(|stream| {
            let result = stream.put_with_stream(&remote);
            if let Err(err) = &result {
                self.record_ftp_error(err);
            }
            Ok(result.map(|transfer| (transfer, self.start_transfer())))
        })?;
        let (transfer, guard) = transfer_result.map_err(|err| {
            error!("Failed to open file: {err}");
            self.classify_create_refusal(path, err)
        })?;
        Ok(WriteStream::new(FtpWriteStream::new(transfer, guard)))
    }

    fn append(&self, path: &Path, opts: &WriteOptions) -> RemoteResult<WriteStream> {
        debug!("Opening {} for append ({opts:?})", path.display());
        let remote = Self::remote_path(path)?;
        let transfer_result = self.with_stream(|stream| {
            let result = stream.append_with_stream(&remote);
            if let Err(err) = &result {
                self.record_ftp_error(err);
            }
            Ok(result.map(|transfer| (transfer, self.start_transfer())))
        })?;
        let (transfer, guard) = transfer_result.map_err(|err| {
            error!("Failed to open file: {err}");
            self.classify_create_refusal(path, err)
        })?;
        Ok(WriteStream::new(FtpWriteStream::new(transfer, guard)))
    }

    fn exec(&self, cmd: &str) -> RemoteResult<ExecOutput> {
        debug!("Executing command: {cmd}");
        let response = self.with_stream(|stream| {
            stream.site(cmd).map_err(|e| {
                error!("Failed to execute command: {}", e);
                ftp_error(e)
            })
        })?;
        let status = response.status.code();
        debug!("Command executed with status {status}");
        Ok(ExecOutput::new(
            status,
            String::from_utf8_lossy(&response.body).into_owned(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead as _, BufReader, Write as _};
    use std::net::{TcpListener, TcpStream};
    use std::thread;
    use std::time::Duration;

    use suppaftp::list::ParseError;
    use suppaftp::types::Response;

    use super::*;

    #[test]
    fn unsupported_operations_validate_every_path() {
        let client = FtpFs::new("localhost", 21);
        let absolute = Path::new("/file");
        let relative = Path::new("file");
        for result in [
            client.set_metadata(relative, &SetMetadata::default()),
            client.copy(relative, absolute),
            client.copy(absolute, relative),
            client.symlink(relative, absolute),
            client.symlink(absolute, relative),
        ] {
            assert_eq!(result.unwrap_err().kind(), RemoteErrorType::InvalidPath);
        }
        for result in [
            client.set_metadata(absolute, &SetMetadata::default()),
            client.copy(absolute, absolute),
            client.symlink(absolute, absolute),
        ] {
            assert_eq!(
                result.unwrap_err().kind(),
                RemoteErrorType::UnsupportedFeature
            );
        }
        let parent = Path::new("/dir/../file");
        for result in [
            client.set_metadata(parent, &SetMetadata::default()),
            client.copy(parent, absolute),
            client.symlink(parent, absolute),
        ] {
            assert_eq!(result.unwrap_err().kind(), RemoteErrorType::InvalidPath);
        }
    }

    #[test]
    fn disconnected_root_queries_fail() {
        let client = FtpFs::new("localhost", 21);
        assert_eq!(
            client.stat(Path::new("/")).unwrap_err().kind(),
            RemoteErrorType::NotConnected
        );
        assert_eq!(
            client.exists(Path::new("/")).unwrap_err().kind(),
            RemoteErrorType::NotConnected
        );
    }

    #[test]
    fn failed_transfer_setup_requires_reconnect_for_unsynchronized_failures() {
        for error in [
            FtpError::ConnectionError(std::io::ErrorKind::ConnectionReset.into()),
            FtpError::BadResponse,
        ] {
            assert!(transfer_setup_requires_reconnect(&error));
        }
        for error in [
            FtpError::InvalidAddress("invalid".parse::<std::net::SocketAddr>().unwrap_err()),
            FtpError::DataConnectionAlreadyOpen,
        ] {
            assert!(transfer_setup_requires_reconnect(&error));
        }
        assert!(!transfer_setup_requires_reconnect(
            &FtpError::UnexpectedResponse(Response {
                status: Status::FileUnavailable,
                body: b"missing or denied".to_vec(),
            },)
        ));
        assert!(transfer_setup_requires_reconnect(
            &FtpError::UnexpectedResponse(Response {
                status: Status::NotAvailable,
                body: b"service closing".to_vec(),
            })
        ));
        for status in [
            Status::CannotOpenDataConnection,
            Status::TransferAborted,
            Status::ActionAborted,
        ] {
            assert!(!transfer_setup_requires_reconnect(
                &FtpError::UnexpectedResponse(Response {
                    status,
                    body: b"server refusal".to_vec(),
                })
            ));
        }
    }

    #[test]
    fn command_error_mapping_distinguishes_sync_failures_from_refusals() {
        let transport = ftp_error(FtpError::ConnectionError(
            std::io::ErrorKind::TimedOut.into(),
        ));
        assert!(remote_error_requires_reconnect(&transport));
        assert!(remote_error_requires_reconnect(&ftp_error(
            FtpError::BadResponse
        )));
        assert!(!remote_error_requires_reconnect(&ftp_error(
            FtpError::UnexpectedResponse(Response {
                status: Status::FileUnavailable,
                body: b"permission denied".to_vec(),
            }),
        )));
        assert!(remote_error_requires_reconnect(&ftp_error(
            FtpError::UnexpectedResponse(Response {
                status: Status::NotAvailable,
                body: b"service closing".to_vec(),
            }),
        )));
        for status in [Status::CannotOpenDataConnection, Status::TransferAborted] {
            assert!(!remote_error_requires_reconnect(&ftp_error(
                FtpError::UnexpectedResponse(Response {
                    status,
                    body: b"server refusal".to_vec(),
                }),
            )));
        }
    }

    #[test]
    fn malformed_list_entries_preserve_a_typed_protocol_error() {
        for line in [
            "",
            "garbage",
            "total nope",
            "total 12 trailing",
            "-rw-r--r-- broken",
        ] {
            let err = parse_list_lines(Path::new("/"), vec![line.to_string()]).unwrap_err();
            assert_eq!(err.kind(), RemoteErrorType::ProtocolError, "line {line:?}");
            assert!(err.source().unwrap().is::<ParseError>());
        }
        let err = parse_list_lines(
            Path::new("/"),
            vec![
                "-rw-r--r-- 1 1000 1000 4 Nov 5 2024 valid.txt".to_string(),
                "garbage".to_string(),
            ],
        )
        .unwrap_err();
        assert_eq!(err.kind(), RemoteErrorType::ProtocolError);
    }

    #[test]
    fn list_parses_entries_and_only_ignores_a_valid_leading_total() {
        let entries = parse_list_lines(
            Path::new("/dir"),
            vec![
                "total 12".to_string(),
                "-rw-r--r-- 1 1000 1000 4 Nov 5 2024 valid.txt".to_string(),
                "10-19-20  03:19PM <DIR> pub".to_string(),
                "lrwxrwxrwx 1 1000 1000 9 Nov 5 2024 link -> valid.txt".to_string(),
            ],
        )
        .unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].path(), Path::new("/dir/valid.txt"));
        assert!(entries[1].is_dir());
        assert!(entries[2].is_symlink());
        assert!(parse_list_lines(Path::new("/"), vec![]).unwrap().is_empty());
        assert!(
            parse_list_lines(Path::new("/"), vec!["total 0".to_string()])
                .unwrap()
                .is_empty()
        );
        assert!(
            parse_list_lines(
                Path::new("/"),
                vec!["total 0".to_string(), "total 0".to_string(),]
            )
            .is_err()
        );
    }

    #[test]
    fn list_rejects_names_that_are_not_direct_children() {
        for name in ["/outside", "../outside", "nested/file", "control\tname"] {
            let line = format!("-rw-r--r-- 1 1000 1000 4 Nov 5 2024 {name}");
            let error = parse_list_lines(Path::new("/dir"), vec![line]).unwrap_err();
            assert_eq!(
                error.kind(),
                RemoteErrorType::ProtocolError,
                "name {name:?}"
            );
        }
        #[cfg(target_os = "windows")]
        {
            let line = r"-rw-r--r-- 1 1000 1000 4 Nov 5 2024 C:\outside";
            let error = parse_list_lines(Path::new("/dir"), vec![line.to_string()]).unwrap_err();
            assert_eq!(error.kind(), RemoteErrorType::ProtocolError);
        }
    }

    #[test]
    fn list_rejects_control_bytes_in_child_names() {
        let error = validate_list_child_name("control\tname").unwrap_err();
        assert_eq!(error.kind(), RemoteErrorType::ProtocolError);
    }

    #[test]
    fn list_rejects_ambiguous_leading_space_names() {
        let line = "-rw-r--r-- 1 1000 1000 4 Nov 5 2024  leading.txt";
        let error = parse_list_lines(Path::new("/dir"), vec![line.to_string()]).unwrap_err();
        assert_eq!(error.kind(), RemoteErrorType::ProtocolError);
        assert!(error.to_string().contains("leading-space"));

        let line = format!("10-19-20  03:19PM 4{}victim", " ".repeat(20));
        let error = parse_list_lines(Path::new("/dir"), vec![line]).unwrap_err();
        assert_eq!(error.kind(), RemoteErrorType::ProtocolError);

        let line = "10-19-20  03:19PM 4  victimAM";
        let error = parse_list_lines(Path::new("/dir"), vec![line.to_string()]).unwrap_err();
        assert_eq!(error.kind(), RemoteErrorType::ProtocolError);
    }

    #[test]
    fn list_rejects_ambiguous_dos_leading_space_names() {
        let line = "10-19-20  03:19PM 4  leading.txt";
        let error = parse_list_lines(Path::new("/dir"), vec![line.to_string()]).unwrap_err();
        assert_eq!(error.kind(), RemoteErrorType::ProtocolError);
        assert!(error.to_string().contains("leading-space"));

        for line in [
            "10-19-20  03:19 PM 4  leading.txt",
            "10-19-20  03:19PM\u{00a0}4 victim.txt",
        ] {
            let error = parse_list_lines(Path::new("/dir"), vec![line.to_string()]).unwrap_err();
            assert_eq!(
                error.kind(),
                RemoteErrorType::ProtocolError,
                "line {line:?}"
            );
        }
    }

    #[test]
    fn list_rejects_unicode_whitespace_leading_names() {
        for line in [
            "-rw-r--r-- 1 1000 1000 4 Nov 5 2024 \u{00a0}leading.txt",
            "-rw-r--r-- 1\u{00a0}root root 4 Nov 5 2024  leading.txt",
            "10-19-20  03:19PM 4 \u{00a0}leading.txt",
            "Drwxr-xr-x 1 root root 0 Nov 5 2024  leading.txt",
            "Drwxr-xr-x 1 root root 0 Nov 5 2024 \u{00a0}leading.txt",
        ] {
            let error = parse_list_lines(Path::new("/dir"), vec![line.to_string()]).unwrap_err();
            assert_eq!(
                error.kind(),
                RemoteErrorType::ProtocolError,
                "line {line:?}"
            );
        }
    }

    #[test]
    fn list_rejects_whitespace_only_names() {
        for line in [
            "-rw-r--r-- 1 1000 1000 4 Nov 5 2024   ",
            "Drwxr-xr-x 1 root root 0 Nov 5 2024   ",
        ] {
            let error = parse_list_lines(Path::new("/dir"), vec![line.to_string()]).unwrap_err();
            assert_eq!(
                error.kind(),
                RemoteErrorType::ProtocolError,
                "line {line:?}"
            );
        }
    }

    #[cfg(target_family = "unix")]
    #[test]
    fn list_accepts_backslashes_in_posix_names() {
        let entries = parse_list_lines(
            Path::new("/dir"),
            vec!["-rw-r--r-- 1 1000 1000 4 Nov 5 2024 a\\b".to_string()],
        )
        .unwrap();
        assert_eq!(entries[0].path(), Path::new("/dir/a\\b"));
    }

    #[test]
    fn list_accepts_padded_dos_metadata() {
        let line = "10-19-20  03:19PM       <DIR>          pub";
        let entries = parse_list_lines(Path::new("/dir"), vec![line.to_string()]).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path(), Path::new("/dir/pub"));
        assert!(entries[0].is_dir());
    }

    #[test]
    fn list_rejects_symlink_names_that_parser_would_truncate() {
        let line = "lrwxrwxrwx 1 1000 1000 9 Nov 5 2024 link -> name -> target";
        let error = parse_list_lines(Path::new("/dir"), vec![line.to_string()]).unwrap_err();
        assert_eq!(error.kind(), RemoteErrorType::ProtocolError);
        assert!(error.to_string().contains("symlink"));
    }

    #[test]
    fn ftp_paths_reject_parent_components() {
        for path in [
            Path::new("/dir/../file"),
            Path::new("/.."),
            Path::new("/dir/.."),
            Path::new("//server/share"),
        ] {
            assert_eq!(
                FtpFs::remote_path(path).unwrap_err().kind(),
                RemoteErrorType::InvalidPath
            );
        }
    }

    #[test]
    fn list_maps_special_posix_entries_to_regular_files() {
        let entries = parse_list_lines(
            Path::new("/dir"),
            vec![
                "srwxrwxrwx 1 root root 0 Nov 5 2024 socket".to_string(),
                "brw-rw---- 1 root disk 8, 0 Nov 5 2024 block".to_string(),
                "crw-rw---- 1 root tty 4, 0 Nov 5 2024 character".to_string(),
                "-rw-r--r-- 1 root root 4 Nov 5 2024 regular.txt".to_string(),
            ],
        )
        .unwrap();

        assert_eq!(entries.len(), 4);
        assert!(entries.iter().all(File::is_file));
    }

    #[test]
    fn list_filters_dot_entries_without_filtering_siblings() {
        let entries = parse_list_lines(
            Path::new("/dir"),
            vec![
                "drwxr-xr-x 2 root root 0 Nov 5 2024 .".to_string(),
                "drwxr-xr-x 2 root root 0 Nov 5 2024 ..".to_string(),
                "-rw-r--r-- 1 root root 4 Nov 5 2024 sibling".to_string(),
            ],
        )
        .unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path(), Path::new("/dir/sibling"));
    }

    #[test]
    fn list_rejects_invalid_utf8_before_parsing() {
        assert!(decode_list_bytes(vec![b'-', 0xff]).is_err());
    }

    #[test]
    fn list_accepts_already_open_and_keeps_control_connection_synchronized() {
        let control_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let control_address = control_listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (control, _) = control_listener.accept().unwrap();
            let mut control = BufReader::new(control);
            let reply = |control: &mut BufReader<TcpStream>, response: &str| {
                control.get_mut().write_all(response.as_bytes()).unwrap();
            };
            let command = |control: &mut BufReader<TcpStream>| {
                let mut line = String::new();
                control.read_line(&mut line).unwrap();
                line
            };

            reply(&mut control, "220 ready\r\n");
            assert_eq!(command(&mut control), "USER anonymous\r\n");
            reply(&mut control, "331 password\r\n");
            assert_eq!(command(&mut control), "PASS \r\n");
            reply(&mut control, "230 logged in\r\n");
            assert_eq!(command(&mut control), "TYPE I\r\n");
            reply(&mut control, "200 binary\r\n");

            let data_listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let data_port = data_listener.local_addr().unwrap().port();
            assert_eq!(command(&mut control), "PASV\r\n");
            reply(
                &mut control,
                &format!(
                    "227 passive (127,0,0,1,{},{})\r\n",
                    data_port / 256,
                    data_port % 256
                ),
            );
            assert_eq!(command(&mut control), "LIST /\r\n");
            reply(&mut control, "125 data connection already open\r\n");
            let (mut data, _) = data_listener.accept().unwrap();
            data.write_all(b"-rw-r--r-- 1 1000 1000 0 Nov 5 2024 file.txt\r\n")
                .unwrap();
            drop(data);
            reply(&mut control, "226 listing complete\r\n");

            assert_eq!(command(&mut control), "SITE NOOP\r\n");
            reply(&mut control, "200 noop\r\n");
            assert_eq!(command(&mut control), "QUIT\r\n");
            reply(&mut control, "221 bye\r\n");
        });

        let mut client = FtpFs::new(control_address.ip().to_string(), control_address.port())
            .passive_stream_builder(|address| {
                TcpStream::connect(address).map_err(FtpError::ConnectionError)
            });
        client.connect().unwrap();
        let entries = client.list_dir(Path::new("/")).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(client.exec("NOOP").unwrap().exit_code, 200);
        client.disconnect().unwrap();
        server.join().unwrap();
    }

    #[test]
    fn list_dir_succeeds_when_only_the_requested_directory_can_be_listed() {
        let control_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let control_address = control_listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (control, _) = control_listener.accept().unwrap();
            let mut control = BufReader::new(control);
            let reply = |control: &mut BufReader<TcpStream>, response: &str| {
                control.get_mut().write_all(response.as_bytes()).unwrap();
            };
            let command = |control: &mut BufReader<TcpStream>| {
                let mut line = String::new();
                control.read_line(&mut line).unwrap();
                line
            };

            reply(&mut control, "220 ready\r\n");
            assert_eq!(command(&mut control), "USER anonymous\r\n");
            reply(&mut control, "331 password\r\n");
            assert_eq!(command(&mut control), "PASS \r\n");
            reply(&mut control, "230 logged in\r\n");
            assert_eq!(command(&mut control), "TYPE I\r\n");
            reply(&mut control, "200 binary\r\n");

            let data_listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let data_port = data_listener.local_addr().unwrap().port();
            assert_eq!(command(&mut control), "PASV\r\n");
            reply(
                &mut control,
                &format!(
                    "227 passive (127,0,0,1,{},{})\r\n",
                    data_port / 256,
                    data_port % 256
                ),
            );
            assert_eq!(command(&mut control), "LIST /allowed\r\n");
            reply(&mut control, "150 opening data\r\n");
            let (mut data, _) = data_listener.accept().unwrap();
            data.write_all(b"-rw-r--r-- 1 1000 1000 0 Nov 5 2024 child.txt\r\n")
                .unwrap();
            drop(data);
            reply(&mut control, "226 listing complete\r\n");

            assert_eq!(command(&mut control), "SITE NOOP\r\n");
            reply(&mut control, "200 noop\r\n");
            assert_eq!(command(&mut control), "QUIT\r\n");
            reply(&mut control, "221 bye\r\n");
        });

        let mut client = FtpFs::new(control_address.ip().to_string(), control_address.port())
            .passive_stream_builder(|address| {
                TcpStream::connect(address).map_err(FtpError::ConnectionError)
            });
        client.connect().unwrap();
        let entries = client.list_dir(Path::new("/allowed")).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path(), Path::new("/allowed/child.txt"));
        assert_eq!(client.exec("NOOP").unwrap().exit_code, 200);
        client.disconnect().unwrap();
        server.join().unwrap();
    }

    #[test]
    fn list_dir_classifies_a_direct_single_file_listing() {
        let control_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let control_address = control_listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (control, _) = control_listener.accept().unwrap();
            let mut control = BufReader::new(control);
            authenticate_scripted_connection(&mut control);
            complete_scripted_list(
                &mut control,
                "/file.txt",
                b"-rw-r--r-- 1 1000 1000 0 Nov 5 2024 file.txt\r\n",
            );
            complete_scripted_list(
                &mut control,
                "/",
                b"-rw-r--r-- 1 1000 1000 0 Nov 5 2024 file.txt\r\n",
            );
            assert_eq!(read_scripted_command(&mut control), "QUIT\r\n");
            write_scripted_reply(&mut control, "221 bye\r\n");
        });

        let mut client = FtpFs::new(control_address.ip().to_string(), control_address.port())
            .passive_stream_builder(|address| {
                TcpStream::connect(address).map_err(FtpError::ConnectionError)
            });
        client.connect().unwrap();
        let error = client.list_dir(Path::new("/file.txt")).unwrap_err();
        assert_eq!(error.kind(), RemoteErrorType::BadFile);
        client.disconnect().unwrap();
        server.join().unwrap();
    }

    #[test]
    fn stat_rejects_children_beneath_a_regular_file_listing() {
        let control_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let control_address = control_listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (control, _) = control_listener.accept().unwrap();
            let mut control = BufReader::new(control);
            authenticate_scripted_connection(&mut control);
            complete_scripted_list(
                &mut control,
                "/file.txt",
                b"-rw-r--r-- 1 1000 1000 0 Nov 5 2024 file.txt\r\n",
            );
            complete_scripted_list(
                &mut control,
                "/",
                b"-rw-r--r-- 1 1000 1000 0 Nov 5 2024 file.txt\r\n",
            );
            assert_eq!(read_scripted_command(&mut control), "QUIT\r\n");
            write_scripted_reply(&mut control, "221 bye\r\n");
        });

        let mut client = FtpFs::new(control_address.ip().to_string(), control_address.port())
            .passive_stream_builder(|address| {
                TcpStream::connect(address).map_err(FtpError::ConnectionError)
            });
        client.connect().unwrap();
        let error = client.stat(Path::new("/file.txt/file.txt")).unwrap_err();
        assert_eq!(error.kind(), RemoteErrorType::BadFile);
        client.disconnect().unwrap();
        server.join().unwrap();
    }

    #[test]
    fn stat_keeps_a_child_when_parent_disambiguation_is_denied() {
        let control_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let control_address = control_listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (control, _) = control_listener.accept().unwrap();
            let mut control = BufReader::new(control);
            authenticate_scripted_connection(&mut control);
            complete_scripted_list(
                &mut control,
                "/pub",
                b"-rw-r--r-- 1 1000 1000 0 Nov 5 2024 pub\r\n",
            );
            refuse_scripted_list(&mut control, "/");
            assert_eq!(read_scripted_command(&mut control), "QUIT\r\n");
            write_scripted_reply(&mut control, "221 bye\r\n");
        });

        let mut client = FtpFs::new(control_address.ip().to_string(), control_address.port())
            .passive_stream_builder(|address| {
                TcpStream::connect(address).map_err(FtpError::ConnectionError)
            });
        client.connect().unwrap();
        let file = client.stat(Path::new("/pub/pub")).unwrap();
        assert_eq!(file.path(), Path::new("/pub/pub"));
        assert!(file.is_file());
        client.disconnect().unwrap();
        server.join().unwrap();
    }

    #[test]
    fn list_dir_classifies_a_missing_path_after_parent_probe() {
        let control_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let control_address = control_listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (control, _) = control_listener.accept().unwrap();
            let mut control = BufReader::new(control);
            authenticate_scripted_connection(&mut control);
            refuse_scripted_list(&mut control, "/missing");
            complete_scripted_list(
                &mut control,
                "/",
                b"-rw-r--r-- 1 1000 1000 0 Nov 5 2024 other.txt\r\n",
            );
            assert_eq!(read_scripted_command(&mut control), "QUIT\r\n");
            write_scripted_reply(&mut control, "221 bye\r\n");
        });

        let mut client = FtpFs::new(control_address.ip().to_string(), control_address.port())
            .passive_stream_builder(|address| {
                TcpStream::connect(address).map_err(FtpError::ConnectionError)
            });
        client.connect().unwrap();
        let error = client.list_dir(Path::new("/missing")).unwrap_err();
        assert_eq!(error.kind(), RemoteErrorType::NoSuchFileOrDirectory);
        assert!(error.source().is_some());
        client.disconnect().unwrap();
        server.join().unwrap();
    }

    #[test]
    fn list_dir_keeps_an_empty_directory_symlink_listing() {
        let control_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let control_address = control_listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (control, _) = control_listener.accept().unwrap();
            let mut control = BufReader::new(control);
            authenticate_scripted_connection(&mut control);
            complete_scripted_list(&mut control, "/link", b"");
            complete_scripted_list(
                &mut control,
                "/",
                b"lrwxrwxrwx 1 1000 1000 6 Nov 5 2024 link -> target\r\n",
            );
            assert_eq!(read_scripted_command(&mut control), "QUIT\r\n");
            write_scripted_reply(&mut control, "221 bye\r\n");
        });

        let mut client = FtpFs::new(control_address.ip().to_string(), control_address.port())
            .passive_stream_builder(|address| {
                TcpStream::connect(address).map_err(FtpError::ConnectionError)
            });
        client.connect().unwrap();
        assert!(client.list_dir(Path::new("/link")).unwrap().is_empty());
        client.disconnect().unwrap();
        server.join().unwrap();
    }

    #[test]
    fn ancestor_probe_lists_each_parent_at_most_once() {
        let control_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let control_address = control_listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (control, _) = control_listener.accept().unwrap();
            let mut control = BufReader::new(control);
            authenticate_scripted_connection(&mut control);
            for path in ["/a/b/c", "/a/b", "/a", "/"] {
                refuse_scripted_list(&mut control, path);
            }
            assert_eq!(read_scripted_command(&mut control), "SITE NOOP\r\n");
            write_scripted_reply(&mut control, "200 noop\r\n");
            assert_eq!(read_scripted_command(&mut control), "QUIT\r\n");
            write_scripted_reply(&mut control, "221 bye\r\n");
        });

        let mut client = FtpFs::new(control_address.ip().to_string(), control_address.port())
            .passive_stream_builder(|address| {
                TcpStream::connect(address).map_err(FtpError::ConnectionError)
            });
        client.connect().unwrap();
        assert_eq!(
            client.stat(Path::new("/a/b/c/file")).unwrap_err().kind(),
            RemoteErrorType::PermissionDenied
        );
        assert_eq!(client.exec("NOOP").unwrap().exit_code, 200);
        client.disconnect().unwrap();
        server.join().unwrap();
    }

    #[test]
    fn command_421_allows_a_fresh_connection() {
        let control_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let control_address = control_listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (control, _) = control_listener.accept().unwrap();
            let mut control = BufReader::new(control);
            authenticate_scripted_connection(&mut control);
            assert_eq!(read_scripted_command(&mut control), "SITE NOOP\r\n");
            write_scripted_reply(&mut control, "421 service closing\r\n");
            drop(control);

            let (control, _) = control_listener.accept().unwrap();
            let mut control = BufReader::new(control);
            authenticate_scripted_connection(&mut control);
            assert_eq!(read_scripted_command(&mut control), "SITE NOOP\r\n");
            write_scripted_reply(&mut control, "200 noop\r\n");
            assert_eq!(read_scripted_command(&mut control), "QUIT\r\n");
            write_scripted_reply(&mut control, "221 bye\r\n");
        });

        let mut client = FtpFs::new(control_address.ip().to_string(), control_address.port());
        client.connect().unwrap();
        assert_eq!(
            client.exec("NOOP").unwrap_err().kind(),
            RemoteErrorType::ConnectionError
        );
        assert!(!client.is_connected());
        assert_eq!(
            client.exec("NOOP").unwrap_err().kind(),
            RemoteErrorType::ConnectionError
        );
        client.connect().unwrap();
        assert_eq!(client.exec("NOOP").unwrap().exit_code, 200);
        client.disconnect().unwrap();
        server.join().unwrap();
    }

    #[test]
    fn list_setup_421_allows_a_fresh_connection() {
        let control_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let control_address = control_listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (control, _) = control_listener.accept().unwrap();
            let mut control = BufReader::new(control);
            authenticate_scripted_connection(&mut control);

            let data_listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let data_port = data_listener.local_addr().unwrap().port();
            assert_eq!(read_scripted_command(&mut control), "PASV\r\n");
            write_scripted_reply(
                &mut control,
                &format!(
                    "227 passive (127,0,0,1,{},{})\r\n",
                    data_port / 256,
                    data_port % 256
                ),
            );
            assert_eq!(read_scripted_command(&mut control), "LIST /\r\n");
            let (data, _) = data_listener.accept().unwrap();
            write_scripted_reply(&mut control, "421 service closing\r\n");
            drop(data);
            drop(control);

            let (control, _) = control_listener.accept().unwrap();
            let mut control = BufReader::new(control);
            authenticate_scripted_connection(&mut control);
            assert_eq!(read_scripted_command(&mut control), "SITE NOOP\r\n");
            write_scripted_reply(&mut control, "200 noop\r\n");
            assert_eq!(read_scripted_command(&mut control), "QUIT\r\n");
            write_scripted_reply(&mut control, "221 bye\r\n");
        });

        let mut client = FtpFs::new(control_address.ip().to_string(), control_address.port())
            .passive_stream_builder(|address| {
                TcpStream::connect(address).map_err(FtpError::ConnectionError)
            });
        client.connect().unwrap();
        assert_eq!(
            client.list_dir(Path::new("/")).unwrap_err().kind(),
            RemoteErrorType::ConnectionError
        );
        assert!(!client.is_connected());
        client.connect().unwrap();
        assert_eq!(client.exec("NOOP").unwrap().exit_code, 200);
        client.disconnect().unwrap();
        server.join().unwrap();
    }

    #[test]
    fn failed_list_completion_requires_reconnect_before_follow_up_commands() {
        let control_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let control_address = control_listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (control, _) = control_listener.accept().unwrap();
            let mut control = BufReader::new(control);
            let reply = |control: &mut BufReader<TcpStream>, response: &str| {
                control.get_mut().write_all(response.as_bytes()).unwrap();
            };
            let command = |control: &mut BufReader<TcpStream>| {
                let mut line = String::new();
                control.read_line(&mut line).unwrap();
                line
            };

            reply(&mut control, "220 ready\r\n");
            assert_eq!(command(&mut control), "USER anonymous\r\n");
            reply(&mut control, "331 password\r\n");
            assert_eq!(command(&mut control), "PASS \r\n");
            reply(&mut control, "230 logged in\r\n");
            assert_eq!(command(&mut control), "TYPE I\r\n");
            reply(&mut control, "200 binary\r\n");

            let data_listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let data_port = data_listener.local_addr().unwrap().port();
            assert_eq!(command(&mut control), "PASV\r\n");
            reply(
                &mut control,
                &format!(
                    "227 passive (127,0,0,1,{},{})\r\n",
                    data_port / 256,
                    data_port % 256
                ),
            );
            assert_eq!(command(&mut control), "LIST /\r\n");
            reply(&mut control, "150 opening data\r\n");
            let (mut data, _) = data_listener.accept().unwrap();
            data.write_all(b"-rw-r--r-- 1 1000 1000 0 Nov 5 2024 file.txt\r\n")
                .unwrap();
            drop(data);
            reply(&mut control, "426 transfer aborted\r\n");

            control
                .get_mut()
                .set_read_timeout(Some(Duration::from_millis(500)))
                .unwrap();
            let mut follow_up = String::new();
            if control.read_line(&mut follow_up).unwrap_or(0) > 0 {
                assert_eq!(follow_up, "SITE NOOP\r\n");
                reply(&mut control, "200 noop\r\n");
                assert_eq!(command(&mut control), "QUIT\r\n");
                reply(&mut control, "221 bye\r\n");
            }
        });

        let mut client = FtpFs::new(control_address.ip().to_string(), control_address.port())
            .passive_stream_builder(|address| {
                TcpStream::connect(address).map_err(FtpError::ConnectionError)
            });
        client.connect().unwrap();
        assert_eq!(
            client.list_dir(Path::new("/")).unwrap_err().kind(),
            RemoteErrorType::ConnectionError
        );
        assert_eq!(
            client.exec("NOOP").unwrap_err().kind(),
            RemoteErrorType::ConnectionError
        );
        assert_eq!(
            client.disconnect().unwrap_err().kind(),
            RemoteErrorType::ConnectionError
        );
        server.join().unwrap();
    }

    #[test]
    fn invalid_utf8_after_successful_list_completion_keeps_connection_usable() {
        let control_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let control_address = control_listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (control, _) = control_listener.accept().unwrap();
            let mut control = BufReader::new(control);
            let reply = |control: &mut BufReader<TcpStream>, response: &str| {
                control.get_mut().write_all(response.as_bytes()).unwrap();
            };
            let command = |control: &mut BufReader<TcpStream>| {
                let mut line = String::new();
                control.read_line(&mut line).unwrap();
                line
            };

            reply(&mut control, "220 ready\r\n");
            assert_eq!(command(&mut control), "USER anonymous\r\n");
            reply(&mut control, "331 password\r\n");
            assert_eq!(command(&mut control), "PASS \r\n");
            reply(&mut control, "230 logged in\r\n");
            assert_eq!(command(&mut control), "TYPE I\r\n");
            reply(&mut control, "200 binary\r\n");

            let data_listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let data_port = data_listener.local_addr().unwrap().port();
            assert_eq!(command(&mut control), "PASV\r\n");
            reply(
                &mut control,
                &format!(
                    "227 passive (127,0,0,1,{},{})\r\n",
                    data_port / 256,
                    data_port % 256
                ),
            );
            assert_eq!(command(&mut control), "LIST /\r\n");
            reply(&mut control, "150 opening data\r\n");
            let (mut data, _) = data_listener.accept().unwrap();
            data.write_all(b"-rw-r--r-- 1 1000 1000 1 Nov 5 2024 bad\xff\r\n")
                .unwrap();
            drop(data);
            reply(&mut control, "226 listing complete\r\n");

            assert_eq!(command(&mut control), "SITE NOOP\r\n");
            reply(&mut control, "200 noop\r\n");
            assert_eq!(command(&mut control), "QUIT\r\n");
            reply(&mut control, "221 bye\r\n");
        });

        let mut client = FtpFs::new(control_address.ip().to_string(), control_address.port())
            .passive_stream_builder(|address| {
                TcpStream::connect(address).map_err(FtpError::ConnectionError)
            });
        client.connect().unwrap();
        assert_eq!(
            client.list_dir(Path::new("/")).unwrap_err().kind(),
            RemoteErrorType::ProtocolError
        );
        assert_eq!(client.exec("NOOP").unwrap().exit_code, 200);
        client.disconnect().unwrap();
        server.join().unwrap();
    }

    #[test]
    fn ranged_open_requests_rest_before_retr() {
        let control_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let control_address = control_listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (control, _) = control_listener.accept().unwrap();
            let mut control = BufReader::new(control);
            let reply = |control: &mut BufReader<TcpStream>, response: &str| {
                control.get_mut().write_all(response.as_bytes()).unwrap();
            };
            let command = |control: &mut BufReader<TcpStream>| {
                let mut line = String::new();
                control.read_line(&mut line).unwrap();
                line
            };
            reply(&mut control, "220 ready\r\n");
            assert_eq!(command(&mut control), "USER anonymous\r\n");
            reply(&mut control, "331 password\r\n");
            assert_eq!(command(&mut control), "PASS \r\n");
            reply(&mut control, "230 logged in\r\n");
            assert_eq!(command(&mut control), "TYPE I\r\n");
            reply(&mut control, "200 binary\r\n");

            assert_eq!(command(&mut control), "REST 2\r\n");
            reply(&mut control, "350 restart position accepted\r\n");
            let transfer_listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let transfer_port = transfer_listener.local_addr().unwrap().port();
            assert_eq!(command(&mut control), "PASV\r\n");
            reply(
                &mut control,
                &format!(
                    "227 passive (127,0,0,1,{},{})\r\n",
                    transfer_port / 256,
                    transfer_port % 256
                ),
            );
            assert_eq!(command(&mut control), "RETR /file\r\n");
            reply(&mut control, "150 opening data\r\n");
            let (mut transfer_data, _) = transfer_listener.accept().unwrap();
            transfer_data.write_all(b"cdef").unwrap();
            drop(transfer_data);
            reply(&mut control, "226 transfer complete\r\n");
            assert_eq!(command(&mut control), "QUIT\r\n");
            reply(&mut control, "221 bye\r\n");
        });

        let mut client = FtpFs::new(control_address.ip().to_string(), control_address.port())
            .passive_stream_builder(|address| {
                TcpStream::connect(address).map_err(FtpError::ConnectionError)
            });
        client.connect().unwrap();
        let mut reader = client
            .open(Path::new("/file"), &ReadOptions::default().offset(2))
            .unwrap();
        let mut contents = Vec::new();
        reader.read_to_end(&mut contents).unwrap();
        reader.finish().unwrap();
        assert_eq!(contents, b"cdef");
        client.disconnect().unwrap();
        server.join().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn remote_path_rejects_unsupported_roots_and_non_utf8() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        for path in [Path::new(r"C:\tmp\file"), Path::new(r"\\server\share\file")] {
            assert_eq!(
                FtpFs::remote_path(path).unwrap_err().kind(),
                RemoteErrorType::InvalidPath
            );
        }

        let non_utf8_os = OsString::from_vec(vec![b'/', b't', b'm', b'p', b'/', 0xff]);
        let non_utf8 = Path::new(&non_utf8_os);
        assert_eq!(
            FtpFs::remote_path(non_utf8).unwrap_err().kind(),
            RemoteErrorType::InvalidPath
        );
        assert_eq!(
            FtpFs::remote_path(Path::new("/tmp/bad\nname"))
                .unwrap_err()
                .kind(),
            RemoteErrorType::InvalidPath
        );
    }

    fn read_scripted_command(control: &mut BufReader<TcpStream>) -> String {
        let mut line = String::new();
        control.read_line(&mut line).unwrap();
        line
    }

    fn write_scripted_reply(control: &mut BufReader<TcpStream>, response: &str) {
        control.get_mut().write_all(response.as_bytes()).unwrap();
    }

    fn authenticate_scripted_connection(control: &mut BufReader<TcpStream>) {
        write_scripted_reply(control, "220 ready\r\n");
        assert_eq!(read_scripted_command(control), "USER anonymous\r\n");
        write_scripted_reply(control, "331 password\r\n");
        assert_eq!(read_scripted_command(control), "PASS \r\n");
        write_scripted_reply(control, "230 logged in\r\n");
        assert_eq!(read_scripted_command(control), "TYPE I\r\n");
        write_scripted_reply(control, "200 binary\r\n");
    }

    fn complete_scripted_list(control: &mut BufReader<TcpStream>, path: &str, body: &[u8]) {
        let data_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let data_port = data_listener.local_addr().unwrap().port();
        assert_eq!(read_scripted_command(control), "PASV\r\n");
        write_scripted_reply(
            control,
            &format!(
                "227 passive (127,0,0,1,{},{})\r\n",
                data_port / 256,
                data_port % 256
            ),
        );
        assert_eq!(read_scripted_command(control), format!("LIST {path}\r\n"));
        write_scripted_reply(control, "150 opening data\r\n");
        let (mut data, _) = data_listener.accept().unwrap();
        data.write_all(body).unwrap();
        drop(data);
        write_scripted_reply(control, "226 listing complete\r\n");
    }

    fn refuse_scripted_list(control: &mut BufReader<TcpStream>, path: &str) {
        let data_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let data_port = data_listener.local_addr().unwrap().port();
        assert_eq!(read_scripted_command(control), "PASV\r\n");
        write_scripted_reply(
            control,
            &format!(
                "227 passive (127,0,0,1,{},{})\r\n",
                data_port / 256,
                data_port % 256
            ),
        );
        assert_eq!(read_scripted_command(control), format!("LIST {path}\r\n"));
        let (data, _) = data_listener.accept().unwrap();
        write_scripted_reply(control, "550 listing denied\r\n");
        drop(data);
    }
}

#[cfg(test)]
mod test {
    use std::io::{Cursor, Write as _};
    use std::sync::Arc;

    use pretty_assertions::assert_eq;

    use super::*;
    use crate::test_container::SyncPureFtpRunner;

    #[test]
    fn should_initialize_ftp_filesystem() {
        let client = FtpFs::new("127.0.0.1", 21);
        assert!(!client.is_connected());
        assert_eq!(client.hostname.as_str(), "127.0.0.1");
        assert_eq!(client.port, 21);
        assert_eq!(client.username.as_str(), "anonymous");
        assert!(client.password.is_none());
        assert_eq!(client.mode, Mode::Passive);
        #[cfg(any(
            feature = "native-tls",
            feature = "rustls-aws-lc-rs",
            feature = "rustls-ring"
        ))]
        assert!(!client.secure);
        #[cfg(feature = "native-tls")]
        assert!(!client.accept_invalid_certs);
        #[cfg(feature = "native-tls")]
        assert!(!client.accept_invalid_hostnames);
    }

    #[test]
    fn should_build_ftp_filesystem() {
        let client = FtpFs::new("127.0.0.1", 21)
            .username("test")
            .password("omar")
            .passive_mode()
            .active_mode();
        assert!(!client.is_connected());
        assert_eq!(client.username.as_str(), "test");
        assert_eq!(client.password.as_deref().unwrap(), "omar");
        assert_eq!(client.mode, Mode::Active);
    }

    #[test]
    #[cfg(any(
        feature = "native-tls",
        feature = "rustls-aws-lc-rs",
        feature = "rustls-ring"
    ))]
    fn should_build_secure_ftp_filesystem() {
        #[cfg(feature = "native-tls")]
        let client = FtpFs::new("127.0.0.1", 21).secure(true, true);
        #[cfg(any(feature = "rustls-aws-lc-rs", feature = "rustls-ring"))]
        let client = FtpFs::new("127.0.0.1", 21).secure();
        assert!(client.secure);
        #[cfg(feature = "native-tls")]
        assert!(client.accept_invalid_certs);
        #[cfg(feature = "native-tls")]
        assert!(client.accept_invalid_hostnames);
    }

    #[test]
    fn should_advertise_capabilities() {
        let caps = FtpFs::new("127.0.0.1", 21).capabilities();
        for cap in [
            Capabilities::STREAM_READ,
            Capabilities::STREAM_WRITE,
            Capabilities::APPEND,
            Capabilities::RANGE_READ,
            Capabilities::EXEC,
        ] {
            assert!(caps.contains(cap), "missing {cap:?}");
        }
        for cap in [
            Capabilities::SEEK_READ,
            Capabilities::SEEK_WRITE,
            Capabilities::COPY,
            Capabilities::SYMLINK,
            Capabilities::SET_METADATA,
            Capabilities::POSIX_MODE,
        ] {
            assert!(!caps.contains(cap), "unexpected {cap:?}");
        }
    }

    #[test]
    fn should_reject_relative_paths_before_checking_the_connection() {
        let client = FtpFs::new("127.0.0.1", 21);
        let relative = Path::new("a.txt");
        assert_eq!(
            client.stat(relative).unwrap_err().kind(),
            RemoteErrorType::InvalidPath
        );
        assert_eq!(
            client.list_dir(relative).unwrap_err().kind(),
            RemoteErrorType::InvalidPath
        );
        assert_eq!(
            client.exists(relative).unwrap_err().kind(),
            RemoteErrorType::InvalidPath
        );
        assert_eq!(
            client
                .open(relative, &ReadOptions::default())
                .unwrap_err()
                .kind(),
            RemoteErrorType::InvalidPath
        );
        assert_eq!(
            client
                .create(relative, &WriteOptions::default())
                .unwrap_err()
                .kind(),
            RemoteErrorType::InvalidPath
        );
        assert_eq!(
            client.create_dir(relative, None).unwrap_err().kind(),
            RemoteErrorType::InvalidPath
        );
        assert_eq!(
            client
                .rename(relative, Path::new("/b.txt"))
                .unwrap_err()
                .kind(),
            RemoteErrorType::InvalidPath
        );
    }

    #[test]
    fn should_return_not_connected_error() {
        let mut client = FtpFs::new("127.0.0.1", 21);
        let p = Path::new("/tmp/pippo.txt");
        for err in [
            client.copy(p, Path::new("/culonia")).unwrap_err(),
            client.symlink(p, Path::new("/b")).unwrap_err(),
            client.set_metadata(p, &SetMetadata::default()).unwrap_err(),
        ] {
            assert_eq!(err.kind(), RemoteErrorType::UnsupportedFeature);
        }
        for err in [
            client.exec("HELP").unwrap_err(),
            client.list_dir(Path::new("/tmp")).unwrap_err(),
            client.create_dir(Path::new("/tmp"), None).unwrap_err(),
            client.remove_dir_all(Path::new("/nowhere")).unwrap_err(),
            client.rename(p, Path::new("/culonia")).unwrap_err(),
            client.stat(p).unwrap_err(),
            client.open(p, &ReadOptions::default()).unwrap_err(),
            client.create(p, &WriteOptions::default()).unwrap_err(),
            client.append(p, &WriteOptions::default()).unwrap_err(),
            client.disconnect().unwrap_err(),
        ] {
            assert_eq!(err.kind(), RemoteErrorType::NotConnected);
        }
        assert!(client.welcome_message().is_none());
    }

    #[test]
    fn should_not_connect_twice_and_should_expose_banner() {
        crate::log_init();
        let container = Arc::new(SyncPureFtpRunner::start());
        let (mut client, _) = setup_client("localhost", container.get_ftp_port(), &container);
        assert!(client.welcome_message().is_some());
        assert_eq!(
            client.connect().unwrap_err().kind(),
            RemoteErrorType::AlreadyConnected
        );
        finalize_client(client);
        drop(container);
    }

    #[test]
    fn should_reuse_the_passive_builder_after_reconnect() {
        with_client(|client, dir| {
            client.disconnect().unwrap();
            client.connect().unwrap();
            assert!(client.list_dir(dir).is_ok());
        });
    }

    #[test]
    fn should_write_and_read_file() {
        with_client(|client, dir| {
            let p = dir.join("a.txt");
            let file_data = b"test data\n";
            let mut reader = Cursor::new(file_data.to_vec());
            assert_eq!(
                client
                    .write_file(
                        &p,
                        &WriteOptions::default().size_hint(file_data.len() as u64),
                        &mut reader
                    )
                    .unwrap(),
                10
            );
            assert_eq!(client.stat(&p).unwrap().metadata().size, Some(10));
            let mut dest = Vec::new();
            assert_eq!(
                client
                    .read_file(&p, &ReadOptions::default(), &mut dest)
                    .unwrap(),
                10
            );
            assert_eq!(dest, file_data);
        });
    }

    #[test]
    fn should_stream_write_flush_and_finish() {
        with_client(|client, dir| {
            let p = dir.join("stream.txt");
            let mut stream = client.create(&p, &WriteOptions::default()).unwrap();
            assert!(!stream.seekable());
            stream.write_all(b"hello, world!").unwrap();
            stream.finish().unwrap();
            assert_eq!(client.stat(&p).unwrap().metadata().size, Some(13));

            let mut stream = client.open(&p, &ReadOptions::default()).unwrap();
            let mut buf = String::new();
            stream.read_to_string(&mut buf).unwrap();
            stream.finish().unwrap();
            assert_eq!(buf, "hello, world!");
        });
    }

    #[test]
    fn should_honor_read_offset_and_length() {
        with_client(|client, dir| {
            let p = dir.join("range.bin");
            let mut src = Cursor::new(b"0123456789".to_vec());
            client
                .write_file(&p, &WriteOptions::default(), &mut src)
                .unwrap();

            let mut out = Vec::new();
            client
                .read_file(&p, &ReadOptions::default().offset(2).length(3), &mut out)
                .unwrap();
            assert_eq!(out, b"234");
            // the control channel is still usable after an early-closed read
            assert!(client.exists(&p).unwrap());

            let mut out = Vec::new();
            client
                .read_file(&p, &ReadOptions::default().offset(7), &mut out)
                .unwrap();
            assert_eq!(out, b"789");

            let mut out = Vec::new();
            client
                .read_file(&p, &ReadOptions::default().offset(2).length(0), &mut out)
                .unwrap();
            assert!(out.is_empty());
            assert!(client.exists(&p).unwrap());

            let mut out = Vec::new();
            client
                .read_file(&p, &ReadOptions::default().offset(100), &mut out)
                .unwrap();
            assert!(out.is_empty());
            assert!(client.exists(&p).unwrap());

            let mut out = Vec::new();
            client
                .read_file(&p, &ReadOptions::default().length(100), &mut out)
                .unwrap();
            assert_eq!(out, b"0123456789");
        });
    }

    #[test]
    fn should_reject_a_second_transfer_while_one_is_alive() {
        with_client(|client, dir| {
            let a = dir.join("a.txt");
            let b = dir.join("b.txt");
            let mut src = Cursor::new(b"aaaa".to_vec());
            client
                .write_file(&a, &WriteOptions::default(), &mut src)
                .unwrap();

            let stream = client.open(&a, &ReadOptions::default()).unwrap();
            assert_eq!(
                client
                    .create(&b, &WriteOptions::default())
                    .unwrap_err()
                    .kind(),
                RemoteErrorType::ProtocolError
            );
            assert_eq!(
                client.remove_file(&a).unwrap_err().kind(),
                RemoteErrorType::ProtocolError
            );
            assert_eq!(
                client.list_dir(dir).unwrap_err().kind(),
                RemoteErrorType::ProtocolError
            );
            stream.finish().unwrap();

            let mut stream = client.create(&b, &WriteOptions::default()).unwrap();
            stream.write_all(b"bb").unwrap();
            stream.finish().unwrap();
            assert_eq!(client.stat(&b).unwrap().metadata().size, Some(2));
        });
    }

    #[test]
    fn should_release_the_client_when_a_stream_is_dropped() {
        with_client(|client, dir| {
            let a = dir.join("a.txt");
            let mut src = Cursor::new(b"aaaa".to_vec());
            client
                .write_file(&a, &WriteOptions::default(), &mut src)
                .unwrap();
            let stream = client.open(&a, &ReadOptions::default()).unwrap();
            drop(stream);
            assert!(client.exists(&a).unwrap());
        });
    }

    #[test]
    fn should_append_to_file() {
        with_client(|client, dir| {
            let p = dir.join("a.txt");
            let mut reader = Cursor::new(b"test data\n".to_vec());
            assert_eq!(
                client
                    .write_file(&p, &WriteOptions::default(), &mut reader)
                    .unwrap(),
                10
            );
            let mut reader = Cursor::new(b"Hello, world!\n".to_vec());
            assert_eq!(
                client
                    .append_file(&p, &WriteOptions::default(), &mut reader)
                    .unwrap(),
                14
            );
            assert_eq!(client.stat(&p).unwrap().metadata().size, Some(24));
        });
    }

    #[test]
    fn should_not_append_to_file() {
        with_client(|client, _| {
            let p = Path::new("/tmp/aaaaaaa/hbbbbb/a.txt");
            let mut reader = Cursor::new(b"Hello, world!\n".to_vec());
            assert!(
                client
                    .append_file(p, &WriteOptions::default(), &mut reader)
                    .is_err()
            );
        });
    }

    #[test]
    fn should_not_copy_file() {
        with_client(|client, dir| {
            assert_eq!(
                client
                    .copy(&dir.join("a.txt"), &dir.join("b.txt"))
                    .unwrap_err()
                    .kind(),
                RemoteErrorType::UnsupportedFeature
            );
        });
    }

    #[test]
    fn should_create_directory() {
        with_client(|client, dir| {
            assert!(
                client
                    .create_dir(&dir.join("mydir"), Some(UnixPex::from(0o755)))
                    .is_ok()
            );
            assert!(client.stat(&dir.join("mydir")).unwrap().is_dir());
        });
    }

    #[test]
    fn should_not_create_directory_cause_already_exists() {
        with_client(|client, dir| {
            let p = dir.join("mydir");
            assert!(client.create_dir(&p, None).is_ok());
            assert_eq!(
                client.create_dir(&p, None).unwrap_err().kind(),
                RemoteErrorType::AlreadyExists
            );
        });
    }

    #[test]
    fn should_not_create_directory() {
        with_client(|client, _| {
            assert!(
                client
                    .create_dir(Path::new("/tmp/werfgjwerughjwurih/iwerjghiwgui"), None)
                    .is_err()
            );
        });
    }

    #[test]
    fn should_not_create_file() {
        with_client(|client, _| {
            let p = Path::new("/tmp/ahsufhauiefhuiashf/hfhfhfhf");
            let mut reader = Cursor::new(b"test data\n".to_vec());
            assert!(
                client
                    .write_file(p, &WriteOptions::default(), &mut reader)
                    .is_err()
            );
        });
    }

    #[test]
    fn should_not_open_file() {
        with_client(|client, _| {
            let mut dest = Vec::new();
            assert_eq!(
                client
                    .read_file(
                        Path::new("/tmp/aashafb/hhh"),
                        &ReadOptions::default(),
                        &mut dest
                    )
                    .unwrap_err()
                    .kind(),
                RemoteErrorType::NoSuchFileOrDirectory
            );
        });
    }

    #[test]
    fn should_report_bad_file_for_wrong_type_operations() {
        with_client(|client, dir| {
            let directory = dir.join("directory");
            client.create_dir(&directory, None).unwrap();

            let error = client
                .open(&directory, &ReadOptions::default())
                .unwrap_err();
            assert_eq!(error.kind(), RemoteErrorType::BadFile);
            assert!(error.source().is_some_and(|source| source.is::<FtpError>()));

            let error = client.remove_file(&directory).unwrap_err();
            assert_eq!(error.kind(), RemoteErrorType::BadFile);
            assert!(error.source().is_some_and(|source| source.is::<FtpError>()));

            let file = dir.join("file.txt");
            let mut reader = Cursor::new(b"test data\n".to_vec());
            client
                .write_file(&file, &WriteOptions::default(), &mut reader)
                .unwrap();
            let error = client.remove_dir(&file).unwrap_err();
            assert_eq!(error.kind(), RemoteErrorType::BadFile);
            assert!(error.source().is_some_and(|source| source.is::<FtpError>()));
        });
    }

    #[test]
    fn should_exec_command() {
        with_client(|client, dir| {
            let p = dir.join("a.txt");
            let mut reader = Cursor::new(b"test data\n".to_vec());
            client
                .write_file(&p, &WriteOptions::default(), &mut reader)
                .unwrap();
            // `SITE CHMOD` is one of the few `SITE` subcommands that reply with
            // `200 CommandOk`, which is the only status `suppaftp::site()` accepts.
            let output = client.exec(&format!("CHMOD 777 {}", p.display())).unwrap();
            assert_eq!(output.exit_code, 200);
        });
    }

    #[test]
    fn should_not_exec_command() {
        with_client(|client, _| {
            assert!(client.exec("echo 5").is_err());
        });
    }

    #[test]
    fn should_tell_whether_file_exists() {
        with_client(|client, dir| {
            let p = dir.join("a.txt");
            let mut reader = Cursor::new(b"test data\n".to_vec());
            client
                .write_file(&p, &WriteOptions::default(), &mut reader)
                .unwrap();
            assert!(client.exists(&p).unwrap());
            assert!(!client.exists(&dir.join("b.txt")).unwrap());
            assert!(!client.exists(Path::new("/tmp/ppppp/bhhrhu")).unwrap());
        });
    }

    #[test]
    fn should_list_dir() {
        with_client(|client, dir| {
            let p = dir.join("a.txt");
            let mut reader = Cursor::new(b"test data\n".to_vec());
            client
                .write_file(&p, &WriteOptions::default(), &mut reader)
                .unwrap();
            let entries = client.list_dir(dir).unwrap();
            assert_eq!(entries.len(), 1);
            let file = &entries[0];
            assert_eq!(file.name().as_str(), "a.txt");
            assert_eq!(file.path(), p.as_path());
            assert_eq!(file.extension().as_deref(), Some("txt"));
            assert!(file.is_file());
            assert_eq!(file.metadata().size, Some(10));
            assert_eq!(file.metadata().mode.unwrap(), UnixPex::from(0o644));
        });
    }

    #[test]
    fn should_reject_listing_a_regular_file() {
        with_client(|client, dir| {
            let p = dir.join("a.txt");
            let mut reader = Cursor::new(b"test data\n".to_vec());
            client
                .write_file(&p, &WriteOptions::default(), &mut reader)
                .unwrap();
            assert_eq!(
                client.list_dir(&p).unwrap_err().kind(),
                RemoteErrorType::BadFile
            );
        });
    }

    #[test]
    fn should_classify_missing_creation_parents_as_missing() {
        with_client(|client, dir| {
            let missing = dir.join("missing");
            let create_path = missing.join("create.txt");
            let append_path = missing.join("append.txt");
            let dir_path = missing.join("child");
            let mut reader = Cursor::new(b"test data\n".to_vec());

            assert_eq!(
                client
                    .write_file(&create_path, &WriteOptions::default(), &mut reader)
                    .unwrap_err()
                    .kind(),
                RemoteErrorType::NoSuchFileOrDirectory
            );

            let mut reader = Cursor::new(b"test data\n".to_vec());
            assert_eq!(
                client
                    .append_file(&append_path, &WriteOptions::default(), &mut reader)
                    .unwrap_err()
                    .kind(),
                RemoteErrorType::NoSuchFileOrDirectory
            );
            assert_eq!(
                client.create_dir(&dir_path, None).unwrap_err().kind(),
                RemoteErrorType::NoSuchFileOrDirectory
            );
        });
    }

    #[test]
    fn should_classify_missing_rename_destination_parent_as_missing() {
        with_client(|client, dir| {
            let source = dir.join("source.txt");
            let destination = dir.join("missing").join("destination.txt");
            let mut reader = Cursor::new(b"test data\n".to_vec());
            client
                .write_file(&source, &WriteOptions::default(), &mut reader)
                .unwrap();
            assert_eq!(
                client.rename(&source, &destination).unwrap_err().kind(),
                RemoteErrorType::NoSuchFileOrDirectory
            );
        });
    }

    #[test]
    fn should_rename_file() {
        with_client(|client, dir| {
            let p = dir.join("a.txt");
            let dest = dir.join("b.txt");
            let mut reader = Cursor::new(b"test data\n".to_vec());
            client
                .write_file(&p, &WriteOptions::default(), &mut reader)
                .unwrap();
            assert!(client.rename(&p, &dest).is_ok());
            assert!(!client.exists(&p).unwrap());
            assert!(client.exists(&dest).unwrap());
        });
    }

    #[test]
    fn should_not_rename_file() {
        with_client(|client, dir| {
            let p = dir.join("a.txt");
            let mut reader = Cursor::new(b"test data\n".to_vec());
            client
                .write_file(&p, &WriteOptions::default(), &mut reader)
                .unwrap();
            let dest = Path::new("/tmp/wuefhiwuerfh/whjhh/b.txt");
            assert!(client.rename(&p, dest).is_err());
            assert_eq!(
                client.rename(dest, &p).unwrap_err().kind(),
                RemoteErrorType::NoSuchFileOrDirectory
            );
        });
    }

    #[test]
    fn should_remove_dir_all() {
        with_client(|client, dir| {
            let sub = dir.join("test");
            client.create_dir(&sub, Some(UnixPex::from(0o775))).unwrap();
            let mut reader = Cursor::new(b"test data\n".to_vec());
            client
                .write_file(&sub.join("a.txt"), &WriteOptions::default(), &mut reader)
                .unwrap();
            assert!(client.remove_dir_all(&sub).is_ok());
            assert!(!client.exists(&sub).unwrap());
        });
    }

    #[test]
    fn should_not_remove_dir_all() {
        with_client(|client, _| {
            assert!(
                client
                    .remove_dir_all(Path::new("/tmp/aaaaaa/asuhi"))
                    .is_err()
            );
        });
    }

    #[test]
    fn should_remove_dir() {
        with_client(|client, dir| {
            let sub = dir.join("test");
            client.create_dir(&sub, None).unwrap();
            assert!(client.remove_dir(&sub).is_ok());
        });
    }

    #[test]
    fn should_not_remove_dir() {
        with_client(|client, dir| {
            let sub = dir.join("test");
            client.create_dir(&sub, None).unwrap();
            let mut reader = Cursor::new(b"test data\n".to_vec());
            client
                .write_file(&sub.join("a.txt"), &WriteOptions::default(), &mut reader)
                .unwrap();
            assert!(client.remove_dir(&sub).is_err());
        });
    }

    #[test]
    fn should_remove_file() {
        with_client(|client, dir| {
            let p = dir.join("a.txt");
            let mut reader = Cursor::new(b"test data\n".to_vec());
            client
                .write_file(&p, &WriteOptions::default(), &mut reader)
                .unwrap();
            assert!(client.remove_file(&p).is_ok());
            assert_eq!(
                client.remove_file(&p).unwrap_err().kind(),
                RemoteErrorType::NoSuchFileOrDirectory
            );
        });
    }

    #[test]
    fn should_not_set_metadata() {
        with_client(|client, dir| {
            assert_eq!(
                client
                    .set_metadata(
                        &dir.join("a.sh"),
                        &SetMetadata::default().mode(UnixPex::from(0o755))
                    )
                    .unwrap_err()
                    .kind(),
                RemoteErrorType::UnsupportedFeature
            );
        });
    }

    #[test]
    fn should_stat_file() {
        with_client(|client, dir| {
            let p = dir.join("a.sh");
            let mut reader = Cursor::new(b"echo 5\n".to_vec());
            client
                .write_file(&p, &WriteOptions::default(), &mut reader)
                .unwrap();
            let entry = client.stat(&p).unwrap();
            assert_eq!(entry.name(), "a.sh");
            assert_eq!(entry.path(), p.as_path());
            assert_eq!(entry.metadata().mode.unwrap(), UnixPex::from(0o644));
            assert_eq!(entry.metadata().size, Some(7));
        });
    }

    #[test]
    fn should_stat_root() {
        with_client(|client, _| {
            let entry = client.stat(Path::new("/")).unwrap();
            assert_eq!(entry.name(), "/");
            assert_eq!(entry.path(), Path::new("/"));
            assert!(entry.is_dir());
        });
    }

    #[test]
    fn should_not_stat_file() {
        with_client(|client, dir| {
            assert_eq!(
                client.stat(&dir.join("a.sh")).unwrap_err().kind(),
                RemoteErrorType::NoSuchFileOrDirectory
            );
        });
    }

    #[test]
    fn should_not_make_symlink() {
        with_client(|client, dir| {
            assert_eq!(
                client
                    .symlink(&dir.join("b.sh"), &dir.join("a.sh"))
                    .unwrap_err()
                    .kind(),
                RemoteErrorType::UnsupportedFeature
            );
        });
    }

    #[cfg(feature = "find")]
    #[test]
    fn should_find_files_from_an_absolute_root() {
        with_client(|client, dir| {
            let sub = dir.join("nested");
            client.create_dir(&sub, None).unwrap();
            for p in [dir.join("a.txt"), sub.join("b.txt"), sub.join("c.log")] {
                let mut reader = Cursor::new(b"x".to_vec());
                client
                    .write_file(&p, &WriteOptions::default(), &mut reader)
                    .unwrap();
            }
            let found = remotefs::find(client, dir, "*.txt").unwrap();
            let mut names: Vec<String> = found.iter().map(File::name).collect();
            names.sort();
            assert_eq!(names, ["a.txt", "b.txt"]);
        });
    }

    fn is_send<T: Send>(_send: T) {}

    fn is_sync<T: Sync>(_sync: T) {}

    #[test]
    fn test_should_be_sync() {
        is_sync(FtpFs::new("127.0.0.1", 10021));
    }

    #[test]
    fn test_should_be_send() {
        is_send(FtpFs::new("127.0.0.1", 10021));
    }

    #[test]
    fn test_should_be_usable_as_trait_object() {
        let _: Box<dyn RemoteFs> = Box::new(FtpFs::new("127.0.0.1", 10021));
        let _: Arc<dyn RemoteFs> = Arc::new(FtpFs::new("127.0.0.1", 10021));
    }

    // -- test utils

    fn generate_tempdir() -> String {
        use rand::distr::Alphanumeric;
        use rand::{RngExt as _, rng};
        let mut rng = rng();
        let name: String = std::iter::repeat(())
            .map(|()| rng.sample(Alphanumeric))
            .map(char::from)
            .take(8)
            .collect();
        format!("temp_{}", name)
    }

    /// Starts a container, connects, creates a scratch directory and hands
    /// `(client, scratch_dir)` to `f`.
    fn with_client<F>(f: F)
    where
        F: FnOnce(&mut FtpFs, &Path),
    {
        crate::log_init();
        let container = Arc::new(SyncPureFtpRunner::start());
        let port = container.get_ftp_port();
        let (mut client, tempdir) = setup_client("localhost", port, &container);
        f(&mut client, &tempdir);
        finalize_client(client);
        drop(container);
    }

    fn setup_client(
        hostname: &str,
        port: u16,
        container: &Arc<SyncPureFtpRunner>,
    ) -> (FtpFs, PathBuf) {
        let container_t = container.clone();

        let mut client = FtpFs::new(hostname, port)
            .username("test")
            .password("test")
            .passive_stream_builder(move |mut addr| {
                let port = addr.port();
                let mapped = container_t.get_mapped_port(port);
                addr.set_port(mapped);
                info!("mapped port {port} to {mapped} for PASV");
                TcpStream::connect(addr).map_err(FtpError::ConnectionError)
            });

        client.connect().expect("connect failed");
        // The server decides where the session starts (chrooted home or `/`);
        // ask the raw stream so the scratch directory is absolute either way.
        let root = PathBuf::from(client.stream().unwrap().pwd().expect("pwd failed"));
        let tempdir = root.join(generate_tempdir());
        client
            .create_dir(&tempdir, Some(UnixPex::from(0o755)))
            .expect("create scratch dir failed");
        (client, tempdir)
    }

    fn finalize_client(mut client: FtpFs) {
        assert!(client.disconnect().is_ok());
    }
}
