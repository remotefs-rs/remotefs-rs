use std::io::{Read, Write};
use std::net::{Shutdown, TcpStream, ToSocketAddrs as _};
#[cfg(unix)]
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, TrySendError};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use ssh2_config::{RemoteForwardDestination, RemoteForwardListen};

pub(super) const MAX_FORWARD_CONNECTIONS: usize = 64;
const CONNECTOR_THREADS: usize = 8;
const CONNECTOR_QUEUE_CAPACITY: usize = 64;

pub(super) fn tcp_remote_forward_endpoint(
    listen: &RemoteForwardListen,
) -> Option<(Option<&str>, u16)> {
    match listen {
        RemoteForwardListen::Port(port) => Some((Some("localhost"), *port)),
        RemoteForwardListen::Host { host, port } => {
            let host = if host.is_empty() || host == "*" {
                None
            } else {
                Some(host.as_str())
            };
            Some((host, *port))
        }
        RemoteForwardListen::UnixSocket(_) => None,
    }
}

pub(super) trait ChannelIo {
    fn read_channel(&mut self, buffer: &mut [u8]) -> std::io::Result<usize>;
    fn write_channel(&mut self, buffer: &[u8]) -> std::io::Result<usize>;
    fn is_eof(&self) -> bool;
    fn send_eof(&mut self) -> std::io::Result<()>;
    fn close(&mut self);
}

enum LocalStream {
    Tcp(TcpStream),
    #[cfg(unix)]
    Unix(UnixStream),
}

impl LocalStream {
    fn connect(
        destination: &RemoteForwardDestination,
        timeout: Duration,
        cancellation: &ConnectionCancellation,
    ) -> std::io::Result<Self> {
        match destination {
            RemoteForwardDestination::Host { host, port } => {
                let addresses = (host.as_str(), *port)
                    .to_socket_addrs()?
                    .collect::<Vec<_>>();
                let mut last_error = None;
                let deadline = (!timeout.is_zero()).then(|| Instant::now() + timeout);
                loop {
                    let mut timed_out = false;
                    for address in &addresses {
                        if cancellation.is_cancelled() {
                            return Err(std::io::Error::new(
                                std::io::ErrorKind::Interrupted,
                                "RemoteForward destination connection was cancelled",
                            ));
                        }
                        let attempt_timeout = deadline.map_or(Duration::from_millis(100), |end| {
                            end.saturating_duration_since(Instant::now())
                                .min(Duration::from_millis(100))
                        });
                        if attempt_timeout.is_zero() {
                            return Err(last_error.unwrap_or_else(|| {
                                std::io::Error::new(
                                    std::io::ErrorKind::TimedOut,
                                    "RemoteForward destination connect timed out",
                                )
                            }));
                        }
                        match TcpStream::connect_timeout(address, attempt_timeout) {
                            Ok(stream) => {
                                stream.set_nonblocking(true)?;
                                return Ok(Self::Tcp(stream));
                            }
                            Err(err)
                                if matches!(
                                    err.kind(),
                                    std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                                ) =>
                            {
                                timed_out = true;
                                last_error = Some(err);
                            }
                            Err(err) => last_error = Some(err),
                        }
                    }
                    if !timed_out || deadline.is_some_and(|end| Instant::now() >= end) {
                        break;
                    }
                }
                Err(last_error.unwrap_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::AddrNotAvailable,
                        format!("no address found for RemoteForward destination {host}:{port}"),
                    )
                }))
            }
            RemoteForwardDestination::UnixSocket(path) => {
                if cancellation.is_cancelled() {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::Interrupted,
                        "RemoteForward destination connection was cancelled",
                    ));
                }
                connect_unix(path, timeout, cancellation)
            }
        }
    }

    fn shutdown(&self, how: Shutdown) -> std::io::Result<()> {
        match self {
            Self::Tcp(stream) => stream.shutdown(how),
            #[cfg(unix)]
            Self::Unix(stream) => stream.shutdown(how),
        }
    }
}

