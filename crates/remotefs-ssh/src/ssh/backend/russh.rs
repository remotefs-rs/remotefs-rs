//! [russh](https://docs.rs/russh/latest/russh/) backend for `remotefs-ssh`.

mod auth;
mod scp;
mod stream;

use std::borrow::Cow;
use std::future::Future as _;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};
use std::task::{Context, Poll};

use remotefs::fs::{
    AsyncReadStream, AsyncWriteStream, FileType, Metadata, ReadOptions, SetMetadata, UnixPex,
};
use remotefs::{File, RemoteError, RemoteErrorType, RemoteResult};
use russh::client::{ChannelOpenHandle, DisconnectReason, Handle, Handler, Msg, Session};
use russh::keys::{Algorithm, PublicKey, PublicKeyOrCertificate};
use russh::{Channel, ChannelId, ChannelOpenFailure, Disconnect, Sig, client};
use russh_sftp::client::SftpSession;
use ssh2_config::{RemoteForwardDestination, RemoteForwardListen};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpSocket, TcpStream};
use tokio::time::{Instant, Sleep};

use super::{MAX_FORWARD_CONNECTIONS, WriteMode, interface, socket};
use crate::SshOpts;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_limit_concurrent_forwarded_channels() {
        let state = Arc::new(RemoteForwardState::default());
        state.active.store(true, Ordering::Release);
        let permits: Vec<_> = (0..MAX_FORWARD_CONNECTIONS)
            .map(|_| state.try_acquire_channel().expect("permit"))
            .collect();
        assert!(state.try_acquire_channel().is_none());
        drop(permits);
        assert!(state.try_acquire_channel().is_some());
    }
}

/// Transparent handler wrapper enforcing configured CA signature algorithms.
struct CaSignaturePolicyHandler<T> {
    inner: T,
    allowed_algorithms: Vec<String>,
    forward_agent: bool,
    remote_forwards: Arc<RemoteForwardState>,
}

impl<T> CaSignaturePolicyHandler<T> {
    fn new(
        inner: T,
        allowed_algorithms: Vec<String>,
        forward_agent: bool,
        remote_forwards: Arc<RemoteForwardState>,
    ) -> Self {
        Self {
            inner,
            allowed_algorithms,
            forward_agent,
            remote_forwards,
        }
    }
}

#[derive(Clone)]
enum RemoteForwardKey {
    Tcp { address: String, port: Option<u32> },
    StreamLocal(PathBuf),
}

#[derive(Clone)]
struct RemoteForwardRoute {
    key: RemoteForwardKey,
    destination: Option<RemoteForwardDestination>,
    connect_timeout: std::time::Duration,
}

#[derive(Default)]
struct RemoteForwardState {
    active: AtomicBool,
    active_channels: AtomicUsize,
    routes: RwLock<Vec<RemoteForwardRoute>>,
}

struct ForwardChannelPermit {
    state: Arc<RemoteForwardState>,
}

impl RemoteForwardState {
    fn try_acquire_channel(self: &Arc<Self>) -> Option<ForwardChannelPermit> {
        self.active_channels
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |active| {
                (active < MAX_FORWARD_CONNECTIONS).then_some(active + 1)
            })
            .ok()
            .map(|_| ForwardChannelPermit {
                state: Arc::clone(self),
            })
    }

    fn tcp_route(
        &self,
        connected_address: &str,
        connected_port: u32,
    ) -> Option<RemoteForwardRoute> {
        if !self.active.load(Ordering::Acquire) {
            return None;
        }
        let routes = self.routes.read().expect("remote forward routes poisoned");
        let mut matching_port = routes.iter().filter(|route| {
            matches!(route.key, RemoteForwardKey::Tcp { port: Some(port), .. } if port == connected_port)
        });
        if let Some(first) = matching_port.next().cloned() {
            if matching_port.next().is_none() {
                return Some(first);
            }
            if let Some(route) = routes.iter().find(|route| {
                matches!(
                    &route.key,
                    RemoteForwardKey::Tcp { address, port: Some(port) }
                        if *port == connected_port && address == connected_address
                )
            }) {
                return Some(route.clone());
            }
        }
        routes
            .iter()
            .find(|route| {
                matches!(
                    &route.key,
                    RemoteForwardKey::Tcp { address, port: None }
                        if address.is_empty() || address == connected_address
                )
            })
            .cloned()
    }

    fn streamlocal_route(&self, socket_path: &str) -> Option<RemoteForwardRoute> {
        if !self.active.load(Ordering::Acquire) {
            return None;
        }
        self.routes
            .read()
            .expect("remote forward routes poisoned")
            .iter()
            .find(|route| {
                matches!(
                    &route.key,
                    RemoteForwardKey::StreamLocal(path) if path == Path::new(socket_path)
                )
            })
            .cloned()
    }
}

impl Drop for ForwardChannelPermit {
    fn drop(&mut self) {
        self.state.active_channels.fetch_sub(1, Ordering::Release);
    }
}

