use std::io::{Cursor, Read, Seek, Write};
#[cfg(unix)]
use std::net::Shutdown;
use std::net::ToSocketAddrs as _;
#[cfg(unix)]
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::str::FromStr as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, UNIX_EPOCH};

use libssh_rs::{AuthMethods, AuthStatus, OpenFlags, SshKey, SshOption};
use remotefs::fs::stream::{ReadAndSeek, WriteAndSeek};
use remotefs::fs::{FileType, Metadata, ReadStream, UnixPex, WriteStream};
use remotefs::{File, RemoteError, RemoteErrorType, RemoteResult};
use ssh2_config::{RemoteForwardDestination, RemoteForwardListen};

use super::{SshSession, interface, socket};
use crate::SshOpts;
use crate::ssh::backend::Sftp;
use crate::ssh::backend::forward::{
    ChannelIo, ForwardConnection, ForwardWorker, MAX_FORWARD_CONNECTIONS,
    tcp_remote_forward_endpoint,
};
use crate::ssh::config::Config;

/// An implementation of [`SshSession`] using libssh as the backend.
///
/// See <https://docs.rs/libssh-rs/0.3.6/libssh_rs/struct.Session.html>
pub struct LibSshSession {
    session: libssh_rs::Session,
    forward_agent: bool,
    remote_forward_ports: Vec<u16>,
    remote_forward_worker: Option<ForwardWorker>,
}

/// A wrapper around [`libssh_rs::Sftp`] to provide a SFTP client for [`LibSshSession`]
///
/// See <https://docs.rs/libssh-rs/0.3.6/libssh_rs/struct.Sftp.html>
pub struct LibSshSftp {
    inner: libssh_rs::Sftp,
}

fn connect_with_timeout(
    session: &libssh_rs::Session,
    timeout: Duration,
) -> libssh_rs::SshResult<()> {
    session.set_blocking(false);
    let started = Instant::now();
    let result = loop {
        match session.connect() {
            Ok(()) => break Ok(()),
            Err(libssh_rs::Error::TryAgain) if started.elapsed() < timeout => {
                let remaining = timeout.saturating_sub(started.elapsed());
                std::thread::sleep(Duration::from_millis(10).min(remaining));
            }
            Err(libssh_rs::Error::TryAgain) => {
                break Err(libssh_rs::Error::fatal(format!(
                    "connection timed out after {timeout:?}"
                )));
            }
            Err(err) => break Err(err),
        }
    };
    session.set_blocking(true);
    result
}

fn connect_libssh_session(opts: &SshOpts, ssh_config: &Config) -> RemoteResult<libssh_rs::Session> {
    let mut session = libssh_rs::Session::new().map_err(|err| {
        error!("Could not create session: {err}");
        RemoteError::new_ex(RemoteErrorType::ConnectionError, err)
    })?;
    session
        .set_option(SshOption::Hostname(opts.host.clone()))
        .map_err(|err| RemoteError::new_ex(RemoteErrorType::ConnectionError, err))?;
    let config_file = opts
        .config_file
        .as_ref()
        .map(|path| path.display().to_string());
    debug!("Using config file: {config_file:?}");
    session
        .options_parse_config(config_file.as_deref())
        .map_err(|err| RemoteError::new_ex(RemoteErrorType::ConnectionError, err))?;
    if let Some(port) = opts.port {
        debug!("Using port: {port}");
        session
            .set_option(SshOption::Port(port))
            .map_err(|err| RemoteError::new_ex(RemoteErrorType::ConnectionError, err))?;
    }

    let bind_addresses = if ssh_config.params.bind_address.is_none()
        && let Some(bind_interface) = ssh_config.params.bind_interface.as_deref()
    {
        let interface_addresses = interface::addresses(bind_interface)
            .map_err(|err| RemoteError::new_ex(RemoteErrorType::ConnectionError, err))?;
        let target_addresses = ssh_config
            .address
            .to_socket_addrs()
            .map_err(|err| RemoteError::new_ex(RemoteErrorType::BadAddress, err.to_string()))?
            .collect::<Vec<_>>();
        let bind_addresses = interface_addresses
            .into_iter()
            .filter(|source| {
                target_addresses
                    .iter()
                    .any(|target| source.is_ipv4() == target.is_ipv4())
            })
            .map(|source| interface::host(&source))
            .collect::<Vec<_>>();
        if bind_addresses.is_empty() {
            return Err(RemoteError::new_ex(
                RemoteErrorType::ConnectionError,
                format!(
                    "BindInterface {bind_interface} has no address compatible with {}",
                    ssh_config.address
                ),
            ));
        }
        debug!("Using bind interface {bind_interface} addresses {bind_addresses:?}");
        bind_addresses
    } else {
        Vec::new()
    };
    session
        .set_option(SshOption::Timeout(ssh_config.connection_timeout))
        .map_err(|err| RemoteError::new_ex(RemoteErrorType::ConnectionError, err))?;
    for option in opts.methods.iter().filter_map(|method| method.ssh_opts()) {
        debug!("Setting SSH option: {option:?}");
        session
            .set_option(option)
            .map_err(|err| RemoteError::new_ex(RemoteErrorType::ConnectionError, err))?;
    }

    let connection_attempts = ssh_config.connection_attempts.max(1);
    let source_attempts = bind_addresses.len().max(1);
    let mut last_error = None;
    let mut connected = false;
    'connection: for attempt in 1..=connection_attempts {
        for source_attempt in 0..source_attempts {
            if let Some(bind_address) = bind_addresses.get(source_attempt) {
                session
                    .set_option(SshOption::BindAddress(bind_address.clone()))
                    .map_err(|err| RemoteError::new_ex(RemoteErrorType::ConnectionError, err))?;
            }
            match connect_with_timeout(&session, ssh_config.connection_timeout) {
                Ok(()) => {
                    connected = true;
                    break 'connection;
                }
                Err(err) => {
                    warn!("SSH connection attempt {attempt} failed: {err}");
                    last_error = Some(err);
                    session.disconnect();
                }
            }
        }
    }
    if !connected {
        let err = last_error.expect("at least one connection was attempted");
        error!("SSH handshake failed: {err}");
        return Err(RemoteError::new_ex(RemoteErrorType::ProtocolError, err));
    }
    socket::set_keepalive(&session, ssh_config.params.tcp_keep_alive.unwrap_or(true))
        .map_err(|err| RemoteError::new_ex(RemoteErrorType::ConnectionError, err))?;
    authenticate(&mut session, opts)?;
    Ok(session)
}