struct PendingConnection {
    cancelled: Arc<AtomicBool>,
    receiver: Receiver<std::io::Result<LocalStream>>,
}

impl Drop for PendingConnection {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
    }
}

struct ConnectionCancellation {
    connection: Arc<AtomicBool>,
    session: Arc<AtomicBool>,
}

impl ConnectionCancellation {
    fn is_cancelled(&self) -> bool {
        self.connection.load(Ordering::Acquire) || self.session.load(Ordering::Acquire)
    }
}

struct ConnectionJob {
    cancellation: ConnectionCancellation,
    destination: RemoteForwardDestination,
    timeout: Duration,
    result: SyncSender<std::io::Result<LocalStream>>,
}

fn connector() -> &'static SyncSender<ConnectionJob> {
    static CONNECTOR: OnceLock<SyncSender<ConnectionJob>> = OnceLock::new();
    CONNECTOR.get_or_init(|| {
        let (sender, receiver) =
            std::sync::mpsc::sync_channel::<ConnectionJob>(CONNECTOR_QUEUE_CAPACITY);
        let receiver = Arc::new(Mutex::new(receiver));
        for _ in 0..CONNECTOR_THREADS {
            let receiver = Arc::clone(&receiver);
            std::thread::spawn(move || {
                loop {
                    let job = receiver
                        .lock()
                        .expect("RemoteForward connector queue poisoned")
                        .recv();
                    let Ok(job) = job else {
                        break;
                    };
                    if job.cancellation.is_cancelled() {
                        continue;
                    }
                    let result =
                        LocalStream::connect(&job.destination, job.timeout, &job.cancellation);
                    let _ = job.result.send(result);
                }
            });
        }
        sender
    })
}

impl PendingConnection {
    fn spawn(
        destination: RemoteForwardDestination,
        timeout: Duration,
        session_cancelled: Arc<AtomicBool>,
    ) -> std::io::Result<Self> {
        let (result, receiver) = std::sync::mpsc::sync_channel(1);
        let cancelled = Arc::new(AtomicBool::new(false));
        connector()
            .try_send(ConnectionJob {
                cancellation: ConnectionCancellation {
                    connection: Arc::clone(&cancelled),
                    session: session_cancelled,
                },
                destination,
                timeout,
                result,
            })
            .map_err(|err| match err {
                TrySendError::Full(_) => std::io::Error::new(
                    std::io::ErrorKind::WouldBlock,
                    "RemoteForward destination connector queue is full",
                ),
                TrySendError::Disconnected(_) => std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "RemoteForward destination connector stopped unexpectedly",
                ),
            })?;
        Ok(Self {
            cancelled,
            receiver,
        })
    }

    fn poll(&self) -> std::io::Result<Option<LocalStream>> {
        match self.receiver.try_recv() {
            Ok(result) => result.map(Some),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => Err(std::io::Error::new(
                std::io::ErrorKind::ConnectionAborted,
                "RemoteForward destination connector stopped unexpectedly",
            )),
        }
    }
}

impl Read for LocalStream {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::Tcp(stream) => stream.read(buffer),
            #[cfg(unix)]
            Self::Unix(stream) => stream.read(buffer),
        }
    }
}

impl Write for LocalStream {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        match self {
            Self::Tcp(stream) => stream.write(buffer),
            #[cfg(unix)]
            Self::Unix(stream) => stream.write(buffer),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(unix)]
fn connect_unix(
    path: &Path,
    _timeout: Duration,
    _cancellation: &ConnectionCancellation,
) -> std::io::Result<LocalStream> {
    let stream = UnixStream::connect(path)?;
    stream.set_nonblocking(true)?;
    Ok(LocalStream::Unix(stream))
}

#[cfg(not(unix))]
fn connect_unix(
    path: &Path,
    _timeout: Duration,
    _cancellation: &ConnectionCancellation,
) -> std::io::Result<LocalStream> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        format!(
            "Unix socket RemoteForward destination {} is unavailable on this platform",
            path.display()
        ),
    ))
}

