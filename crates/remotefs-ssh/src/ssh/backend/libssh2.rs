use std::io::{Read, Seek, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs as _};
use std::path::{Path, PathBuf};
use std::str::FromStr as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};

use remotefs::fs::stream::{ReadAndSeek, WriteAndSeek};
use remotefs::fs::{FileType, Metadata, ReadStream, UnixPex, WriteStream};
use remotefs::{File, RemoteError, RemoteErrorType, RemoteResult};
use socket2::{Domain, Protocol, Socket, Type};
use ssh2::{FileStat, OpenType, RenameFlags};

use super::{SshSession, interface, socket};
use crate::ssh::backend::Sftp;
use crate::ssh::config::Config;
use crate::{SshAgentIdentity, SshOpts};

/// An implementation of [`SshSession`] using libssh2 as the backend.
pub struct LibSsh2Session {
    session: ssh2::Session,
}

/// A wrapper around [`ssh2::Sftp`] to provide a SFTP client for [`LibSsh2Session`]
pub struct LibSsh2Sftp {
    inner: ssh2::Sftp,
}

/// Authentication method
#[derive(Debug, Clone, PartialEq, Eq)]
enum Authentication {
    RsaKey {
        private_key: PathBuf,
        certificate: Option<PathBuf>,
    },
    Password(String),
}

impl SshSession for LibSsh2Session {
    type Sftp = LibSsh2Sftp;

    fn connect(opts: &SshOpts) -> RemoteResult<Self> {
        let ssh_config = Config::try_from(opts)?;
        debug!("Connecting to '{}'", ssh_config.address);
        let mut session = connect_libssh2_transport(opts, &ssh_config)?;
        authenticate_libssh2_session(&mut session, opts, &ssh_config)?;
        Ok(Self { session })
    }

    fn disconnect(&self) -> RemoteResult<()> {
        self.session
            .disconnect(None, "Mandi!", None)
            .map_err(|err| RemoteError::new_ex(RemoteErrorType::ConnectionError, err))
    }

    fn authenticated(&self) -> RemoteResult<bool> {
        Ok(self.session.authenticated())
    }

    fn banner(&self) -> RemoteResult<Option<String>> {
        Ok(self.session.banner().map(String::from))
    }