#[derive(Clone)]
struct LibSshForwardRoute {
    port: u16,
    destination: Option<RemoteForwardDestination>,
    connect_timeout: Duration,
}

impl ChannelIo for libssh_rs::Channel {
    fn read_channel(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        self.read_timeout(buffer, false, Some(Duration::ZERO))
            .map_err(Into::into)
    }

    fn write_channel(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        self.stdin().write(buffer)
    }

    fn is_eof(&self) -> bool {
        libssh_rs::Channel::is_eof(self)
    }

    fn send_eof(&mut self) -> std::io::Result<()> {
        libssh_rs::Channel::send_eof(self).map_err(Into::into)
    }

    fn close(&mut self) {
        let _ = libssh_rs::Channel::close(self);
    }
}

fn setup_libssh_remote_forwards(
    opts: &SshOpts,
    config: &Config,
) -> RemoteResult<(Option<ForwardWorker>, Vec<u16>)> {
    if config.params.remote_forward.is_empty() {
        return Ok((None, Vec::new()));
    }
    #[cfg(not(unix))]
    if config.params.remote_forward.iter().any(|forward| {
        matches!(
            forward.destination,
            Some(RemoteForwardDestination::UnixSocket(_))
        )
    }) {
        return Err(RemoteError::new_ex(
            RemoteErrorType::UnsupportedFeature,
            "Unix socket RemoteForward destinations are unavailable on this platform",
        ));
    }
    if let Some(forward) = config
        .params
        .remote_forward
        .iter()
        .find(|forward| matches!(forward.listen, RemoteForwardListen::UnixSocket(_)))
    {
        return Err(RemoteError::new_ex(
            RemoteErrorType::UnsupportedFeature,
            format!(
                "libssh cannot create Unix socket RemoteForward listener {}",
                forward.listen
            ),
        ));
    }
    for (index, forward) in config.params.remote_forward.iter().enumerate() {
        let Some((_, port)) = tcp_remote_forward_endpoint(&forward.listen) else {
            unreachable!("Unix socket listeners were rejected above");
        };
        if port != 0
            && config.params.remote_forward[..index].iter().any(|other| {
                tcp_remote_forward_endpoint(&other.listen)
                    .is_some_and(|(_, other_port)| other_port == port)
            })
        {
            return Err(RemoteError::new_ex(
                RemoteErrorType::UnsupportedFeature,
                format!(
                    "libssh cannot distinguish multiple RemoteForward listeners on port {port}"
                ),
            ));
        }
    }

    let session = connect_libssh_session(opts, config)?;
    let mut routes = Vec::<LibSshForwardRoute>::new();
    for forward in &config.params.remote_forward {
        let (mut bind_address, port) = tcp_remote_forward_endpoint(&forward.listen)
            .expect("Unix socket listeners were rejected above");
        if bind_address.is_some_and(|address| address.is_empty() || address == "*") {
            bind_address = None;
        }
        let returned_port = session.listen_forward(bind_address, port).map_err(|err| {
            RemoteError::new_ex(
                RemoteErrorType::ProtocolError,
                format!(
                    "Could not configure RemoteForward {}: {err}",
                    forward.listen
                ),
            )
        })?;
        let actual_port = if port == 0 { returned_port } else { port };
        if routes.iter().any(|route| route.port == actual_port) {
            session.disconnect();
            return Err(RemoteError::new_ex(
                RemoteErrorType::UnsupportedFeature,
                format!(
                    "libssh cannot distinguish multiple RemoteForward listeners on port {actual_port}"
                ),
            ));
        }
        debug!(
            "RemoteForward {} listening on assigned port {actual_port}",
            forward.listen
        );
        routes.push(LibSshForwardRoute {
            port: actual_port,
            destination: forward.destination.clone(),
            connect_timeout: config.connection_timeout,
        });
    }

    session.set_blocking(false);
    let remote_forward_ports = routes.iter().map(|route| route.port).collect();
    let cancelled = Arc::new(AtomicBool::new(false));
    let worker_cancelled = cancelled.clone();
    let worker = std::thread::spawn(move || {
        let mut connections = Vec::<ForwardConnection<libssh_rs::Channel>>::new();
        'forwarding: while !worker_cancelled.load(Ordering::Acquire) {
            let mut progressed = false;
            while connections.len() < MAX_FORWARD_CONNECTIONS {
                match session.accept_forward(Duration::ZERO) {
                    Ok((port, channel)) => {
                        let Some(route) = routes.iter().find(|route| route.port == port) else {
                            warn!("Received RemoteForward connection for unknown port {port}");
                            continue;
                        };
                        let connection = match &route.destination {
                            Some(destination) => ForwardConnection::fixed(
                                channel,
                                destination,
                                route.connect_timeout,
                                Arc::clone(&worker_cancelled),
                            ),
                            None => Ok(ForwardConnection::dynamic(
                                channel,
                                route.connect_timeout,
                                Arc::clone(&worker_cancelled),
                            )),
                        };
                        match connection {
                            Ok(connection) => connections.push(connection),
                            Err(err) => warn!("Could not queue RemoteForward destination: {err}"),
                        }
                        progressed = true;
                    }
                    Err(libssh_rs::Error::TryAgain) => break,
                    Err(err) => {
                        warn!("Could not accept RemoteForward connection: {err}");
                        break 'forwarding;
                    }
                }
            }

            let mut index = 0;
            while index < connections.len() {
                match connections[index].pump() {
                    Ok((true, connection_progressed)) => {
                        progressed |= connection_progressed;
                        index += 1;
                    }
                    Ok((false, connection_progressed)) => {
                        progressed |= connection_progressed;
                        connections.swap_remove(index);
                    }
                    Err(err) => {
                        warn!("RemoteForward connection failed: {err}");
                        connections.swap_remove(index);
                    }
                }
            }
            if !progressed {
                std::thread::sleep(Duration::from_millis(2));
            }
        }
        drop(connections);
        session.disconnect();
    });
    Ok((
        Some(ForwardWorker::new(cancelled, worker)),
        remote_forward_ports,
    ))
}