impl<T> Handler for CaSignaturePolicyHandler<T>
where
    T: Handler,
{
    type Error = T::Error;

    fn auth_banner(
        &mut self,
        banner: &str,
        session: &mut Session,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send {
        self.inner.auth_banner(banner, session)
    }

    async fn check_server_key(
        &mut self,
        server_public_key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        if let PublicKeyOrCertificate::Certificate(certificate) = server_public_key {
            let signature_algorithm = certificate.signature().algorithm();
            if !self
                .allowed_algorithms
                .iter()
                .any(|allowed| allowed == signature_algorithm.as_ref())
            {
                return Ok(false);
            }
        }
        self.inner.check_server_key(server_public_key).await
    }

    fn kex_done(
        &mut self,
        shared_secret: Option<&[u8]>,
        names: &russh::Names,
        session: &mut Session,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send {
        self.inner.kex_done(shared_secret, names, session)
    }

    fn channel_open_confirmation(
        &mut self,
        id: ChannelId,
        max_packet_size: u32,
        window_size: u32,
        session: &mut Session,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send {
        self.inner
            .channel_open_confirmation(id, max_packet_size, window_size, session)
    }

    fn channel_success(
        &mut self,
        channel: ChannelId,
        session: &mut Session,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send {
        self.inner.channel_success(channel, session)
    }

    fn channel_failure(
        &mut self,
        channel: ChannelId,
        session: &mut Session,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send {
        self.inner.channel_failure(channel, session)
    }

    fn channel_close(
        &mut self,
        channel: ChannelId,
        session: &mut Session,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send {
        self.inner.channel_close(channel, session)
    }

    fn channel_eof(
        &mut self,
        channel: ChannelId,
        session: &mut Session,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send {
        self.inner.channel_eof(channel, session)
    }

    fn channel_open_failure(
        &mut self,
        channel: ChannelId,
        reason: ChannelOpenFailure,
        description: &str,
        language: &str,
        session: &mut Session,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send {
        self.inner
            .channel_open_failure(channel, reason, description, language, session)
    }

    async fn server_channel_open_forwarded_tcpip(
        &mut self,
        channel: Channel<Msg>,
        connected_address: &str,
        connected_port: u32,
        originator_address: &str,
        originator_port: u32,
        reply: ChannelOpenHandle,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        if let Some(route) = self
            .remote_forwards
            .tcp_route(connected_address, connected_port)
        {
            let Some(permit) = self.remote_forwards.try_acquire_channel() else {
                reply.reject(ChannelOpenFailure::ResourceShortage).await;
                return Ok(());
            };
            forward_remote_channel(channel, reply, route, permit).await;
            return Ok(());
        }
        self.inner
            .server_channel_open_forwarded_tcpip(
                channel,
                connected_address,
                connected_port,
                originator_address,
                originator_port,
                reply,
                session,
            )
            .await
    }

    async fn server_channel_open_forwarded_streamlocal(
        &mut self,
        channel: Channel<Msg>,
        socket_path: &str,
        reply: ChannelOpenHandle,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        if let Some(route) = self.remote_forwards.streamlocal_route(socket_path) {
            let Some(permit) = self.remote_forwards.try_acquire_channel() else {
                reply.reject(ChannelOpenFailure::ResourceShortage).await;
                return Ok(());
            };
            forward_remote_channel(channel, reply, route, permit).await;
            return Ok(());
        }
        self.inner
            .server_channel_open_forwarded_streamlocal(channel, socket_path, reply, session)
            .await
    }

    async fn server_channel_open_agent_forward(
        &mut self,
        channel: Channel<Msg>,
        reply: ChannelOpenHandle,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        if !self.forward_agent {
            return self
                .inner
                .server_channel_open_agent_forward(channel, reply, session)
                .await;
        }

        let Some(permit) = self.remote_forwards.try_acquire_channel() else {
            reply.reject(ChannelOpenFailure::ResourceShortage).await;
            return Ok(());
        };
        forward_agent_channel(channel, reply, permit).await;
        Ok(())
    }

    fn should_accept_unknown_server_channel(
        &mut self,
        id: ChannelId,
        channel_type: &str,
    ) -> impl Future<Output = bool> + Send {
        self.inner
            .should_accept_unknown_server_channel(id, channel_type)
    }

    fn server_channel_open_unknown(
        &mut self,
        channel: Channel<Msg>,
        reply: ChannelOpenHandle,
        session: &mut Session,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send {
        self.inner
            .server_channel_open_unknown(channel, reply, session)
    }

    fn server_channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        reply: ChannelOpenHandle,
        session: &mut Session,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send {
        self.inner
            .server_channel_open_session(channel, reply, session)
    }

    fn server_channel_open_direct_tcpip(
        &mut self,
        channel: Channel<Msg>,
        host_to_connect: &str,
        port_to_connect: u32,
        originator_address: &str,
        originator_port: u32,
        reply: ChannelOpenHandle,
        session: &mut Session,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send {
        self.inner.server_channel_open_direct_tcpip(
            channel,
            host_to_connect,
            port_to_connect,
            originator_address,
            originator_port,
            reply,
            session,
        )
    }

    fn server_channel_open_direct_streamlocal(
        &mut self,
        channel: Channel<Msg>,
        socket_path: &str,
        reply: ChannelOpenHandle,
        session: &mut Session,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send {
        self.inner
            .server_channel_open_direct_streamlocal(channel, socket_path, reply, session)
    }

    fn server_channel_open_x11(
        &mut self,
        channel: Channel<Msg>,
        originator_address: &str,
        originator_port: u32,
        reply: ChannelOpenHandle,
        session: &mut Session,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send {
        self.inner.server_channel_open_x11(
            channel,
            originator_address,
            originator_port,
            reply,
            session,
        )
    }

    fn data(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send {
        self.inner.data(channel, data, session)
    }

    fn extended_data(
        &mut self,
        channel: ChannelId,
        ext: u32,
        data: &[u8],
        session: &mut Session,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send {
        self.inner.extended_data(channel, ext, data, session)
    }

    fn xon_xoff(
        &mut self,
        channel: ChannelId,
        client_can_do: bool,
        session: &mut Session,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send {
        self.inner.xon_xoff(channel, client_can_do, session)
    }

    fn exit_status(
        &mut self,
        channel: ChannelId,
        exit_status: u32,
        session: &mut Session,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send {
        self.inner.exit_status(channel, exit_status, session)
    }

    fn exit_signal(
        &mut self,
        channel: ChannelId,
        signal_name: Sig,
        core_dumped: bool,
        error_message: &str,
        lang_tag: &str,
        session: &mut Session,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send {
        self.inner.exit_signal(
            channel,
            signal_name,
            core_dumped,
            error_message,
            lang_tag,
            session,
        )
    }

    fn window_adjusted(
        &mut self,
        channel: ChannelId,
        new_size: u32,
        session: &mut Session,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send {
        self.inner.window_adjusted(channel, new_size, session)
    }

    fn adjust_window(&mut self, channel: ChannelId, window: u32) -> u32 {
        self.inner.adjust_window(channel, window)
    }

    fn openssh_ext_host_keys_announced(
        &mut self,
        keys: Vec<PublicKey>,
        session: &mut Session,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send {
        self.inner.openssh_ext_host_keys_announced(keys, session)
    }

    fn disconnected(
        &mut self,
        reason: DisconnectReason<Self::Error>,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send {
        self.inner.disconnected(reason)
    }
}

#[cfg(unix)]
async fn forward_agent_channel(
    channel: Channel<Msg>,
    reply: ChannelOpenHandle,
    permit: ForwardChannelPermit,
) {
    let agent = match russh::keys::agent::client::AgentClient::connect_env().await {
        Ok(agent) => agent,
        Err(err) => {
            warn!("Could not connect to the configured SSH agent: {err}");
            return;
        }
    };

    reply.accept().await;
    tokio::spawn(async move {
        let _permit = permit;
        let mut channel = channel.into_stream();
        let mut agent = agent.into_inner();
        if let Err(err) = tokio::io::copy_bidirectional(&mut channel, &mut agent).await {
            debug!("SSH agent forwarding channel closed with an error: {err}");
        }
    });
}

#[cfg(not(unix))]
async fn forward_agent_channel(
    _channel: Channel<Msg>,
    _reply: ChannelOpenHandle,
    _permit: ForwardChannelPermit,
) {
    warn!("SSH agent forwarding is unavailable on this platform");
}

async fn forward_remote_channel(
    channel: Channel<Msg>,
    reply: ChannelOpenHandle,
    route: RemoteForwardRoute,
    permit: ForwardChannelPermit,
) {
    match route.destination {
        Some(RemoteForwardDestination::Host { host, port }) => {
            reply.accept().await;
            tokio::spawn(async move {
                let _permit = permit;
                let destination = connect_remote_tcp(&host, port, route.connect_timeout).await;
                match destination {
                    Ok(destination) => relay_remote_channel(channel, destination).await,
                    Err(err) => {
                        warn!("Could not connect RemoteForward destination {host}:{port}: {err}");
                        if let Err(close_err) = channel.close().await {
                            debug!("Could not close failed RemoteForward channel: {close_err}");
                        }
                    }
                }
            });
        }
        Some(RemoteForwardDestination::UnixSocket(path)) => {
            forward_remote_unix_channel(channel, reply, path, route.connect_timeout, permit).await;
        }
        None => {
            reply.accept().await;
            tokio::spawn(async move {
                let _permit = permit;
                let mut channel = channel.into_stream();
                match socks_connect(&mut channel, route.connect_timeout).await {
                    Ok(mut destination) => {
                        if let Err(err) =
                            tokio::io::copy_bidirectional(&mut channel, &mut destination).await
                        {
                            debug!("Dynamic RemoteForward channel closed with an error: {err}");
                        }
                    }
                    Err(err) => warn!("Dynamic RemoteForward failed: {err}"),
                }
            });
        }
    }
}

async fn connect_remote_tcp(
    host: &str,
    port: u16,
    connect_timeout: std::time::Duration,
) -> std::io::Result<TcpStream> {
    if connect_timeout.is_zero() {
        TcpStream::connect((host, port)).await
    } else {
        tokio::time::timeout(connect_timeout, TcpStream::connect((host, port)))
            .await
            .map_err(|_| {
                std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "RemoteForward destination connect timed out",
                )
            })?
    }
}

async fn relay_remote_channel<S>(channel: Channel<Msg>, mut destination: S)
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut channel = channel.into_stream();
    if let Err(err) = tokio::io::copy_bidirectional(&mut channel, &mut destination).await {
        debug!("RemoteForward channel closed with an error: {err}");
    }
}

#[cfg(unix)]
async fn forward_remote_unix_channel(
    channel: Channel<Msg>,
    reply: ChannelOpenHandle,
    path: PathBuf,
    connect_timeout: std::time::Duration,
    permit: ForwardChannelPermit,
) {
    reply.accept().await;
    tokio::spawn(async move {
        let _permit = permit;
        match connect_remote_unix(&path, connect_timeout).await {
            Ok(destination) => relay_remote_channel(channel, destination).await,
            Err(err) => {
                warn!(
                    "Could not connect RemoteForward destination {}: {err}",
                    path.display()
                );
                if let Err(close_err) = channel.close().await {
                    debug!("Could not close failed RemoteForward channel: {close_err}");
                }
            }
        }
    });
}

#[cfg(unix)]
async fn connect_remote_unix(
    path: &Path,
    connect_timeout: std::time::Duration,
) -> std::io::Result<tokio::net::UnixStream> {
    if connect_timeout.is_zero() {
        tokio::net::UnixStream::connect(path).await
    } else {
        tokio::time::timeout(connect_timeout, tokio::net::UnixStream::connect(path))
            .await
            .map_err(|_| {
                std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "RemoteForward destination connect timed out",
                )
            })?
    }
}

#[cfg(not(unix))]
async fn forward_remote_unix_channel(
    _channel: Channel<Msg>,
    reply: ChannelOpenHandle,
    path: PathBuf,
    _connect_timeout: std::time::Duration,
    _permit: ForwardChannelPermit,
) {
    warn!(
        "Unix socket RemoteForward destination {} is unavailable on this platform",
        path.display()
    );
    reply.reject(ChannelOpenFailure::ConnectFailed).await;
}

async fn socks_connect<S>(
    stream: &mut S,
    connect_timeout: std::time::Duration,
) -> std::io::Result<TcpStream>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    let version = stream.read_u8().await?;
    let (host, port) = match version {
        4 => read_socks4_target(stream).await?,
        5 => read_socks5_target(stream).await?,
        _ => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("unsupported SOCKS version {version}"),
            ));
        }
    };

    let destination = connect_remote_tcp(&host, port, connect_timeout).await;
    match destination {
        Ok(destination) => {
            if version == 4 {
                stream.write_all(&[0, 90, 0, 0, 0, 0, 0, 0]).await?;
            } else {
                stream.write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0]).await?;
            }
            Ok(destination)
        }
        Err(err) => {
            if version == 4 {
                let _ = stream.write_all(&[0, 91, 0, 0, 0, 0, 0, 0]).await;
            } else {
                let _ = stream.write_all(&[5, 1, 0, 1, 0, 0, 0, 0, 0, 0]).await;
            }
            Err(err)
        }
    }
}

async fn read_socks4_target<S>(stream: &mut S) -> std::io::Result<(String, u16)>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    let command = stream.read_u8().await?;
    if command != 1 {
        let _ = stream.write_all(&[0, 91, 0, 0, 0, 0, 0, 0]).await;
        return Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "SOCKS4 supports CONNECT only",
        ));
    }
    let port = stream.read_u16().await?;
    let mut address = [0u8; 4];
    stream.read_exact(&mut address).await?;
    read_socks_string(stream).await?;
    let host = if address[..3] == [0, 0, 0] && address[3] != 0 {
        String::from_utf8(read_socks_string(stream).await?)
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err.to_string()))?
    } else {
        std::net::Ipv4Addr::from(address).to_string()
    };
    Ok((host, port))
}

async fn read_socks5_target<S>(stream: &mut S) -> std::io::Result<(String, u16)>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    let method_count = stream.read_u8().await? as usize;
    let mut methods = vec![0u8; method_count];
    stream.read_exact(&mut methods).await?;
    if !methods.contains(&0) {
        stream.write_all(&[5, 0xff]).await?;
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "SOCKS5 client did not offer no-authentication",
        ));
    }
    stream.write_all(&[5, 0]).await?;
    let request_version = stream.read_u8().await?;
    let command = stream.read_u8().await?;
    let reserved = stream.read_u8().await?;
    if request_version != 5 || command != 1 || reserved != 0 {
        let _ = stream.write_all(&[5, 7, 0, 1, 0, 0, 0, 0, 0, 0]).await;
        return Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "SOCKS5 supports CONNECT only",
        ));
    }
    let host = match stream.read_u8().await? {
        1 => {
            let mut address = [0u8; 4];
            stream.read_exact(&mut address).await?;
            std::net::Ipv4Addr::from(address).to_string()
        }
        3 => {
            let length = stream.read_u8().await? as usize;
            let mut domain = vec![0u8; length];
            stream.read_exact(&mut domain).await?;
            String::from_utf8(domain).map_err(|err| {
                std::io::Error::new(std::io::ErrorKind::InvalidData, err.to_string())
            })?
        }
        4 => {
            let mut address = [0u8; 16];
            stream.read_exact(&mut address).await?;
            std::net::Ipv6Addr::from(address).to_string()
        }
        address_type => {
            let _ = stream.write_all(&[5, 8, 0, 1, 0, 0, 0, 0, 0, 0]).await;
            return Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                format!("unsupported SOCKS5 address type {address_type}"),
            ));
        }
    };
    let port = stream.read_u16().await?;
    Ok((host, port))
}

