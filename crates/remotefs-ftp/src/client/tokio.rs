//! Tokio-backed FTP client.

mod stream;

use std::future::Future;
use std::net::SocketAddr;
use std::path::Path;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex, PoisonError};

use log::{debug, error, info, trace, warn};
use remotefs::fs::{
    AsyncReadStream, AsyncRemoteFs, AsyncWriteStream, Capabilities, ExecOutput, FileType, Metadata,
    ReadOptions, RemoteError, RemoteErrorType, RemoteResult, SetMetadata, UnixPex, WriteOptions,
};
use remotefs::{File, async_trait};
#[cfg(not(any(
    feature = "tokio-native-tls",
    feature = "tokio-rustls-aws-lc-rs",
    feature = "tokio-rustls-ring"
)))]
pub use suppaftp::tokio::AsyncFtpStream;
#[cfg(feature = "tokio-native-tls")]
use suppaftp::tokio::AsyncNativeTlsConnector as TlsConnector;
#[cfg(feature = "tokio-native-tls")]
pub use suppaftp::tokio::AsyncNativeTlsFtpStream as AsyncFtpStream;
#[cfg(any(feature = "tokio-rustls-aws-lc-rs", feature = "tokio-rustls-ring"))]
use suppaftp::tokio::AsyncRustlsConnector as TlsConnector;
#[cfg(any(feature = "tokio-rustls-aws-lc-rs", feature = "tokio-rustls-ring"))]
pub use suppaftp::tokio::AsyncRustlsFtpStream as AsyncFtpStream;
use suppaftp::types::{FileType as SuppaFtpFileType, Mode};
use suppaftp::{FtpError, FtpResult, Status};
use tokio::io::AsyncReadExt as _;
use tokio::net::TcpStream;
use tokio::sync::{Mutex, MutexGuard};

use self::stream::{TokioReadStream, TokioWriteStream};
use super::error::{
    ListCleanupFailure, ambiguous_path_permission, ftp_error, is_ambiguous_path_refusal,
    is_creation_refusal, is_file_unavailable, remote_error_requires_reconnect,
    transfer_setup_requires_reconnect,
};
use super::guard::TransferGuard;
use super::list::{decode_list_bytes, parse_list_lines};
use super::path::{remote_path, resolve};

/// A function that creates a new stream for the data connection in passive mode.
pub type TokioPassiveStreamBuilder = dyn Fn(SocketAddr) -> Pin<Box<dyn Future<Output = FtpResult<TcpStream>> + Send + Sync>>
    + Send
    + Sync;

/// Asynchronous FTP and FTPS client implementing [`AsyncRemoteFs`] on Tokio.
///
/// Every path passed to the filesystem operations must be absolute; the client
/// keeps no working directory. Operations take `&self` and serialise access to
/// the control connection with an async mutex, so concurrent callers wait for
/// each other. FTP allows a single data connection: while a stream returned by
/// [`AsyncRemoteFs::open`], [`AsyncRemoteFs::create`] or [`AsyncRemoteFs::append`]
/// is alive, other control-connection operations return
/// [`RemoteErrorType::ProtocolError`] until the stream is finished or dropped.
///
/// Call `finish().await` on every stream. Dropping an unfinished download that
/// has not reached EOF marks the control connection unusable until the client
/// reconnects. Operation futures are not cancellation safe: cancelling one
/// after its command was sent also marks the connection unusable, because the
/// server reply can no longer be matched to a command.
///
/// # Examples
///
/// ```rust,no_run
/// use std::path::Path;
///
/// use remotefs::AsyncRemoteFs;
/// use remotefs::fs::WriteOptions;
/// use remotefs_ftp::TokioFtpFs;
///
/// # async fn run() -> remotefs::RemoteResult<()> {
/// let mut client = TokioFtpFs::new("127.0.0.1", 21)
///     .username("test")
///     .password("password");
/// client.connect().await?;
/// let mut source = futures::io::Cursor::new(b"hello".to_vec());
/// client
///     .write_file(
///         Path::new("/upload/hello.txt"),
///         &WriteOptions::default().size_hint(5),
///         &mut source,
///     )
///     .await?;
/// client.disconnect().await?;
/// # Ok(())
/// # }
/// ```
pub struct TokioFtpFs {
    /// Control connection; `None` until [`AsyncRemoteFs::connect`] succeeds.
    stream: Mutex<Option<AsyncFtpStream>>,
    /// Mirrors `stream.is_some()` for the synchronous `is_connected` query.
    connected: AtomicBool,
    /// Raised while a transfer stream is alive.
    transfer_active: Arc<AtomicBool>,
    /// Cleared when cleanup or cancellation may have left unread FTP replies.
    connection_usable: Arc<AtomicBool>,
    /// Banner cached at connect time.
    welcome: StdMutex<Option<String>>,
    // -- options
    hostname: String,
    port: u16,
    /// Username to login as; default: `anonymous`
    username: String,
    password: Option<String>,
    passive_stream_builder: Option<Arc<TokioPassiveStreamBuilder>>,
    /// Client mode; default: `Mode::Passive`
    mode: Mode,
    #[cfg(any(
        feature = "tokio-native-tls",
        feature = "tokio-rustls-aws-lc-rs",
        feature = "tokio-rustls-ring"
    ))]
    /// use FTPS; default: `false`
    secure: bool,
    #[cfg(feature = "tokio-native-tls")]
    accept_invalid_certs: bool,
    #[cfg(feature = "tokio-native-tls")]
    accept_invalid_hostnames: bool,
}

/// Marks the connection unusable if an operation future is dropped mid-flight.
struct OperationGuard<'a> {
    client: &'a TokioFtpFs,
    completed: bool,
}

impl OperationGuard<'_> {
    fn complete(mut self) {
        self.completed = true;
    }
}

impl Drop for OperationGuard<'_> {
    fn drop(&mut self) {
        if !self.completed {
            self.client.mark_connection_unusable();
            warn!("an FTP operation was cancelled mid-flight; reconnect before retrying");
        }
    }
}

/// Locked control stream plus the cancellation guard for one operation.
struct StreamGuard<'a> {
    guard: MutexGuard<'a, Option<AsyncFtpStream>>,
    operation: OperationGuard<'a>,
}

impl StreamGuard<'_> {
    fn stream(&mut self) -> &mut AsyncFtpStream {
        self.guard
            .as_mut()
            .expect("lock_stream verified the connection")
    }

    /// Releases the lock and records the operation as completed.
    fn complete(self) {
        self.operation.complete();
    }
}