/// A wrapper around [`libssh_rs::Channel`] to provide a SCP recv channel for [`LibSshSession`]
struct ScpRecvChannel {
    channel: libssh_rs::Channel,
    /// We must keep track of the total file size
    /// otherwise read will hang
    filesize: usize,
    read: usize,
}

impl Read for ScpRecvChannel {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.read >= self.filesize {
            return Ok(0);
        }

        // read up to
        let max_read = (self.filesize - self.read).min(buf.len());
        let res = self.channel.stdout().read(&mut buf[..max_read])?;

        self.read += res;
        Ok(res)
    }
}

impl Drop for ScpRecvChannel {
    fn drop(&mut self) {
        debug!("Dropping SCP recv channel");
        if let Err(err) = self.channel.send_eof() {
            debug!("Error sending EOF: {err}");
        }
        if let Err(err) = self.channel.close() {
            debug!("Error closing channel: {err}");
        }
    }
}

/// A wrapper around [`libssh_rs::Channel`] to provide a SCP send channel for [`LibSshSession`]
struct ScpSendChannel {
    channel: libssh_rs::Channel,
}

impl Write for ScpSendChannel {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.channel.stdin().write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.channel.stdin().flush()
    }
}

impl Drop for ScpSendChannel {
    fn drop(&mut self) {
        debug!("Dropping SCP send channel");
        if let Err(err) = self.channel.send_eof() {
            debug!("Error sending EOF: {err}");
        }
        if let Err(err) = self.channel.close() {
            debug!("Error closing channel: {err}");
        }
    }
}

impl SshSession for LibSshSession {
    type Sftp = LibSshSftp;

    fn connect(opts: &SshOpts) -> remotefs::RemoteResult<Self> {
        // Resolve host
        debug!("Connecting to '{}'", opts.host);
        let ssh_config = Config::try_from(opts)?;
        let session = connect_libssh_session(opts, &ssh_config)?;

        let forward_agent = ssh_config.params.forward_agent.unwrap_or(false);
        session.enable_accept_agent_forward(forward_agent);
        let (remote_forward_worker, remote_forward_ports) =
            setup_libssh_remote_forwards(opts, &ssh_config)?;

        Ok(Self {
            session,
            forward_agent,
            remote_forward_ports,
            remote_forward_worker,
        })
    }

    fn authenticated(&self) -> RemoteResult<bool> {
        Ok(self.session.is_connected())
    }

    fn banner(&self) -> RemoteResult<Option<String>> {
        self.session.get_server_banner().map(Some).map_err(|e| {
            RemoteError::new_ex(
                RemoteErrorType::ProtocolError,
                format!("Failed to get banner: {e}"),
            )
        })
    }

    fn disconnect(&self) -> RemoteResult<()> {
        if let Some(worker) = &self.remote_forward_worker {
            worker.stop();
        }
        self.session.disconnect();

        Ok(())
    }

    fn remote_forward_ports(&self) -> &[u16] {
        &self.remote_forward_ports
    }

    fn cmd<S>(&mut self, cmd: S) -> RemoteResult<(u32, String)>
    where
        S: AsRef<str>,
    {
        let output = perform_shell_cmd(
            &mut self.session,
            format!("{}; echo $?", cmd.as_ref()),
            self.forward_agent,
        )?;
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

        // open channel
        debug!("Opening channel for scp recv");
        let channel = self.session.new_channel().map_err(|err| {
            RemoteError::new_ex(
                RemoteErrorType::ProtocolError,
                format!("Could not open channel: {err}"),
            )
        })?;
        debug!("Opening channel session");
        channel.open_session().map_err(|err| {
            RemoteError::new_ex(
                RemoteErrorType::ProtocolError,
                format!("Could not open session: {err}"),
            )
        })?;
        // exec `scp -f %s`
        let cmd = format!("scp -f {}", path.display());
        channel.request_exec(cmd.as_ref()).map_err(|err| {
            RemoteError::new_ex(
                RemoteErrorType::ProtocolError,
                format!("Could not request command execution: {err}"),
            )
        })?;
        debug!("ACK with 0");
        // write \0
        channel.stdin().write_all(b"\0").map_err(|err| {
            RemoteError::new_ex(
                RemoteErrorType::ProtocolError,
                format!("Could not write to channel: {err}"),
            )
        })?;

        // read header
        debug!("Reading SCP header");
        let mut header = [0u8; 1024];
        let bytes = channel.stdout().read(&mut header).map_err(|err| {
            RemoteError::new_ex(
                RemoteErrorType::ProtocolError,
                format!("Could not read from channel: {err}"),
            )
        })?;
        // read filesize from header
        let filesize = parse_scp_header_filesize(&header[..bytes])?;
        debug!("File size: {filesize}");
        // send OK
        debug!("Sending OK");
        channel.stdin().write_all(b"\0").map_err(|err| {
            RemoteError::new_ex(
                RemoteErrorType::ProtocolError,
                format!("Could not write to channel: {err}"),
            )
        })?;

        debug!("Creating SCP recv channel");
        let reader = ScpRecvChannel {
            channel,
            filesize,
            read: 0,
        };

        Ok(Box::new(reader) as Box<dyn Read + Send>)
    }

    fn scp_send(
        &self,
        remote_path: &Path,
        mode: i32,
        size: u64,
        _times: Option<(u64, u64)>,
    ) -> RemoteResult<Box<dyn Write + Send>> {
        self.session.set_blocking(true);

        // open channel
        debug!("Opening channel for scp send");
        let channel = self.session.new_channel().map_err(|err| {
            RemoteError::new_ex(
                RemoteErrorType::ProtocolError,
                format!("Could not open channel: {err}"),
            )
        })?;
        debug!("Opening channel session");
        channel.open_session().map_err(|err| {
            RemoteError::new_ex(
                RemoteErrorType::ProtocolError,
                format!("Could not open session: {err}"),
            )
        })?;
        // exec `scp -t %s`
        let cmd = format!("scp -t {}", remote_path.display());
        channel.request_exec(cmd.as_ref()).map_err(|err| {
            RemoteError::new_ex(
                RemoteErrorType::ProtocolError,
                format!("Could not request command execution: {err}"),
            )
        })?;

        // wait for ACK
        wait_for_ack(&channel)?;

        let Some(filename) = remote_path.file_name().map(|f| f.to_string_lossy()) else {
            return Err(RemoteError::new_ex(
                RemoteErrorType::ProtocolError,
                format!("Could not get file name: {remote_path:?}"),
            ));
        };

        // send file header
        let header = format!("C{mode:04o} {size} {filename}\n", mode = mode & 0o7777,);
        debug!("Sending SCP header: {header}");
        channel
            .stdin()
            .write_all(header.as_bytes())
            .map_err(|err| {
                RemoteError::new_ex(
                    RemoteErrorType::ProtocolError,
                    format!("Could not write to channel: {err}"),
                )
            })?;

        // wait for ACK
        wait_for_ack(&channel)?;

        // return channel
        let writer = ScpSendChannel { channel };
        Ok(Box::new(writer) as Box<dyn Write + Send>)
    }

