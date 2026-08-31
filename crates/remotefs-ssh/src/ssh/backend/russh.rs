//! [russh](https://docs.rs/russh/latest/russh/) backend for `remotefs-ssh`.

mod auth;
mod scp;

use std::borrow::Cow;
use std::future::Future as _;
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll};

use remotefs::fs::{Metadata, ReadStream, WriteStream};
use remotefs::{File, RemoteError, RemoteErrorType, RemoteResult};
use russh::client::{Handle, Handler};
use russh::keys::{Algorithm, PublicKeyOrCertificate};
use russh::{Disconnect, client};
use russh_sftp::client::SftpSession;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpSocket, TcpStream};
use tokio::runtime::Runtime;
use tokio::time::{Instant, Sleep};

use super::{SshSession, WriteMode};
use crate::SshOpts;
use crate::ssh::backend::Sftp;
use crate::ssh::config::Config;
use crate::ssh::key_method::MethodType;

/// The default SSH client handler for russh.
///
/// Accepts all server host keys. Host key verification should be implemented
/// by the caller if stricter security is required.
///
/// You can implement your own [`Handler`] and use it with [`RusshSession`] if you want a different behaviour.
#[derive(Default)]
pub struct NoCheckServerKey;

impl Handler for NoCheckServerKey {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        _server_public_key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        Ok(true)
    }
}

/// [`russh`](https://docs.rs/russh/latest/russh) session.
pub struct RusshSession<T>
where
    T: Handler + Default + Send + 'static,
{
    runtime: Arc<Runtime>,
    session: Handle<T>,
}

/// SFTP handle for russh.
pub struct RusshSftp {
    runtime: Arc<Runtime>,
    session: Arc<SftpSession>,
}

struct ConnectionDeadlineStream {
    inner: TcpStream,
    deadline: Pin<Box<Sleep>>,
    deadline_active: Arc<AtomicBool>,
}

impl ConnectionDeadlineStream {
    fn new(inner: TcpStream, deadline: Instant, deadline_active: Arc<AtomicBool>) -> Self {
        Self {
            inner,
            deadline: Box::pin(tokio::time::sleep_until(deadline)),
            deadline_active,
        }
    }

    fn poll_timed_out(&mut self, context: &mut Context<'_>) -> bool {
        self.deadline_active.load(Ordering::Acquire)
            && self.deadline.as_mut().poll(context).is_ready()
    }
}

impl AsyncRead for ConnectionDeadlineStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        if self.poll_timed_out(context) {
            return Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "SSH connection timed out",
            )));
        }
        Pin::new(&mut self.inner).poll_read(context, buffer)
    }
}

impl AsyncWrite for ConnectionDeadlineStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<Result<usize, std::io::Error>> {
        if self.poll_timed_out(context) {
            return Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "SSH connection timed out",
            )));
        }
        Pin::new(&mut self.inner).poll_write(context, buffer)
    }

    fn poll_flush(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        if self.poll_timed_out(context) {
            return Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "SSH connection timed out",
            )));
        }
        Pin::new(&mut self.inner).poll_flush(context)
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        if self.poll_timed_out(context) {
            return Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "SSH connection timed out",
            )));
        }
        Pin::new(&mut self.inner).poll_shutdown(context)
    }
}

fn connect_with_timeout<T>(
    runtime: &Runtime,
    config: Arc<client::Config>,
    address: &str,
    bind_address: Option<&str>,
    timeout: std::time::Duration,
) -> RemoteResult<Handle<T>>
where
    T: Handler + Default + Send + 'static,
{
    let deadline = Instant::now() + timeout;
    let stream = runtime
        .block_on(async {
            tokio::time::timeout_at(deadline, connect_tcp(address, bind_address)).await
        })
        .map_err(|err| {
            let msg = format!("SSH connection timed out: {err}");
            error!("{msg}");
            RemoteError::new_ex(RemoteErrorType::ConnectionError, msg)
        })?
        .map_err(|err| {
            let msg = format!("SSH connection failed: {err}");
            error!("{msg}");
            RemoteError::new_ex(RemoteErrorType::ConnectionError, msg)
        })?;
    if config.nodelay
        && let Err(err) = stream.set_nodelay(true)
    {
        warn!("Failed to enable TCP_NODELAY: {err}");
    }

    let deadline_active = Arc::new(AtomicBool::new(true));
    let connection_deadline_active = deadline_active.clone();
    let session_result = runtime.block_on(async {
        let stream = ConnectionDeadlineStream::new(stream, deadline, connection_deadline_active);
        client::connect_stream(config, stream, T::default()).await
    });
    deadline_active.store(false, Ordering::Release);
    session_result.map_err(|err| {
        let msg = format!("SSH connection failed: {err:?}");
        error!("{msg}");
        RemoteError::new_ex(RemoteErrorType::ConnectionError, msg)
    })
}

async fn connect_tcp(address: &str, bind_address: Option<&str>) -> std::io::Result<TcpStream> {
    let Some(bind_address) = bind_address else {
        return TcpStream::connect(address).await;
    };

    let source_addresses: Vec<_> = tokio::net::lookup_host((bind_address, 0)).await?.collect();
    let target_addresses: Vec<_> = tokio::net::lookup_host(address).await?.collect();
    let mut last_error = None;
    for target_address in target_addresses {
        for source_address in source_addresses
            .iter()
            .filter(|source| source.is_ipv4() == target_address.is_ipv4())
        {
            let socket = if target_address.is_ipv4() {
                TcpSocket::new_v4()
            } else {
                TcpSocket::new_v6()
            };
            let socket = match socket {
                Ok(socket) => socket,
                Err(err) => {
                    last_error = Some(err);
                    continue;
                }
            };
            if let Err(err) = socket.bind(*source_address) {
                last_error = Some(err);
                continue;
            }
            match socket.connect(target_address).await {
                Ok(stream) => return Ok(stream),
                Err(err) => last_error = Some(err),
            }
        }
    }

    Err(last_error.unwrap_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::AddrNotAvailable,
            format!("no compatible address family found for BindAddress {bind_address}"),
        )
    }))
}