impl TokioFtpFs {
    /// Instantiates a new `TokioFtpFs` for `hostname:port`; connect with [`AsyncRemoteFs::connect`].
    ///
    /// # Examples
    ///
    /// ```
    /// use remotefs_ftp::TokioFtpFs;
    ///
    /// let client = TokioFtpFs::new("127.0.0.1", 21).username("test").password("secret");
    /// ```
    pub fn new<S: AsRef<str>>(hostname: S, port: u16) -> Self {
        Self {
            stream: Mutex::new(None),
            connected: AtomicBool::new(false),
            transfer_active: Arc::new(AtomicBool::new(false)),
            connection_usable: Arc::new(AtomicBool::new(true)),
            welcome: StdMutex::new(None),
            hostname: hostname.as_ref().to_string(),
            port,
            username: String::from("anonymous"),
            password: None,
            passive_stream_builder: None,
            mode: Mode::Passive,
            #[cfg(any(
                feature = "tokio-native-tls",
                feature = "tokio-rustls-aws-lc-rs",
                feature = "tokio-rustls-ring"
            ))]
            secure: false,
            #[cfg(feature = "tokio-native-tls")]
            accept_invalid_certs: false,
            #[cfg(feature = "tokio-native-tls")]
            accept_invalid_hostnames: false,
        }
    }

    /// Sets the username used when connecting.
    ///
    /// # Examples
    ///
    /// ```
    /// use remotefs_ftp::TokioFtpFs;
    /// let client = TokioFtpFs::new("localhost", 21).username("test");
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
    /// use remotefs_ftp::TokioFtpFs;
    /// let client = TokioFtpFs::new("localhost", 21).password("secret");
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
    /// use remotefs_ftp::TokioFtpFs;
    /// let client = TokioFtpFs::new("localhost", 21).active_mode();
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
    /// use remotefs_ftp::TokioFtpFs;
    /// let client = TokioFtpFs::new("localhost", 21).passive_mode();
    /// ```
    pub fn passive_mode(mut self) -> Self {
        self.mode = Mode::Passive;
        self
    }

    #[cfg(feature = "tokio-native-tls")]
    #[cfg_attr(docsrs, doc(cfg(feature = "tokio-native-tls")))]
    /// Enables FTPS with the specified certificate and hostname validation options.
    ///
    /// Passing `true` disables the corresponding validation when connecting.
    ///
    /// # Examples
    ///
    /// ```
    /// use remotefs_ftp::TokioFtpFs;
    /// let client = TokioFtpFs::new("localhost", 21).secure(false, false);
    /// ```
    pub fn secure(mut self, accept_invalid_certs: bool, accept_invalid_hostnames: bool) -> Self {
        self.secure = true;
        self.accept_invalid_certs = accept_invalid_certs;
        self.accept_invalid_hostnames = accept_invalid_hostnames;
        self
    }

    #[cfg(any(feature = "tokio-rustls-aws-lc-rs", feature = "tokio-rustls-ring"))]
    #[cfg_attr(
        docsrs,
        doc(cfg(any(feature = "tokio-rustls-aws-lc-rs", feature = "tokio-rustls-ring")))
    )]
    /// Enables FTPS using rustls and the webpki root store.
    ///
    /// # Examples
    ///
    /// ```
    /// use remotefs_ftp::TokioFtpFs;
    /// let client = TokioFtpFs::new("localhost", 21).secure();
    /// ```
    pub fn secure(mut self) -> Self {
        self.secure = true;
        self
    }

    /// Sets a custom [`TokioPassiveStreamBuilder`] for passive mode.
    ///
    /// The builder receives the passive address announced by the server and
    /// returns the [`TcpStream`] to use for the data connection.
    ///
    /// # Examples
    ///
    /// ```
    /// use remotefs_ftp::TokioFtpFs;
    /// use suppaftp::FtpError;
    /// use tokio::net::TcpStream;
    ///
    /// let client = TokioFtpFs::new("localhost", 21).passive_stream_builder(|address| async move {
    ///     TcpStream::connect(address)
    ///         .await
    ///         .map_err(FtpError::ConnectionError)
    /// });
    /// ```
    pub fn passive_stream_builder<F, Fut>(mut self, builder: F) -> Self
    where
        F: Fn(SocketAddr) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = FtpResult<TcpStream>> + Send + Sync + 'static,
    {
        let boxed = move |address: SocketAddr| -> Pin<
            Box<dyn Future<Output = FtpResult<TcpStream>> + Send + Sync>,
        > { Box::pin(builder(address)) };
        self.passive_stream_builder = Some(Arc::new(boxed));
        self
    }

    /// Returns the control stream when connected and no managed transfer is active.
    ///
    /// Returns `None` until a managed transfer is finished or dropped, and
    /// while the connection is unusable. Commands issued through it bypass
    /// managed coordination: finish any raw transfer and consume its
    /// completion reply before using filesystem operations again, and leave
    /// the connection authenticated and in binary transfer mode.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use remotefs::AsyncRemoteFs;
    /// use remotefs_ftp::TokioFtpFs;
    ///
    /// # async fn run() {
    /// let mut client = TokioFtpFs::new("127.0.0.1", 21);
    /// client.connect().await.unwrap();
    /// let cwd = client.stream().unwrap().pwd().await.unwrap();
    /// # }
    /// ```
    pub fn stream(&mut self) -> Option<&mut AsyncFtpStream> {
        if self.transfer_active.load(Ordering::SeqCst)
            || !self.connection_usable.load(Ordering::SeqCst)
        {
            return None;
        }
        self.stream.get_mut().as_mut()
    }

    /// Returns the banner the server sent on connection, if connected.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use remotefs::AsyncRemoteFs;
    /// use remotefs_ftp::TokioFtpFs;
    ///
    /// # async fn run() {
    /// let mut client = TokioFtpFs::new("127.0.0.1", 21);
    /// client.connect().await.unwrap();
    /// if let Some(banner) = client.welcome_message() {
    ///     println!("{banner}");
    /// }
    /// # }
    /// ```
    pub fn welcome_message(&self) -> Option<String> {
        self.welcome
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    // -- private

    /// Locks the control stream for one operation.
    async fn lock_stream(&self) -> RemoteResult<StreamGuard<'_>> {
        let guard = self.stream.lock().await;
        self.check_operation_state()?;
        if guard.is_none() {
            return Err(RemoteError::new(RemoteErrorType::NotConnected));
        }
        Ok(StreamGuard {
            guard,
            operation: OperationGuard {
                client: self,
                completed: false,
            },
        })
    }

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

    fn mark_connection_unusable(&self) {
        self.connection_usable.store(false, Ordering::SeqCst);
    }

    #[allow(dead_code)]
    fn record_ftp_error(&self, error: &FtpError) {
        if transfer_setup_requires_reconnect(error) {
            self.mark_connection_unusable();
        }
    }

    fn record_remote_error(&self, error: &RemoteError) {
        if remote_error_requires_reconnect(error) {
            self.mark_connection_unusable();
        }
    }

    /// Raises the transfer flag and returns the guard that clears it.
    #[allow(dead_code)]
    fn start_transfer(&self) -> TransferGuard {
        self.transfer_active.store(true, Ordering::SeqCst);
        TransferGuard::new(
            Arc::clone(&self.transfer_active),
            Arc::clone(&self.connection_usable),
        )
    }

    /// Classifies a file operation refusal after probing for positive evidence.
    async fn classify_file_refusal(&self, path: &Path, err: FtpError) -> RemoteError {
        if !is_file_unavailable(&err) {
            return ftp_error(err);
        }
        match self.stat(path).await {
            Ok(file) if file.is_dir() => RemoteError::with_source(RemoteErrorType::BadFile, err),
            Err(probe) if probe.kind() == RemoteErrorType::NoSuchFileOrDirectory => {
                RemoteError::with_source(RemoteErrorType::NoSuchFileOrDirectory, err)
            }
            _ => ftp_error(err),
        }
    }

    /// Classifies a failed STOR or APPE while preserving ambiguous refusals.
    async fn classify_create_refusal(&self, path: &Path, err: FtpError) -> RemoteError {
        if !is_creation_refusal(&err) {
            return ftp_error(err);
        }
        match self.stat(path).await {
            Ok(file) if file.is_dir() => {
                RemoteError::with_source(RemoteErrorType::AlreadyExists, err)
            }
            Err(error)
                if error.kind() == RemoteErrorType::NoSuchFileOrDirectory
                    && self.destination_parent_is_missing(path).await =>
            {
                RemoteError::with_source(RemoteErrorType::NoSuchFileOrDirectory, err)
            }
            _ => RemoteError::with_source(RemoteErrorType::PermissionDenied, err),
        }
    }

    /// Classifies a failed RMD while preserving ambiguous refusals.
    async fn classify_remove_dir_refusal(&self, path: &Path, err: FtpError) -> RemoteError {
        if !is_file_unavailable(&err) {
            return ftp_error(err);
        }
        match self.stat(path).await {
            Ok(file) if !file.is_dir() => RemoteError::with_source(RemoteErrorType::BadFile, err),
            Ok(_) => match self.list_dir(path).await {
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
    async fn ancestor_proves_absent(&self, path: &Path) -> RemoteResult<bool> {
        let mut candidate = path.to_path_buf();
        for _ in 0..8 {
            let Some(parent) = candidate.parent() else {
                return Ok(false);
            };
            let remote_parent = remote_path(parent)?;
            match self.list_dir_raw(parent, &remote_parent).await {
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
    async fn destination_parent_is_missing(&self, path: &Path) -> bool {
        let Some(parent) = path.parent() else {
            return false;
        };
        matches!(
            self.stat(parent).await,
            Err(error) if error.kind() == RemoteErrorType::NoSuchFileOrDirectory
        )
    }

    /// Classifies a failed `MKD` while preserving ambiguous 550 refusals.
    async fn classify_create_dir_refusal(&self, path: &Path, err: FtpError) -> RemoteError {
        if !is_creation_refusal(&err) {
            return ftp_error(err);
        }
        match self.stat(path).await {
            Ok(_) => RemoteError::with_source(RemoteErrorType::AlreadyExists, err),
            Err(error)
                if error.kind() == RemoteErrorType::NoSuchFileOrDirectory
                    && self.destination_parent_is_missing(path).await =>
            {
                RemoteError::with_source(RemoteErrorType::NoSuchFileOrDirectory, err)
            }
            _ => RemoteError::with_source(RemoteErrorType::PermissionDenied, err),
        }
    }

    /// Classifies a failed rename while distinguishing source and destination lookup.
    async fn classify_rename_refusal(&self, src: &Path, dest: &Path, err: FtpError) -> RemoteError {
        if !is_creation_refusal(&err) {
            return ftp_error(err);
        }
        match self.stat(src).await {
            Err(error) if error.kind() == RemoteErrorType::NoSuchFileOrDirectory => {
                RemoteError::with_source(RemoteErrorType::NoSuchFileOrDirectory, err)
            }
            Err(_) => RemoteError::with_source(RemoteErrorType::PermissionDenied, err),
            Ok(_) if self.destination_parent_is_missing(dest).await => {
                RemoteError::with_source(RemoteErrorType::NoSuchFileOrDirectory, err)
            }
            Ok(_) => RemoteError::with_source(RemoteErrorType::PermissionDenied, err),
        }
    }

    /// Lists a path without first asserting that the path is a directory.
    async fn list_dir_raw(&self, path: &Path, remote: &str) -> RemoteResult<Vec<File>> {
        let mut guard = self.lock_stream().await?;
        let (result, requires_reconnect) = read_list_bytes(guard.stream(), remote).await;
        guard.complete();
        if requires_reconnect {
            self.mark_connection_unusable();
        }
        let bytes = result.inspect_err(|error| error!("Failed to list directory: {error}"))?;
        let lines = decode_list_bytes(bytes).map_err(ftp_error)?;
        parse_list_lines(path, lines)
    }

    /// Runs a plain control command and records reconnect-worthy failures.
    async fn run_command<R>(
        &self,
        command: impl for<'s> FnOnce(
            &'s mut AsyncFtpStream,
        ) -> Pin<Box<dyn Future<Output = FtpResult<R>> + Send + 's>>,
    ) -> RemoteResult<FtpResult<R>> {
        let mut guard = self.lock_stream().await?;
        let result = command(guard.stream()).await;
        guard.complete();
        if let Err(error) = &result {
            self.record_ftp_error(error);
        }
        Ok(result)
    }

    #[cfg(feature = "tokio-native-tls")]
    fn setup_tls_connector(&self) -> RemoteResult<TlsConnector> {
        let connector = suppaftp::async_native_tls::TlsConnector::new()
            .danger_accept_invalid_certs(self.accept_invalid_certs)
            .danger_accept_invalid_hostnames(self.accept_invalid_hostnames);
        Ok(TlsConnector::from(connector))
    }

    #[cfg(any(feature = "tokio-rustls-aws-lc-rs", feature = "tokio-rustls-ring"))]
    fn setup_tls_connector(&self) -> RemoteResult<TlsConnector> {
        use suppaftp::tokio_rustls::rustls::{ClientConfig, RootCertStore};

        let mut root_store = RootCertStore::empty();
        root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().map(|ta| {
            rustls_pki_types::TrustAnchor {
                subject: ta.subject.clone(),
                subject_public_key_info: ta.subject_public_key_info.clone(),
                name_constraints: ta.name_constraints.clone(),
            }
        }));
        let config = ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth();
        Ok(TlsConnector::from(
            suppaftp::tokio_rustls::TlsConnector::from(Arc::new(config)),
        ))
    }
}

#[async_trait]
impl AsyncRemoteFs for TokioFtpFs {
    async fn connect(&mut self) -> RemoteResult<()> {
        if self.transfer_active.load(Ordering::SeqCst) {
            return Err(RemoteError::with_message(
                RemoteErrorType::ProtocolError,
                "a data transfer is in progress; finish or drop its stream first",
            ));
        }
        if self.connection_usable.load(Ordering::SeqCst) && self.stream.get_mut().is_some() {
            return Err(RemoteError::new(RemoteErrorType::AlreadyConnected));
        }
        if !self.connection_usable.load(Ordering::SeqCst) {
            self.stream.get_mut().take();
            self.connected.store(false, Ordering::SeqCst);
            self.connection_usable.store(true, Ordering::SeqCst);
        }
        info!("Connecting to {}:{}", self.hostname, self.port);
        let mut stream = AsyncFtpStream::connect(format!("{}:{}", self.hostname, self.port))
            .await
            .map_err(|e| {
                error!("Failed to connect to remote server: {}", e);
                ftp_error(e)
            })?;
        if let Some(builder) = &self.passive_stream_builder {
            debug!("Setting up a custom passive stream builder");
            let builder = Arc::clone(builder);
            let connection_usable = Arc::clone(&self.connection_usable);
            stream = stream.passive_stream_builder(move |address| -> Pin<
                Box<dyn Future<Output = FtpResult<TcpStream>> + Send + Sync>,
            > {
                let builder = Arc::clone(&builder);
                let connection_usable = Arc::clone(&connection_usable);
                Box::pin(async move {
                    let result = builder(address).await;
                    if result.is_err() {
                        connection_usable.store(false, Ordering::SeqCst);
                    }
                    result
                })
            });
        }
        stream.set_mode(self.mode);
        #[cfg(any(
            feature = "tokio-native-tls",
            feature = "tokio-rustls-aws-lc-rs",
            feature = "tokio-rustls-ring"
        ))]
        if self.secure {
            debug!("Setting up TLS stream...");
            stream = stream
                .into_secure(self.setup_tls_connector()?, self.hostname.as_str())
                .await
                .map_err(|e| {
                    error!("Failed to negotiate TLS with server: {}", e);
                    RemoteError::with_source(RemoteErrorType::ConnectionError, e)
                })?;
            debug!("TLS handshake OK!");
        }
        debug!("Signin in as {}", self.username);
        stream
            .login(
                self.username.as_str(),
                self.password.as_deref().unwrap_or(""),
            )
            .await
            .map_err(|e| {
                error!("Login failed: {e}");
                ftp_error(e)
            })?;
        trace!("Setting transfer type to Binary");
        stream
            .transfer_type(SuppaFtpFileType::Binary)
            .await
            .map_err(|e| {
                error!("Failed to set transfer type to Binary: {}", e);
                ftp_error(e)
            })?;
        info!("Connection established!");
        *self.welcome.lock().unwrap_or_else(PoisonError::into_inner) =
            stream.get_welcome_msg().map(str::to_string);
        *self.stream.get_mut() = Some(stream);
        self.connected.store(true, Ordering::SeqCst);
        Ok(())
    }

    async fn disconnect(&mut self) -> RemoteResult<()> {
        info!("Disconnecting from FTP server...");
        if self.transfer_active.load(Ordering::SeqCst) {
            return Err(RemoteError::with_message(
                RemoteErrorType::ProtocolError,
                "a data transfer is in progress; finish or drop its stream first",
            ));
        }
        let usable = self.connection_usable.load(Ordering::SeqCst);
        let Some(mut stream) = self.stream.get_mut().take() else {
            return Err(RemoteError::new(RemoteErrorType::NotConnected));
        };
        self.connected.store(false, Ordering::SeqCst);
        self.welcome
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        self.connection_usable.store(true, Ordering::SeqCst);
        if !usable {
            drop(stream);
            return Err(RemoteError::with_message(
                RemoteErrorType::ConnectionError,
                "the control connection was unusable and has been closed",
            ));
        }
        let result = stream.quit().await.map_err(|e| {
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
            && (self.connected.load(Ordering::SeqCst)
                || self.transfer_active.load(Ordering::SeqCst))
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities::STREAM_READ
            | Capabilities::STREAM_WRITE
            | Capabilities::APPEND
            | Capabilities::RANGE_READ
            | Capabilities::EXEC
    }

    async fn list_dir(&self, path: &Path) -> RemoteResult<Vec<File>> {
        debug!("Getting list entries for {}", path.display());
        let remote = remote_path(path)?;
        let path = resolve(path);
        let entries = match self.list_dir_raw(&path, &remote).await {
            Ok(entries) => entries,
            Err(error) if is_ambiguous_path_refusal(&error) => {
                if self.ancestor_proves_absent(&path).await? {
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
            if target_is_file && matches!(self.stat(&path).await, Ok(entry) if entry.is_file()) {
                return Err(RemoteError::with_message(
                    RemoteErrorType::BadFile,
                    "LIST requires a directory path",
                ));
            }
            if entries.is_empty() && matches!(self.stat(&path).await, Ok(entry) if entry.is_file())
            {
                return Err(RemoteError::with_message(
                    RemoteErrorType::BadFile,
                    "LIST requires a directory path",
                ));
            }
        }
        Ok(entries)
    }

    async fn stat(&self, path: &Path) -> RemoteResult<File> {
        debug!("Getting file information for {}", path.display());
        remote_path(path)?;
        let path = resolve(path);
        if path == Path::new("/") {
            trace!("{} has no parent: returning root", path.display());
            let guard = self.lock_stream().await?;
            guard.complete();
            return Ok(File::new(
                path,
                Metadata::default().file_type(FileType::Directory),
            ));
        }
        let parent = path
            .parent()
            .expect("a non-root absolute path must have a parent");
        trace!("Listing entries for stat path file: {}", parent.display());
        let remote_parent = remote_path(parent)?;
        let entries = match self.list_dir_raw(parent, &remote_parent).await {
            Ok(entries) => entries,
            Err(error) if is_ambiguous_path_refusal(&error) => {
                if self.ancestor_proves_absent(parent).await? {
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
            match self.stat(parent).await {
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

    async fn exists(&self, path: &Path) -> RemoteResult<bool> {
        debug!("Checking whether {} exists", path.display());
        match self.stat(path).await {
            Ok(_) => Ok(true),
            Err(err) if err.kind() == RemoteErrorType::NoSuchFileOrDirectory => Ok(false),
            Err(err) => Err(err),
        }
    }

    async fn set_metadata(&self, path: &Path, _metadata: &SetMetadata) -> RemoteResult<()> {
        remote_path(path)?;
        Err(RemoteError::new(RemoteErrorType::UnsupportedFeature))
    }

    async fn create_dir(&self, path: &Path, _mode: Option<UnixPex>) -> RemoteResult<()> {
        debug!("Trying to create directory {}", path.display());
        let remote = remote_path(path)?;
        let result = self
            .run_command(|stream| Box::pin(stream.mkdir(remote.clone())))
            .await?;
        match result {
            Ok(()) => Ok(()),
            Err(FtpError::UnexpectedResponse(response))
                if matches!(
                    response.status,
                    Status::FileUnavailable | Status::BadFilename
                ) =>
            {
                let err = FtpError::UnexpectedResponse(response);
                let error = self.classify_create_dir_refusal(path, err).await;
                error!("Failed to create directory: {error}");
                Err(error)
            }
            Err(e) => {
                error!("Failed to create directory: {}", e);
                Err(ftp_error(e))
            }
        }
    }

    async fn remove_file(&self, path: &Path) -> RemoteResult<()> {
        debug!("Removing file {}", path.display());
        let remote = remote_path(path)?;
        let result = self
            .run_command(|stream| Box::pin(stream.rm(remote.clone())))
            .await?;
        match result {
            Ok(()) => Ok(()),
            Err(err) => {
                let error = self.classify_file_refusal(path, err).await;
                error!("Failed to remove file: {error}");
                Err(error)
            }
        }
    }

    async fn remove_dir(&self, path: &Path) -> RemoteResult<()> {
        debug!("Removing directory {}", path.display());
        let remote = remote_path(path)?;
        let result = self
            .run_command(|stream| Box::pin(stream.rmdir(remote.clone())))
            .await?;
        match result {
            Ok(()) => Ok(()),
            Err(err) => {
                let error = self.classify_remove_dir_refusal(path, err).await;
                error!("Failed to remove directory: {error}");
                Err(error)
            }
        }
    }

    async fn rename(&self, src: &Path, dest: &Path) -> RemoteResult<()> {
        debug!("Trying to rename {} to {}", src.display(), dest.display());
        let remote_src = remote_path(src)?;
        let remote_dest = remote_path(dest)?;
        let result = self
            .run_command(|stream| Box::pin(stream.rename(remote_src.clone(), remote_dest.clone())))
            .await?;
        match result {
            Ok(()) => Ok(()),
            Err(err) => {
                let error = self.classify_rename_refusal(src, dest, err).await;
                error!("Failed to rename file: {error}");
                Err(error)
            }
        }
    }

    async fn copy(&self, src: &Path, dest: &Path) -> RemoteResult<()> {
        remote_path(src)?;
        remote_path(dest)?;
        Err(RemoteError::new(RemoteErrorType::UnsupportedFeature))
    }

    async fn symlink(&self, path: &Path, target: &Path) -> RemoteResult<()> {
        remote_path(path)?;
        remote_path(target)?;
        Err(RemoteError::new(RemoteErrorType::UnsupportedFeature))
    }

    async fn open(&self, path: &Path, opts: &ReadOptions) -> RemoteResult<AsyncReadStream> {
        debug!("Opening {} for read ({opts:?})", path.display());
        let remote = remote_path(path)?;
        let offset = opts.offset.unwrap_or(0);
        let mut guard = self.lock_stream().await?;
        let stream = guard.stream();
        let mut resumed = false;
        if offset > 0 {
            match request_offset(stream, offset).await {
                Ok(value) => resumed = value,
                Err(error) => {
                    self.record_remote_error(&error);
                    guard.complete();
                    return Err(error);
                }
            }
        }
        let mut result = stream.retr_as_stream(&remote).await;
        if resumed && result.is_err() {
            match stream.resume_transfer(0).await {
                Ok(()) => {
                    resumed = false;
                    result = stream.retr_as_stream(&remote).await;
                }
                Err(error) => {
                    self.mark_connection_unusable();
                    guard.complete();
                    error!("Failed to reset REST marker after RETR refusal: {error}");
                    return Err(ftp_error(error));
                }
            }
        }
        if let Err(err) = &result {
            self.record_ftp_error(err);
        }
        let transfer = match result {
            Ok(transfer) => transfer,
            Err(err) => {
                guard.complete();
                error!("Failed to open file: {err}");
                return Err(self.classify_file_refusal(path, err).await);
            }
        };
        let transfer_guard = self.start_transfer();
        guard.complete();
        let mut reader = TokioReadStream::new(transfer, opts.length, transfer_guard);
        if offset > 0 && !resumed {
            debug!("Skipping {offset} bytes locally");
            if let Err(skip) = reader.skip_prefix(offset).await {
                return Err(reader.finish_after_skip(skip).await);
            }
        }
        Ok(AsyncReadStream::new(reader))
    }

    async fn create(&self, path: &Path, opts: &WriteOptions) -> RemoteResult<AsyncWriteStream> {
        debug!("Opening {} for write ({opts:?})", path.display());
        let remote = remote_path(path)?;
        let mut guard = self.lock_stream().await?;
        let result = guard.stream().put_with_stream(&remote).await;
        if let Err(err) = &result {
            self.record_ftp_error(err);
        }
        let transfer = match result {
            Ok(transfer) => transfer,
            Err(err) => {
                guard.complete();
                error!("Failed to open file: {err}");
                return Err(self.classify_create_refusal(path, err).await);
            }
        };
        let transfer_guard = self.start_transfer();
        guard.complete();
        Ok(AsyncWriteStream::new(TokioWriteStream::new(
            transfer,
            transfer_guard,
        )))
    }

    async fn append(&self, path: &Path, opts: &WriteOptions) -> RemoteResult<AsyncWriteStream> {
        debug!("Opening {} for append ({opts:?})", path.display());
        let remote = remote_path(path)?;
        let mut guard = self.lock_stream().await?;
        let result = guard.stream().append_with_stream(&remote).await;
        if let Err(err) = &result {
            self.record_ftp_error(err);
        }
        let transfer = match result {
            Ok(transfer) => transfer,
            Err(err) => {
                guard.complete();
                error!("Failed to open file: {err}");
                return Err(self.classify_create_refusal(path, err).await);
            }
        };
        let transfer_guard = self.start_transfer();
        guard.complete();
        Ok(AsyncWriteStream::new(TokioWriteStream::new(
            transfer,
            transfer_guard,
        )))
    }

    async fn exec(&self, cmd: &str) -> RemoteResult<ExecOutput> {
        debug!("Executing command: {cmd}");
        let mut guard = self.lock_stream().await?;
        let result = guard.stream().site(cmd).await;
        guard.complete();
        let response = result.map_err(|e| {
            error!("Failed to execute command: {}", e);
            let error = ftp_error(e);
            self.record_remote_error(&error);
            error
        })?;
        let status = response.status.code();
        debug!("Command executed with status {status}");
        Ok(ExecOutput::new(
            status,
            String::from_utf8_lossy(&response.body).into_owned(),
        ))
    }
}

/// Requests a native FTP restart marker, falling back to a local skip when refused.
async fn request_offset(stream: &mut AsyncFtpStream, offset: u64) -> RemoteResult<bool> {
    let Ok(offset) = usize::try_from(offset) else {
        warn!("offset {offset} does not fit the REST command; skipping locally");
        return Ok(false);
    };
    match stream.resume_transfer(offset).await {
        Ok(()) => Ok(true),
        Err(FtpError::UnexpectedResponse(response)) if response.status != Status::NotAvailable => {
            warn!(
                "server refused REST {offset} ({}); skipping locally",
                response.status
            );
            Ok(false)
        }
        Err(error) => {
            error!("Failed to request offset {offset}: {error}");
            Err(ftp_error(error))
        }
    }
}

/// Reads raw `LIST` bytes and reports whether cleanup requires reconnecting.
async fn read_list_bytes(
    stream: &mut AsyncFtpStream,
    remote: &str,
) -> (RemoteResult<Vec<u8>>, bool) {
    let (_, mut transfer) = match stream
        .custom_data_command(
            format!("LIST {remote}"),
            &[Status::AboutToSend, Status::AlreadyOpen],
        )
        .await
    {
        Ok(transfer) => transfer,
        Err(err) => {
            let requires_reconnect = transfer_setup_requires_reconnect(&err);
            return (Err(ftp_error(err)), requires_reconnect);
        }
    };
    let mut bytes = Vec::new();
    let read_result = transfer
        .read_to_end(&mut bytes)
        .await
        .map(|_| ())
        .map_err(FtpError::ConnectionError);
    let finish_result = transfer.finish().await;
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

#[cfg(test)]
mod tests {
    use std::io::BufReader;
    use std::net::TcpListener;
    use std::path::Path;
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;

    use pretty_assertions::assert_eq;

    use super::super::scripted::*;
    use super::*;

    #[tokio::test]
    async fn unsupported_operations_validate_every_path() {
        let client = TokioFtpFs::new("localhost", 21);
        let absolute = Path::new("/file");
        let relative = Path::new("file");
        for result in [
            client.set_metadata(relative, &SetMetadata::default()).await,
            client.copy(relative, absolute).await,
            client.copy(absolute, relative).await,
            client.symlink(relative, absolute).await,
            client.symlink(absolute, relative).await,
        ] {
            assert_eq!(result.unwrap_err().kind(), RemoteErrorType::InvalidPath);
        }
        for result in [
            client.set_metadata(absolute, &SetMetadata::default()).await,
            client.copy(absolute, absolute).await,
            client.symlink(absolute, absolute).await,
        ] {
            assert_eq!(
                result.unwrap_err().kind(),
                RemoteErrorType::UnsupportedFeature
            );
        }
    }

    #[tokio::test]
    async fn disconnected_client_reports_not_connected() {
        let mut client = TokioFtpFs::new("localhost", 21);
        assert!(!client.is_connected());
        assert_eq!(
            client.exec("NOOP").await.unwrap_err().kind(),
            RemoteErrorType::NotConnected
        );
        assert_eq!(
            client.disconnect().await.unwrap_err().kind(),
            RemoteErrorType::NotConnected
        );
        assert!(client.welcome_message().is_none());
        assert!(client.stream().is_none());
    }

    #[tokio::test]
    async fn command_421_allows_a_fresh_connection() {
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

        let mut client = TokioFtpFs::new(control_address.ip().to_string(), control_address.port());
        client.connect().await.unwrap();
        assert!(client.is_connected());
        assert!(client.welcome_message().is_some());
        assert_eq!(
            client.connect().await.unwrap_err().kind(),
            RemoteErrorType::AlreadyConnected
        );
        assert_eq!(
            client.exec("NOOP").await.unwrap_err().kind(),
            RemoteErrorType::ConnectionError
        );
        assert!(!client.is_connected());
        assert_eq!(
            client.exec("NOOP").await.unwrap_err().kind(),
            RemoteErrorType::ConnectionError
        );
        client.connect().await.unwrap();
        assert_eq!(client.exec("NOOP").await.unwrap().exit_code, 200);
        client.disconnect().await.unwrap();
        assert!(!client.is_connected());
        tokio::task::spawn_blocking(move || server.join().unwrap())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn cancelled_operation_requires_reconnect() {
        let control_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let control_address = control_listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (control, _) = control_listener.accept().unwrap();
            let mut control = BufReader::new(control);
            authenticate_scripted_connection(&mut control);
            assert_eq!(read_scripted_command(&mut control), "SITE NOOP\r\n");
            let mut trailing = String::new();
            assert_eq!(read_scripted_command_opt(&mut control, &mut trailing), 0);
        });

        let mut client = TokioFtpFs::new(control_address.ip().to_string(), control_address.port());
        client.connect().await.unwrap();
        let cancelled = tokio::time::timeout(Duration::from_millis(200), client.exec("NOOP")).await;
        assert!(cancelled.is_err());
        assert!(!client.is_connected());
        assert_eq!(
            client.exec("NOOP").await.unwrap_err().kind(),
            RemoteErrorType::ConnectionError
        );
        assert_eq!(
            client.disconnect().await.unwrap_err().kind(),
            RemoteErrorType::ConnectionError
        );
        tokio::task::spawn_blocking(move || server.join().unwrap())
            .await
            .unwrap();
    }

    fn is_send<T: Send>(_send: T) {}

    fn is_sync<T: Sync>(_sync: T) {}

    #[test]
    fn test_should_be_send_and_sync_and_object_safe() {
        is_send(TokioFtpFs::new("127.0.0.1", 10021));
        is_sync(TokioFtpFs::new("127.0.0.1", 10021));
        let _: Box<dyn AsyncRemoteFs> = Box::new(TokioFtpFs::new("127.0.0.1", 10021));
        let _: Arc<dyn AsyncRemoteFs> = Arc::new(TokioFtpFs::new("127.0.0.1", 10021));
    }

    #[tokio::test]
    async fn ranged_open_requests_rest_before_retr() {
        let control_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let control_address = control_listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (control, _) = control_listener.accept().unwrap();
            let mut control = BufReader::new(control);
            authenticate_scripted_connection(&mut control);
            assert_eq!(read_scripted_command(&mut control), "REST 2\r\n");
            write_scripted_reply(&mut control, "350 restart position accepted\r\n");
            let transfer_listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let transfer_port = transfer_listener.local_addr().unwrap().port();
            assert_eq!(read_scripted_command(&mut control), "PASV\r\n");
            write_scripted_reply(
                &mut control,
                &format!(
                    "227 passive (127,0,0,1,{},{})\r\n",
                    transfer_port / 256,
                    transfer_port % 256
                ),
            );
            assert_eq!(read_scripted_command(&mut control), "RETR /file\r\n");
            write_scripted_reply(&mut control, "150 opening data\r\n");
            let (mut transfer_data, _) = transfer_listener.accept().unwrap();
            std::io::Write::write_all(&mut transfer_data, b"cdef").unwrap();
            drop(transfer_data);
            write_scripted_reply(&mut control, "226 transfer complete\r\n");
            assert_eq!(read_scripted_command(&mut control), "QUIT\r\n");
            write_scripted_reply(&mut control, "221 bye\r\n");
        });

        let mut client = TokioFtpFs::new(control_address.ip().to_string(), control_address.port());
        client.connect().await.unwrap();
        let mut reader = client
            .open(Path::new("/file"), &ReadOptions::default().offset(2))
            .await
            .unwrap();
        let mut contents = Vec::new();
        futures::AsyncReadExt::read_to_end(&mut reader, &mut contents)
            .await
            .unwrap();
        reader.finish().await.unwrap();
        assert_eq!(contents, b"cdef");
        client.disconnect().await.unwrap();
        tokio::task::spawn_blocking(move || server.join().unwrap())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn list_dir_classifies_a_direct_single_file_listing() {
        let control_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let control_address = control_listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (control, _) = control_listener.accept().unwrap();
            let mut control = BufReader::new(control);
            authenticate_scripted_connection(&mut control);
            complete_scripted_list(
                &mut control,
                "/dir/file",
                b"-rw-r--r-- 1 test test 4 Jan 01 12:00 file\r\n",
            );
            complete_scripted_list(
                &mut control,
                "/dir",
                b"-rw-r--r-- 1 test test 4 Jan 01 12:00 file\r\n",
            );
            assert_eq!(read_scripted_command(&mut control), "QUIT\r\n");
            write_scripted_reply(&mut control, "221 bye\r\n");
        });

        let mut client = TokioFtpFs::new(control_address.ip().to_string(), control_address.port());
        client.connect().await.unwrap();
        assert_eq!(
            client
                .list_dir(Path::new("/dir/file"))
                .await
                .unwrap_err()
                .kind(),
            RemoteErrorType::BadFile
        );
        client.disconnect().await.unwrap();
        tokio::task::spawn_blocking(move || server.join().unwrap())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn list_dir_classifies_a_missing_path_after_parent_probe() {
        let control_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let control_address = control_listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (control, _) = control_listener.accept().unwrap();
            let mut control = BufReader::new(control);
            authenticate_scripted_connection(&mut control);
            refuse_scripted_list(&mut control, "/dir/missing");
            complete_scripted_list(
                &mut control,
                "/dir",
                b"-rw-r--r-- 1 test test 4 Jan 01 12:00 other\r\n",
            );
            assert_eq!(read_scripted_command(&mut control), "QUIT\r\n");
            write_scripted_reply(&mut control, "221 bye\r\n");
        });

        let mut client = TokioFtpFs::new(control_address.ip().to_string(), control_address.port());
        client.connect().await.unwrap();
        assert_eq!(
            client
                .list_dir(Path::new("/dir/missing"))
                .await
                .unwrap_err()
                .kind(),
            RemoteErrorType::NoSuchFileOrDirectory
        );
        client.disconnect().await.unwrap();
        tokio::task::spawn_blocking(move || server.join().unwrap())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn list_accepts_already_open_and_keeps_control_connection_synchronized() {
        let control_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let control_address = control_listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (control, _) = control_listener.accept().unwrap();
            let mut control = BufReader::new(control);
            let reply = |control: &mut BufReader<std::net::TcpStream>, response: &str| {
                std::io::Write::write_all(control.get_mut(), response.as_bytes()).unwrap();
            };
            let command = |control: &mut BufReader<std::net::TcpStream>| {
                let mut line = String::new();
                std::io::BufRead::read_line(control, &mut line).unwrap();
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
            std::io::Write::write_all(
                &mut data,
                b"-rw-r--r-- 1 1000 1000 0 Nov 5 2024 file.txt\r\n",
            )
            .unwrap();
            drop(data);
            reply(&mut control, "226 listing complete\r\n");

            assert_eq!(command(&mut control), "SITE NOOP\r\n");
            reply(&mut control, "200 noop\r\n");
            assert_eq!(command(&mut control), "QUIT\r\n");
            reply(&mut control, "221 bye\r\n");
        });

        let mut client = TokioFtpFs::new(control_address.ip().to_string(), control_address.port());
        client.connect().await.unwrap();
        let entries = client.list_dir(Path::new("/")).await.unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(client.exec("NOOP").await.unwrap().exit_code, 200);
        client.disconnect().await.unwrap();
        tokio::task::spawn_blocking(move || server.join().unwrap())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn list_dir_succeeds_when_only_the_requested_directory_can_be_listed() {
        let control_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let control_address = control_listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (control, _) = control_listener.accept().unwrap();
            let mut control = BufReader::new(control);
            let reply = |control: &mut BufReader<std::net::TcpStream>, response: &str| {
                std::io::Write::write_all(control.get_mut(), response.as_bytes()).unwrap();
            };
            let command = |control: &mut BufReader<std::net::TcpStream>| {
                let mut line = String::new();
                std::io::BufRead::read_line(control, &mut line).unwrap();
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
            std::io::Write::write_all(
                &mut data,
                b"-rw-r--r-- 1 1000 1000 0 Nov 5 2024 child.txt\r\n",
            )
            .unwrap();
            drop(data);
            reply(&mut control, "226 listing complete\r\n");

            assert_eq!(command(&mut control), "SITE NOOP\r\n");
            reply(&mut control, "200 noop\r\n");
            assert_eq!(command(&mut control), "QUIT\r\n");
            reply(&mut control, "221 bye\r\n");
        });

        let mut client = TokioFtpFs::new(control_address.ip().to_string(), control_address.port());
        client.connect().await.unwrap();
        let entries = client.list_dir(Path::new("/allowed")).await.unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path(), Path::new("/allowed/child.txt"));
        assert_eq!(client.exec("NOOP").await.unwrap().exit_code, 200);
        client.disconnect().await.unwrap();
        tokio::task::spawn_blocking(move || server.join().unwrap())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn stat_rejects_children_beneath_a_regular_file_listing() {
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

        let mut client = TokioFtpFs::new(control_address.ip().to_string(), control_address.port());
        client.connect().await.unwrap();
        assert_eq!(
            client
                .stat(Path::new("/file.txt/file.txt"))
                .await
                .unwrap_err()
                .kind(),
            RemoteErrorType::BadFile
        );
        client.disconnect().await.unwrap();
        tokio::task::spawn_blocking(move || server.join().unwrap())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn stat_keeps_a_child_when_parent_disambiguation_is_denied() {
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

        let mut client = TokioFtpFs::new(control_address.ip().to_string(), control_address.port());
        client.connect().await.unwrap();
        let file = client.stat(Path::new("/pub/pub")).await.unwrap();
        assert_eq!(file.path(), Path::new("/pub/pub"));
        assert!(file.is_file());
        client.disconnect().await.unwrap();
        tokio::task::spawn_blocking(move || server.join().unwrap())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn list_dir_keeps_an_empty_directory_symlink_listing() {
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

        let mut client = TokioFtpFs::new(control_address.ip().to_string(), control_address.port());
        client.connect().await.unwrap();
        assert!(
            client
                .list_dir(Path::new("/link"))
                .await
                .unwrap()
                .is_empty()
        );
        client.disconnect().await.unwrap();
        tokio::task::spawn_blocking(move || server.join().unwrap())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn ancestor_probe_lists_each_parent_at_most_once() {
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

        let mut client = TokioFtpFs::new(control_address.ip().to_string(), control_address.port());
        client.connect().await.unwrap();
        assert_eq!(
            client
                .stat(Path::new("/a/b/c/file"))
                .await
                .unwrap_err()
                .kind(),
            RemoteErrorType::PermissionDenied
        );
        assert_eq!(client.exec("NOOP").await.unwrap().exit_code, 200);
        client.disconnect().await.unwrap();
        tokio::task::spawn_blocking(move || server.join().unwrap())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn list_setup_421_allows_a_fresh_connection() {
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

        let mut client = TokioFtpFs::new(control_address.ip().to_string(), control_address.port());
        client.connect().await.unwrap();
        assert_eq!(
            client.list_dir(Path::new("/")).await.unwrap_err().kind(),
            RemoteErrorType::ConnectionError
        );
        assert!(!client.is_connected());
        client.connect().await.unwrap();
        assert_eq!(client.exec("NOOP").await.unwrap().exit_code, 200);
        client.disconnect().await.unwrap();
        tokio::task::spawn_blocking(move || server.join().unwrap())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn failed_list_completion_requires_reconnect_before_follow_up_commands() {
        let control_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let control_address = control_listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (control, _) = control_listener.accept().unwrap();
            let mut control = BufReader::new(control);
            let reply = |control: &mut BufReader<std::net::TcpStream>, response: &str| {
                std::io::Write::write_all(control.get_mut(), response.as_bytes()).unwrap();
            };
            let command = |control: &mut BufReader<std::net::TcpStream>| {
                let mut line = String::new();
                std::io::BufRead::read_line(control, &mut line).unwrap();
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
            std::io::Write::write_all(
                &mut data,
                b"-rw-r--r-- 1 1000 1000 0 Nov 5 2024 file.txt\r\n",
            )
            .unwrap();
            drop(data);
            reply(&mut control, "426 transfer aborted\r\n");

            control
                .get_mut()
                .set_read_timeout(Some(Duration::from_millis(500)))
                .unwrap();
            let mut follow_up = String::new();
            if std::io::BufRead::read_line(&mut control, &mut follow_up).unwrap_or(0) > 0 {
                assert_eq!(follow_up, "SITE NOOP\r\n");
                reply(&mut control, "200 noop\r\n");
                assert_eq!(command(&mut control), "QUIT\r\n");
                reply(&mut control, "221 bye\r\n");
            }
        });

        let mut client = TokioFtpFs::new(control_address.ip().to_string(), control_address.port());
        client.connect().await.unwrap();
        assert_eq!(
            client.list_dir(Path::new("/")).await.unwrap_err().kind(),
            RemoteErrorType::ConnectionError
        );
        assert_eq!(
            client.exec("NOOP").await.unwrap_err().kind(),
            RemoteErrorType::ConnectionError
        );
        assert_eq!(
            client.disconnect().await.unwrap_err().kind(),
            RemoteErrorType::ConnectionError
        );
        tokio::task::spawn_blocking(move || server.join().unwrap())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn invalid_utf8_after_successful_list_completion_keeps_connection_usable() {
        let control_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let control_address = control_listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (control, _) = control_listener.accept().unwrap();
            let mut control = BufReader::new(control);
            let reply = |control: &mut BufReader<std::net::TcpStream>, response: &str| {
                std::io::Write::write_all(control.get_mut(), response.as_bytes()).unwrap();
            };
            let command = |control: &mut BufReader<std::net::TcpStream>| {
                let mut line = String::new();
                std::io::BufRead::read_line(control, &mut line).unwrap();
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
            std::io::Write::write_all(
                &mut data,
                b"-rw-r--r-- 1 1000 1000 1 Nov 5 2024 bad\xff\r\n",
            )
            .unwrap();
            drop(data);
            reply(&mut control, "226 listing complete\r\n");

            assert_eq!(command(&mut control), "SITE NOOP\r\n");
            reply(&mut control, "200 noop\r\n");
            assert_eq!(command(&mut control), "QUIT\r\n");
            reply(&mut control, "221 bye\r\n");
        });

        let mut client = TokioFtpFs::new(control_address.ip().to_string(), control_address.port());
        client.connect().await.unwrap();
        assert_eq!(
            client.list_dir(Path::new("/")).await.unwrap_err().kind(),
            RemoteErrorType::ProtocolError
        );
        assert_eq!(client.exec("NOOP").await.unwrap().exit_code, 200);
        client.disconnect().await.unwrap();
        tokio::task::spawn_blocking(move || server.join().unwrap())
            .await
            .unwrap();
    }
}