    fn sftp(&self) -> RemoteResult<Self::Sftp> {
        self.session
            .sftp()
            .map(|sftp| LibSshSftp { inner: sftp })
            .map_err(|e| RemoteError::new_ex(RemoteErrorType::ProtocolError, e))
    }
}

/// Number of bytes per SFTP read call for buffered reads.
///
/// libssh caps each `sftp_read` at the server's maximum packet payload
/// (typically 64 KiB). Using a larger request size lets the C library
/// issue fewer round-trips when possible, while still working correctly
/// when the server returns less.
const SFTP_READ_BUF_SIZE: usize = 256 * 1024;

struct SftpFileWriter(libssh_rs::SftpFile);

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

/// A seekable, in-memory read buffer wrapping file data fetched via SFTP.
struct BufferedSftpReader(Cursor<Vec<u8>>);

impl Read for BufferedSftpReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.0.read(buf)
    }
}

impl Seek for BufferedSftpReader {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        self.0.seek(pos)
    }
}

impl ReadAndSeek for BufferedSftpReader {}

impl Sftp for LibSshSftp {
    fn mkdir(&self, path: &Path, mode: i32) -> RemoteResult<()> {
        self.inner
            .create_dir(conv_path_to_str(path), mode as u32)
            .map_err(|err| {
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
        let data = buffered_sftp_read(&self.inner, path)?;
        Ok(ReadStream::from(
            Box::new(BufferedSftpReader(Cursor::new(data))) as Box<dyn ReadAndSeek>,
        ))
    }

    fn open_write(
        &self,
        path: &Path,
        flags: super::WriteMode,
        mode: i32,
    ) -> RemoteResult<WriteStream> {
        let flags = match flags {
            super::WriteMode::Append => {
                OpenFlags::WRITE_ONLY | OpenFlags::APPEND | OpenFlags::CREATE
            }
            super::WriteMode::Truncate => {
                OpenFlags::WRITE_ONLY | OpenFlags::CREATE | OpenFlags::TRUNCATE
            }
        };

        //panic!("Figa");

        self.inner
            .open(conv_path_to_str(path), flags, mode as u32)
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
            .read_dir(conv_path_to_str(dirname.as_ref()))
            .map(|files| {
                files
                    .into_iter()
                    .filter(|metadata| {
                        metadata.name() != Some(".") && metadata.name() != Some("..")
                    })
                    .map(|metadata| {
                        self.make_fsentry(MakePath::Directory(dirname.as_ref()), metadata)
                    })
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
        self.inner
            .canonicalize(conv_path_to_str(path))
            .map(PathBuf::from)
            .map_err(|err| {
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
            .rename(conv_path_to_str(src), conv_path_to_str(dest))
            .map_err(|err| {
                RemoteError::new_ex(
                    RemoteErrorType::ProtocolError,
                    format!("Could not rename file '{src}': {err}", src = src.display()),
                )
            })
    }

    fn rmdir(&self, path: &Path) -> RemoteResult<()> {
        self.inner
            .remove_dir(conv_path_to_str(path))
            .map_err(|err| {
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
            .set_metadata(conv_path_to_str(path), &Self::set_attributes(metadata))
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
            .metadata(conv_path_to_str(filename))
            .map(|metadata| self.make_fsentry(MakePath::File(filename), metadata))
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
        self.inner
            .symlink(conv_path_to_str(path), conv_path_to_str(target))
            .map_err(|err| {
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
        self.inner
            .remove_file(conv_path_to_str(path))
            .map_err(|err| {
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

fn conv_path_to_str(path: &Path) -> &str {
    path.to_str().unwrap_or_default()
}

/// Reads an entire remote file into memory using a large buffer to minimize
/// SFTP round-trips.
///
/// Each `sftp_read` call in libssh is a synchronous request-response cycle,
/// and file handles from the same session serialize through a mutex, so true
/// pipelining is not possible without the AIO FFI. Reading with a large
/// buffer (256 KiB) reduces the number of round-trips compared to the
/// default 64 KiB reads the caller would otherwise perform.
fn buffered_sftp_read(sftp: &libssh_rs::Sftp, path: &Path) -> RemoteResult<Vec<u8>> {
    let path_str = conv_path_to_str(path);

    let file_size = sftp
        .metadata(path_str)
        .map(|m| m.len().unwrap_or(0) as usize)
        .map_err(|err| {
            RemoteError::new_ex(
                RemoteErrorType::ProtocolError,
                format!("Could not stat '{path}': {err}", path = path.display()),
            )
        })?;

    if file_size == 0 {
        return Ok(Vec::new());
    }

    let mut file = sftp
        .open(path_str, OpenFlags::READ_ONLY, 0)
        .map_err(|err| {
            RemoteError::new_ex(
                RemoteErrorType::ProtocolError,
                format!(
                    "Could not open file at '{path}': {err}",
                    path = path.display()
                ),
            )
        })?;

    let mut data = Vec::with_capacity(file_size);
    let mut buf = [0_u8; SFTP_READ_BUF_SIZE];

    loop {
        let n = file.read(&mut buf).map_err(|err| {
            RemoteError::new_ex(
                RemoteErrorType::IoError,
                format!("Failed to read file '{path}': {err}", path = path.display()),
            )
        })?;
        if n == 0 {
            break;
        }
        data.extend_from_slice(&buf[..n]);
    }

    Ok(data)
}

enum MakePath<'a> {
    Directory(&'a Path),
    File(&'a Path),
}

impl LibSshSftp {
    fn set_attributes(metadata: Metadata) -> libssh_rs::SetAttributes {
        let atime = metadata.accessed.unwrap_or(UNIX_EPOCH);
        let mtime = metadata.modified.unwrap_or(UNIX_EPOCH);

        let uid_gid = match (metadata.uid, metadata.gid) {
            (Some(uid), Some(gid)) => Some((uid, gid)),
            _ => None,
        };

        libssh_rs::SetAttributes {
            size: Some(metadata.size),
            uid_gid,
            permissions: metadata.mode.map(|m| m.into()),
            atime_mtime: Some((atime, mtime)),
        }
    }

    fn make_fsentry(&self, path: MakePath<'_>, metadata: libssh_rs::Metadata) -> File {
        let name = match metadata.name() {
            None => "/".to_string(),
            Some(name) => name.to_string(),
        };
        debug!("Found file {name}");

        let path = match path {
            MakePath::Directory(dir) => dir.join(&name),
            MakePath::File(file) => file.to_path_buf(),
        };
        debug!("Computed path for {name}: {path}", path = path.display());

        // parse metadata
        let uid = metadata.uid();
        let gid = metadata.gid();
        let mode = metadata.permissions().map(UnixPex::from);
        let size = metadata.len().unwrap_or(0);
        let accessed = metadata.accessed();
        let modified = metadata.modified();
        let symlink = match metadata.file_type() {
            Some(libssh_rs::FileType::Symlink) => {
                match self.inner.read_link(conv_path_to_str(&path)) {
                    Ok(target) => Some(PathBuf::from(target)),
                    Err(err) => {
                        error!(
                            "Failed to read link of {} (even it's supposed to be a symlink): {err}",
                            path.display(),
                        );
                        None
                    }
                }
            }
            _ => None,
        };
        let file_type = if symlink.is_some() {
            FileType::Symlink
        } else if matches!(metadata.file_type(), Some(libssh_rs::FileType::Directory)) {
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

fn authenticate(session: &mut libssh_rs::Session, opts: &SshOpts) -> RemoteResult<()> {
    // parse configuration
    let ssh_config = Config::try_from(opts)?;
    let username = ssh_config.username.clone();

    debug!("Authenticating to {}", opts.host);
    session
        .set_option(SshOption::User(Some(username)))
        .map_err(|e| {
            RemoteError::new_ex(
                RemoteErrorType::AuthenticationFailed,
                format!("Failed to set username: {e}"),
            )
        })?;

    debug!("Trying with userauth_none");
    match session.userauth_none(opts.username.as_deref()) {
        Ok(AuthStatus::Success) => {
            debug!("Authenticated with userauth_none");
            return Ok(());
        }
        Ok(status) => {
            debug!("userauth_none returned status: {status:?}");
        }
        Err(err) => {
            debug!("userauth_none failed: {err}");
        }
    }

    let auth_methods = session
        .userauth_list(opts.username.as_deref())
        .map_err(|e| RemoteError::new_ex(RemoteErrorType::AuthenticationFailed, e))?;
    debug!("Available authentication methods: {auth_methods:?}");

    if ssh_config.params.pubkey_authentication.unwrap_or(true)
        && auth_methods.contains(AuthMethods::PUBLIC_KEY)
    {
        debug!("Trying public key authentication");
        // try with known key to config
        match session.userauth_public_key_auto(None, None) {
            Ok(AuthStatus::Success) => {
                debug!("Authenticated with public key");
                return Ok(());
            }
            Ok(status) => {
                debug!("userauth_public_key_auto returned status: {status:?}");
            }
            Err(err) => {
                debug!("userauth_public_key_auto failed: {err}");
            }
        }

        // try with storage
        match key_storage_auth(session, opts, &ssh_config) {
            Ok(()) => {
                debug!("Authenticated with public key from storage");
                return Ok(());
            }
            Err(err) => {
                debug!("Key storage authentication failed: {err}");
            }
        }
    }

    if auth_methods.contains(AuthMethods::PASSWORD) {
        debug!("Trying password authentication");

        // NOTE: you cannot pass password None. It causes SEGFAULT
        match session.userauth_password(None, Some(opts.password.as_deref().unwrap_or_default())) {
            Ok(AuthStatus::Success) => {
                debug!("Authenticated with password");
                return Ok(());
            }
            Ok(status) => {
                debug!("userauth_password returned status: {status:?}");
            }
            Err(err) => {
                debug!("userauth_password failed: {err}");
            }
        }
    }

    Err(RemoteError::new_ex(
        RemoteErrorType::AuthenticationFailed,
        "all authentication methods failed",
    ))
}

fn key_storage_auth(
    session: &mut libssh_rs::Session,
    opts: &SshOpts,
    ssh_config: &Config,
) -> RemoteResult<()> {
    let Some(key_storage) = &opts.key_storage else {
        return Err(RemoteError::new_ex(
            RemoteErrorType::AuthenticationFailed,
            "no key storage available",
        ));
    };

    let Some(priv_key_path) = key_storage
        .resolve(&ssh_config.host, &ssh_config.username)
        .or(key_storage.resolve(
            ssh_config.resolved_host.as_str(),
            ssh_config.username.as_str(),
        ))
    else {
        return Err(RemoteError::new_ex(
            RemoteErrorType::AuthenticationFailed,
            "no key found in storage",
        ));
    };

    let Ok(privkey) =
        SshKey::from_privkey_file(conv_path_to_str(&priv_key_path), opts.password.as_deref())
    else {
        return Err(RemoteError::new_ex(
            RemoteErrorType::AuthenticationFailed,
            format!(
                "could not load private key from file: {}",
                priv_key_path.display()
            ),
        ));
    };

    match session
        .userauth_publickey(opts.username.as_deref(), &privkey)
        .map_err(|e| RemoteError::new_ex(RemoteErrorType::AuthenticationFailed, e))
    {
        Ok(AuthStatus::Success) => Ok(()),
        Ok(status) => Err(RemoteError::new_ex(
            RemoteErrorType::AuthenticationFailed,
            format!("authentication failed: {status:?}"),
        )),
        Err(err) => Err(err),
    }
}

fn perform_shell_cmd<S: AsRef<str>>(
    session: &mut libssh_rs::Session,
    cmd: S,
    forward_agent: bool,
) -> RemoteResult<String> {
    // Create channel
    trace!("Running command: {}", cmd.as_ref());
    let channel = match session.new_channel() {
        Ok(ch) => ch,
        Err(err) => {
            return Err(RemoteError::new_ex(
                RemoteErrorType::ProtocolError,
                format!("Could not open channel: {err}"),
            ));
        }
    };

    debug!("Opening channel session");
    channel.open_session().map_err(|err| {
        RemoteError::new_ex(
            RemoteErrorType::ProtocolError,
            format!("Could not open session: {err}"),
        )
    })?;
    if forward_agent {
        channel.request_auth_agent().map_err(|err| {
            RemoteError::new_ex(
                RemoteErrorType::ProtocolError,
                format!("Could not request SSH agent forwarding: {err}"),
            )
        })?;
    }

    // escape single quotes in command
    let cmd = cmd.as_ref().replace('\'', r#"'\''"#); // close, escape, and reopen

    debug!("Requesting command execution: {cmd}",);
    channel
        .request_exec(&format!("sh -c '{cmd}'"))
        .map_err(|err| {
            RemoteError::new_ex(
                RemoteErrorType::ProtocolError,
                format!("Could not execute command \"{cmd}\": {err}"),
            )
        })?;
    // send EOF
    debug!("Sending EOF");
    channel.send_eof().map_err(|err| {
        RemoteError::new_ex(
            RemoteErrorType::ProtocolError,
            format!("Could not send EOF: {err}"),
        )
    })?;

    let output = if forward_agent {
        read_command_with_agent_forwarding(session, &channel)?
    } else {
        let mut output = String::new();
        channel
            .stdout()
            .read_to_string(&mut output)
            .map_err(|err| {
                RemoteError::new_ex(
                    RemoteErrorType::ProtocolError,
                    format!("Could not read output: {err}"),
                )
            })?;
        output
    };

    let res = channel.get_exit_status();
    trace!("Command output (res: {res:?}): {output}");
    Ok(output)
}

#[cfg(unix)]
struct AgentForwardRelay {
    channel: libssh_rs::Channel,
    socket: UnixStream,
    to_agent: Vec<u8>,
    to_remote: Vec<u8>,
    remote_eof: bool,
    local_eof: bool,
    agent_write_shutdown: bool,
    sent_eof: bool,
}

#[cfg(unix)]
impl AgentForwardRelay {
    fn connect(channel: libssh_rs::Channel) -> std::io::Result<Self> {
        let socket_path = std::env::var_os("SSH_AUTH_SOCK").ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "SSH_AUTH_SOCK is not configured",
            )
        })?;
        let socket = UnixStream::connect(socket_path)?;
        socket.set_nonblocking(true)?;
        Ok(Self {
            channel,
            socket,
            to_agent: Vec::new(),
            to_remote: Vec::new(),
            remote_eof: false,
            local_eof: false,
            agent_write_shutdown: false,
            sent_eof: false,
        })
    }

    fn pump(&mut self) -> Result<(bool, bool), String> {
        let mut progressed = false;
        let mut buffer = [0u8; 8192];

        if !self.remote_eof && self.to_agent.len() < 64 * 1024 {
            match self
                .channel
                .read_timeout(&mut buffer, false, Some(Duration::ZERO))
            {
                Ok(0) => self.remote_eof = self.channel.is_eof(),
                Ok(bytes) => {
                    self.to_agent.extend_from_slice(&buffer[..bytes]);
                    progressed = true;
                }
                Err(libssh_rs::Error::TryAgain) => {
                    self.remote_eof = self.channel.is_eof();
                }
                Err(err) => return Err(format!("Could not read forwarded agent request: {err}")),
            }
        }

        if !self.to_agent.is_empty() {
            match self.socket.write(&self.to_agent) {
                Ok(0) => return Err("Local SSH agent closed while writing".to_string()),
                Ok(bytes) => {
                    self.to_agent.drain(..bytes);
                    progressed = true;
                }
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(err) => return Err(format!("Could not write to local SSH agent: {err}")),
            }
        }

        if self.remote_eof && self.to_agent.is_empty() && !self.agent_write_shutdown {
            self.socket
                .shutdown(Shutdown::Write)
                .map_err(|err| format!("Could not half-close local SSH agent socket: {err}"))?;
            self.agent_write_shutdown = true;
            progressed = true;
        }

        if !self.local_eof && self.to_remote.len() < 64 * 1024 {
            match self.socket.read(&mut buffer) {
                Ok(0) => self.local_eof = true,
                Ok(bytes) => {
                    self.to_remote.extend_from_slice(&buffer[..bytes]);
                    progressed = true;
                }
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(err) => return Err(format!("Could not read from local SSH agent: {err}")),
            }
        }

        if !self.to_remote.is_empty() {
            match self.channel.stdin().write(&self.to_remote) {
                Ok(0) => return Err("Forwarded SSH agent channel closed while writing".to_string()),
                Ok(bytes) => {
                    self.to_remote.drain(..bytes);
                    progressed = true;
                }
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(err) => {
                    return Err(format!("Could not write forwarded agent response: {err}"));
                }
            }
        }

        if self.local_eof && self.to_remote.is_empty() && !self.sent_eof {
            match self.channel.send_eof() {
                Ok(()) => {
                    self.sent_eof = true;
                    progressed = true;
                }
                Err(libssh_rs::Error::TryAgain) => {}
                Err(err) => return Err(format!("Could not close forwarded agent channel: {err}")),
            }
        }

        let keep = !(self.remote_eof
            && self.to_agent.is_empty()
            && self.local_eof
            && self.to_remote.is_empty()
            && self.sent_eof);
        if !keep {
            let _ = self.socket.shutdown(Shutdown::Both);
        }
        Ok((keep, progressed))
    }
}

#[cfg(unix)]
fn read_command_with_agent_forwarding(
    session: &libssh_rs::Session,
    channel: &libssh_rs::Channel,
) -> RemoteResult<String> {
    session.set_blocking(false);
    let result = (|| {
        let mut output = Vec::new();
        let mut relays = Vec::<AgentForwardRelay>::new();
        let mut buffer = [0u8; 8192];

        loop {
            let mut progressed = false;
            match channel.read_timeout(&mut buffer, false, Some(Duration::ZERO)) {
                Ok(0) => {}
                Ok(bytes) => {
                    output.extend_from_slice(&buffer[..bytes]);
                    progressed = true;
                }
                Err(libssh_rs::Error::TryAgain) => {}
                Err(err) => {
                    return Err(RemoteError::new_ex(
                        RemoteErrorType::ProtocolError,
                        format!("Could not read command output: {err}"),
                    ));
                }
            }

            while let Some(agent_channel) = session.accept_agent_forward() {
                match AgentForwardRelay::connect(agent_channel) {
                    Ok(relay) => relays.push(relay),
                    Err(err) => warn!("Could not connect forwarded SSH agent channel: {err}"),
                }
                progressed = true;
            }

            let mut index = 0;
            while index < relays.len() {
                match relays[index].pump() {
                    Ok((true, relay_progressed)) => {
                        progressed |= relay_progressed;
                        index += 1;
                    }
                    Ok((false, relay_progressed)) => {
                        progressed |= relay_progressed;
                        relays.swap_remove(index);
                    }
                    Err(err) => {
                        warn!("SSH agent forwarding failed: {err}");
                        relays.swap_remove(index);
                    }
                }
            }

            if channel.is_eof() {
                break;
            }
            if !progressed {
                std::thread::sleep(Duration::from_millis(2));
            }
        }

        String::from_utf8(output).map_err(|err| {
            RemoteError::new_ex(
                RemoteErrorType::ProtocolError,
                format!("Command output is not valid UTF-8: {err}"),
            )
        })
    })();
    session.set_blocking(true);
    result
}

#[cfg(not(unix))]
fn read_command_with_agent_forwarding(
    _session: &libssh_rs::Session,
    _channel: &libssh_rs::Channel,
) -> RemoteResult<String> {
    Err(RemoteError::new_ex(
        RemoteErrorType::UnsupportedFeature,
        "SSH agent forwarding is unavailable on this platform",
    ))
}

/// Read filesize from scp header
fn parse_scp_header_filesize(header: &[u8]) -> RemoteResult<usize> {
    // Header format: C<mode> <size> <filename>\n
    let header_str = std::str::from_utf8(header).map_err(|e| {
        RemoteError::new_ex(
            RemoteErrorType::ProtocolError,
            format!("Could not parse header: {e}"),
        )
    })?;
    let parts: Vec<&str> = header_str.split_whitespace().collect();
    if parts.len() < 3 {
        return Err(RemoteError::new_ex(
            RemoteErrorType::ProtocolError,
            "Invalid SCP header: not enough parts",
        ));
    }
    if !parts[0].starts_with('C') {
        return Err(RemoteError::new_ex(
            RemoteErrorType::ProtocolError,
            "Invalid SCP header: missing 'C'",
        ));
    }
    let size = parts[1].parse::<usize>().map_err(|e| {
        RemoteError::new_ex(
            RemoteErrorType::ProtocolError,
            format!("Invalid file size: {e}"),
        )
    })?;

    Ok(size)
}

/// Wait for channel ACK
fn wait_for_ack(channel: &libssh_rs::Channel) -> RemoteResult<()> {
    debug!("Waiting for channel acknowledgment");
    // read ACK
    let mut ack = [0u8; 1024];
    let n = channel.stdout().read(&mut ack).map_err(|err| {
        RemoteError::new_ex(
            RemoteErrorType::ProtocolError,
            format!("Could not read from channel: {err}"),
        )
    })?;
    if n == 1 && ack[0] != 0 {
        Err(RemoteError::new_ex(
            RemoteErrorType::ProtocolError,
            format!("Unexpected ACK: {ack:?} (read {n} bytes)"),
        ))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use ssh2_config::ParseRule;
    use tempfile::NamedTempFile;

    use super::*;
    use crate::mock::ssh as ssh_mock;
    use crate::ssh::container::OpensshServer;

    #[test]
    fn should_connect_with_identity_file_from_ssh_config() {
        let container = OpensshServer::start();
        let key_file = ssh_mock::create_key_file();
        let config_file =
            ssh_mock::create_ssh_config_with_identity(container.port(), key_file.path());
        let opts =
            SshOpts::new("sftp").config_file(config_file.path(), ParseRule::ALLOW_UNKNOWN_FIELDS);

        let session = LibSshSession::connect(&opts)
            .expect("failed to authenticate with IdentityFile from SSH config");
        assert!(
            session
                .authenticated()
                .expect("failed to query session state")
        );
    }

    #[test]
    fn should_disable_public_key_authentication_from_ssh_config() {
        let container = OpensshServer::start();
        let key_file = ssh_mock::create_key_file();
        let config_file = ssh_mock::create_ssh_config_with_identity_and_pubkey_authentication(
            container.port(),
            key_file.path(),
            false,
        );
        let opts =
            SshOpts::new("sftp").config_file(config_file.path(), ParseRule::ALLOW_UNKNOWN_FIELDS);

        assert!(LibSshSession::connect(&opts).is_err());

        let opts = SshOpts::new("sftp")
            .config_file(config_file.path(), ParseRule::ALLOW_UNKNOWN_FIELDS)
            .password("password");
        let session =
            LibSshSession::connect(&opts).expect("password authentication should remain enabled");
        assert!(
            session
                .authenticated()
                .expect("failed to query session state")
        );
    }

    #[test]
    fn should_prefer_explicit_port_over_ssh_config() {
        let container = OpensshServer::start();
        let port = container.port();
        let mut config_file = NamedTempFile::new().expect("failed to create SSH config");
        writeln!(
            config_file,
            "Host sftp\n    HostName 127.0.0.1\n    Port 1\n    User sftp"
        )
        .expect("failed to write SSH config");
        let opts = SshOpts::new("sftp")
            .port(port)
            .username("sftp")
            .password("password")
            .config_file(config_file.path(), ParseRule::STRICT);

        let session = LibSshSession::connect(&opts)
            .expect("the explicit port should override the SSH configuration");
        session.disconnect().expect("failed to disconnect");
    }

    #[test]
    fn should_apply_explicit_connection_timeout() {
        let (port, server) = ssh_mock::start_unresponsive_server(Duration::from_secs(2));
        let mut config_file = NamedTempFile::new().expect("failed to create SSH config");
        writeln!(
            config_file,
            "Host unresponsive\n    HostName 127.0.0.1\n    Port {port}\n    ConnectTimeout 5"
        )
        .expect("failed to write SSH config");
        let opts = SshOpts::new("unresponsive")
            .config_file(config_file.path(), ParseRule::STRICT)
            .connection_timeout(Duration::from_millis(100));

        let started = Instant::now();
        let result = LibSshSession::connect(&opts);
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
        let container = OpensshServer::start();
        let (port, proxy) = ssh_mock::start_flaky_proxy(container.port(), 1);
        let mut config_file = NamedTempFile::new().expect("failed to create SSH config");
        writeln!(
            config_file,
            "Host flaky\n    HostName 127.0.0.1\n    Port {port}\n    User sftp\n    ConnectionAttempts 2"
        )
        .expect("failed to write SSH config");
        let opts = SshOpts::new("flaky")
            .config_file(config_file.path(), ParseRule::STRICT)
            .password("password");

        let session = LibSshSession::connect(&opts)
            .expect("connection should succeed on the configured retry");
        session.disconnect().expect("failed to disconnect");
        drop(session);
        proxy.join().expect("test proxy panicked");
    }

    #[test]
    fn should_apply_configured_bind_interface() {
        let container = OpensshServer::start();
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
        let session = LibSshSession::connect(&opts)
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
        assert!(LibSshSession::connect(&opts).is_err());

        let mut precedence_config = NamedTempFile::new().expect("failed to create SSH config");
        writeln!(
            precedence_config,
            "Host bound\n    HostName 127.0.0.1\n    Port {port}\n    User sftp\n    BindAddress 127.0.0.1\n    BindInterface remotefs-ssh-missing-interface"
        )
        .expect("failed to write SSH config");
        let opts = SshOpts::new("bound")
            .config_file(precedence_config.path(), ParseRule::STRICT)
            .password("password");
        let session = LibSshSession::connect(&opts)
            .expect("BindAddress should take precedence over BindInterface");
        session.disconnect().expect("failed to disconnect");
    }

    #[test]
    fn should_apply_configured_tcp_keep_alive() {
        let container = OpensshServer::start();
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
            let session = LibSshSession::connect(&opts).expect("failed to connect");

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
        let session = LibSshSession::connect(&opts).expect("failed to connect");
        assert!(
            crate::ssh::backend::socket::keepalive(&session.session)
                .expect("failed to read default SO_KEEPALIVE")
        );
        session.disconnect().expect("failed to disconnect");
    }

    #[test]
    #[cfg(unix)]
    fn should_forward_configured_ssh_agent() {
        let agent = ssh_mock::TestSshAgent::start();
        let key_file = ssh_mock::create_key_file();
        agent.add_key(key_file.path());
        let container = OpensshServer::start();
        let mut config_file = NamedTempFile::new().expect("failed to create SSH config");
        writeln!(
            config_file,
            "Host forwarded\n    HostName 127.0.0.1\n    Port {port}\n    User sftp\n    ForwardAgent yes",
            port = container.port(),
        )
        .expect("failed to write SSH config");
        let opts = SshOpts::new("forwarded")
            .config_file(config_file.path(), ParseRule::STRICT)
            .password("password");
        let mut session = LibSshSession::connect(&opts).expect("failed to connect");

        let (status, output) = session
            .cmd("ssh-add -L")
            .expect("failed to query remote agent");
        assert_eq!(status, 0, "remote ssh-add failed: {output}");
        assert!(output.contains("ssh-rsa"));
    }

    #[test]
    fn should_apply_configured_remote_forward() {
        crate::mock::logger();
        let container = OpensshServer::start_with_tcp_forwarding();
        let (destination_port, destination) = ssh_mock::start_tcp_echo_server();
        let (dynamic_destination_port, dynamic_destination) = ssh_mock::start_tcp_echo_server();
        let remote_port = 43002;
        let mut config_file = NamedTempFile::new().expect("failed to create SSH config");
        writeln!(
            config_file,
            "Host forwarded\n    HostName 127.0.0.1\n    Port {port}\n    User sftp\n    RemoteForward 127.0.0.1:{remote_port} 127.0.0.1:{destination_port}\n    RemoteForward 0",
            port = container.port(),
        )
        .expect("failed to write SSH config");
        let opts = SshOpts::new("forwarded")
            .config_file(config_file.path(), ParseRule::STRICT)
            .password("password");
        let mut session = LibSshSession::connect(&opts).expect("failed to connect");
        let dynamic_port = *session
            .remote_forward_ports()
            .get(1)
            .expect("dynamic RemoteForward did not report its assigned port");

        let (status, output) = session
            .cmd(format!("printf ping | nc -w 5 127.0.0.1 {remote_port}"))
            .expect("failed to use configured remote forward");
        assert_eq!(status, 0, "remote forwarding command failed: {output}");
        assert_eq!(output, "pong\n");
        destination.join().expect("TCP echo server panicked");

        let (status, output) = session
            .cmd(ssh_mock::socks5_test_command(
                dynamic_port,
                dynamic_destination_port,
            ))
            .expect("failed to use dynamic RemoteForward");
        assert_eq!(status, 0, "dynamic forwarding command failed: {output}");
        assert_eq!(output, "pong\n");
        dynamic_destination
            .join()
            .expect("dynamic TCP echo server panicked");
    }

    #[test]
    #[cfg(unix)]
    fn should_apply_remote_forward_to_unix_socket() {
        let container = OpensshServer::start_with_tcp_forwarding();
        let (_directory, destination_path, destination) = ssh_mock::start_unix_echo_server();
        let remote_port = 43202;
        let mut config_file = NamedTempFile::new().expect("failed to create SSH config");
        writeln!(
            config_file,
            "Host forwarded\n    HostName 127.0.0.1\n    Port {port}\n    User sftp\n    RemoteForward 127.0.0.1:{remote_port} {destination}",
            port = container.port(),
            destination = destination_path.display(),
        )
        .expect("failed to write SSH config");
        let opts = SshOpts::new("forwarded")
            .config_file(config_file.path(), ParseRule::STRICT)
            .password("password");
        let mut session = LibSshSession::connect(&opts).expect("failed to connect");

        let (status, output) = session
            .cmd(format!("printf ping | nc -w 5 127.0.0.1 {remote_port}"))
            .expect("failed to use Unix destination RemoteForward");
        assert_eq!(status, 0, "remote forwarding command failed: {output}");
        assert_eq!(output, "pong\n");
        destination.join().expect("Unix echo server panicked");
    }
}