impl<T> SshSession for RusshSession<T>
where
    T: Handler + Default + Send + 'static,
{
    type Sftp = RusshSftp;

    fn connect(opts: &SshOpts) -> RemoteResult<Self> {
        let runtime = opts.runtime.as_ref().cloned().ok_or_else(|| {
            RemoteError::new_ex(
                RemoteErrorType::UnsupportedFeature,
                "RusshSession requires a Tokio runtime",
            )
        })?;

        let ssh_config = Config::try_from(opts)?;
        debug!("Connecting to '{}'", ssh_config.address);

        let mut config = client::Config::default();

        // Apply algorithm preferences from ssh config
        apply_config_algo_prefs(&mut config, &ssh_config);

        // Apply algorithm preferences from opts
        apply_opts_algo_prefs(&mut config, opts);

        let config = Arc::new(config);
        let connection_attempts = ssh_config.connection_attempts.max(1);
        let mut attempt = 1;
        let mut session = loop {
            match connect_with_timeout::<T>(
                &runtime,
                config.clone(),
                &ssh_config.address,
                ssh_config.params.bind_address.as_deref(),
                ssh_config.connection_timeout,
            ) {
                Ok(session) => break session,
                Err(err) if attempt < connection_attempts => {
                    warn!("SSH connection attempt {attempt} failed: {err}");
                    attempt += 1;
                }
                Err(err) => return Err(err),
            }
        };

        // Authenticate
        auth::authenticate(&mut session, &runtime, opts, &ssh_config)?;

        Ok(Self { runtime, session })
    }

    fn disconnect(&self) -> RemoteResult<()> {
        self.runtime
            .block_on(async {
                self.session
                    .disconnect(Disconnect::ByApplication, "Closed by user", "en_US")
                    .await
            })
            .map_err(|err| {
                log::error!("failed to disconnect {err}");
                RemoteError::new_ex(RemoteErrorType::ConnectionError, err.to_string())
            })
    }

    fn banner(&self) -> RemoteResult<Option<String>> {
        // russh delivers the auth banner via the Handler::auth_banner callback
        // during authentication, but does not expose it from the Handle after the fact.
        // <https://docs.rs/russh/latest/russh/client/struct.Handle.html>
        // <https://docs.rs/russh/latest/russh/client/trait.Handler.html#method.auth_banner>
        Ok(None)
    }

    fn authenticated(&self) -> RemoteResult<bool> {
        Ok(!self.session.is_closed())
    }

    fn cmd<S>(&mut self, cmd: S) -> RemoteResult<(u32, String)>
    where
        S: AsRef<str>,
    {
        let cmd = cmd.as_ref();
        trace!("Running command: {cmd}");

        // Escape single quotes and wrap in sh -c for consistent shell behavior.
        // Without this, commands like "cd /some/dir; somecommand" fail if the
        // remote user's login shell is fish or another non-POSIX shell.
        let escaped = cmd.replace('\'', r#"'\''"#);
        let wrapped = format!("sh -c '{escaped}'");

        self.runtime
            .block_on(async { perform_shell_cmd(&self.session, &wrapped).await })
    }

    fn scp_recv(&self, path: &Path) -> RemoteResult<Box<dyn Read + Send>> {
        self.runtime
            .block_on(async { scp::recv(&self.session, path).await })
    }

    fn scp_send(
        &self,
        remote_path: &Path,
        mode: i32,
        size: u64,
        _times: Option<(u64, u64)>,
    ) -> RemoteResult<Box<dyn Write + Send>> {
        let runtime = self.runtime.clone();
        self.runtime
            .block_on(async { scp::send(&self.session, remote_path, mode, size, runtime).await })
    }

    fn sftp(&self) -> RemoteResult<Self::Sftp> {
        let channel = self
            .runtime
            .block_on(async {
                let channel = self.session.channel_open_session().await?;
                channel.request_subsystem(true, "sftp").await?;
                Ok(channel)
            })
            .map_err(|err: russh::Error| {
                error!("Failed to init SFTP session: {err}");
                RemoteError::new_ex(RemoteErrorType::ProtocolError, err.to_string())
            })?;

        self.runtime
            .block_on(async { SftpSession::new(channel.into_stream()).await })
            .map(|session| RusshSftp {
                runtime: self.runtime.clone(),
                session: Arc::new(session),
            })
            .map_err(|err| {
                error!("Failed to init SFTP session: {err}");
                RemoteError::new_ex(RemoteErrorType::ProtocolError, err.to_string())
            })
    }
}

impl Sftp for RusshSftp {
    fn mkdir(&self, path: &Path, mode: i32) -> RemoteResult<()> {
        let path_str = path.to_string_lossy().to_string();
        self.runtime.block_on(async {
            self.session.create_dir(&path_str).await.map_err(|err| {
                RemoteError::new_ex(
                    RemoteErrorType::FileCreateDenied,
                    format!("Could not create directory '{}': {err}", path.display()),
                )
            })?;
            // create_dir does not set permissions; apply them separately
            let mut attrs = russh_sftp::protocol::FileAttributes::empty();
            attrs.permissions = Some(mode as u32 & 0o7777);
            self.session
                .set_metadata(&path_str, attrs)
                .await
                .map_err(|err| {
                    RemoteError::new_ex(
                        RemoteErrorType::ProtocolError,
                        format!("Could not set permissions on '{}': {err}", path.display()),
                    )
                })
        })
    }

    fn open_read(&self, path: &Path) -> RemoteResult<ReadStream> {
        let path_str = path.to_string_lossy().to_string();
        let reader = PipelinedSftpReader::new(self.runtime.clone(), self.session.clone(), path_str)
            .map_err(|err| {
                RemoteError::new_ex(
                    RemoteErrorType::ProtocolError,
                    format!("Could not read file at '{}': {err}", path.display()),
                )
            })?;
        Ok(ReadStream::from(Box::new(reader) as Box<dyn Read + Send>))
    }

    fn open_write(&self, path: &Path, flags: WriteMode, mode: i32) -> RemoteResult<WriteStream> {
        let path_str = path.to_string_lossy().to_string();
        self.runtime.block_on(async {
            let open_flags = match flags {
                WriteMode::Append => {
                    russh_sftp::protocol::OpenFlags::WRITE
                        | russh_sftp::protocol::OpenFlags::APPEND
                        | russh_sftp::protocol::OpenFlags::CREATE
                }
                WriteMode::Truncate => {
                    russh_sftp::protocol::OpenFlags::WRITE
                        | russh_sftp::protocol::OpenFlags::CREATE
                        | russh_sftp::protocol::OpenFlags::TRUNCATE
                }
            };

            let mut attrs = russh_sftp::protocol::FileAttributes::empty();
            attrs.permissions = Some(mode as u32 & 0o7777);

            let file = self
                .session
                .open_with_flags_and_attributes(&path_str, open_flags, attrs)
                .await
                .map_err(|err| {
                    RemoteError::new_ex(
                        RemoteErrorType::ProtocolError,
                        format!("Could not open file at '{}': {err}", path.display()),
                    )
                })?;

            let writer = SftpFileWriter {
                file,
                runtime: self.runtime.clone(),
            };
            Ok(WriteStream::from(
                Box::new(writer) as Box<dyn remotefs::fs::stream::WriteAndSeek>
            ))
        })
    }

    fn readdir<T>(&self, dirname: T) -> RemoteResult<Vec<File>>
    where
        T: AsRef<Path>,
    {
        let dirname = dirname.as_ref();
        let dir_str = dirname.to_string_lossy().to_string();
        self.runtime.block_on(async {
            let entries = self.session.read_dir(&dir_str).await.map_err(|err| {
                RemoteError::new_ex(
                    RemoteErrorType::ProtocolError,
                    format!("Could not read directory: {err}"),
                )
            })?;

            let mut files = Vec::new();
            for entry in entries {
                let entry_path = dirname.join(entry.file_name());
                let symlink = if entry.file_type().is_symlink() {
                    match self
                        .session
                        .read_link(entry_path.to_string_lossy().as_ref())
                        .await
                    {
                        Ok(target) => Some(PathBuf::from(target)),
                        Err(err) => {
                            error!(
                                "Failed to read link of {} (even though it's a symlink): {err}",
                                entry_path.display()
                            );
                            None
                        }
                    }
                } else {
                    None
                };
                files.push(make_fsentry(&entry_path, &entry.metadata(), symlink));
            }

            Ok(files)
        })
    }

    fn realpath(&self, path: &Path) -> RemoteResult<PathBuf> {
        let path_str = path.to_string_lossy().to_string();
        self.runtime.block_on(async {
            self.session
                .canonicalize(&path_str)
                .await
                .map(PathBuf::from)
                .map_err(|err| {
                    RemoteError::new_ex(
                        RemoteErrorType::ProtocolError,
                        format!(
                            "Could not resolve real path for '{}': {err}",
                            path.display()
                        ),
                    )
                })
        })
    }

    fn rename(&self, src: &Path, dest: &Path) -> RemoteResult<()> {
        let src_str = src.to_string_lossy().to_string();
        let dest_str = dest.to_string_lossy().to_string();
        self.runtime.block_on(async {
            self.session
                .rename(&src_str, &dest_str)
                .await
                .map_err(|err| {
                    RemoteError::new_ex(
                        RemoteErrorType::ProtocolError,
                        format!("Could not rename file '{}': {err}", src.display()),
                    )
                })
        })
    }

    fn rmdir(&self, path: &Path) -> RemoteResult<()> {
        let path_str = path.to_string_lossy().to_string();
        self.runtime.block_on(async {
            self.session.remove_dir(&path_str).await.map_err(|err| {
                RemoteError::new_ex(
                    RemoteErrorType::CouldNotRemoveFile,
                    format!("Could not remove directory '{}': {err}", path.display()),
                )
            })
        })
    }

    fn setstat(&self, path: &Path, metadata: Metadata) -> RemoteResult<()> {
        let path_str = path.to_string_lossy().to_string();
        let attrs = metadata_to_file_attributes(metadata);
        self.runtime.block_on(async {
            self.session
                .set_metadata(&path_str, attrs)
                .await
                .map_err(|err| {
                    RemoteError::new_ex(
                        RemoteErrorType::ProtocolError,
                        format!(
                            "Could not set file attributes for '{}': {err}",
                            path.display()
                        ),
                    )
                })
        })
    }

    fn stat(&self, filename: &Path) -> RemoteResult<File> {
        let path_str = filename.to_string_lossy().to_string();
        self.runtime.block_on(async {
            let attrs = self.session.metadata(&path_str).await.map_err(|err| {
                RemoteError::new_ex(
                    RemoteErrorType::ProtocolError,
                    format!(
                        "Could not get file attributes for '{}': {err}",
                        filename.display()
                    ),
                )
            })?;

            let symlink = if attrs.is_symlink() {
                match self.session.read_link(&path_str).await {
                    Ok(target) => Some(PathBuf::from(target)),
                    Err(err) => {
                        error!(
                            "Failed to read link of {} (even though it's a symlink): {err}",
                            filename.display()
                        );
                        None
                    }
                }
            } else {
                None
            };

            Ok(make_fsentry(filename, &attrs, symlink))
        })
    }

    fn symlink(&self, path: &Path, target: &Path) -> RemoteResult<()> {
        let path_str = path.to_string_lossy().to_string();
        let target_str = target.to_string_lossy().to_string();
        self.runtime.block_on(async {
            self.session
                .symlink(&path_str, &target_str)
                .await
                .map_err(|err| {
                    RemoteError::new_ex(
                        RemoteErrorType::FileCreateDenied,
                        format!("Could not create symlink '{}': {err}", path.display()),
                    )
                })
        })
    }

    fn unlink(&self, path: &Path) -> RemoteResult<()> {
        let path_str = path.to_string_lossy().to_string();
        self.runtime.block_on(async {
            self.session.remove_file(&path_str).await.map_err(|err| {
                RemoteError::new_ex(
                    RemoteErrorType::CouldNotRemoveFile,
                    format!("Could not remove file '{}': {err}", path.display()),
                )
            })
        })
    }
}

/// Convert `remotefs::fs::Metadata` to `russh_sftp::protocol::FileAttributes`.
fn metadata_to_file_attributes(metadata: Metadata) -> russh_sftp::protocol::FileAttributes {
    let atime = metadata
        .accessed
        .and_then(|x| x.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|x| x.as_secs() as u32);
    let mtime = metadata
        .modified
        .and_then(|x| x.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|x| x.as_secs() as u32);
    russh_sftp::protocol::FileAttributes {
        size: Some(metadata.size),
        uid: metadata.uid,
        user: None,
        gid: metadata.gid,
        group: None,
        permissions: metadata.mode.map(u32::from),
        atime,
        mtime,
    }
}

/// Build a `remotefs::File` from a path and russh-sftp `FileAttributes`.
fn make_fsentry(
    path: &Path,
    attrs: &russh_sftp::protocol::FileAttributes,
    symlink: Option<PathBuf>,
) -> File {
    let name = match path.file_name() {
        None => "/".to_string(),
        Some(name) => name.to_string_lossy().to_string(),
    };
    debug!("Found file {name}");

    let uid = attrs.uid;
    let gid = attrs.gid;
    let mode = attrs.permissions.map(remotefs::fs::UnixPex::from);
    let size = attrs.size.unwrap_or(0);
    let accessed = attrs.atime.map(|x| {
        std::time::UNIX_EPOCH
            .checked_add(std::time::Duration::from_secs(u64::from(x)))
            .unwrap_or(std::time::UNIX_EPOCH)
    });
    let modified = attrs.mtime.map(|x| {
        std::time::UNIX_EPOCH
            .checked_add(std::time::Duration::from_secs(u64::from(x)))
            .unwrap_or(std::time::UNIX_EPOCH)
    });

    let file_type = if symlink.is_some() {
        remotefs::fs::FileType::Symlink
    } else if attrs.is_dir() {
        remotefs::fs::FileType::Directory
    } else {
        remotefs::fs::FileType::File
    };

    let entry_metadata = Metadata {
        accessed,
        created: None,
        file_type,
        gid,
        mode,
        modified,
        size,
        symlink,
        uid,
    };
    trace!("Metadata for {}: {:?}", path.display(), entry_metadata);
    File {
        path: path.to_path_buf(),
        metadata: entry_metadata,
    }
}

/// Synchronous writer wrapping a russh-sftp [`russh_sftp::client::fs::File`].
///
/// Stores the full `Arc<Runtime>` rather than just a `Handle` so that
/// `Runtime::block_on` drives IO and background tasks on a current-thread
/// runtime.
struct SftpFileWriter {
    file: russh_sftp::client::fs::File,
    runtime: Arc<Runtime>,
}

impl Write for SftpFileWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        use tokio::io::AsyncWriteExt as _;
        self.runtime.block_on(self.file.write(buf))
    }

    fn flush(&mut self) -> std::io::Result<()> {
        use tokio::io::AsyncWriteExt as _;
        self.runtime.block_on(self.file.flush())
    }
}

