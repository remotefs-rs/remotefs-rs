//! ## Ssh mock
//!
//! Contains mock for SSH protocol

use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use tempfile::NamedTempFile;

use crate::SshKeyStorage;

#[cfg(unix)]
static SSH_AGENT_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Isolated SSH agent for tests that mutate `SSH_AUTH_SOCK`.
#[cfg(unix)]
pub struct TestSshAgent {
    _env_lock: std::sync::MutexGuard<'static, ()>,
    auth_sock: String,
    pid: String,
    previous_auth_sock: Option<std::ffi::OsString>,
}

#[cfg(unix)]
impl TestSshAgent {
    pub fn start() -> Self {
        let env_lock = SSH_AGENT_ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let output = std::process::Command::new("ssh-agent")
            .arg("-s")
            .output()
            .expect("failed to spawn ssh-agent (is openssh installed?)");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let auth_sock = parse_agent_var(&stdout, "SSH_AUTH_SOCK")
            .expect("ssh-agent did not report SSH_AUTH_SOCK");
        let pid = parse_agent_var(&stdout, "SSH_AGENT_PID")
            .expect("ssh-agent did not report SSH_AGENT_PID");
        let previous_auth_sock = std::env::var_os("SSH_AUTH_SOCK");
        // SAFETY: `TestSshAgent` holds the global agent environment lock.
        unsafe {
            std::env::set_var("SSH_AUTH_SOCK", &auth_sock);
        }

        Self {
            _env_lock: env_lock,
            auth_sock,
            pid,
            previous_auth_sock,
        }
    }

    pub fn auth_sock(&self) -> &str {
        &self.auth_sock
    }

    pub fn add_key(&self, key: &std::path::Path) {
        std::process::Command::new("chmod")
            .args(["600", &key.display().to_string()])
            .status()
            .expect("chmod failed");
        let added = std::process::Command::new("ssh-add")
            .arg(key)
            .env("SSH_AUTH_SOCK", &self.auth_sock)
            .status()
            .expect("ssh-add failed to run");
        assert!(added.success(), "ssh-add could not load the test key");
    }
}

#[cfg(unix)]
impl Drop for TestSshAgent {
    fn drop(&mut self) {
        // SAFETY: `TestSshAgent` holds the global agent environment lock.
        unsafe {
            if let Some(previous_auth_sock) = self.previous_auth_sock.as_ref() {
                std::env::set_var("SSH_AUTH_SOCK", previous_auth_sock);
            } else {
                std::env::remove_var("SSH_AUTH_SOCK");
            }
        }
        let _ = std::process::Command::new("kill").arg(&self.pid).status();
    }
}

#[cfg(unix)]
fn parse_agent_var(output: &str, name: &str) -> Option<String> {
    let needle = format!("{name}=");
    let start = output.find(&needle)? + needle.len();
    let rest = &output[start..];
    let end = rest.find(';')?;
    Some(rest[..end].to_string())
}

/// Return the name of an interface with an IPv4 loopback address.
#[cfg(any(feature = "libssh", feature = "libssh2", feature = "russh"))]
pub fn ipv4_loopback_interface() -> String {
    if_addrs::get_if_addrs()
        .expect("failed to enumerate network interfaces")
        .into_iter()
        .find(|interface| interface.ip().is_ipv4() && interface.ip().is_loopback())
        .expect("missing IPv4 loopback interface")
        .name
}