    fn cmd<S>(&mut self, cmd: S) -> RemoteResult<(u32, String)>
    where
        S: AsRef<str>,
    {
        let output = perform_shell_cmd(&mut self.session, format!("{}; echo $?", cmd.as_ref()))?;
        if let Some(index) = output.trim().rfind('\n') {
            trace!("Read from stdout: '{output}'");
            let actual_output = (output[0..index + 1]).to_string();
            trace!("Actual output '{actual_output}'");
            trace!("Parsing return code '{}'", output[index..].trim());
            let rc = match u32::from_str(output[index..].trim()).ok() {
                Some(val) => val,
                None => {
                    return Err(RemoteError::new_ex(
                        RemoteErrorType::ProtocolError,
                        "Failed to get command exit code",
                    ));
                }
            };
            debug!(r#"Command output: "{actual_output}"; exit code: {rc}"#);
            Ok((rc, actual_output))
        } else {
            match u32::from_str(output.trim()).ok() {
                Some(val) => Ok((val, String::new())),
                None => Err(RemoteError::new_ex(
                    RemoteErrorType::ProtocolError,
                    "Failed to get command exit code",
                )),
            }
        }
    }

    fn scp_recv(&self, path: &Path) -> RemoteResult<Box<dyn Read + Send>> {
        self.session.set_blocking(true);

        self.session
            .scp_recv(path)
            .map(|(reader, _stat)| Box::new(reader) as Box<dyn Read + Send>)
            .map_err(|err| {
                RemoteError::new_ex(
                    RemoteErrorType::ProtocolError,
                    format!("Could not receive file over SCP: {err}"),
                )
            })
    }

    fn scp_send(
        &self,
        remote_path: &Path,
        mode: i32,
        size: u64,
        times: Option<(u64, u64)>,
    ) -> RemoteResult<Box<dyn Write + Send>> {
        self.session.set_blocking(true);

        self.session
            .scp_send(remote_path, mode, size, times)
            .map(|writer| Box::new(writer) as Box<dyn Write + Send>)
            .map_err(|err| {
                RemoteError::new_ex(
                    RemoteErrorType::ProtocolError,
                    format!("Could not send file over SCP: {err}"),
                )
            })
    }

    fn sftp(&self) -> RemoteResult<Self::Sftp> {
        self.session.set_blocking(true);

        Ok(LibSsh2Sftp {
            inner: self.session.sftp().map_err(|err| {
                RemoteError::new_ex(
                    RemoteErrorType::ProtocolError,
                    format!("Could not create SFTP session: {err}"),
                )
            })?,
        })
    }
}

fn connect_libssh2_transport(opts: &SshOpts, destination: &Config) -> RemoteResult<ssh2::Session> {
    let proxy_jumps = destination.proxy_jump_configs(opts)?;
    let Some(first_jump) = proxy_jumps.first() else {
        return connect_libssh2_direct(opts, destination);
    };

    let mut session = connect_libssh2_direct(opts, first_jump)?;
    authenticate_libssh2_session(&mut session, opts, first_jump)?;
    for next_hop in proxy_jumps
        .iter()
        .skip(1)
        .chain(std::iter::once(destination))
    {
        session = connect_libssh2_through_jump(opts, &session, next_hop)?;
        if !std::ptr::eq(next_hop, destination) {
            authenticate_libssh2_session(&mut session, opts, next_hop)?;
        }
    }
    Ok(session)
}

fn connect_libssh2_direct(opts: &SshOpts, config: &Config) -> RemoteResult<ssh2::Session> {
    let socket_addresses: Vec<SocketAddr> = config
        .address
        .to_socket_addrs()
        .map_err(|err| RemoteError::new_ex(RemoteErrorType::BadAddress, err))?
        .collect();
    let mut stream = None;
    let mut last_connection_error = None;
    for _ in 0..config.connection_attempts.max(1) {
        for socket_addr in &socket_addresses {
            trace!(
                "Trying to connect to socket address '{}' (timeout: {}s)",
                socket_addr,
                config.connection_timeout.as_secs()
            );
            match tcp_connect(
                socket_addr,
                config.connection_timeout,
                config.params.bind_address.as_deref(),
                config.params.bind_interface.as_deref(),
            ) {
                Ok(tcp_stream) => {
                    stream = Some(tcp_stream);
                    break;
                }
                Err(err) => last_connection_error = Some(err),
            }
        }
        if stream.is_some() {
            break;
        }
    }
    let stream = stream.ok_or_else(|| {
        let message = last_connection_error.map_or_else(
            || "No suitable socket address found; connection timeout".to_string(),
            |err| err.to_string(),
        );
        RemoteError::new_ex(RemoteErrorType::ConnectionError, message)
    })?;
    socket::set_keepalive(&stream, config.params.tcp_keep_alive.unwrap_or(true))
        .map_err(|err| RemoteError::new_ex(RemoteErrorType::ConnectionError, err))?;
    connect_libssh2_stream(opts, config, stream)
}

fn connect_libssh2_through_jump(
    opts: &SshOpts,
    jump_session: &ssh2::Session,
    target: &Config,
) -> RemoteResult<ssh2::Session> {
    let mut last_error = None;
    for attempt in 1..=target.connection_attempts.max(1) {
        let (stream, cancelled, relay) = match libssh2_tunnel_stream(
            jump_session,
            &target.resolved_host,
            target.port,
            target.connection_timeout,
        ) {
            Ok(tunnel) => tunnel,
            Err(err) if attempt < target.connection_attempts.max(1) => {
                warn!("SSH connection attempt {attempt} through ProxyJump failed: {err}");
                last_error = Some(err);
                continue;
            }
            Err(err) => return Err(err),
        };
        let result = connect_libssh2_stream(opts, target, stream);
        match result {
            Ok(session) => {
                drop(relay);
                return Ok(session);
            }
            Err(err) if attempt < target.connection_attempts.max(1) => {
                cancelled.store(true, Ordering::Release);
                let _ = relay.join();
                warn!("SSH connection attempt {attempt} through ProxyJump failed: {err}");
                last_error = Some(err);
            }
            Err(err) => {
                cancelled.store(true, Ordering::Release);
                let _ = relay.join();
                return Err(err);
            }
        }
    }
    Err(last_error.expect("at least one connection attempt"))
}

fn connect_libssh2_stream(
    opts: &SshOpts,
    config: &Config,
    stream: TcpStream,
) -> RemoteResult<ssh2::Session> {
    let mut session = ssh2::Session::new()
        .map_err(|err| RemoteError::new_ex(RemoteErrorType::ConnectionError, err))?;
    session.set_timeout(config.connection_timeout.as_millis().min(u32::MAX.into()) as u32);
    session.set_tcp_stream(stream);
    set_algo_prefs(&mut session, opts, config)?;
    session.handshake().map_err(|err| {
        error!("SSH handshake failed: {err}");
        RemoteError::new_ex(RemoteErrorType::ProtocolError, err)
    })?;
    Ok(session)
}

fn authenticate_libssh2_session(
    session: &mut ssh2::Session,
    opts: &SshOpts,
    config: &Config,
) -> RemoteResult<()> {
    let pubkey_authentication = config.params.pubkey_authentication.unwrap_or(true);
    if pubkey_authentication && let Some(agent_identity) = &opts.ssh_agent_identity {
        match session_auth_with_agent(
            session,
            &config.username,
            agent_identity,
            config.params.pubkey_accepted_algorithms.algorithms(),
        ) {
            Ok(()) => return Ok(()),
            Err(err) => error!("Could not authenticate with ssh agent: {err}"),
        }
    }

    let mut methods = Vec::new();
    if pubkey_authentication {
        if let Some(private_key) = opts.key_storage.as_ref().and_then(|storage| {
            storage
                .resolve(&config.host, &config.username)
                .or_else(|| storage.resolve(&config.resolved_host, &config.username))
        }) {
            methods.push(Authentication::RsaKey {
                private_key,
                certificate: config.params.certificate_file.clone(),
            });
        }
        if let Some(identity_files) = config.params.identity_file.as_deref() {
            methods.extend(identity_files.iter().cloned().map(|private_key| {
                Authentication::RsaKey {
                    private_key,
                    certificate: config.params.certificate_file.clone(),
                }
            }));
        }
    }
    if let Some(password) = opts.password.as_ref() {
        methods.push(Authentication::Password(password.clone()));
    }

    let mut last_error = None;
    for method in methods {
        match session_auth(session, opts, config, method) {
            Ok(()) => return Ok(()),
            Err(err) => last_error = Some(err),
        }
    }
    Err(last_error.unwrap_or_else(|| {
        RemoteError::new_ex(
            RemoteErrorType::AuthenticationFailed,
            "no authentication method provided",
        )
    }))
}

fn libssh2_tunnel_stream(
    session: &ssh2::Session,
    target_host: &str,
    target_port: u16,
    timeout: Duration,
) -> RemoteResult<(TcpStream, Arc<AtomicBool>, std::thread::JoinHandle<()>)> {
    session.set_blocking(true);
    session.set_timeout(timeout.as_millis().min(u32::MAX.into()) as u32);
    let channel = session.channel_direct_tcpip(target_host, target_port, None);
    session.set_blocking(false);
    let channel =
        channel.map_err(|err| RemoteError::new_ex(RemoteErrorType::ConnectionError, err))?;
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .map_err(|err| RemoteError::new_ex(RemoteErrorType::ConnectionError, err))?;
    let client = TcpStream::connect(
        listener
            .local_addr()
            .map_err(|err| RemoteError::new_ex(RemoteErrorType::ConnectionError, err))?,
    )
    .map_err(|err| RemoteError::new_ex(RemoteErrorType::ConnectionError, err))?;
    let (relay, _) = listener
        .accept()
        .map_err(|err| RemoteError::new_ex(RemoteErrorType::ConnectionError, err))?;
    let cancelled = Arc::new(AtomicBool::new(false));
    let relay_cancelled = Arc::clone(&cancelled);
    let relay = std::thread::spawn(move || {
        relay_libssh2_channel(channel, relay, &relay_cancelled);
    });
    Ok((client, cancelled, relay))
}

fn relay_libssh2_channel(
    mut channel: ssh2::Channel,
    mut socket: TcpStream,
    cancelled: &AtomicBool,
) {
    let _ = socket.set_nonblocking(true);
    let mut socket_eof = false;
    let mut channel_eof = false;
    let mut channel_eof_sent = false;
    let mut to_channel = Vec::new();
    let mut to_socket = Vec::new();
    let mut buffer = [0_u8; 32 * 1024];

    while !cancelled.load(Ordering::Acquire) {
        let mut progressed = false;
        if !socket_eof && to_channel.is_empty() {
            match socket.read(&mut buffer) {
                Ok(0) => socket_eof = true,
                Ok(read) => {
                    to_channel.extend_from_slice(&buffer[..read]);
                    progressed = true;
                }
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(_) => break,
            }
        }
        if !to_channel.is_empty() {
            match channel.write(&to_channel) {
                Ok(written) => {
                    to_channel.drain(..written);
                    progressed = written > 0;
                }
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(_) => break,
            }
        }
        if socket_eof && to_channel.is_empty() && !channel_eof_sent {
            channel_eof_sent = channel.send_eof().is_ok();
        }

        if !channel_eof && to_socket.is_empty() {
            match channel.read(&mut buffer) {
                Ok(0) => channel_eof = true,
                Ok(read) => {
                    to_socket.extend_from_slice(&buffer[..read]);
                    progressed = true;
                }
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(_) => break,
            }
        }
        if !to_socket.is_empty() {
            match socket.write(&to_socket) {
                Ok(written) => {
                    to_socket.drain(..written);
                    progressed = written > 0;
                }
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(_) => break,
            }
        }
        if channel_eof && to_socket.is_empty() {
            let _ = socket.shutdown(Shutdown::Write);
            if socket_eof {
                break;
            }
        }
        if !progressed {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

struct SftpFileReader(ssh2::File);

struct SftpFileWriter(ssh2::File);

impl Write for SftpFileWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.0.flush()
    }
}

impl Seek for SftpFileWriter {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        self.0.seek(pos)
    }
}

impl WriteAndSeek for SftpFileWriter {}

impl Read for SftpFileReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.0.read(buf)
    }
}