impl Seek for SftpFileWriter {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        use tokio::io::AsyncSeekExt as _;
        self.runtime.block_on(self.file.seek(pos))
    }
}

impl remotefs::fs::stream::WriteAndSeek for SftpFileWriter {}

impl Drop for SftpFileWriter {
    fn drop(&mut self) {
        use tokio::io::AsyncWriteExt as _;
        // Close the handle with an awaited close. russh-sftp's `File::drop`
        // uses `close_nowait`, which never decrements the client's open-handle
        // counter and would leak a handle per upload until the negotiated limit
        // is reached ("Handle limit reached").
        let _ = self.runtime.block_on(self.file.shutdown());
    }
}

/// Number of concurrent SFTP file handles used per batch in pipelined reads.
const SFTP_PIPELINE_DEPTH: usize = 4;

/// Size of each chunk read by a single pipeline task (4 MiB).
const SFTP_CHUNK_SIZE: usize = 4 * 1024 * 1024;

/// Maximum number of completed batches to buffer ahead of the current read
/// position. Caps memory usage to roughly `(MAX_PREFETCH + 1) * BATCH_SIZE`.
const MAX_PREFETCH: usize = 2;

/// Batch size: [`SFTP_PIPELINE_DEPTH`] * [`SFTP_CHUNK_SIZE`] = 16 MiB.
const BATCH_SIZE: usize = SFTP_PIPELINE_DEPTH * SFTP_CHUNK_SIZE;