/// Start a TCP server that accepts one connection without completing an SSH handshake.
pub fn start_unresponsive_server(hold: Duration) -> (u16, JoinHandle<Duration>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("failed to bind test server");
    let port = listener
        .local_addr()
        .expect("failed to read test server address")
        .port();
    let handle = thread::spawn(move || {
        let (mut stream, _address) = listener.accept().expect("failed to accept connection");
        stream
            .set_read_timeout(Some(hold))
            .expect("failed to set test server timeout");
        stream
            .write_all(b"SSH-2.0-timeout-test\r\n")
            .expect("failed to write test server banner");
        let started = Instant::now();
        let mut buffer = [0_u8; 256];
        loop {
            match stream.read(&mut buffer) {
                Ok(0) => break,
                Ok(_) => {}
                Err(err)
                    if matches!(
                        err.kind(),
                        std::io::ErrorKind::ConnectionReset
                            | std::io::ErrorKind::TimedOut
                            | std::io::ErrorKind::WouldBlock
                    ) =>
                {
                    break;
                }
                Err(err) => panic!("test server read failed: {err}"),
            }
        }
        started.elapsed()
    });

    (port, handle)
}

/// Start a TCP proxy that drops a number of connections before relaying one.
pub fn start_flaky_proxy(target_port: u16, failures: usize) -> (u16, JoinHandle<()>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("failed to bind test proxy");
    let port = listener
        .local_addr()
        .expect("failed to read test proxy address")
        .port();
    let handle = thread::spawn(move || {
        for _ in 0..failures {
            let (stream, _address) = listener
                .accept()
                .expect("failed to accept rejected connection");
            drop(stream);
        }

        let (mut client, _address) = listener
            .accept()
            .expect("failed to accept relayed connection");
        let mut server = TcpStream::connect(("127.0.0.1", target_port))
            .expect("failed to connect test proxy target");
        let mut client_reader = client.try_clone().expect("failed to clone client stream");
        let mut server_writer = server.try_clone().expect("failed to clone server stream");
        let upstream = thread::spawn(move || {
            let _ = std::io::copy(&mut client_reader, &mut server_writer);
            let _ = server_writer.shutdown(Shutdown::Write);
        });
        let _ = std::io::copy(&mut server, &mut client);
        let _ = client.shutdown(Shutdown::Write);
        upstream.join().expect("test proxy panicked");
    });

    (port, handle)
}