pub(super) struct ForwardConnection<C: ChannelIo> {
    channel: C,
    state: ConnectionState,
}

enum ConnectionState {
    Connecting(PendingConnection),
    Socks(SocksHandshake),
    Relay(RelayState),
}

struct SocksHandshake {
    phase: SocksPhase,
    input: Vec<u8>,
    output: Vec<u8>,
    destination: Option<LocalStream>,
    pending_connection: Option<PendingConnection>,
    pending_version: Option<u8>,
    failed: bool,
    connect_timeout: Duration,
    session_cancelled: Arc<AtomicBool>,
}

enum SocksPhase {
    Greeting,
    Socks5Request,
    Connecting,
    Reply,
}

struct RelayState {
    destination: LocalStream,
    to_destination: Vec<u8>,
    to_remote: Vec<u8>,
    remote_eof: bool,
    local_eof: bool,
    local_write_shutdown: bool,
    remote_eof_sent: bool,
}

impl<C> ForwardConnection<C>
where
    C: ChannelIo,
{
    pub(super) fn fixed(
        channel: C,
        destination: &RemoteForwardDestination,
        timeout: Duration,
        session_cancelled: Arc<AtomicBool>,
    ) -> std::io::Result<Self> {
        Ok(Self {
            channel,
            state: ConnectionState::Connecting(PendingConnection::spawn(
                destination.clone(),
                timeout,
                session_cancelled,
            )?),
        })
    }

    pub(super) fn dynamic(
        channel: C,
        connect_timeout: Duration,
        session_cancelled: Arc<AtomicBool>,
    ) -> Self {
        Self {
            channel,
            state: ConnectionState::Socks(SocksHandshake {
                phase: SocksPhase::Greeting,
                input: Vec::new(),
                output: Vec::new(),
                destination: None,
                pending_connection: None,
                pending_version: None,
                failed: false,
                connect_timeout,
                session_cancelled,
            }),
        }
    }

    pub(super) fn pump(&mut self) -> std::io::Result<(bool, bool)> {
        let transition = match &mut self.state {
            ConnectionState::Connecting(connection) => match connection.poll()? {
                Some(destination) => {
                    self.state = ConnectionState::Relay(RelayState::new(destination));
                    return Ok((true, true));
                }
                None => return Ok((true, false)),
            },
            ConnectionState::Socks(handshake) => handshake.pump(&mut self.channel)?,
            ConnectionState::Relay(relay) => return relay.pump(&mut self.channel),
        };
        match transition {
            SocksTransition::Pending(progressed) => Ok((true, progressed)),
            SocksTransition::Connected(destination, initial_data) => {
                let mut relay = RelayState::new(destination);
                relay.to_destination = initial_data;
                self.state = ConnectionState::Relay(relay);
                Ok((true, true))
            }
            SocksTransition::Failed => Ok((false, true)),
        }
    }
}

impl<C> Drop for ForwardConnection<C>
where
    C: ChannelIo,
{
    fn drop(&mut self) {
        self.channel.close();
    }
}

impl RelayState {
    fn new(destination: LocalStream) -> Self {
        Self {
            destination,
            to_destination: Vec::new(),
            to_remote: Vec::new(),
            remote_eof: false,
            local_eof: false,
            local_write_shutdown: false,
            remote_eof_sent: false,
        }
    }