/// A streaming SFTP reader that pipelines reads in batches.
///
/// Each batch spawns [`SFTP_PIPELINE_DEPTH`] concurrent SFTP read tasks of
/// [`SFTP_CHUNK_SIZE`] bytes. Up to [`MAX_PREFETCH`] batches are fetched ahead
/// of the current read position so the caller receives data immediately while
/// keeping memory bounded.
struct PipelinedSftpReader {
    runtime: Arc<Runtime>,
    session: Arc<SftpSession>,
    path: String,
    file_size: usize,
    /// Next byte offset to start fetching from the remote file.
    fetch_offset: usize,
    /// Completed batches ready for consumption, front = current.
    batches: std::collections::VecDeque<Vec<u8>>,
    /// Read cursor within `batches[0]`.
    buf_cursor: usize,
    /// Background pre-fetch task, if any.
    pending: Option<PrefetchTask>,
}

/// In-flight background batch fetch.
struct PrefetchTask {
    /// The byte offset this batch starts at — used to roll back
    /// `fetch_offset` on failure.
    batch_offset: usize,
    handle: tokio::task::JoinHandle<Result<Vec<u8>, std::io::Error>>,
}

impl PipelinedSftpReader {
    /// Creates a new streaming reader.
    ///
    /// Eagerly fetches the first batch and starts a background pre-fetch for
    /// the second batch so the caller can start reading immediately.
    fn new(
        runtime: Arc<Runtime>,
        session: Arc<SftpSession>,
        path: String,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let metadata = runtime.block_on(session.metadata(&path))?;
        let file_size = metadata.size.unwrap_or(0) as usize;

        let mut reader = Self {
            runtime,
            session,
            path,
            file_size,
            fetch_offset: 0,
            batches: std::collections::VecDeque::new(),
            buf_cursor: 0,
            pending: None,
        };

        if file_size == 0 {
            return Ok(reader);
        }

        // Eagerly fetch the first batch so data is available immediately.
        let first_batch = reader.fetch_batch_blocking()?;
        reader.batches.push_back(first_batch);

        // Start background pre-fetch for the next batch.
        reader.maybe_start_prefetch();

        Ok(reader)
    }