/// Mock RSA private key (matches the `PUBLIC_KEY` authorized in the test container).
pub const MOCK_PRIVATE_KEY: &str = r"-----BEGIN OPENSSH PRIVATE KEY-----
b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAABFwAAAAdzc2gtcn
NhAAAAAwEAAQAAAQEAxKyYUMRCNPlb4ZV1VMofrzApu2l3wgP4Ot9wBvHsw/+RMpcHIbQK
9iQqAVp8Z+M1fJyPXTKjoJtIzuCLF6Sjo0KI7/tFTh+yPnA5QYNLZOIRZb8skumL4gwHww
5Z942FDPuUDQ30C2mZR9lr3Cd5pA8S1ZSPTAV9QQHkpgoS8cAL8QC6dp3CJjUC8wzvXh3I
oN3bTKxCpM10KMEVuWO3lM4Nvr71auB9gzo1sFJ3bwebCZIRH01FROyA/GXRiaOtJFG/9N
nWWI/iG5AJzArKpLZNHIP+FxV/NoRH0WBXm9Wq5MrBYrD1NQzm+kInpS/2sXk3m1aZWqLm
HF2NKRXSbQAAA8iI+KSniPikpwAAAAdzc2gtcnNhAAABAQDErJhQxEI0+VvhlXVUyh+vMC
m7aXfCA/g633AG8ezD/5EylwchtAr2JCoBWnxn4zV8nI9dMqOgm0jO4IsXpKOjQojv+0VO
H7I+cDlBg0tk4hFlvyyS6YviDAfDDln3jYUM+5QNDfQLaZlH2WvcJ3mkDxLVlI9MBX1BAe
SmChLxwAvxALp2ncImNQLzDO9eHcig3dtMrEKkzXQowRW5Y7eUzg2+vvVq4H2DOjWwUndv
B5sJkhEfTUVE7ID8ZdGJo60kUb/02dZYj+IbkAnMCsqktk0cg/4XFX82hEfRYFeb1arkys
FisPU1DOb6QielL/axeTebVplaouYcXY0pFdJtAAAAAwEAAQAAAP8u3PFuTVV5SfGazwIm
MgNaux82iOsAT/HWFWecQAkqqrruUw5f+YajH/riV61NE9aq2qNOkcJrgpTWtqpt980GGd
SHWlgpRWQzfIooEiDk6Pk8RVFZsEykkDlJQSIu2onZjhi5A5ojHgZoGGabDsztSqoyOjPq
6WPvGYRiDAR3leBMyp1WufBCJqAsC4L8CjPJSmnZhc5a0zXkC9Syz74Fa08tdM7bGhtvP1
GmzuYxkgxHH2IFeoumUSBHRiTZayGuRUDel6jgEiUMxenaDKXe7FpYzMm9tQZA10Mm4LhK
5rP9nd2/KRTFRnfZMnKvtIRC9vtlSLBe14qw+4ZCl60AAACAf1kghlO3+HIWplOmk/lCL0
w75Zz+RdvueL9UuoyNN1QrUEY420LsixgWSeRPby+Rb/hW+XSAZJQHowQ8acFJhU85So7f
4O4wcDuE4f6hpsW9tTfkCEUdLCQJ7EKLCrod6jIV7hvI6rvXiVucRpeAzdOaq4uzj2cwDd
tOdYVsnmQAAACBAOVxBsvO/Sr3rZUbNtA6KewZh/09HNGoKNaCeiD7vaSn2UJbbPRByF/o
Oo5zv8ee8r3882NnmG808XfSn7pPZAzbbTmOaJt0fmyZhivCghSNzV6njW3o0PdnC0fGZQ
ruVXgkd7RJFbsIiD4dDcF4VCjwWHfTK21EOgJUA5pN6TNvAAAAgQDbcJWRx8Uyhkj2+srb
3n2Rt6CR7kEl9cw17ItFjMn+pO81/5U2aGw0iLlX7E06TAMQC+dyW/WaxQRey8RRdtbJ1e
TNKCN34QCWkyuYRHGhcNc0quEDayPw5QWGXlP4BzjfRUcPxY9cCXLe5wDLYsX33HwOAc59
RorU9FCmS/654wAAABFyb290QDhjNTBmZDRjMzQ1YQECAw==
-----END OPENSSH PRIVATE KEY-----";

/// Mock Ed25519 private key corresponding to [`MOCK_USER_CERTIFICATE`].
pub const MOCK_CERTIFICATE_PRIVATE_KEY: &str = r"-----BEGIN OPENSSH PRIVATE KEY-----
b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW
QyNTUxOQAAACDJ56wnEEIs+S6Q3HSkOG9CL52gyGhiks7nIpjnC7J2oQAAAJC5vJ0Eubyd
BAAAAAtzc2gtZWQyNTUxOQAAACDJ56wnEEIs+S6Q3HSkOG9CL52gyGhiks7nIpjnC7J2oQ
AAAECQinML6DJh4UnRw9JnmYxid2/uokrMij8QHexNdWbGbMnnrCcQQiz5LpDcdKQ4b0Iv
naDIaGKSzucimOcLsnahAAAADXJlbW90ZWZzLXRlc3Q=
-----END OPENSSH PRIVATE KEY-----";