async fn read_socks_string<S>(stream: &mut S) -> std::io::Result<Vec<u8>>
where
    S: AsyncRead + Unpin,
{
    use tokio::io::AsyncReadExt as _;

    let mut value = Vec::new();
    loop {
        let byte = stream.read_u8().await?;
        if byte == 0 {
            return Ok(value);
        }
        if value.len() >= 1024 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "SOCKS string exceeds 1024 bytes",
            ));
        }
        value.push(byte);
    }
}

/// [`russh`](https://docs.rs/russh/latest/russh) session.
pub struct RusshSession<T>
where
    T: Handler + Default + Send + 'static,
{
    session: Handle<CaSignaturePolicyHandler<T>>,
    forward_agent: bool,
    remote_forward_ports: Vec<u16>,
    remote_forward_state: Arc<RemoteForwardState>,
    remote_forwards: Vec<RegisteredRemoteForward>,
}

enum RegisteredRemoteForward {
    Tcp { address: String, port: u32 },
    StreamLocal(String),
}

/// SFTP handle for russh.
pub(crate) struct RusshSftp {
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

struct ConnectionTarget<'a> {
    address: &'a str,
    bind_address: Option<&'a str>,
    bind_interface: Option<&'a str>,
}

async fn connect_with_timeout<T>(
    config: Arc<client::Config>,
    handler: T,
    target: ConnectionTarget<'_>,
    tcp_keep_alive: Option<bool>,
    timeout: std::time::Duration,
) -> RemoteResult<Handle<T>>
where
    T: Handler + Send + 'static,
{
    let deadline = (!timeout.is_zero())
        .then(|| Instant::now().checked_add(timeout))
        .flatten();
    let connect = || {
        connect_tcp(
            target.address,
            target.bind_address,
            target.bind_interface,
            tcp_keep_alive,
        )
    };
    let stream = match deadline {
        Some(deadline) => tokio::time::timeout_at(deadline, connect())
            .await
            .map_err(|err| {
                let msg = format!("SSH connection timed out: {err}");
                error!("{msg}");
                RemoteError::with_message(RemoteErrorType::ConnectionError, msg)
            })?,
        None => connect().await,
    }
    .map_err(|err| {
        let msg = format!("SSH connection failed: {err}");
        error!("{msg}");
        RemoteError::with_message(RemoteErrorType::ConnectionError, msg)
    })?;
    if config.nodelay
        && let Err(err) = stream.set_nodelay(true)
    {
        warn!("Failed to enable TCP_NODELAY: {err}");
    }

    let session_result = match deadline {
        Some(deadline) => {
            let deadline_active = Arc::new(AtomicBool::new(true));
            let connection_deadline_active = deadline_active.clone();
            let stream =
                ConnectionDeadlineStream::new(stream, deadline, connection_deadline_active);
            let result = client::connect_stream(config, stream, handler).await;
            deadline_active.store(false, Ordering::Release);
            result
        }
        None => client::connect_stream(config, stream, handler).await,
    };
    session_result.map_err(|err| {
        let msg = format!("SSH connection failed: {err:?}");
        error!("{msg}");
        RemoteError::with_message(RemoteErrorType::ConnectionError, msg)
    })
}