    /// Fetches the next batch synchronously by blocking on the runtime.
    fn fetch_batch_blocking(
        &mut self,
    ) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
        let remaining = self.file_size.saturating_sub(self.fetch_offset);
        if remaining == 0 {
            return Ok(Vec::new());
        }

        let batch_len = remaining.min(BATCH_SIZE);
        let offset = self.fetch_offset;
        let batch = self
            .runtime
            .block_on(Self::fetch_batch(
                self.session.clone(),
                self.path.clone(),
                offset,
                batch_len,
            ))
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;

        self.fetch_offset += batch_len;
        Ok(batch)
    }

    /// Spawns a background batch fetch if there is more data and the prefetch
    /// queue is not full.
    fn maybe_start_prefetch(&mut self) {
        if self.pending.is_some() {
            return;
        }
        if self.batches.len() > MAX_PREFETCH {
            return;
        }
        let remaining = self.file_size.saturating_sub(self.fetch_offset);
        if remaining == 0 {
            return;
        }

        let batch_len = remaining.min(BATCH_SIZE);
        let session = self.session.clone();
        let path = self.path.clone();
        let offset = self.fetch_offset;
        // Speculatively advance; rolled back in collect_pending on failure.
        self.fetch_offset += batch_len;

        let handle = self
            .runtime
            .spawn(async move { Self::fetch_batch(session, path, offset, batch_len).await });

        self.pending = Some(PrefetchTask {
            batch_offset: offset,
            handle,
        });
    }

    /// Collects the result of a pending pre-fetch task.
    ///
    /// On failure, rolls back `fetch_offset` so the batch can be retried.
    fn collect_pending(&mut self) -> std::io::Result<Option<Vec<u8>>> {
        let task = match self.pending.take() {
            Some(t) => t,
            None => return Ok(None),
        };

        match self
            .runtime
            .block_on(task.handle)
            .map_err(std::io::Error::other)?
        {
            Ok(batch) if batch.is_empty() => Ok(None),
            Ok(batch) => Ok(Some(batch)),
            Err(err) => {
                // Roll back so the caller (or a retry) can re-fetch this range.
                self.fetch_offset = task.batch_offset;
                Err(std::io::Error::other(err))
            }
        }
    }

    /// Fetches a single batch: spawns [`SFTP_PIPELINE_DEPTH`] concurrent reads
    /// and assembles the result into a contiguous buffer.
    async fn fetch_batch(
        session: Arc<SftpSession>,
        path: String,
        batch_offset: usize,
        batch_len: usize,
    ) -> Result<Vec<u8>, std::io::Error> {
        use tokio::io::{AsyncReadExt as _, AsyncSeekExt as _, AsyncWriteExt as _};

        let chunk_count = batch_len.div_ceil(SFTP_CHUNK_SIZE);
        let mut tasks = Vec::with_capacity(chunk_count);

        for i in 0..chunk_count {
            let chunk_offset = i * SFTP_CHUNK_SIZE;
            let len = SFTP_CHUNK_SIZE.min(batch_len - chunk_offset);
            let abs_offset = batch_offset + chunk_offset;
            let session = Arc::clone(&session);
            let path = path.clone();

            tasks.push(tokio::spawn(async move {
                let mut file = session.open(&path).await.map_err(std::io::Error::other)?;
                file.seek(std::io::SeekFrom::Start(abs_offset as u64))
                    .await?;
                let mut buf = vec![0_u8; len];
                let read_res = file.read_exact(&mut buf).await;
                // Explicitly close the handle with an awaited close. russh-sftp's
                // `File::drop` uses `close_nowait`, which frees the handle
                // server-side but never decrements the client's open-handle
                // counter. Relying on it leaks handles until the negotiated
                // limit is hit ("Handle limit reached") after many opens.
                let _ = file.shutdown().await;
                read_res?;
                Ok::<(usize, Vec<u8>), std::io::Error>((chunk_offset, buf))
            }));
        }

        let mut result = vec![0_u8; batch_len];
        for task in tasks {
            let (chunk_offset, chunk) = task
                .await
                .map_err(std::io::Error::other)?
                .map_err(std::io::Error::other)?;
            result[chunk_offset..chunk_offset + chunk.len()].copy_from_slice(&chunk);
        }

        Ok(result)
    }
}