/// Mock OpenSSH user certificate authorized by certificate-specific tests.
pub const MOCK_USER_CERTIFICATE: &str = "ssh-ed25519-cert-v01@openssh.com AAAAIHNzaC1lZDI1NTE5LWNlcnQtdjAxQG9wZW5zc2guY29tAAAAIAz/P/5CFqsedANCc3LTG39ioP5DjNVmIYFhLTkyZKicAAAAIMnnrCcQQiz5LpDcdKQ4b0IvnaDIaGKSzucimOcLsnahAAAAAAAAAAAAAAABAAAADXJlbW90ZWZzLXRlc3QAAAAIAAAABHNmdHAAAAAAXgvS8AAAAAD0hPdwAAAAAAAAAIIAAAAVcGVybWl0LVgxMS1mb3J3YXJkaW5nAAAAAAAAABdwZXJtaXQtYWdlbnQtZm9yd2FyZGluZwAAAAAAAAAWcGVybWl0LXBvcnQtZm9yd2FyZGluZwAAAAAAAAAKcGVybWl0LXB0eQAAAAAAAAAOcGVybWl0LXVzZXItcmMAAAAAAAAAAAAAADMAAAALc3NoLWVkMjU1MTkAAAAgTuSDWPlyniZISIy9nFEAIdcOxZqEWXAPXMW66gMlskUAAABTAAAAC3NzaC1lZDI1NTE5AAAAQEJOYJzhcZ2sz7BblmD/8kbL/ZjNvngxew+XIBjx1xich5HLFMYYOuY4MnpzlZHPaR9tAwo/gxiRQ5AbPd0owgQ= remotefs-test";

/// Mock certificate authority entry for the test server's `authorized_keys`.
pub const MOCK_CERTIFICATE_AUTHORITY: &str = "cert-authority ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIE7kg1j5cp4mSEiMvZxRACHXDsWahFlwD1zFuuoDJbJF remotefs-test";

/// Write the mock private key to a temp file and return it.
pub fn create_key_file() -> NamedTempFile {
    let mut key = NamedTempFile::new().expect("Failed to create tempfile");
    writeln!(key, "{MOCK_PRIVATE_KEY}").expect("Failed to write key");
    key
}

/// Write the mock certificate private key and certificate to temporary files.
pub fn create_certificate_key_files() -> (NamedTempFile, NamedTempFile) {
    let mut key = NamedTempFile::new().expect("Failed to create certificate key tempfile");
    writeln!(key, "{MOCK_CERTIFICATE_PRIVATE_KEY}")
        .expect("Failed to write certificate private key");
    let mut certificate = NamedTempFile::new().expect("Failed to create certificate tempfile");
    writeln!(certificate, "{MOCK_USER_CERTIFICATE}").expect("Failed to write certificate");
    (key, certificate)
}

/// Mock ssh key storage
pub struct MockSshKeyStorage {
    key: NamedTempFile,
}

