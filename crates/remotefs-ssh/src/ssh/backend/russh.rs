//! [russh](https://docs.rs/russh/latest/russh/) backend for `remotefs-ssh`.

mod auth;
mod scp;

use std::borrow::Cow;
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use remotefs::fs::{Metadata, ReadStream, WriteStream};
use remotefs::{File, RemoteError, RemoteErrorType, RemoteResult};
use russh::client::{Handle, Handler};
use russh::keys::{Algorithm, PublicKey};
use russh::{Disconnect, client};
use russh_sftp::client::SftpSession;
use tokio::runtime::Runtime;

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
        _server_public_key: &PublicKey,
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
    session: SftpSession,
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

        let mut config = client::Config {
            inactivity_timeout: Some(ssh_config.connection_timeout),
            ..Default::default()
        };

        // Apply algorithm preferences from ssh config
        apply_config_algo_prefs(&mut config, &ssh_config);

        // Apply algorithm preferences from opts
        apply_opts_algo_prefs(&mut config, opts);

        let config = Arc::new(config);

        let mut session = runtime
            .block_on(async {
                client::connect(config, ssh_config.address.as_str(), T::default()).await
            })
            .map_err(|err| {
                let msg = format!("SSH connection failed: {err:?}");
                error!("{msg}");
                RemoteError::new_ex(RemoteErrorType::ConnectionError, msg)
            })?;

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
                session,
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
        self.runtime.block_on(async {
            let data = pipelined_sftp_read(&self.session, &path_str)
                .await
                .map_err(|err| {
                    RemoteError::new_ex(
                        RemoteErrorType::ProtocolError,
                        format!("Could not read file at '{}': {err}", path.display()),
                    )
                })?;
            Ok(ReadStream::from(
                Box::new(std::io::Cursor::new(data)) as Box<dyn Read + Send>
            ))
        })
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

/// Number of concurrent SFTP file handles used for pipelined reads.
const SFTP_READ_PIPELINE_DEPTH: usize = 4;

/// Read a remote file using multiple concurrent SFTP file handles to pipeline
/// reads.
///
/// Each task owns its chunk buffer and the final result is assembled only after
/// all tasks complete, avoiding shared mutable state across tasks.
async fn pipelined_sftp_read(
    session: &russh_sftp::client::SftpSession,
    path: &str,
) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
    use tokio::io::{AsyncReadExt as _, AsyncSeekExt as _};

    let metadata = session.metadata(path).await?;
    let file_size = metadata.size.unwrap_or(0) as usize;

    if file_size == 0 {
        return Ok(Vec::new());
    }

    let chunk_size = file_size.div_ceil(SFTP_READ_PIPELINE_DEPTH);
    let mut tasks = Vec::with_capacity(SFTP_READ_PIPELINE_DEPTH);

    for i in 0..SFTP_READ_PIPELINE_DEPTH {
        let offset = i * chunk_size;
        if offset >= file_size {
            break;
        }
        let len = chunk_size.min(file_size - offset);
        let mut file = session.open(path).await.map_err(std::io::Error::other)?;
        file.seek(std::io::SeekFrom::Start(offset as u64)).await?;

        tasks.push(tokio::spawn(async move {
            let mut buf = vec![0_u8; len];
            file.read_exact(&mut buf).await?;
            Ok::<(usize, Vec<u8>), std::io::Error>((offset, buf))
        }));
    }

    let mut result = vec![0_u8; file_size];
    let mut first_err: Option<std::io::Error> = None;

    for task in tasks {
        match task.await {
            Ok(Ok((offset, chunk))) => {
                result[offset..offset + chunk.len()].copy_from_slice(&chunk);
            }
            Ok(Err(err)) => {
                if first_err.is_none() {
                    first_err = Some(err);
                }
            }
            Err(err) => {
                if first_err.is_none() {
                    first_err = Some(std::io::Error::other(err));
                }
            }
        }
    }

    if let Some(err) = first_err {
        return Err(Box::new(err));
    }

    Ok(result)
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

    use ssh2_config::ParseRule;

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
