//! Tokio-backed FTP client.

#![allow(unused_imports)]

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
        let _ = path;
        todo!("task 5")
    }

    async fn stat(&self, path: &Path) -> RemoteResult<File> {
        let _ = path;
        todo!("task 5")
    }

    async fn exists(&self, path: &Path) -> RemoteResult<bool> {
        let _ = path;
        todo!("task 5")
    }

    async fn set_metadata(&self, path: &Path, _metadata: &SetMetadata) -> RemoteResult<()> {
        remote_path(path)?;
        Err(RemoteError::new(RemoteErrorType::UnsupportedFeature))
    }

    async fn create_dir(&self, path: &Path, _mode: Option<UnixPex>) -> RemoteResult<()> {
        let _ = path;
        todo!("task 5")
    }

    async fn remove_file(&self, path: &Path) -> RemoteResult<()> {
        let _ = path;
        todo!("task 5")
    }

    async fn remove_dir(&self, path: &Path) -> RemoteResult<()> {
        let _ = path;
        todo!("task 5")
    }

    async fn rename(&self, src: &Path, dest: &Path) -> RemoteResult<()> {
        let _ = (src, dest);
        todo!("task 5")
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
        let _ = (path, opts);
        todo!("task 5")
    }

    async fn create(&self, path: &Path, opts: &WriteOptions) -> RemoteResult<AsyncWriteStream> {
        let _ = (path, opts);
        todo!("task 5")
    }

    async fn append(&self, path: &Path, opts: &WriteOptions) -> RemoteResult<AsyncWriteStream> {
        let _ = (path, opts);
        todo!("task 5")
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
}