impl Default for MockSshKeyStorage {
    fn default() -> Self {
        let mut key = NamedTempFile::new().expect("Failed to create tempfile");
        assert!(
            writeln!(
                key,
                r"-----BEGIN OPENSSH PRIVATE KEY-----
b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAABFwAAAAdzc2gtcn
NhAAAAAwEAAQAAAQEAxKyYUMRCNPlb4ZV1VMofrzApu2l3wgP4Ot9wBvHsw/+RMpcHIbQK
9iQqAVp8Z+M1fJyPXTKjoJtIzuCLF6Sjo0KI7/tFTh+yPnA5QYNLZOIRZb8skumL4gwHww
5Z942FDPuUDQ30C2mZR9lr3Cd5pA8S1ZSPTAV9QQHkpgoS8cAL8QC6dp3CJjUC8wzvXh3I
oN3bTKxCpM10KMEVuWO3lM4Nvr71auB9gzo1sFJ3bwebCZIRH01FROyA/GXRiaOtJFG/9N
nWWI/iG5AJzArKpLZNHIP+FxV/NoRH0WBXm9Wq5MrBYrD1NQzm+kInpS/2sXk3m1aZWqLm
HF2NKRXSbQAAA8iI+KSniPikpwAAAAdzc2gtcnNhAAABAQDErJhQxEI0+VvhlXVUyh+vMC
m7aXfCA/g633AG8ezD/5EylwchtAr2JCoBWnxn4zV8nI9dMqOgm0jO4IsXpKOjQojv+0VO
H7I+cDlBg0tk4hFlvyyS6YviDAfDDln3jYUM+5QNDfQLaZlH2WvcJ3mkDxLVlI9MBX1BAe
SmChLxwAvxALp2ncImNQLzDO9eHcig3dtMrEKkzXQowRW5Y7eUzg2+vvVq4H2DOjWwUndv
B5sJkhEfTUVE7ID8ZdGJo60kUb/02dZYj+IbkAnMCsqktk0cg/4XFX82hEfRYFeb1arkys
FisPU1DOb6QielL/axeTebVplaouYcXY0pFdJtAAAAAwEAAQAAAP8u3PFuTVV5SfGazwIm
MgNaux82iOsAT/HWFWecQAkqqrruUw5f+YajH/riV61NE9aq2qNOkcJrgpTWtqpt980GGd
SHWlgpRWQzfIooEiDk6Pk8RVFZsEykkDlJQSIu2onZjhi5A5ojHgZoGGabDsztSqoyOjPq
6WPvGYRiDAR3leBMyp1WufBCJqAsC4L8CjPJSmnZhc5a0zXkC9Syz74Fa08tdM7bGhtvP1
GmzuYxkgxHH2IFeoumUSBHRiTZayGuRUDel6jgEiUMxenaDKXe7FpYzMm9tQZA10Mm4LhK
5rP9nd2/KRTFRnfZMnKvtIRC9vtlSLBe14qw+4ZCl60AAACAf1kghlO3+HIWplOmk/lCL0
w75Zz+RdvueL9UuoyNN1QrUEY420LsixgWSeRPby+Rb/hW+XSAZJQHowQ8acFJhU85So7f
4O4wcDuE4f6hpsW9tTfkCEUdLCQJ7EKLCrod6jIV7hvI6rvXiVucRpeAzdOaq4uzj2cwDd
tOdYVsnmQAAACBAOVxBsvO/Sr3rZUbNtA6KewZh/09HNGoKNaCeiD7vaSn2UJbbPRByF/o
Oo5zv8ee8r3882NnmG808XfSn7pPZAzbbTmOaJt0fmyZhivCghSNzV6njW3o0PdnC0fGZQ
ruVXgkd7RJFbsIiD4dDcF4VCjwWHfTK21EOgJUA5pN6TNvAAAAgQDbcJWRx8Uyhkj2+srb
3n2Rt6CR7kEl9cw17ItFjMn+pO81/5U2aGw0iLlX7E06TAMQC+dyW/WaxQRey8RRdtbJ1e
TNKCN34QCWkyuYRHGhcNc0quEDayPw5QWGXlP4BzjfRUcPxY9cCXLe5wDLYsX33HwOAc59
RorU9FCmS/654wAAABFyb290QDhjNTBmZDRjMzQ1YQECAw==
-----END OPENSSH PRIVATE KEY-----"
            )
            .is_ok()
        );
        Self { key }
    }
}

impl SshKeyStorage for MockSshKeyStorage {
    fn resolve(&self, host: &str, username: &str) -> Option<std::path::PathBuf> {
        match (host, username) {
            ("sftp", "sftp") => Some(self.key.path().to_path_buf()),
            ("scp", "sftp") => Some(self.key.path().to_path_buf()),
            _ => None,
        }
    }
}

// -- config file

/// Create ssh config file
pub fn create_ssh_config(port: u16) -> NamedTempFile {
    let mut temp = NamedTempFile::new().expect("Failed to create tempfile");
    let config = format!(
        r##"
# ssh config
Compression yes
ConnectionAttempts  3
ConnectTimeout      60
Ciphers             aes128-ctr,aes192-ctr,aes256-ctr
KexAlgorithms       curve25519-sha256,diffie-hellman-group-exchange-sha256
MACs                hmac-sha2-512,hmac-sha2-256
# Hosts
Host sftp
    HostName    127.0.0.1
    Port        {port}
    User        sftp
Host scp
    HostName    127.0.0.1
    Port        {port}
    User        sftp
"##
    );
    temp.write_all(config.as_bytes()).unwrap();
    temp
}