impl Seek for SftpFileReader {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        self.0.seek(pos)
    }
}

impl ReadAndSeek for SftpFileReader {}

impl Sftp for LibSsh2Sftp {
    fn mkdir(&self, path: &Path, mode: i32) -> RemoteResult<()> {
        self.inner.mkdir(path, mode).map_err(|err| {
            RemoteError::new_ex(
                RemoteErrorType::FileCreateDenied,
                format!(
                    "Could not create directory '{path}': {err}",
                    path = path.display()
                ),
            )
        })
    }

    fn open_read(&self, path: &Path) -> RemoteResult<ReadStream> {
        self.inner
            .open(path)
            .map(|file| ReadStream::from(Box::new(SftpFileReader(file)) as Box<dyn ReadAndSeek>))
            .map_err(|err| {
                RemoteError::new_ex(
                    RemoteErrorType::ProtocolError,
                    format!(
                        "Could not open file at '{path}': {err}",
                        path = path.display()
                    ),
                )
            })
    }

    fn open_write(
        &self,
        path: &Path,
        flags: super::WriteMode,
        mode: i32,
    ) -> RemoteResult<WriteStream> {
        let flags = match flags {
            super::WriteMode::Append => {
                ssh2::OpenFlags::WRITE | ssh2::OpenFlags::APPEND | ssh2::OpenFlags::CREATE
            }
            super::WriteMode::Truncate => {
                ssh2::OpenFlags::WRITE | ssh2::OpenFlags::CREATE | ssh2::OpenFlags::TRUNCATE
            }
        };

        self.inner
            .open_mode(path, flags, mode, OpenType::File)
            .map(|file| WriteStream::from(Box::new(SftpFileWriter(file)) as Box<dyn WriteAndSeek>))
            .map_err(|err| {
                RemoteError::new_ex(
                    RemoteErrorType::ProtocolError,
                    format!(
                        "Could not open file at '{path}': {err}",
                        path = path.display()
                    ),
                )
            })
    }

    fn readdir<T>(&self, dirname: T) -> RemoteResult<Vec<remotefs::File>>
    where
        T: AsRef<Path>,
    {
        self.inner
            .readdir(dirname)
            .map(|files| {
                files
                    .into_iter()
                    .map(|(path, metadata)| self.make_fsentry(path.as_path(), &metadata))
                    .collect()
            })
            .map_err(|err| {
                RemoteError::new_ex(
                    RemoteErrorType::ProtocolError,
                    format!("Could not read directory: {err}",),
                )
            })
    }

    fn realpath(&self, path: &Path) -> RemoteResult<PathBuf> {
        self.inner.realpath(path).map_err(|err| {
            RemoteError::new_ex(
                RemoteErrorType::ProtocolError,
                format!(
                    "Could not resolve real path for '{path}': {err}",
                    path = path.display()
                ),
            )
        })
    }

    fn rename(&self, src: &Path, dest: &Path) -> RemoteResult<()> {
        self.inner
            .rename(src, dest, Some(RenameFlags::OVERWRITE))
            .map_err(|err| {
                RemoteError::new_ex(
                    RemoteErrorType::ProtocolError,
                    format!("Could not rename file '{src}': {err}", src = src.display()),
                )
            })
    }

    fn rmdir(&self, path: &Path) -> RemoteResult<()> {
        self.inner.rmdir(path).map_err(|err| {
            RemoteError::new_ex(
                RemoteErrorType::CouldNotRemoveFile,
                format!(
                    "Could not remove directory '{path}': {err}",
                    path = path.display()
                ),
            )
        })
    }

    fn setstat(&self, path: &Path, metadata: Metadata) -> RemoteResult<()> {
        self.inner
            .setstat(path, Self::metadata_to_filestat(metadata))
            .map_err(|err| {
                RemoteError::new_ex(
                    RemoteErrorType::ProtocolError,
                    format!(
                        "Could not set file attributes for '{path}': {err}",
                        path = path.display()
                    ),
                )
            })
    }

    fn stat(&self, filename: &Path) -> RemoteResult<File> {
        self.inner
            .stat(filename)
            .map(|metadata| self.make_fsentry(filename, &metadata))
            .map_err(|err| {
                RemoteError::new_ex(
                    RemoteErrorType::ProtocolError,
                    format!(
                        "Could not get file attributes for '{filename}': {err}",
                        filename = filename.display()
                    ),
                )
            })
    }

    fn symlink(&self, path: &Path, target: &Path) -> RemoteResult<()> {
        self.inner.symlink(path, target).map_err(|err| {
            RemoteError::new_ex(
                RemoteErrorType::FileCreateDenied,
                format!(
                    "Could not create symlink '{path}': {err}",
                    path = path.display()
                ),
            )
        })
    }

    fn unlink(&self, path: &Path) -> RemoteResult<()> {
        self.inner.unlink(path).map_err(|err| {
            RemoteError::new_ex(
                RemoteErrorType::CouldNotRemoveFile,
                format!(
                    "Could not remove file '{path}': {err}",
                    path = path.display()
                ),
            )
        })
    }
}