impl Read for PipelinedSftpReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        loop {
            // Try to serve from the current front batch.
            if let Some(front) = self.batches.front() {
                let available = &front[self.buf_cursor..];
                if !available.is_empty() {
                    let to_copy = available.len().min(buf.len());
                    buf[..to_copy].copy_from_slice(&available[..to_copy]);
                    self.buf_cursor += to_copy;
                    return Ok(to_copy);
                }

                // Current batch fully consumed — pop it.
                self.batches.pop_front();
                self.buf_cursor = 0;

                // Collect the pending pre-fetch if any.
                if let Some(batch) = self.collect_pending()? {
                    self.batches.push_back(batch);
                }

                // Kick off next pre-fetch.
                self.maybe_start_prefetch();

                continue;
            }

            // No batches buffered — try to collect pending.
            if let Some(batch) = self.collect_pending()? {
                self.batches.push_back(batch);
                self.maybe_start_prefetch();
                continue;
            }

            // Nothing left — EOF.
            return Ok(0);
        }
    }
}

/// Apply algorithm preferences from SSH config to the russh [`client::Config`].
fn apply_config_algo_prefs(config: &mut client::Config, ssh_config: &Config) {
    let params = &ssh_config.params;

    // KEX algorithms
    let kex: Vec<russh::kex::Name> = params
        .kex_algorithms
        .algorithms()
        .iter()
        .filter_map(|name| {
            russh::kex::Name::try_from(name.as_str())
                .map_err(|()| warn!("Unsupported KEX algorithm: {name}"))
                .ok()
        })
        .collect();
    if !kex.is_empty() {
        config.preferred.kex = Cow::Owned(kex);
    }

    // Host key algorithms
    let host_keys: Vec<Algorithm> = params
        .host_key_algorithms
        .algorithms()
        .iter()
        .filter_map(|name| {
            name.parse::<Algorithm>()
                .map_err(|err| warn!("Unsupported host key algorithm '{name}': {err}"))
                .ok()
        })
        .collect();
    if !host_keys.is_empty() {
        config.preferred.key = Cow::Owned(host_keys);
    }

    // Cipher algorithms
    let ciphers: Vec<russh::cipher::Name> = params
        .ciphers
        .algorithms()
        .iter()
        .filter_map(|name| {
            russh::cipher::Name::try_from(name.as_str())
                .map_err(|()| warn!("Unsupported cipher algorithm: {name}"))
                .ok()
        })
        .collect();
    if !ciphers.is_empty() {
        config.preferred.cipher = Cow::Owned(ciphers);
    }

    // MAC algorithms
    let macs: Vec<russh::mac::Name> = params
        .mac
        .algorithms()
        .iter()
        .filter_map(|name| {
            russh::mac::Name::try_from(name.as_str())
                .map_err(|()| warn!("Unsupported MAC algorithm: {name}"))
                .ok()
        })
        .collect();
    if !macs.is_empty() {
        config.preferred.mac = Cow::Owned(macs);
    }
}

/// Apply algorithm preferences from [`SshOpts`] methods to the russh [`client::Config`].
///
/// Options from `SshOpts::methods` override those from the SSH config file.
fn apply_opts_algo_prefs(config: &mut client::Config, opts: &SshOpts) {
    for method in opts.methods.iter() {
        let algos = method.prefs();
        let names: Vec<&str> = algos.split(',').collect();

        match method.method_type {
            MethodType::Kex => {
                let kex: Vec<russh::kex::Name> = names
                    .iter()
                    .filter_map(|name| {
                        russh::kex::Name::try_from(*name)
                            .map_err(|()| warn!("Unsupported KEX algorithm: {name}"))
                            .ok()
                    })
                    .collect();
                if !kex.is_empty() {
                    config.preferred.kex = Cow::Owned(kex);
                }
            }
            MethodType::HostKey => {
                let keys: Vec<Algorithm> = names
                    .iter()
                    .filter_map(|name| {
                        name.parse::<Algorithm>()
                            .map_err(|err| warn!("Unsupported host key algorithm '{name}': {err}"))
                            .ok()
                    })
                    .collect();
                if !keys.is_empty() {
                    config.preferred.key = Cow::Owned(keys);
                }
            }
            MethodType::CryptClientServer | MethodType::CryptServerClient => {
                let ciphers: Vec<russh::cipher::Name> = names
                    .iter()
                    .filter_map(|name| {
                        russh::cipher::Name::try_from(*name)
                            .map_err(|()| warn!("Unsupported cipher algorithm: {name}"))
                            .ok()
                    })
                    .collect();
                if !ciphers.is_empty() {
                    config.preferred.cipher = Cow::Owned(ciphers);
                }
            }
            MethodType::MacClientServer | MethodType::MacServerClient => {
                let macs: Vec<russh::mac::Name> = names
                    .iter()
                    .filter_map(|name| {
                        russh::mac::Name::try_from(*name)
                            .map_err(|()| warn!("Unsupported MAC algorithm: {name}"))
                            .ok()
                    })
                    .collect();
                if !macs.is_empty() {
                    config.preferred.mac = Cow::Owned(macs);
                }
            }
            _ => {
                trace!(
                    "Ignoring unsupported method type {:?} for russh backend",
                    method.method_type
                );
            }
        }
    }
}