/// Create an ssh config file that authenticates via `IdentityFile` (no key storage).
pub fn create_ssh_config_with_identity(
    port: u16,
    identity_file: &std::path::Path,
) -> NamedTempFile {
    create_ssh_config_with_identity_options(port, identity_file, true, None)
}

/// Create an ssh config file with `IdentityFile` and `PubkeyAuthentication`.
pub fn create_ssh_config_with_identity_and_pubkey_authentication(
    port: u16,
    identity_file: &std::path::Path,
    pubkey_authentication: bool,
) -> NamedTempFile {
    create_ssh_config_with_identity_options(port, identity_file, pubkey_authentication, None)
}

/// Create an ssh config file with `IdentityFile` and `PubkeyAcceptedAlgorithms`.
pub fn create_ssh_config_with_identity_and_pubkey_algorithms(
    port: u16,
    identity_file: &std::path::Path,
    pubkey_algorithms: &str,
) -> NamedTempFile {
    create_ssh_config_with_identity_options(port, identity_file, true, Some(pubkey_algorithms))
}

/// Create an ssh config file with `IdentityFile` and `CertificateFile`.
pub fn create_ssh_config_with_certificate(
    port: u16,
    identity_file: &std::path::Path,
    certificate_file: &std::path::Path,
) -> NamedTempFile {
    let mut temp = NamedTempFile::new().expect("Failed to create tempfile");
    writeln!(
        temp,
        "Host sftp\n    HostName 127.0.0.1\n    Port {port}\n    User sftp\n    IdentityFile {identity}\n    CertificateFile {certificate}",
        identity = identity_file.display(),
        certificate = certificate_file.display(),
    )
    .expect("Failed to write certificate SSH config");
    temp
}

/// Create an ssh config file that reaches a container-only host through a jump host.
pub fn create_ssh_config_with_proxy_jump(
    target_host: &str,
    target_port: u16,
    first_jump_port: u16,
    second_jump_host: &str,
) -> NamedTempFile {
    let mut temp = NamedTempFile::new().expect("Failed to create tempfile");
    writeln!(
        temp,
        "Host target\n    HostName {target_host}\n    Port {target_port}\n    User sftp\n    ProxyJump jump1,jump2\nHost jump1\n    HostName 127.0.0.1\n    Port {first_jump_port}\n    User sftp\n    ServerAliveInterval 30\nHost jump2\n    HostName {second_jump_host}\n    Port 2222\n    User sftp\n    ServerAliveInterval 30",
    )
    .expect("Failed to write proxy jump SSH config");
    temp
}

fn create_ssh_config_with_identity_options(
    port: u16,
    identity_file: &std::path::Path,
    pubkey_authentication: bool,
    pubkey_algorithms: Option<&str>,
) -> NamedTempFile {
    let mut temp = NamedTempFile::new().expect("Failed to create tempfile");
    let identity = identity_file.display();
    let pubkey_authentication = if pubkey_authentication { "yes" } else { "no" };
    let pubkey_algorithms = pubkey_algorithms
        .map(|algorithms| format!("    PubkeyAcceptedAlgorithms {algorithms}\n"))
        .unwrap_or_default();
    let config = format!(
        r##"
# ssh config
Compression yes
Ciphers             aes128-ctr,aes192-ctr,aes256-ctr
KexAlgorithms       curve25519-sha256,diffie-hellman-group-exchange-sha256
MACs                hmac-sha2-512,hmac-sha2-256
# Hosts
Host sftp
    HostName     127.0.0.1
    Port         {port}
    User         sftp
    IdentityFile {identity}
    PubkeyAuthentication {pubkey_authentication}
{pubkey_algorithms}Host scp
    HostName     127.0.0.1
    Port         {port}
    User         sftp
    IdentityFile {identity}
    PubkeyAuthentication {pubkey_authentication}
{pubkey_algorithms}"##
    );
    temp.write_all(config.as_bytes()).unwrap();
    temp
}