    fn pump<C>(&mut self, channel: &mut C) -> std::io::Result<(bool, bool)>
    where
        C: ChannelIo,
    {
        let mut progressed = false;
        let mut buffer = [0u8; 8192];

        if !self.remote_eof && self.to_destination.len() < 64 * 1024 {
            match channel.read_channel(&mut buffer) {
                Ok(0) => self.remote_eof = channel.is_eof(),
                Ok(bytes) => {
                    self.to_destination.extend_from_slice(&buffer[..bytes]);
                    progressed = true;
                }
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                    self.remote_eof = channel.is_eof();
                }
                Err(err) => return Err(err),
            }
        }
        if !self.to_destination.is_empty() {
            match self.destination.write(&self.to_destination) {
                Ok(0) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::WriteZero,
                        "RemoteForward destination closed while writing",
                    ));
                }
                Ok(bytes) => {
                    self.to_destination.drain(..bytes);
                    progressed = true;
                }
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(err) => return Err(err),
            }
        }
        if self.remote_eof && self.to_destination.is_empty() && !self.local_write_shutdown {
            self.destination.shutdown(Shutdown::Write)?;
            self.local_write_shutdown = true;
            progressed = true;
        }

        if !self.local_eof && self.to_remote.len() < 64 * 1024 {
            match self.destination.read(&mut buffer) {
                Ok(0) => self.local_eof = true,
                Ok(bytes) => {
                    self.to_remote.extend_from_slice(&buffer[..bytes]);
                    progressed = true;
                }
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(err) => return Err(err),
            }
        }
        if !self.to_remote.is_empty() {
            match channel.write_channel(&self.to_remote) {
                Ok(0) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::WriteZero,
                        "RemoteForward channel closed while writing",
                    ));
                }
                Ok(bytes) => {
                    self.to_remote.drain(..bytes);
                    progressed = true;
                }
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(err) => return Err(err),
            }
        }
        if self.local_eof && self.to_remote.is_empty() && !self.remote_eof_sent {
            match channel.send_eof() {
                Ok(()) => {
                    self.remote_eof_sent = true;
                    progressed = true;
                }
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(err) => return Err(err),
            }
        }

        let keep = !(self.remote_eof
            && self.local_write_shutdown
            && self.local_eof
            && self.to_remote.is_empty()
            && self.remote_eof_sent);
        Ok((keep, progressed))
    }
}

enum SocksTransition {
    Pending(bool),
    Connected(LocalStream, Vec<u8>),
    Failed,
}

impl SocksHandshake {
    fn pump<C>(&mut self, channel: &mut C) -> std::io::Result<SocksTransition>
    where
        C: ChannelIo,
    {
        let mut progressed = false;
        if !self.output.is_empty() {
            match channel.write_channel(&self.output) {
                Ok(0) => return Ok(SocksTransition::Failed),
                Ok(bytes) => {
                    self.output.drain(..bytes);
                    progressed = true;
                }
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(err) => return Err(err),
            }
        }
        if matches!(self.phase, SocksPhase::Reply) && self.output.is_empty() {
            if self.failed {
                return Ok(SocksTransition::Failed);
            }
            return Ok(SocksTransition::Connected(
                self.destination
                    .take()
                    .expect("successful SOCKS reply has a destination"),
                std::mem::take(&mut self.input),
            ));
        }

        if matches!(self.phase, SocksPhase::Connecting) {
            let Some(connection) = &self.pending_connection else {
                unreachable!("connecting SOCKS handshake has a pending destination");
            };
            let result = match connection.poll() {
                Ok(Some(stream)) => Ok(stream),
                Ok(None) => return Ok(SocksTransition::Pending(progressed)),
                Err(err) => Err(err),
            };
            let version = self
                .pending_version
                .take()
                .expect("connecting SOCKS handshake has a protocol version");
            self.pending_connection = None;
            match result {
                Ok(stream) => {
                    self.destination = Some(stream);
                    if version == 4 {
                        self.output.extend_from_slice(&[0, 90, 0, 0, 0, 0, 0, 0]);
                    } else {
                        self.output
                            .extend_from_slice(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0]);
                    }
                }
                Err(err) => {
                    warn!("Dynamic RemoteForward destination failed: {err}");
                    self.failed = true;
                    if version == 4 {
                        self.output.extend_from_slice(&[0, 91, 0, 0, 0, 0, 0, 0]);
                    } else {
                        self.output
                            .extend_from_slice(&[5, 1, 0, 1, 0, 0, 0, 0, 0, 0]);
                    }
                }
            }
            self.phase = SocksPhase::Reply;
            return Ok(SocksTransition::Pending(true));
        }