/// Execute a shell command on the remote server via a russh channel.
///
/// Opens a session channel, executes the command, collects stdout,
/// and returns the exit code with the output.
async fn perform_shell_cmd<T>(session: &Handle<T>, cmd: &str) -> RemoteResult<(u32, String)>
where
    T: Handler,
{
    let mut channel = open_channel(session).await?;

    channel.exec(true, cmd).await.map_err(|err| {
        RemoteError::new_ex(
            RemoteErrorType::ProtocolError,
            format!("Could not execute command \"{cmd}\": {err}"),
        )
    })?;

    let mut output = String::new();
    let mut exit_code: Option<u32> = None;

    while let Some(msg) = channel.wait().await {
        match msg {
            russh::ChannelMsg::Data { data } => {
                output.push_str(&String::from_utf8_lossy(&data));
            }
            russh::ChannelMsg::ExitStatus { exit_status } => {
                exit_code = Some(exit_status);
            }
            russh::ChannelMsg::Close => break,
            russh::ChannelMsg::Eof => {}
            _ => {}
        }
    }

    let rc = exit_code.unwrap_or_else(|| {
        warn!("No exit status received for command \"{cmd}\", defaulting to 1");
        1
    });

    trace!("Command output: {output}");
    debug!(r#"Command output: "{output}"; exit code: {rc}"#);

    Ok((rc, output))
}

/// Open a session channel on the given handle.
async fn open_channel<T>(session: &Handle<T>) -> RemoteResult<russh::Channel<russh::client::Msg>>
where
    T: Handler,
{
    session.channel_open_session().await.map_err(|err| {
        RemoteError::new_ex(
            RemoteErrorType::ProtocolError,
            format!("Could not open channel: {err}"),
        )
    })
}

#[cfg(test)]
mod test {

    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use ssh2_config::ParseRule;
    use tempfile::NamedTempFile;

    use super::*;
    use crate::mock::ssh as ssh_mock;

    fn test_runtime() -> Arc<Runtime> {
        Arc::new(
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap(),
        )
    }

    #[test]
    fn should_connect_to_ssh_server_auth_user_password() {
        use crate::ssh::container::OpensshServer;

        let container = OpensshServer::start();
        let port = container.port();

        crate::mock::logger();
        let runtime = test_runtime();
        let config_file = ssh_mock::create_ssh_config(port);
        let opts = SshOpts::new("sftp")
            .config_file(config_file.path(), ParseRule::ALLOW_UNKNOWN_FIELDS)
            .password("password")
            .runtime(runtime);

        if let Err(err) = RusshSession::<NoCheckServerKey>::connect(&opts) {
            panic!("Could not connect to server: {err}");
        }
        let session = RusshSession::<NoCheckServerKey>::connect(&opts).unwrap();
        assert!(session.authenticated().unwrap());

        drop(container);
    }

    #[test]
    fn should_connect_to_ssh_server_auth_key() {
        use crate::ssh::container::OpensshServer;

        let container = OpensshServer::start();
        let port = container.port();

        crate::mock::logger();
        let runtime = test_runtime();
        let config_file = ssh_mock::create_ssh_config(port);
        let opts = SshOpts::new("sftp")
            .config_file(config_file.path(), ParseRule::ALLOW_UNKNOWN_FIELDS)
            .key_storage(Box::new(ssh_mock::MockSshKeyStorage::default()))
            .runtime(runtime);
        let session = RusshSession::<NoCheckServerKey>::connect(&opts).unwrap();
        assert!(session.authenticated().unwrap());
    }

    #[test]
    fn should_connect_to_ssh_server_auth_key_from_ssh_config() {
        use crate::ssh::container::OpensshServer;

        let container = OpensshServer::start();
        let port = container.port();

        crate::mock::logger();
        let runtime = test_runtime();
        // Authenticate purely via the `IdentityFile` directive of the ssh config,
        // with no key storage configured.
        let key_file = ssh_mock::create_key_file();
        let config_file = ssh_mock::create_ssh_config_with_identity(port, key_file.path());
        let opts = SshOpts::new("sftp")
            .config_file(config_file.path(), ParseRule::ALLOW_UNKNOWN_FIELDS)
            .runtime(runtime);
        let session = RusshSession::<NoCheckServerKey>::connect(&opts).unwrap();
        assert!(session.authenticated().unwrap());
    }

    #[test]
    #[cfg(unix)]
    fn should_connect_to_ssh_server_auth_ssh_agent() {
        use std::process::Command;

        use crate::SshAgentIdentity;
        use crate::ssh::container::OpensshServer;

        crate::mock::logger();

        // Spawn a dedicated ssh-agent and load the mock key into it.
        let agent_out = Command::new("ssh-agent")
            .arg("-s")
            .output()
            .expect("failed to spawn ssh-agent (is openssh installed?)");
        let agent_stdout = String::from_utf8_lossy(&agent_out.stdout);
        let auth_sock = parse_agent_var(&agent_stdout, "SSH_AUTH_SOCK")
            .expect("ssh-agent did not report SSH_AUTH_SOCK");
        let agent_pid = parse_agent_var(&agent_stdout, "SSH_AGENT_PID")
            .expect("ssh-agent did not report SSH_AGENT_PID");

        let key_file = ssh_mock::create_key_file();
        // ssh-add refuses keys with loose permissions.
        Command::new("chmod")
            .args(["600", &key_file.path().display().to_string()])
            .status()
            .expect("chmod failed");
        let added = Command::new("ssh-add")
            .arg(key_file.path())
            .env("SSH_AUTH_SOCK", &auth_sock)
            .status()
            .expect("ssh-add failed to run");
        assert!(added.success(), "ssh-add could not load the mock key");

        // Point the russh agent client at our agent. No key storage, no password:
        // authentication must succeed through the agent alone.
        // SAFETY: tests in this module run single-threaded (`--test-threads=1`).
        unsafe {
            std::env::set_var("SSH_AUTH_SOCK", &auth_sock);
        }

        let container = OpensshServer::start();
        let port = container.port();
        let runtime = test_runtime();
        let config_file = ssh_mock::create_ssh_config(port);
        let opts = SshOpts::new("sftp")
            .config_file(config_file.path(), ParseRule::ALLOW_UNKNOWN_FIELDS)
            .ssh_agent_identity(Some(SshAgentIdentity::All))
            .runtime(runtime);

        let result = RusshSession::<NoCheckServerKey>::connect(&opts);

        // Tear the agent down regardless of the outcome.
        // SAFETY: see above.
        unsafe {
            std::env::remove_var("SSH_AUTH_SOCK");
        }
        let _ = Command::new("kill").arg(&agent_pid).status();

        let session = result.expect("could not authenticate via ssh agent");
        assert!(session.authenticated().unwrap());
    }

    /// Parse a `NAME=value;` assignment from `ssh-agent -s` output.
    #[cfg(unix)]
    fn parse_agent_var(output: &str, name: &str) -> Option<String> {
        let needle = format!("{name}=");
        let start = output.find(&needle)? + needle.len();
        let rest = &output[start..];
        let end = rest.find(';')?;
        Some(rest[..end].to_string())
    }

    #[test]
    fn should_perform_shell_command_on_server() {
        crate::mock::logger();
        let container = crate::ssh::container::OpensshServer::start();
        let port = container.port();

        let runtime = test_runtime();
        let opts = SshOpts::new("127.0.0.1")
            .port(port)
            .username("sftp")
            .password("password")
            .runtime(runtime);
        let mut session = RusshSession::<NoCheckServerKey>::connect(&opts).unwrap();
        assert!(session.authenticated().unwrap());
        assert!(session.cmd("pwd").is_ok());
    }

    #[test]
    fn should_perform_shell_command_on_server_and_return_exit_code() {
        crate::mock::logger();
        let container = crate::ssh::container::OpensshServer::start();
        let port = container.port();

        let runtime = test_runtime();
        let opts = SshOpts::new("127.0.0.1")
            .port(port)
            .username("sftp")
            .password("password")
            .runtime(runtime);
        let mut session = RusshSession::<NoCheckServerKey>::connect(&opts).unwrap();
        assert!(session.authenticated().unwrap());
        assert_eq!(
            session.cmd_at("pwd", Path::new("/tmp")).ok().unwrap(),
            (0, String::from("/tmp\n"))
        );
        assert_eq!(
            session
                .cmd_at("pippopluto", Path::new("/tmp"))
                .ok()
                .unwrap()
                .0,
            127
        );
    }

    #[test]
    fn should_fail_authentication() {
        crate::mock::logger();
        let container = crate::ssh::container::OpensshServer::start();
        let port = container.port();

        let runtime = test_runtime();
        let opts = SshOpts::new("127.0.0.1")
            .port(port)
            .username("sftp")
            .password("ippopotamo")
            .runtime(runtime);
        assert!(RusshSession::<NoCheckServerKey>::connect(&opts).is_err());
    }

    #[test]
    fn should_apply_connection_timeout_to_handshake() {
        let (port, server) = ssh_mock::start_unresponsive_server(Duration::from_secs(2));
        let mut config_file = NamedTempFile::new().expect("failed to create SSH config");
        writeln!(
            config_file,
            "Host unresponsive\n    HostName 127.0.0.1\n    Port {port}\n    ConnectTimeout 5"
        )
        .expect("failed to write SSH config");
        let opts = SshOpts::new("unresponsive")
            .config_file(config_file.path(), ParseRule::STRICT)
            .connection_timeout(Duration::from_millis(100))
            .runtime(test_runtime());

        let started = Instant::now();
        let result = RusshSession::<NoCheckServerKey>::connect(&opts);
        let elapsed = started.elapsed();
        let server_elapsed = server.join().expect("test server panicked");

        assert!(result.is_err());
        assert!(
            elapsed < Duration::from_secs(1),
            "connection exceeded explicit timeout: {elapsed:?}"
        );
        assert!(
            server_elapsed < Duration::from_secs(1),
            "connection remained open after timeout: {server_elapsed:?}"
        );
    }

    #[test]
    fn should_retry_connection_using_configured_attempts() {
        let container = crate::ssh::container::OpensshServer::start();
        let (port, _proxy) = ssh_mock::start_flaky_proxy(container.port(), 1);
        let mut config_file = NamedTempFile::new().expect("failed to create SSH config");
        writeln!(
            config_file,
            "Host flaky\n    HostName 127.0.0.1\n    Port {port}\n    User sftp\n    ConnectionAttempts 2"
        )
        .expect("failed to write SSH config");
        let opts = SshOpts::new("flaky")
            .config_file(config_file.path(), ParseRule::STRICT)
            .password("password")
            .runtime(test_runtime());

        let session = RusshSession::<NoCheckServerKey>::connect(&opts)
            .expect("connection should succeed on the configured retry");
        session.disconnect().expect("failed to disconnect");
        drop(session);
    }

    #[test]
    fn should_apply_configured_bind_address() {
        let container = crate::ssh::container::OpensshServer::start();
        let port = container.port();
        let mut valid_config = NamedTempFile::new().expect("failed to create SSH config");
        writeln!(
            valid_config,
            "Host bound\n    HostName 127.0.0.1\n    Port {port}\n    User sftp\n    BindAddress 127.0.0.1"
        )
        .expect("failed to write SSH config");
        let opts = SshOpts::new("bound")
            .config_file(valid_config.path(), ParseRule::STRICT)
            .password("password")
            .runtime(test_runtime());
        let session = RusshSession::<NoCheckServerKey>::connect(&opts)
            .expect("failed to connect with an available bind address");
        session.disconnect().expect("failed to disconnect");

        let mut invalid_config = NamedTempFile::new().expect("failed to create SSH config");
        writeln!(
            invalid_config,
            "Host bound\n    HostName 127.0.0.1\n    Port {port}\n    User sftp\n    BindAddress 192.0.2.1"
        )
        .expect("failed to write SSH config");
        let opts = SshOpts::new("bound")
            .config_file(invalid_config.path(), ParseRule::STRICT)
            .password("password")
            .runtime(test_runtime());
        assert!(RusshSession::<NoCheckServerKey>::connect(&opts).is_err());
    }

    #[test]
    fn test_filetransfer_sftp_bad_server() {
        crate::mock::logger();
        let runtime = test_runtime();
        let opts = SshOpts::new("myverybad.verybad.server")
            .port(10022)
            .username("sftp")
            .password("ippopotamo")
            .runtime(runtime);
        assert!(RusshSession::<NoCheckServerKey>::connect(&opts).is_err());
    }
}