async fn connect_tcp(
    address: &str,
    bind_address: Option<&str>,
    bind_interface: Option<&str>,
    tcp_keep_alive: Option<bool>,
) -> std::io::Result<TcpStream> {
    let tcp_keep_alive = tcp_keep_alive.unwrap_or(true);
    if bind_address.is_none() && bind_interface.is_none() {
        let stream = TcpStream::connect(address).await?;
        socket::set_keepalive(&stream, tcp_keep_alive)?;
        return Ok(stream);
    }

    let source_addresses = if let Some(bind_address) = bind_address {
        tokio::net::lookup_host((bind_address, 0))
            .await?
            .collect::<Vec<_>>()
    } else {
        interface::addresses(bind_interface.expect("checked above"))?
            .into_iter()
            .collect()
    };
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
            if let Err(err) = socket.set_keepalive(tcp_keep_alive) {
                last_error = Some(err);
                continue;
            }
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

    let source = bind_address.map_or_else(
        || format!("BindInterface {}", bind_interface.expect("checked above")),
        |address| format!("BindAddress {address}"),
    );
    Err(last_error.unwrap_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::AddrNotAvailable,
            format!("no compatible address family found for {source}"),
        )
    }))
}

impl<T> RusshSession<T>
where
    T: Handler + Default + Send + 'static,
{
    pub(crate) async fn connect(opts: &SshOpts) -> RemoteResult<Self> {
        let ssh_config = Config::try_from(opts)?;
        debug!("Connecting to '{}'", ssh_config.address);
        let proxy_jumps = ssh_config.proxy_jump_configs(opts)?;
        let remote_forward_state = Arc::new(RemoteForwardState::default());
        let mut session = if let Some(first_jump) = proxy_jumps.first() {
            let mut session = connect_russh_direct::<T>(
                opts,
                first_jump,
                Arc::new(RemoteForwardState::default()),
            )
            .await?;
            auth::authenticate(&mut session, opts, first_jump).await?;
            for next_hop in proxy_jumps
                .iter()
                .skip(1)
                .chain(std::iter::once(&ssh_config))
            {
                let routes = if std::ptr::eq(next_hop, &ssh_config) {
                    remote_forward_state.clone()
                } else {
                    Arc::new(RemoteForwardState::default())
                };
                session = connect_russh_through_jump::<T>(opts, &session, next_hop, routes).await?;
                if !std::ptr::eq(next_hop, &ssh_config) {
                    auth::authenticate(&mut session, opts, next_hop).await?;
                }
            }
            session
        } else {
            connect_russh_direct::<T>(opts, &ssh_config, remote_forward_state.clone()).await?
        };
        auth::authenticate(&mut session, opts, &ssh_config).await?;
        let remote_forwards =
            setup_russh_remote_forwards(&session, &ssh_config, &remote_forward_state).await?;
        let remote_forward_ports = remote_forwards
            .iter()
            .filter_map(|forward| match forward {
                RegisteredRemoteForward::Tcp { port, .. } => Some(
                    u16::try_from(*port)
                        .expect("RemoteForward assigned ports were validated during setup"),
                ),
                RegisteredRemoteForward::StreamLocal(_) => None,
            })
            .collect();

        Ok(Self {
            session,
            forward_agent: ssh_config.params.forward_agent.unwrap_or(false),
            remote_forward_ports,
            remote_forward_state,
            remote_forwards,
        })
    }

    pub(crate) async fn disconnect(&self) -> RemoteResult<()> {
        self.remote_forward_state
            .active
            .store(false, Ordering::Release);
        for forward in self.remote_forwards.iter().rev() {
            let result = match forward {
                RegisteredRemoteForward::Tcp { address, port } => {
                    self.session.cancel_tcpip_forward(address, *port).await
                }
                RegisteredRemoteForward::StreamLocal(path) => {
                    self.session.cancel_streamlocal_forward(path).await
                }
            };
            if let Err(err) = result {
                warn!("Failed to cancel RemoteForward: {err}");
            }
        }
        self.session
            .disconnect(Disconnect::ByApplication, "Closed by user", "en_US")
            .await
            .map_err(|err| {
                log::error!("failed to disconnect {err}");
                RemoteError::with_message(RemoteErrorType::ConnectionError, err.to_string())
            })
    }

    /// Returns ports assigned to configured TCP `RemoteForward` listeners.
    pub fn remote_forward_ports(&self) -> &[u16] {
        &self.remote_forward_ports
    }

    pub(crate) fn is_alive(&self) -> bool {
        !self.session.is_closed()
    }

    pub(crate) async fn cmd(&self, cmd: &str) -> RemoteResult<(u32, String)> {
        trace!("Running command: {cmd}");

        // Escape single quotes and wrap in sh -c for consistent shell behavior.
        // Without this, commands like "cd /some/dir; somecommand" fail if the
        // remote user's login shell is fish or another non-POSIX shell.
        let escaped = cmd.replace('\'', r#"'\''"#);
        let wrapped = format!("sh -c '{escaped}'");

        perform_shell_cmd(&self.session, &wrapped, self.forward_agent).await
    }

    pub(crate) async fn scp_recv(
        &self,
        path: &Path,
        opts: &ReadOptions,
    ) -> RemoteResult<AsyncReadStream> {
        scp::recv(&self.session, path, opts, self.forward_agent).await
    }

    pub(crate) async fn scp_send(
        &self,
        remote_path: &Path,
        mode: u32,
        size: u64,
        modified: Option<std::time::SystemTime>,
    ) -> RemoteResult<AsyncWriteStream> {
        scp::send(
            &self.session,
            remote_path,
            mode,
            size,
            modified,
            self.forward_agent,
        )
        .await
    }

    pub(crate) async fn sftp(&self) -> RemoteResult<RusshSftp> {
        let channel = self
            .session
            .channel_open_session()
            .await
            .map_err(|err: russh::Error| {
                error!("Failed to init SFTP session: {err}");
                RemoteError::with_message(RemoteErrorType::ProtocolError, err.to_string())
            })?;
        if self.forward_agent {
            channel.agent_forward(true).await.map_err(|err| {
                RemoteError::with_message(RemoteErrorType::ProtocolError, err.to_string())
            })?;
        }
        channel
            .request_subsystem(true, "sftp")
            .await
            .map_err(|err| {
                RemoteError::with_message(RemoteErrorType::ProtocolError, err.to_string())
            })?;
        SftpSession::new(channel.into_stream())
            .await
            .map(|session| RusshSftp {
                session: Arc::new(session),
            })
            .map_err(|err| {
                error!("Failed to init SFTP session: {err}");
                RemoteError::with_message(RemoteErrorType::ProtocolError, err.to_string())
            })
    }
}

fn russh_client_config(opts: &SshOpts, ssh_config: &Config) -> Arc<client::Config> {
    let mut config = client::Config {
        keepalive_interval: ssh_config
            .params
            .server_alive_interval
            .filter(|interval| !interval.is_zero()),
        ..client::Config::default()
    };
    apply_config_algo_prefs(&mut config, ssh_config);
    apply_opts_algo_prefs(&mut config, opts);
    Arc::new(config)
}

async fn connect_russh_direct<T>(
    opts: &SshOpts,
    ssh_config: &Config,
    remote_forwards: Arc<RemoteForwardState>,
) -> RemoteResult<Handle<CaSignaturePolicyHandler<T>>>
where
    T: Handler + Default + Send + 'static,
{
    let config = russh_client_config(opts, ssh_config);
    let connection_attempts = ssh_config.connection_attempts.max(1);
    let ca_signature_algorithms = ssh_config
        .params
        .ca_signature_algorithms
        .algorithms()
        .to_vec();
    let mut attempt = 1;
    loop {
        let handler = CaSignaturePolicyHandler::new(
            T::default(),
            ca_signature_algorithms.clone(),
            ssh_config.params.forward_agent.unwrap_or(false),
            remote_forwards.clone(),
        );
        match connect_with_timeout(
            config.clone(),
            handler,
            ConnectionTarget {
                address: &ssh_config.address,
                bind_address: ssh_config.params.bind_address.as_deref(),
                bind_interface: ssh_config.params.bind_interface.as_deref(),
            },
            ssh_config.params.tcp_keep_alive,
            ssh_config.connection_timeout,
        )
        .await
        {
            Ok(session) => return Ok(session),
            Err(err) if attempt < connection_attempts => {
                warn!("SSH connection attempt {attempt} failed: {err}");
                attempt += 1;
            }
            Err(err) => return Err(err),
        }
    }
}

async fn connect_russh_through_jump<T>(
    opts: &SshOpts,
    jump_session: &Handle<CaSignaturePolicyHandler<T>>,
    target: &Config,
    remote_forwards: Arc<RemoteForwardState>,
) -> RemoteResult<Handle<CaSignaturePolicyHandler<T>>>
where
    T: Handler + Default + Send + 'static,
{
    let config = russh_client_config(opts, target);
    let ca_signature_algorithms = target.params.ca_signature_algorithms.algorithms().to_vec();
    let mut attempt = 1;
    loop {
        let handler = CaSignaturePolicyHandler::new(
            T::default(),
            ca_signature_algorithms.clone(),
            target.params.forward_agent.unwrap_or(false),
            remote_forwards.clone(),
        );
        let connect = async {
            let channel = jump_session
                .channel_open_direct_tcpip(
                    target.resolved_host.clone(),
                    u32::from(target.port),
                    "127.0.0.1",
                    0,
                )
                .await?;
            client::connect_stream(config.clone(), channel.into_stream(), handler).await
        };
        let result = if target.connection_timeout.is_zero() {
            Ok(connect.await)
        } else {
            tokio::time::timeout(target.connection_timeout, connect).await
        };
        match result {
            Ok(Ok(session)) => return Ok(session),
            Ok(Err(err)) if attempt < target.connection_attempts.max(1) => {
                warn!("SSH connection attempt {attempt} through ProxyJump failed: {err:?}");
                attempt += 1;
            }
            Ok(Err(err)) => {
                return Err(RemoteError::with_message(
                    RemoteErrorType::ConnectionError,
                    format!("SSH connection through ProxyJump failed: {err:?}"),
                ));
            }
            Err(err) if attempt < target.connection_attempts.max(1) => {
                warn!("SSH connection attempt {attempt} through ProxyJump timed out: {err}");
                attempt += 1;
            }
            Err(err) => {
                return Err(RemoteError::with_message(
                    RemoteErrorType::ConnectionError,
                    format!("SSH connection through ProxyJump timed out: {err}"),
                ));
            }
        }
    }
}

async fn setup_russh_remote_forwards<T>(
    session: &Handle<T>,
    config: &Config,
    state: &RemoteForwardState,
) -> RemoteResult<Vec<RegisteredRemoteForward>>
where
    T: Handler,
{
    #[cfg(not(unix))]
    if config.params.remote_forward.iter().any(|forward| {
        matches!(
            forward.destination,
            Some(RemoteForwardDestination::UnixSocket(_))
        )
    }) {
        return Err(RemoteError::with_message(
            RemoteErrorType::UnsupportedFeature,
            "Unix socket RemoteForward destinations are unavailable on this platform",
        ));
    }

    let mut registered = Vec::new();
    state
        .routes
        .write()
        .expect("remote forward routes poisoned")
        .clear();
    state.active.store(true, Ordering::Release);
    for forward in &config.params.remote_forward {
        let pending_key = match &forward.listen {
            RemoteForwardListen::Port(port) => RemoteForwardKey::Tcp {
                address: "localhost".to_string(),
                port: (*port != 0).then_some(u32::from(*port)),
            },
            RemoteForwardListen::Host { host, port } => RemoteForwardKey::Tcp {
                address: if host.is_empty() || host == "*" {
                    String::new()
                } else {
                    host.clone()
                },
                port: (*port != 0).then_some(u32::from(*port)),
            },
            RemoteForwardListen::UnixSocket(path) => {
                if path.to_str().is_none() {
                    state.active.store(false, Ordering::Release);
                    state
                        .routes
                        .write()
                        .expect("remote forward routes poisoned")
                        .clear();
                    cancel_russh_remote_forwards(session, &registered).await;
                    return Err(RemoteError::with_message(
                        RemoteErrorType::ProtocolError,
                        "RemoteForward Unix socket path is not valid UTF-8",
                    ));
                }
                RemoteForwardKey::StreamLocal(path.clone())
            }
        };
        let route_index = {
            let mut routes = state
                .routes
                .write()
                .expect("remote forward routes poisoned");
            let route_index = routes.len();
            routes.push(RemoteForwardRoute {
                key: pending_key.clone(),
                destination: forward.destination.clone(),
                connect_timeout: config.connection_timeout,
            });
            route_index
        };

        let result = match &pending_key {
            RemoteForwardKey::Tcp { address, port } => session
                .tcpip_forward(address, port.unwrap_or(0))
                .await
                .map(|returned_port| RemoteForwardKey::Tcp {
                    address: address.clone(),
                    port: Some(port.unwrap_or(returned_port)),
                }),
            RemoteForwardKey::StreamLocal(path) => session
                .streamlocal_forward(path.to_str().expect("Unix socket path was validated above"))
                .await
                .map(|()| pending_key.clone()),
        };

        match result {
            Ok(key) => {
                let registration = match &key {
                    RemoteForwardKey::Tcp {
                        address,
                        port: Some(port),
                    } => RegisteredRemoteForward::Tcp {
                        address: address.clone(),
                        port: *port,
                    },
                    RemoteForwardKey::StreamLocal(path) => RegisteredRemoteForward::StreamLocal(
                        path.to_str()
                            .expect("Unix socket path was validated above")
                            .to_string(),
                    ),
                    RemoteForwardKey::Tcp { port: None, .. } => {
                        unreachable!("successful TCP registration has an assigned port")
                    }
                };
                if matches!(registration, RegisteredRemoteForward::Tcp { port, .. } if port > u32::from(u16::MAX))
                {
                    registered.push(registration);
                    state.active.store(false, Ordering::Release);
                    state
                        .routes
                        .write()
                        .expect("remote forward routes poisoned")
                        .clear();
                    cancel_russh_remote_forwards(session, &registered).await;
                    return Err(RemoteError::with_message(
                        RemoteErrorType::ProtocolError,
                        "SSH server returned an invalid RemoteForward port",
                    ));
                }
                state
                    .routes
                    .write()
                    .expect("remote forward routes poisoned")[route_index]
                    .key = key;
                registered.push(registration);
            }
            Err(err) => {
                state.active.store(false, Ordering::Release);
                state
                    .routes
                    .write()
                    .expect("remote forward routes poisoned")
                    .clear();
                cancel_russh_remote_forwards(session, &registered).await;
                return Err(RemoteError::with_message(
                    RemoteErrorType::ProtocolError,
                    format!("Could not configure RemoteForward: {err}"),
                ));
            }
        }
    }

    Ok(registered)
}

async fn cancel_russh_remote_forwards<T>(
    session: &Handle<T>,
    registered: &[RegisteredRemoteForward],
) where
    T: Handler,
{
    for forward in registered.iter().rev() {
        let result = match forward {
            RegisteredRemoteForward::Tcp { address, port } => {
                session.cancel_tcpip_forward(address, *port).await
            }
            RegisteredRemoteForward::StreamLocal(path) => {
                session.cancel_streamlocal_forward(path).await
            }
        };
        if let Err(err) = result {
            warn!("Failed to roll back RemoteForward: {err}");
        }
    }
}

fn sftp_error(err: russh_sftp::client::error::Error, fallback: RemoteErrorType) -> RemoteError {
    use russh_sftp::protocol::StatusCode;

    let kind = match &err {
        russh_sftp::client::error::Error::Status(status) => match status.status_code {
            StatusCode::NoSuchFile => RemoteErrorType::NoSuchFileOrDirectory,
            StatusCode::PermissionDenied => RemoteErrorType::PermissionDenied,
            _ => fallback,
        },
        _ => fallback,
    };
    RemoteError::with_source(kind, err)
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

impl RusshSftp {
    pub(crate) async fn mkdir(&self, path: &Path, mode: u32) -> RemoteResult<()> {
        let path_str = path_string(path);
        self.session
            .create_dir(&path_str)
            .await
            .map_err(|err| sftp_error(err, RemoteErrorType::FileCreateDenied))?;
        let mut attrs = russh_sftp::protocol::FileAttributes::empty();
        attrs.permissions = Some(mode & 0o7777);
        self.session
            .set_metadata(&path_str, attrs)
            .await
            .map_err(|err| sftp_error(err, RemoteErrorType::StatFailed))
    }

    pub(crate) async fn open_read(
        &self,
        path: &Path,
        opts: &ReadOptions,
    ) -> RemoteResult<AsyncReadStream> {
        let reader =
            stream::RusshSftpReader::open(Arc::clone(&self.session), path_string(path), opts)
                .await
                .map_err(|err| sftp_error(err, RemoteErrorType::CouldNotOpenFile))?;
        Ok(AsyncReadStream::new(reader))
    }

    pub(crate) async fn open_write(
        &self,
        path: &Path,
        flags: WriteMode,
        mode: u32,
    ) -> RemoteResult<AsyncWriteStream> {
        use russh_sftp::protocol::OpenFlags;

        let open_flags = match flags {
            WriteMode::Append => OpenFlags::WRITE | OpenFlags::APPEND | OpenFlags::CREATE,
            WriteMode::Truncate => OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::TRUNCATE,
        };
        let mut attrs = russh_sftp::protocol::FileAttributes::empty();
        attrs.permissions = Some(mode & 0o7777);
        let file = self
            .session
            .open_with_flags_and_attributes(path_string(path), open_flags, attrs)
            .await
            .map_err(|err| sftp_error(err, RemoteErrorType::FileCreateDenied))?;
        Ok(AsyncWriteStream::new(stream::RusshSftpWriter::new(file)))
    }

    pub(crate) async fn readdir(&self, dirname: &Path) -> RemoteResult<Vec<File>> {
        let entries = self
            .session
            .read_dir(path_string(dirname))
            .await
            .map_err(|err| sftp_error(err, RemoteErrorType::StatFailed))?;
        let mut files = Vec::new();
        for entry in entries {
            let entry_path = dirname.join(entry.file_name());
            let symlink = if entry.file_type().is_symlink() {
                self.read_link(&entry_path).await
            } else {
                None
            };
            files.push(make_fsentry(&entry_path, &entry.metadata(), symlink));
        }
        Ok(files)
    }

    async fn read_link(&self, path: &Path) -> Option<PathBuf> {
        match self.session.read_link(path_string(path)).await {
            Ok(target) => Some(PathBuf::from(target)),
            Err(err) => {
                error!(
                    "Failed to read link of {path}: {err}",
                    path = path.display()
                );
                None
            }
        }
    }

    pub(crate) async fn rename(&self, src: &Path, dest: &Path) -> RemoteResult<()> {
        self.session
            .rename(path_string(src), path_string(dest))
            .await
            .map_err(|err| sftp_error(err, RemoteErrorType::FileCreateDenied))
    }

    pub(crate) async fn rmdir(&self, path: &Path) -> RemoteResult<()> {
        self.session
            .remove_dir(path_string(path))
            .await
            .map_err(|err| sftp_error(err, RemoteErrorType::CouldNotRemoveFile))
    }

    pub(crate) async fn set_metadata(
        &self,
        path: &Path,
        metadata: &SetMetadata,
    ) -> RemoteResult<()> {
        self.session
            .set_metadata(path_string(path), set_metadata_to_attributes(metadata))
            .await
            .map_err(|err| sftp_error(err, RemoteErrorType::StatFailed))
    }

    pub(crate) async fn stat(&self, path: &Path) -> RemoteResult<File> {
        let attrs = self
            .session
            .symlink_metadata(path_string(path))
            .await
            .map_err(|err| sftp_error(err, RemoteErrorType::StatFailed))?;
        let symlink = if attrs.is_symlink() {
            self.read_link(path).await
        } else {
            None
        };
        Ok(make_fsentry(path, &attrs, symlink))
    }

    pub(crate) async fn symlink(&self, path: &Path, target: &Path) -> RemoteResult<()> {
        self.session
            .symlink(path_string(path), path_string(target))
            .await
            .map_err(|err| sftp_error(err, RemoteErrorType::FileCreateDenied))
    }

    pub(crate) async fn unlink(&self, path: &Path) -> RemoteResult<()> {
        self.session
            .remove_file(path_string(path))
            .await
            .map_err(|err| sftp_error(err, RemoteErrorType::CouldNotRemoveFile))
    }
}

fn set_metadata_to_attributes(metadata: &SetMetadata) -> russh_sftp::protocol::FileAttributes {
    let atime = metadata
        .accessed
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|time| time.as_secs() as u32);
    let mtime = metadata
        .modified
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|time| time.as_secs() as u32);
    russh_sftp::protocol::FileAttributes {
        size: None,
        uid: metadata.uid,
        user: None,
        gid: metadata.gid,
        group: None,
        permissions: metadata.mode.map(u32::from),
        atime,
        mtime,
    }
}