impl LibSsh2Sftp {
    fn metadata_to_filestat(metadata: Metadata) -> FileStat {
        let atime = metadata
            .accessed
            .and_then(|x| x.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map(|x| x.as_secs());
        let mtime = metadata
            .modified
            .and_then(|x| x.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map(|x| x.as_secs());
        FileStat {
            size: Some(metadata.size),
            uid: metadata.uid,
            gid: metadata.gid,
            perm: metadata.mode.map(u32::from),
            atime,
            mtime,
        }
    }

    fn make_fsentry(&self, path: &Path, metadata: &FileStat) -> File {
        let name = match path.file_name() {
            None => "/".to_string(),
            Some(name) => name.to_string_lossy().to_string(),
        };
        debug!("Found file {name}");
        // parse metadata
        let uid = metadata.uid;
        let gid = metadata.gid;
        let mode = metadata.perm.map(UnixPex::from);
        let size = metadata.size.unwrap_or(0);
        let accessed = metadata.atime.map(|x| {
            SystemTime::UNIX_EPOCH
                .checked_add(Duration::from_secs(x))
                .unwrap_or(SystemTime::UNIX_EPOCH)
        });
        let modified = metadata.mtime.map(|x| {
            SystemTime::UNIX_EPOCH
                .checked_add(Duration::from_secs(x))
                .unwrap_or(SystemTime::UNIX_EPOCH)
        });
        let symlink = match metadata.file_type().is_symlink() {
            false => None,
            true => match self.inner.readlink(path) {
                Ok(p) => Some(p),
                Err(err) => {
                    error!(
                        "Failed to read link of {} (even it's supposed to be a symlink): {}",
                        path.display(),
                        err
                    );
                    None
                }
            },
        };
        let file_type = if symlink.is_some() {
            FileType::Symlink
        } else if metadata.is_dir() {
            FileType::Directory
        } else {
            FileType::File
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
}

fn perform_shell_cmd<S: AsRef<str>>(session: &mut ssh2::Session, cmd: S) -> RemoteResult<String> {
    // Create channel
    trace!("Running command: {}", cmd.as_ref());
    let mut channel = match session.channel_session() {
        Ok(ch) => ch,
        Err(err) => {
            return Err(RemoteError::new_ex(
                RemoteErrorType::ProtocolError,
                format!("Could not open channel: {err}"),
            ));
        }
    };

    // escape single quotes in command
    let cmd = cmd.as_ref().replace('\'', r#"'\''"#); // close, escape, and reopen

    // Execute command; always execute inside of sh -c to have proper shell behavior.
    // if the remote peer has fish or other non-bash shell as default, commands like
    // "cd /some/dir; somecommand" may fail.
    if let Err(err) = channel.exec(format!("sh -c '{cmd}'").as_str()) {
        return Err(RemoteError::new_ex(
            RemoteErrorType::ProtocolError,
            format!("Could not execute command \"{cmd}\": {err}"),
        ));
    }
    // Read output
    let mut output: String = String::new();
    match channel.read_to_string(&mut output) {
        Ok(_) => {
            // Wait close
            let _ = channel.wait_close();
            trace!("Command output: {output}");
            Ok(output)
        }
        Err(err) => Err(RemoteError::new_ex(
            RemoteErrorType::ProtocolError,
            format!("Could not read output: {err}"),
        )),
    }
}

/// connect to socket address with provided timeout.
/// If timeout is zero, don't set timeout
fn tcp_connect(
    address: &SocketAddr,
    timeout: Duration,
    bind_address: Option<&str>,
    bind_interface: Option<&str>,
) -> std::io::Result<TcpStream> {
    if bind_address.is_none() && bind_interface.is_none() {
        return if timeout.is_zero() {
            TcpStream::connect(address)
        } else {
            TcpStream::connect_timeout(address, timeout)
        };
    }

    let source_addresses = if let Some(bind_address) = bind_address {
        (bind_address, 0).to_socket_addrs()?.collect::<Vec<_>>()
    } else {
        interface::addresses(bind_interface.expect("checked above"))?
            .into_iter()
            .collect()
    };

    let mut last_error = None;
    for source_address in source_addresses
        .into_iter()
        .filter(|source| source.is_ipv4() == address.is_ipv4())
    {
        let result = (|| {
            let socket = Socket::new(
                Domain::for_address(*address),
                Type::STREAM,
                Some(Protocol::TCP),
            )?;
            socket.bind(&source_address.into())?;
            if timeout.is_zero() {
                socket.connect(&(*address).into())?;
            } else {
                socket.connect_timeout(&(*address).into(), timeout)?;
            }
            Ok::<TcpStream, std::io::Error>(socket.into())
        })();
        match result {
            Ok(stream) => return Ok(stream),
            Err(err) => last_error = Some(err),
        }
    }
    let source = bind_address.map_or_else(
        || format!("BindInterface {}", bind_interface.expect("checked above")),
        |address| format!("BindAddress {address}"),
    );
    Err(last_error.unwrap_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::AddrNotAvailable,
            format!("no {address} address family found for {source}"),
        )
    }))
}

/// Configure algorithm preferences into session
fn set_algo_prefs(
    session: &mut ssh2::Session,
    opts: &SshOpts,
    config: &Config,
) -> RemoteResult<()> {
    // Configure preferences from config
    let params = &config.params;
    trace!("Configuring algorithm preferences...");
    if let Some(compress) = params.compression {
        trace!("compression: {compress}");
        session.set_compress(compress);
    }

    // kex
    let algos = params.kex_algorithms.algorithms().join(",");
    trace!("Configuring KEX algorithms: {algos}");
    if let Err(err) = session.method_pref(ssh2::MethodType::Kex, algos.as_str()) {
        error!("Could not set KEX algorithms: {err}");
        return Err(RemoteError::new_ex(RemoteErrorType::ProtocolError, err));
    }

    // HostKey
    let algos = params.host_key_algorithms.algorithms().join(",");
    trace!("Configuring HostKey algorithms: {algos}");
    if let Err(err) = session.method_pref(ssh2::MethodType::HostKey, algos.as_str()) {
        error!("Could not set host key algorithms: {err}");
        return Err(RemoteError::new_ex(RemoteErrorType::ProtocolError, err));
    }

    // Public key signature algorithms
    let algos = libssh2_signature_algorithms(params.pubkey_accepted_algorithms.algorithms());
    trace!("Configuring public key signature algorithms: {algos}");
    if let Err(err) = session.method_pref(ssh2::MethodType::SignAlgo, algos.as_str()) {
        error!("Could not set public key signature algorithms: {err}");
        return Err(RemoteError::new_ex(RemoteErrorType::ProtocolError, err));
    }

    // ciphers
    let algos = params.ciphers.algorithms().join(",");
    trace!("Configuring Crypt algorithms: {algos}");
    if let Err(err) = session.method_pref(ssh2::MethodType::CryptCs, algos.as_str()) {
        error!("Could not set crypt algorithms (client-server): {err}");
        return Err(RemoteError::new_ex(RemoteErrorType::ProtocolError, err));
    }
    if let Err(err) = session.method_pref(ssh2::MethodType::CryptSc, algos.as_str()) {
        error!("Could not set crypt algorithms (server-client): {err}");
        return Err(RemoteError::new_ex(RemoteErrorType::ProtocolError, err));
    }

    // MAC
    let algos = params.mac.algorithms().join(",");
    trace!("Configuring MAC algorithms: {algos}");
    if let Err(err) = session.method_pref(ssh2::MethodType::MacCs, algos.as_str()) {
        error!("Could not set MAC algorithms (client-server): {err}");
        return Err(RemoteError::new_ex(RemoteErrorType::ProtocolError, err));
    }
    if let Err(err) = session.method_pref(ssh2::MethodType::MacSc, algos.as_str()) {
        error!("Could not set MAC algorithms (server-client): {err}");
        return Err(RemoteError::new_ex(RemoteErrorType::ProtocolError, err));
    }

    // -- configure algos from opts
    for method in opts.methods.iter() {
        let algos = method.prefs();
        trace!("Configuring {:?} algorithm: {}", method.method_type, algos);
        if let Err(err) = session.method_pref(method.method_type.into(), algos.as_str()) {
            error!("Could not set {:?} algorithms: {}", method.method_type, err);
            return Err(RemoteError::new_ex(RemoteErrorType::ProtocolError, err));
        }
    }
    Ok(())
}

/// Convert OpenSSH certificate algorithm names to libssh2 RSA signature names.
fn libssh2_signature_algorithms(configured_algorithms: &[String]) -> String {
    let mut algorithms = Vec::with_capacity(configured_algorithms.len());

    for configured_algorithm in configured_algorithms {
        let algorithm = match configured_algorithm.as_str() {
            "rsa-sha2-512-cert-v01@openssh.com" => "rsa-sha2-512",
            "rsa-sha2-256-cert-v01@openssh.com" => "rsa-sha2-256",
            "ssh-rsa-cert-v01@openssh.com" => "ssh-rsa",
            algorithm => algorithm,
        };
        if !algorithms.contains(&algorithm) {
            algorithms.push(algorithm);
        }
    }

    algorithms.join(",")
}

/// Authenticate on session with ssh agent
fn session_auth_with_agent(
    session: &mut ssh2::Session,
    username: &str,
    ssh_agent_config: &SshAgentIdentity,
    accepted_algorithms: &[String],
) -> RemoteResult<()> {
    let mut agent = session
        .agent()
        .map_err(|err| RemoteError::new_ex(RemoteErrorType::ConnectionError, err))?;

    agent
        .connect()
        .map_err(|err| RemoteError::new_ex(RemoteErrorType::ConnectionError, err))?;

    agent
        .list_identities()
        .map_err(|err| RemoteError::new_ex(RemoteErrorType::ConnectionError, err))?;

    let mut connection_result = Err(RemoteError::new(RemoteErrorType::AuthenticationFailed));

    for identity in agent
        .identities()
        .map_err(|err| RemoteError::new_ex(RemoteErrorType::ConnectionError, err))?
    {
        if !public_key_blob_is_accepted(identity.blob(), accepted_algorithms) {
            debug!("Skipping SSH agent identity excluded by PubkeyAcceptedAlgorithms");
            continue;
        }
        if ssh_agent_config.pubkey_matches(identity.blob()) {
            debug!("Trying to authenticate with ssh agent with key: {identity:?}");
        } else {
            continue;
        }
        match agent.userauth(username, &identity) {
            Ok(()) => {
                connection_result = Ok(());
                debug!("Authenticated with ssh agent with key: {identity:?}");
                break;
            }
            Err(err) => {
                debug!("SSH agent auth failed: {err}");
                connection_result = Err(RemoteError::new_ex(
                    RemoteErrorType::AuthenticationFailed,
                    err,
                ));
            }
        }
    }

    if let Err(err) = agent.disconnect() {
        warn!("Could not disconnect from ssh agent: {err}");
    }

    connection_result
}

/// Authenticate on session with private key
fn session_auth_with_rsakey(
    session: &mut ssh2::Session,
    username: &str,
    private_key: &Path,
    certificate: Option<&Path>,
    password: Option<&str>,
    accepted_algorithms: &[String],
) -> RemoteResult<()> {
    if let Some(certificate) = certificate {
        let certificate_algorithm = ssh_key::Certificate::read_file(certificate)
            .map(|certificate| certificate.algorithm().to_certificate_type())
            .map_err(|err| {
                RemoteError::new_ex(
                    RemoteErrorType::AuthenticationFailed,
                    format!(
                        "could not inspect certificate at '{}': {err}",
                        certificate.display()
                    ),
                )
            })?;
        if !public_key_blob_algorithm_is_accepted(&certificate_algorithm, accepted_algorithms) {
            return Err(RemoteError::new_ex(
                RemoteErrorType::AuthenticationFailed,
                format!(
                    "certificate algorithm {certificate_algorithm} is not accepted by SSH config"
                ),
            ));
        }
    } else {
        let private_key_algorithm = private_key_algorithm(private_key, password)?;
        if !pubkey_algorithm_is_accepted(&private_key_algorithm, accepted_algorithms) {
            return Err(RemoteError::new_ex(
                RemoteErrorType::AuthenticationFailed,
                format!(
                    "private key algorithm {private_key_algorithm} is not accepted by SSH config"
                ),
            ));
        }
    }

    debug!("Authenticating with username '{username}' and RSA key");
    trace!(
        "Trying to authenticate with RSA key at '{}'",
        private_key.display()
    );
    session
        .userauth_pubkey_file(username, certificate, private_key, password)
        .map(|()| debug!("Authenticated with key at '{}'", private_key.display()))
        .map_err(|err| {
            error!("Authentication failed: {err}");
            RemoteError::new_ex(RemoteErrorType::AuthenticationFailed, err)
        })
}

/// Authenticate on session with the provided [`Authentication`] method.
fn session_auth(
    session: &mut ssh2::Session,
    opts: &SshOpts,
    ssh_config: &Config,
    authentication: Authentication,
) -> RemoteResult<()> {
    match authentication {
        Authentication::RsaKey {
            private_key,
            certificate,
        } => session_auth_with_rsakey(
            session,
            &ssh_config.username,
            private_key.as_path(),
            certificate.as_deref(),
            opts.password.as_deref(),
            ssh_config.params.pubkey_accepted_algorithms.algorithms(),
        ),
        Authentication::Password(password) => {
            session_auth_with_password(session, &ssh_config.username, &password)
        }
    }
}

/// Read the algorithm from an OpenSSH or PEM private key.
fn private_key_algorithm(
    private_key: &Path,
    password: Option<&str>,
) -> RemoteResult<ssh_key::Algorithm> {
    if let Ok(key) = ssh_key::PrivateKey::read_openssh_file(private_key) {
        return Ok(key.algorithm());
    }

    let pem = std::fs::read(private_key).map_err(|err| {
        RemoteError::new_ex(
            RemoteErrorType::AuthenticationFailed,
            format!(
                "could not read private key at '{}': {err}",
                private_key.display()
            ),
        )
    })?;
    let key = password
        .and_then(|password| {
            openssl::pkey::PKey::private_key_from_pem_passphrase(&pem, password.as_bytes()).ok()
        })
        .or_else(|| openssl::pkey::PKey::private_key_from_pem(&pem).ok())
        .ok_or_else(|| {
            RemoteError::new_ex(
                RemoteErrorType::AuthenticationFailed,
                format!(
                    "could not inspect private key at '{}' for PubkeyAcceptedAlgorithms",
                    private_key.display()
                ),
            )
        })?;

    openssl_key_algorithm(&key).ok_or_else(|| {
        RemoteError::new_ex(
            RemoteErrorType::AuthenticationFailed,
            format!(
                "private key at '{}' uses an unsupported algorithm",
                private_key.display()
            ),
        )
    })
}

/// Convert an OpenSSL private key identifier to its SSH algorithm.
fn openssl_key_algorithm(
    key: &openssl::pkey::PKey<openssl::pkey::Private>,
) -> Option<ssh_key::Algorithm> {
    use openssl::pkey::Id;
    use ssh_key::{Algorithm, EcdsaCurve};

    match key.id() {
        Id::RSA | Id::RSA_PSS => Some(Algorithm::Rsa { hash: None }),
        Id::DSA => Some(Algorithm::Dsa),
        Id::ED25519 => Some(Algorithm::Ed25519),
        Id::EC => {
            let curve = match key.ec_key().ok()?.group().curve_name()? {
                openssl::nid::Nid::X9_62_PRIME256V1 => EcdsaCurve::NistP256,
                openssl::nid::Nid::SECP384R1 => EcdsaCurve::NistP384,
                openssl::nid::Nid::SECP521R1 => EcdsaCurve::NistP521,
                _ => return None,
            };
            Some(Algorithm::Ecdsa { curve })
        }
        _ => None,
    }
}

/// Return whether a public key algorithm is allowed by the configured list.
fn pubkey_algorithm_is_accepted(
    key_algorithm: &ssh_key::Algorithm,
    accepted_algorithms: &[String],
) -> bool {
    if matches!(key_algorithm, ssh_key::Algorithm::Rsa { .. }) {
        accepted_algorithms.iter().any(|algorithm| {
            matches!(
                algorithm.as_str(),
                "rsa-sha2-512" | "rsa-sha2-256" | "ssh-rsa"
            )
        })
    } else {
        accepted_algorithms
            .iter()
            .any(|algorithm| algorithm == key_algorithm.as_ref())
    }
}

/// Return whether the exact algorithm named by an SSH public key blob is accepted.
fn public_key_blob_is_accepted(blob: &[u8], accepted_algorithms: &[String]) -> bool {
    let Some(name_length) = blob.get(..4) else {
        return false;
    };
    let name_length = u32::from_be_bytes(
        name_length
            .try_into()
            .expect("the public key algorithm length is four bytes"),
    ) as usize;
    let Some(name_end) = 4_usize.checked_add(name_length) else {
        return false;
    };
    let Some(name) = blob.get(4..name_end) else {
        return false;
    };
    let Ok(name) = std::str::from_utf8(name) else {
        return false;
    };

    public_key_blob_algorithm_is_accepted(name, accepted_algorithms)
}

/// Return whether a public key blob algorithm is allowed by the configured list.
fn public_key_blob_algorithm_is_accepted(
    key_algorithm: &str,
    accepted_algorithms: &[String],
) -> bool {
    const RSA_ALGORITHMS: [&str; 3] = ["rsa-sha2-512", "rsa-sha2-256", "ssh-rsa"];
    const RSA_CERTIFICATE_ALGORITHMS: [&str; 3] = [
        "rsa-sha2-512-cert-v01@openssh.com",
        "rsa-sha2-256-cert-v01@openssh.com",
        "ssh-rsa-cert-v01@openssh.com",
    ];

    let equivalent_algorithms = if RSA_ALGORITHMS.contains(&key_algorithm) {
        &RSA_ALGORITHMS
    } else if RSA_CERTIFICATE_ALGORITHMS.contains(&key_algorithm) {
        &RSA_CERTIFICATE_ALGORITHMS
    } else {
        return accepted_algorithms
            .iter()
            .any(|algorithm| algorithm == key_algorithm);
    };

    accepted_algorithms
        .iter()
        .any(|algorithm| equivalent_algorithms.contains(&algorithm.as_str()))
}

/// Authenticate on session with username and password
fn session_auth_with_password(
    session: &mut ssh2::Session,
    username: &str,
    password: &str,
) -> RemoteResult<()> {
    // Username / password
    debug!("Authenticating with username '{username}' and password");
    if let Err(err) = session.userauth_password(username, password) {
        error!("Authentication failed: {err}");
        Err(RemoteError::new_ex(
            RemoteErrorType::AuthenticationFailed,
            err,
        ))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod test {

    use ssh2_config::ParseRule;
    use tempfile::NamedTempFile;

    use super::*;
    use crate::mock::ssh as ssh_mock;

    #[test]
    fn should_filter_non_rsa_pubkey_algorithms() {
        let rsa_only = vec!["rsa-sha2-256".to_string()];
        let ed25519_only = vec!["ssh-ed25519".to_string()];

        assert!(!pubkey_algorithm_is_accepted(
            &ssh_key::Algorithm::Ed25519,
            &rsa_only,
        ));
        assert!(pubkey_algorithm_is_accepted(
            &ssh_key::Algorithm::Ed25519,
            &ed25519_only,
        ));
        assert!(pubkey_algorithm_is_accepted(
            &ssh_key::Algorithm::Rsa { hash: None },
            &rsa_only,
        ));
    }

    #[test]
    fn should_distinguish_plain_and_certificate_pubkey_algorithms() {
        let ed25519_only = vec!["ssh-ed25519".to_string()];
        let ed25519_certificate_only = vec!["ssh-ed25519-cert-v01@openssh.com".to_string()];
        let rsa_only = vec!["rsa-sha2-256".to_string()];
        let rsa_certificate_only = vec!["rsa-sha2-256-cert-v01@openssh.com".to_string()];

        assert!(!public_key_blob_algorithm_is_accepted(
            "ssh-ed25519-cert-v01@openssh.com",
            &ed25519_only,
        ));
        assert!(!public_key_blob_algorithm_is_accepted(
            "ssh-ed25519",
            &ed25519_certificate_only,
        ));
        assert!(!public_key_blob_algorithm_is_accepted(
            "ssh-rsa-cert-v01@openssh.com",
            &rsa_only,
        ));
        assert!(!public_key_blob_algorithm_is_accepted(
            "ssh-rsa",
            &rsa_certificate_only,
        ));
        assert!(public_key_blob_algorithm_is_accepted(
            "ssh-rsa-cert-v01@openssh.com",
            &rsa_certificate_only,
        ));
    }

    #[test]
    fn should_normalize_rsa_certificate_signature_algorithms() {
        let configured = vec![
            "ssh-ed25519-cert-v01@openssh.com".to_string(),
            "rsa-sha2-512-cert-v01@openssh.com".to_string(),
            "rsa-sha2-256-cert-v01@openssh.com".to_string(),
            "ssh-rsa-cert-v01@openssh.com".to_string(),
        ];

        assert_eq!(
            libssh2_signature_algorithms(&configured),
            "ssh-ed25519-cert-v01@openssh.com,rsa-sha2-512,rsa-sha2-256,ssh-rsa"
        );
    }

    #[test]
    fn should_inspect_legacy_pem_private_key_algorithm() {
        let rsa = openssl::rsa::Rsa::generate(2048).expect("failed to generate RSA key");
        let pem = rsa
            .private_key_to_pem()
            .expect("failed to encode RSA key as PEM");
        let mut key_file = NamedTempFile::new().expect("failed to create private key file");
        key_file
            .write_all(&pem)
            .expect("failed to write PEM private key");

        let algorithm = private_key_algorithm(key_file.path(), None)
            .expect("failed to inspect legacy PEM private key");
        assert!(matches!(algorithm, ssh_key::Algorithm::Rsa { .. }));
    }

    #[test]
    fn should_connect_with_identity_file_from_ssh_config() {
        let container = crate::ssh::container::OpensshServer::start();
        let key_file = ssh_mock::create_key_file();
        let config_file =
            ssh_mock::create_ssh_config_with_identity(container.port(), key_file.path());
        let opts =
            SshOpts::new("sftp").config_file(config_file.path(), ParseRule::ALLOW_UNKNOWN_FIELDS);

        let session = LibSsh2Session::connect(&opts)
            .expect("failed to authenticate with IdentityFile from SSH config");
        assert!(
            session
                .authenticated()
                .expect("failed to query session state")
        );
    }

    #[test]
    fn should_connect_with_certificate_file_from_ssh_config() {
        let (key_file, certificate_file) = ssh_mock::create_certificate_key_files();
        let container = crate::ssh::container::OpensshServer::start_with_public_key(
            ssh_mock::MOCK_CERTIFICATE_AUTHORITY,
        );
        let config_file = ssh_mock::create_ssh_config_with_certificate(
            container.port(),
            key_file.path(),
            certificate_file.path(),
        );
        let opts =
            SshOpts::new("sftp").config_file(config_file.path(), ParseRule::ALLOW_UNKNOWN_FIELDS);

        let session = LibSsh2Session::connect(&opts)
            .expect("failed to authenticate with CertificateFile from SSH config");
        assert!(
            session
                .authenticated()
                .expect("failed to query session state")
        );
    }

    #[test]
    fn should_disable_public_key_authentication_from_ssh_config() {
        let container = crate::ssh::container::OpensshServer::start();
        let key_file = ssh_mock::create_key_file();
        let config_file = ssh_mock::create_ssh_config_with_identity_and_pubkey_authentication(
            container.port(),
            key_file.path(),
            false,
        );
        let opts =
            SshOpts::new("sftp").config_file(config_file.path(), ParseRule::ALLOW_UNKNOWN_FIELDS);

        assert!(LibSsh2Session::connect(&opts).is_err());

        let opts = SshOpts::new("sftp")
            .config_file(config_file.path(), ParseRule::ALLOW_UNKNOWN_FIELDS)
            .password("password");
        let session =
            LibSsh2Session::connect(&opts).expect("password authentication should remain enabled");
        assert!(
            session
                .authenticated()
                .expect("failed to query session state")
        );
    }

    #[test]
    fn should_try_each_identity_file_from_ssh_config() {
        let container = crate::ssh::container::OpensshServer::start();
        let key_file = ssh_mock::create_key_file();
        let missing_key = key_file.path().with_extension("missing");
        assert!(!missing_key.exists());
        let mut config_file = NamedTempFile::new().expect("failed to create SSH config");
        writeln!(
            config_file,
            "Host sftp\n    HostName 127.0.0.1\n    Port {port}\n    User sftp\n    IdentityFile {missing}\n    IdentityFile {valid}",
            port = container.port(),
            missing = missing_key.display(),
            valid = key_file.path().display(),
        )
        .expect("failed to write SSH config");
        let opts =
            SshOpts::new("sftp").config_file(config_file.path(), ParseRule::ALLOW_UNKNOWN_FIELDS);

        let session = LibSsh2Session::connect(&opts)
            .expect("failed to fall back to the second configured IdentityFile");
        assert!(
            session
                .authenticated()
                .expect("failed to query session state")
        );
    }

    #[test]
    fn should_apply_pubkey_accepted_algorithms_from_ssh_config() {
        let container = crate::ssh::container::OpensshServer::start();
        let key_file = ssh_mock::create_key_file();
        let legacy_config = ssh_mock::create_ssh_config_with_identity_and_pubkey_algorithms(
            container.port(),
            key_file.path(),
            "ssh-rsa",
        );
        let opts =
            SshOpts::new("sftp").config_file(legacy_config.path(), ParseRule::ALLOW_UNKNOWN_FIELDS);
        assert!(LibSsh2Session::connect(&opts).is_err());

        let modern_config = ssh_mock::create_ssh_config_with_identity_and_pubkey_algorithms(
            container.port(),
            key_file.path(),
            "rsa-sha2-256",
        );
        let opts =
            SshOpts::new("sftp").config_file(modern_config.path(), ParseRule::ALLOW_UNKNOWN_FIELDS);
        let session = LibSsh2Session::connect(&opts)
            .expect("failed to authenticate with the accepted RSA SHA-2 algorithm");
        assert!(
            session
                .authenticated()
                .expect("failed to query session state")
        );
    }

    #[test]
    fn should_connect_to_ssh_server_auth_user_password() {
        use crate::ssh::container::OpensshServer;

        let container = OpensshServer::start();
        let port = container.port();

        crate::mock::logger();
        let config_file = ssh_mock::create_ssh_config(port);
        let opts = SshOpts::new("sftp")
            .config_file(config_file.path(), ParseRule::ALLOW_UNKNOWN_FIELDS)
            .password("password");

        if let Err(err) = LibSsh2Session::connect(&opts) {
            panic!("Could not connect to server: {err}");
        }
        let session = LibSsh2Session::connect(&opts).unwrap();
        assert!(session.authenticated().unwrap());

        drop(container);
    }

    #[test]
    fn should_connect_to_ssh_server_auth_key() {
        use crate::ssh::container::OpensshServer;

        let container = OpensshServer::start();
        let port = container.port();

        crate::mock::logger();
        let config_file = ssh_mock::create_ssh_config(port);
        let opts = SshOpts::new("sftp")
            .config_file(config_file.path(), ParseRule::ALLOW_UNKNOWN_FIELDS)
            .key_storage(Box::new(ssh_mock::MockSshKeyStorage::default()));
        let session = LibSsh2Session::connect(&opts).unwrap();
        assert!(session.authenticated().unwrap());
    }

    #[test]
    fn should_connect_through_proxy_jump() {
        use crate::ssh::container::ProxyJumpServers;

        let servers = ProxyJumpServers::start();
        let config_file = ssh_mock::create_ssh_config_with_proxy_jump(
            &servers.target_host,
            2222,
            servers.first_jump.port(),
            &servers.second_jump_host,
        );
        let opts = SshOpts::new("target")
            .config_file(config_file.path(), ParseRule::ALLOW_UNKNOWN_FIELDS)
            .password("password");

        let mut session =
            LibSsh2Session::connect(&opts).expect("failed to connect through ProxyJump");
        assert!(
            session
                .authenticated()
                .expect("failed to query session state")
        );
        assert_eq!(
            session.cmd("pwd").expect("command through proxy failed").0,
            0
        );
    }

    #[test]

    fn should_perform_shell_command_on_server() {
        crate::mock::logger();
        let container = crate::ssh::container::OpensshServer::start();
        let port = container.port();

        let opts = SshOpts::new("127.0.0.1")
            .port(port)
            .username("sftp")
            .password("password");
        let mut session = LibSsh2Session::connect(&opts).unwrap();
        assert!(session.authenticated().unwrap());
        // run commands
        assert!(session.cmd("pwd").is_ok());
    }

    #[test]

    fn should_perform_shell_command_on_server_and_return_exit_code() {
        crate::mock::logger();
        let container = crate::ssh::container::OpensshServer::start();
        let port = container.port();

        let opts = SshOpts::new("127.0.0.1")
            .port(port)
            .username("sftp")
            .password("password");
        let mut session = LibSsh2Session::connect(&opts).unwrap();
        assert!(session.authenticated().unwrap());
        // run commands
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

        let opts = SshOpts::new("127.0.0.1")
            .port(port)
            .username("sftp")
            .password("ippopotamo");
        assert!(LibSsh2Session::connect(&opts).is_err());
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
            .password("password");
        let session = LibSsh2Session::connect(&opts)
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
            .password("password");
        assert!(LibSsh2Session::connect(&opts).is_err());

        let mut precedence_config = NamedTempFile::new().expect("failed to create SSH config");
        writeln!(
            precedence_config,
            "Host bound\n    HostName 127.0.0.1\n    Port {port}\n    User sftp\n    BindAddress 127.0.0.1\n    BindInterface remotefs-ssh-missing-interface"
        )
        .expect("failed to write SSH config");
        let opts = SshOpts::new("bound")
            .config_file(precedence_config.path(), ParseRule::STRICT)
            .password("password");
        let session = LibSsh2Session::connect(&opts)
            .expect("BindAddress should take precedence over BindInterface");
        session.disconnect().expect("failed to disconnect");
    }

    #[test]
    fn should_apply_configured_bind_interface() {
        let container = crate::ssh::container::OpensshServer::start();
        let port = container.port();
        let interface = ssh_mock::ipv4_loopback_interface();
        let mut valid_config = NamedTempFile::new().expect("failed to create SSH config");
        writeln!(
            valid_config,
            "Host bound\n    HostName 127.0.0.1\n    Port {port}\n    User sftp\n    BindInterface {interface}"
        )
        .expect("failed to write SSH config");
        let opts = SshOpts::new("bound")
            .config_file(valid_config.path(), ParseRule::STRICT)
            .password("password");
        let session = LibSsh2Session::connect(&opts)
            .expect("failed to connect through the configured interface");
        session.disconnect().expect("failed to disconnect");

        let mut invalid_config = NamedTempFile::new().expect("failed to create SSH config");
        writeln!(
            invalid_config,
            "Host bound\n    HostName 127.0.0.1\n    Port {port}\n    User sftp\n    BindInterface remotefs-ssh-missing-interface"
        )
        .expect("failed to write SSH config");
        let opts = SshOpts::new("bound")
            .config_file(invalid_config.path(), ParseRule::STRICT)
            .password("password");
        assert!(LibSsh2Session::connect(&opts).is_err());
    }

    #[test]
    fn should_apply_configured_tcp_keep_alive() {
        let container = crate::ssh::container::OpensshServer::start();
        let port = container.port();
        for (configured, expected) in [("yes", true), ("no", false)] {
            let mut config_file = NamedTempFile::new().expect("failed to create SSH config");
            writeln!(
                config_file,
                "Host keepalive\n    HostName 127.0.0.1\n    Port {port}\n    User sftp\n    TCPKeepAlive {configured}"
            )
            .expect("failed to write SSH config");
            let opts = SshOpts::new("keepalive")
                .config_file(config_file.path(), ParseRule::STRICT)
                .password("password");
            let session = LibSsh2Session::connect(&opts).expect("failed to connect");

            assert_eq!(
                crate::ssh::backend::socket::keepalive(&session.session)
                    .expect("failed to read SO_KEEPALIVE"),
                expected
            );
            session.disconnect().expect("failed to disconnect");
        }

        let opts = SshOpts::new("127.0.0.1")
            .port(port)
            .username("sftp")
            .password("password");
        let session = LibSsh2Session::connect(&opts).expect("failed to connect");
        assert!(
            crate::ssh::backend::socket::keepalive(&session.session)
                .expect("failed to read default SO_KEEPALIVE")
        );
        session.disconnect().expect("failed to disconnect");
    }

    #[test]
    fn test_filetransfer_sftp_bad_server() {
        crate::mock::logger();
        let opts = SshOpts::new("myverybad.verybad.server")
            .port(10022)
            .username("sftp")
            .password("ippopotamo");
        assert!(LibSsh2Session::connect(&opts).is_err());
    }
}