        let mut buffer = [0u8; 1024];
        match channel.read_channel(&mut buffer) {
            Ok(0) if channel.is_eof() => return Ok(SocksTransition::Failed),
            Ok(0) => {}
            Ok(bytes) => {
                if self.input.len() + bytes > 4096 {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "SOCKS handshake exceeds 4096 bytes",
                    ));
                }
                self.input.extend_from_slice(&buffer[..bytes]);
                progressed = true;
            }
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(err) => return Err(err),
        }

        match self.phase {
            SocksPhase::Greeting => self.parse_greeting()?,
            SocksPhase::Socks5Request => self.parse_socks5_request()?,
            SocksPhase::Connecting | SocksPhase::Reply => {}
        }
        Ok(SocksTransition::Pending(progressed))
    }

    fn parse_greeting(&mut self) -> std::io::Result<()> {
        let Some(version) = self.input.first().copied() else {
            return Ok(());
        };
        match version {
            4 => match parse_socks4_request(&self.input) {
                Ok(Some((consumed, host, port))) => {
                    self.input.drain(..consumed);
                    self.connect(host, port, 4);
                }
                Ok(None) => {}
                Err(err) => {
                    warn!("Dynamic RemoteForward rejected SOCKS4 request: {err}");
                    self.output.extend_from_slice(&[0, 91, 0, 0, 0, 0, 0, 0]);
                    self.failed = true;
                    self.phase = SocksPhase::Reply;
                }
            },
            5 => {
                let Some(method_count) = self.input.get(1).copied().map(usize::from) else {
                    return Ok(());
                };
                let length = 2 + method_count;
                if self.input.len() < length {
                    return Ok(());
                }
                let supports_no_auth = self.input[2..length].contains(&0);
                self.input.drain(..length);
                if supports_no_auth {
                    self.output.extend_from_slice(&[5, 0]);
                    self.phase = SocksPhase::Socks5Request;
                } else {
                    self.output.extend_from_slice(&[5, 0xff]);
                    self.failed = true;
                    self.phase = SocksPhase::Reply;
                }
            }
            _ => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("unsupported SOCKS version {version}"),
                ));
            }
        }
        Ok(())
    }

    fn parse_socks5_request(&mut self) -> std::io::Result<()> {
        match parse_socks5_request(&self.input) {
            Ok(Some((consumed, host, port))) => {
                self.input.drain(..consumed);
                self.connect(host, port, 5);
            }
            Ok(None) => {}
            Err(err) => {
                warn!("Dynamic RemoteForward rejected SOCKS5 request: {err}");
                self.output
                    .extend_from_slice(&[5, 7, 0, 1, 0, 0, 0, 0, 0, 0]);
                self.failed = true;
                self.phase = SocksPhase::Reply;
            }
        }
        Ok(())
    }

    fn connect(&mut self, host: String, port: u16, version: u8) {
        let destination = RemoteForwardDestination::Host { host, port };
        match PendingConnection::spawn(
            destination,
            self.connect_timeout,
            Arc::clone(&self.session_cancelled),
        ) {
            Ok(connection) => {
                self.pending_connection = Some(connection);
                self.pending_version = Some(version);
                self.phase = SocksPhase::Connecting;
            }
            Err(err) => {
                warn!("Dynamic RemoteForward destination could not be queued: {err}");
                self.failed = true;
                if version == 4 {
                    self.output.extend_from_slice(&[0, 91, 0, 0, 0, 0, 0, 0]);
                } else {
                    self.output
                        .extend_from_slice(&[5, 1, 0, 1, 0, 0, 0, 0, 0, 0]);
                }
                self.phase = SocksPhase::Reply;
            }
        }
    }
}