fn make_fsentry(
    path: &Path,
    attrs: &russh_sftp::protocol::FileAttributes,
    symlink: Option<PathBuf>,
) -> File {
    let file_type = if symlink.is_some() {
        FileType::Symlink
    } else if attrs.is_dir() {
        FileType::Directory
    } else {
        FileType::File
    };
    let mut metadata = Metadata::default().file_type(file_type);
    if let Some(size) = attrs.size {
        metadata = metadata.size(size);
    }
    if let Some(uid) = attrs.uid {
        metadata = metadata.uid(uid);
    }
    if let Some(gid) = attrs.gid {
        metadata = metadata.gid(gid);
    }
    if let Some(mode) = attrs.permissions {
        metadata = metadata.mode(UnixPex::from(mode));
    }
    if let Some(atime) = attrs.atime {
        metadata = metadata
            .accessed(std::time::UNIX_EPOCH + std::time::Duration::from_secs(u64::from(atime)));
    }
    if let Some(mtime) = attrs.mtime {
        metadata = metadata
            .modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(u64::from(mtime)));
    }
    if let Some(symlink) = symlink {
        metadata = metadata.symlink(symlink);
    }
    File::new(path.to_path_buf(), metadata)
}

/// Apply algorithm preferences from SSH config to the russh [`client::Config`].
fn apply_config_algo_prefs(config: &mut client::Config, ssh_config: &Config) {
    let params = &ssh_config.params;

    config.preferred.compression = if params.compression.unwrap_or(false) {
        Cow::Owned(vec![
            russh::compression::ZLIB_LEGACY,
            russh::compression::ZLIB,
            russh::compression::NONE,
        ])
    } else {
        Cow::Owned(vec![russh::compression::NONE])
    };

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
    let (host_keys, host_key_certificates) = parse_host_key_algorithms(
        params
            .host_key_algorithms
            .algorithms()
            .iter()
            .map(String::as_str),
    );
    config.preferred.host_key_certificates = Cow::Owned(host_key_certificates);
    config.preferred.key = Cow::Owned(host_keys);

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
                let (keys, certificates) = parse_host_key_algorithms(names.iter().copied());
                if !keys.is_empty() || !certificates.is_empty() {
                    config.preferred.host_key_certificates = Cow::Owned(certificates);
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

/// Split host key algorithm names into plain key and certificate preferences.
///
/// Russh advertises certificate preferences before plain key preferences because its API exposes
/// them as separate lists, so cross-list ordering cannot be preserved.
fn parse_host_key_algorithms<'a>(
    names: impl IntoIterator<Item = &'a str>,
) -> (Vec<Algorithm>, Vec<Algorithm>) {
    let mut keys = Vec::new();
    let mut certificates = Vec::new();

    for name in names {
        let result = if name.ends_with("-cert-v01@openssh.com") {
            Algorithm::new_certificate(name)
                .map(|algorithm| certificates.push(algorithm))
                .map_err(|err| err.to_string())
        } else {
            name.parse::<Algorithm>()
                .map(|algorithm| keys.push(algorithm))
                .map_err(|err| err.to_string())
        };
        if let Err(err) = result {
            warn!("Unsupported host key algorithm '{name}': {err}");
        }
    }

    (keys, certificates)
}

/// Execute a shell command on the remote server via a russh channel.
///
/// Opens a session channel, executes the command, collects stdout,
/// and returns the exit code with the output.
async fn perform_shell_cmd<T>(
    session: &Handle<T>,
    cmd: &str,
    forward_agent: bool,
) -> RemoteResult<(u32, String)>
where
    T: Handler,
{
    let mut channel = open_channel(session, forward_agent).await?;

    channel.exec(true, cmd).await.map_err(|err| {
        RemoteError::with_message(
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
async fn open_channel<T>(
    session: &Handle<T>,
    forward_agent: bool,
) -> RemoteResult<russh::Channel<russh::client::Msg>>
where
    T: Handler,
{
    let channel = session.channel_open_session().await.map_err(|err| {
        RemoteError::with_message(
            RemoteErrorType::ProtocolError,
            format!("Could not open channel: {err}"),
        )
    })?;
    if forward_agent {
        channel.agent_forward(true).await.map_err(|err| {
            RemoteError::with_message(
                RemoteErrorType::ProtocolError,
                format!("Could not request SSH agent forwarding: {err}"),
            )
        })?;
    }
    Ok(channel)
}