fn parse_socks4_request(input: &[u8]) -> std::io::Result<Option<(usize, String, u16)>> {
    if input.len() < 9 {
        return Ok(None);
    }
    if input[1] != 1 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "SOCKS4 supports CONNECT only",
        ));
    }
    let Some(user_end) = input[8..].iter().position(|byte| *byte == 0) else {
        return Ok(None);
    };
    let mut consumed = 8 + user_end + 1;
    let address = [input[4], input[5], input[6], input[7]];
    let host = if address[..3] == [0, 0, 0] && address[3] != 0 {
        let Some(domain_end) = input[consumed..].iter().position(|byte| *byte == 0) else {
            return Ok(None);
        };
        let domain = String::from_utf8(input[consumed..consumed + domain_end].to_vec())
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err.to_string()))?;
        consumed += domain_end + 1;
        domain
    } else {
        std::net::Ipv4Addr::from(address).to_string()
    };
    Ok(Some((
        consumed,
        host,
        u16::from_be_bytes([input[2], input[3]]),
    )))
}

fn parse_socks5_request(input: &[u8]) -> std::io::Result<Option<(usize, String, u16)>> {
    if input.len() < 4 {
        return Ok(None);
    }
    if input[0] != 5 || input[1] != 1 || input[2] != 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "SOCKS5 supports CONNECT only",
        ));
    }
    let (address_end, host) = match input[3] {
        1 => {
            if input.len() < 10 {
                return Ok(None);
            }
            (
                8,
                std::net::Ipv4Addr::new(input[4], input[5], input[6], input[7]).to_string(),
            )
        }
        3 => {
            let Some(length) = input.get(4).copied().map(usize::from) else {
                return Ok(None);
            };
            let address_end = 5 + length;
            if input.len() < address_end + 2 {
                return Ok(None);
            }
            let domain = String::from_utf8(input[5..address_end].to_vec()).map_err(|err| {
                std::io::Error::new(std::io::ErrorKind::InvalidData, err.to_string())
            })?;
            (address_end, domain)
        }
        4 => {
            if input.len() < 22 {
                return Ok(None);
            }
            let mut address = [0u8; 16];
            address.copy_from_slice(&input[4..20]);
            (20, std::net::Ipv6Addr::from(address).to_string())
        }
        address_type => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                format!("unsupported SOCKS5 address type {address_type}"),
            ));
        }
    };
    let port_end = address_end + 2;
    if input.len() < port_end {
        return Ok(None);
    }
    Ok(Some((
        port_end,
        host,
        u16::from_be_bytes([input[address_end], input[address_end + 1]]),
    )))
}

pub(super) struct ForwardWorker {
    cancelled: Arc<AtomicBool>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl ForwardWorker {
    pub(super) fn new(cancelled: Arc<AtomicBool>, worker: JoinHandle<()>) -> Self {
        Self {
            cancelled,
            worker: Mutex::new(Some(worker)),
        }
    }

    pub(super) fn stop(&self) {
        self.cancelled.store(true, Ordering::Release);
        let worker = self
            .worker
            .lock()
            .expect("RemoteForward worker lock poisoned")
            .take();
        if let Some(worker) = worker
            && worker.join().is_err()
        {
            warn!("RemoteForward worker panicked");
        }
    }
}

impl Drop for ForwardWorker {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use ssh2_config::RemoteForwardListen;

    use super::{
        ConnectionCancellation, LocalStream, PendingConnection, RemoteForwardDestination,
        parse_socks4_request, parse_socks5_request, tcp_remote_forward_endpoint,
    };

    #[test]
    fn should_cancel_pending_destination_without_configured_timeout() {
        let session_cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let connection = PendingConnection::spawn(
            RemoteForwardDestination::Host {
                host: "203.0.113.1".to_string(),
                port: 9,
            },
            Duration::ZERO,
            std::sync::Arc::clone(&session_cancelled),
        )
        .expect("failed to queue destination connection");
        session_cancelled.store(true, std::sync::atomic::Ordering::Release);
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            match connection.poll() {
                Ok(None) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(_) => break,
                Ok(Some(_)) => panic!("cancelled destination unexpectedly connected"),
                Ok(None) => panic!("cancelled destination did not stop promptly"),
            }
        }
    }

    #[test]
    fn should_connect_destination_without_timeout() {
        let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .expect("failed to bind destination listener");
        let port = listener
            .local_addr()
            .expect("failed to inspect destination listener")
            .port();
        let destination = RemoteForwardDestination::Host {
            host: "127.0.0.1".to_string(),
            port,
        };
        let _stream = LocalStream::connect(
            &destination,
            Duration::ZERO,
            &ConnectionCancellation {
                connection: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
                session: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            },
        )
        .expect("zero timeout should allow a blocking destination connection");
        listener
            .accept()
            .expect("failed to accept destination connection");
    }

    #[test]
    fn should_normalize_remote_forward_bind_addresses() {
        assert_eq!(
            tcp_remote_forward_endpoint(&RemoteForwardListen::Port(8080)),
            Some((Some("localhost"), 8080))
        );
        for host in ["", "*"] {
            assert_eq!(
                tcp_remote_forward_endpoint(&RemoteForwardListen::Host {
                    host: host.to_string(),
                    port: 8080,
                }),
                Some((None, 8080))
            );
        }
        assert_eq!(
            tcp_remote_forward_endpoint(&RemoteForwardListen::Host {
                host: "127.0.0.1".to_string(),
                port: 8080,
            }),
            Some((Some("127.0.0.1"), 8080))
        );
    }

    #[test]
    fn should_parse_socks4_and_socks4a_connect_requests() {
        assert_eq!(
            parse_socks4_request(&[4, 1, 0, 80, 127, 0, 0, 1, 0])
                .expect("failed to parse SOCKS4")
                .expect("SOCKS4 request is incomplete"),
            (9, "127.0.0.1".to_string(), 80)
        );
        let request = [
            4, 1, 0, 22, 0, 0, 0, 1, 0, b'e', b'x', b'a', b'm', b'p', b'l', b'e', 0,
        ];
        assert_eq!(
            parse_socks4_request(&request)
                .expect("failed to parse SOCKS4a")
                .expect("SOCKS4a request is incomplete"),
            (request.len(), "example".to_string(), 22)
        );
    }

    #[test]
    fn should_parse_socks5_connect_address_variants() {
        assert_eq!(
            parse_socks5_request(&[5, 1, 0, 1, 127, 0, 0, 1, 0, 80])
                .expect("failed to parse SOCKS5 IPv4")
                .expect("SOCKS5 IPv4 request is incomplete"),
            (10, "127.0.0.1".to_string(), 80)
        );
        let domain = [
            5, 1, 0, 3, 7, b'e', b'x', b'a', b'm', b'p', b'l', b'e', 0, 22,
        ];
        assert_eq!(
            parse_socks5_request(&domain)
                .expect("failed to parse SOCKS5 domain")
                .expect("SOCKS5 domain request is incomplete"),
            (domain.len(), "example".to_string(), 22)
        );
        let mut ipv6 = vec![5, 1, 0, 4];
        ipv6.extend_from_slice(&std::net::Ipv6Addr::LOCALHOST.octets());
        ipv6.extend_from_slice(&443_u16.to_be_bytes());
        assert_eq!(
            parse_socks5_request(&ipv6)
                .expect("failed to parse SOCKS5 IPv6")
                .expect("SOCKS5 IPv6 request is incomplete"),
            (ipv6.len(), "::1".to_string(), 443)
        );
    }

    #[test]
    fn should_wait_for_complete_socks_requests() {
        assert!(
            parse_socks4_request(&[4, 1, 0, 80, 127, 0, 0, 1])
                .expect("failed to inspect partial SOCKS4")
                .is_none()
        );
        assert!(
            parse_socks5_request(&[5, 1, 0, 3, 7, b'e'])
                .expect("failed to inspect partial SOCKS5")
                .is_none()
        );
    }
}
