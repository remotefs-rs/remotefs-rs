//! ## SSH
//!
//! implements the file transfer for SSH based protocols: SFTP and SCP

/**
 * MIT License
 *
 * remotefs - Copyright (c) 2021 Christian Visintin
 *
 * Permission is hereby granted, free of charge, to any person obtaining a copy
 * of this software and associated documentation files (the "Software"), to deal
 * in the Software without restriction, including without limitation the rights
 * to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
 * copies of the Software, and to permit persons to whom the Software is
 * furnished to do so, subject to the following conditions:
 *
 * The above copyright notice and this permission notice shall be included in all
 * copies or substantial portions of the Software.
 *
 * THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
 * IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
 * FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
 * AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
 * LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
 * OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
 * SOFTWARE.
 */
// -- ext
use std::path::{Path, PathBuf};
use std::time::Duration;

// -- modules
mod commons;
mod config;
mod scp;
mod sftp;
mod stream;
// -- export
pub use scp::ScpFs;
pub use sftp::SftpFs;
pub use ssh2::MethodType as SshMethodType;
use stream::{SftpReadStream, SftpWriteStream};

// -- Ssh key storage

/// This trait must be implemented in order to use ssh keys for authentication for sftp/scp.
pub trait SshKeyStorage {
    /// Return RSA key path from host and username
    fn resolve(&self, host: &str, username: &str) -> Option<&Path>;
}

// -- key method

/// Ssh key method.
/// Defined by `MethodType` (see ssh2 docs) and the list of supported algorithms.
pub struct KeyMethod {
    pub(crate) method_type: MethodType,
    algos: Vec<String>,
}

impl KeyMethod {
    /// Instantiates a new `KeyMethod`
    pub fn new(method_type: MethodType, algos: &[String]) -> Self {
        Self {
            method_type,
            algos: algos.to_vec(),
        }
    }

    /// Get preferred algos in ssh protocol syntax
    pub(crate) fn prefs(&self) -> String {
        self.algos.join(",")
    }
}

// -- ssh options

/// Ssh options;
/// used to build and configure SCP/SFTP client.
///
/// ### Conflict resolution
///
/// You may specify some options that can be in conflict (e.g. `port` and `Port` parameter in ssh configuration).
/// In these cases, the resolution is performed in this order (from highest, to lower priority):
///
/// 1. SshOpts attribute (e.g. `port` or `username`)
/// 2. Ssh configuration
///
/// This applies also to ciphers and key exchange methods.
///
pub struct SshOpts {
    /// hostname of the remote ssh server
    host: String,
    /// Port of the remote ssh server
    port: Option<u16>,
    /// Username to authenticate with
    username: Option<String>,
    /// Password to authenticate or to decrypt RSA key
    password: Option<String>,
    /// Connection timeout (default 30 seconds)
    connection_timeout: Option<Duration>,
    /// SSH configuration file. If provided will be parsed on connect.
    config_file: Option<PathBuf>,
    /// Key storage
    key_storage: Option<Box<dyn SshKeyStorage>>,
    /// Preferred key exchange methods.
    methods: Vec<KeyMethod>,
}

impl SshOpts {
    /// Initialize SshOpts.
    /// You must define the host you want to connect to.
    /// Host may be resolved by ssh configuration, if specified.
    ///
    /// Other options can be specified with other constructors.
    pub fn new<S: AsRef<str>>(host: S) -> Self {
        Self {
            host: host.as_ref().to_string(),
            port: None,
            username: None,
            password: None,
            connection_timeout: None,
            config_file: None,
            key_storage: None,
            methods: Vec::default(),
        }
    }

    /// Specify the port the remote server is listening to.
    /// This option will override an eventual port specified for the current host in the ssh configuration
    pub fn port(mut self, port: u16) -> Self {
        self.port = Some(port);
        self
    }

    /// Set username to log in as
    /// This option will override an eventual username specified for the current host in the ssh configuration
    pub fn username<S: AsRef<str>>(mut self, username: S) -> Self {
        self.username = Some(username.as_ref().to_string());
        self
    }

    /// Set password to authenticate with
    pub fn password<S: AsRef<str>>(mut self, password: S) -> Self {
        self.password = Some(password.as_ref().to_string());
        self
    }

    /// Set connection timeout
    /// This option will override an eventual connection timeout specified for the current host in the ssh configuration
    pub fn connection_timeout(mut self, timeout: Duration) -> Self {
        self.connection_timeout = Some(timeout);
        self
    }

    /// Set SSH configuration file to read
    ///
    /// The supported options are:
    ///
    /// - Host block
    /// - HostName
    /// - Port
    /// - User
    /// - Ciphers
    /// - MACs
    /// - KexAlgorithms
    /// - HostKeyAlgorithms
    /// - ConnectionAttempts
    /// - ConnectTimeout
    pub fn config_file<P: AsRef<Path>>(mut self, p: P) -> Self {
        self.config_file = Some(p.as_ref().to_path_buf());
        self
    }

    /// Set key storage to read RSA keys from
    pub fn key_storage(mut self, storage: Box<dyn SshKeyStorage>) -> Self {
        self.key_storage = Some(storage);
        self
    }

    /// Add key method to ssh options
    pub fn method(mut self, method: KeyMethod) -> Self {
        self.methods.push(method);
        self
    }
}

impl From<SshOpts> for SftpFs {
    fn from(opts: SshOpts) -> Self {
        SftpFs::new(opts)
    }
}

impl From<SshOpts> for ScpFs {
    fn from(opts: SshOpts) -> Self {
        ScpFs::new(opts)
    }
}

/// Re-implementation of ssh key method, in order to use `Eq`
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum MethodType {
    CryptClientServer,
    CryptServerClient,
    HostKey,
    Kex,
    MacClientServer,
    MacServerClient,
}

impl From<MethodType> for SshMethodType {
    fn from(t: MethodType) -> Self {
        match t {
            MethodType::CryptClientServer => SshMethodType::CryptCs,
            MethodType::CryptServerClient => SshMethodType::CryptSc,
            MethodType::HostKey => SshMethodType::HostKey,
            MethodType::Kex => SshMethodType::Kex,
            MethodType::MacClientServer => SshMethodType::MacCs,
            MethodType::MacServerClient => SshMethodType::MacSc,
        }
    }
}

#[cfg(test)]
mod test {

    use super::*;
    use crate::mock::ssh::MockSshKeyStorage;

    use pretty_assertions::assert_eq;

    #[test]
    fn should_create_key_method() {
        let key_method = KeyMethod::new(
            MethodType::CryptClientServer,
            &[
                "aes128-ctr".to_string(),
                "aes192-ctr".to_string(),
                "aes256-ctr".to_string(),
                "aes128-cbc".to_string(),
                "3des-cbc".to_string(),
            ],
        );
        assert_eq!(
            key_method.prefs().as_str(),
            "aes128-ctr,aes192-ctr,aes256-ctr,aes128-cbc,3des-cbc"
        );
    }

    #[test]
    fn should_initialize_ssh_opts() {
        let opts = SshOpts::new("localhost");
        assert_eq!(opts.host.as_str(), "localhost");
        assert!(opts.port.is_none());
        assert!(opts.username.is_none());
        assert!(opts.password.is_none());
        assert!(opts.connection_timeout.is_none());
        assert!(opts.config_file.is_none());
        assert!(opts.key_storage.is_none());
        assert!(opts.methods.is_empty());
    }

    #[test]
    fn should_build_ssh_opts() {
        let opts = SshOpts::new("localhost")
            .port(22)
            .username("foobar")
            .password("qwerty123")
            .connection_timeout(Duration::from_secs(10))
            .config_file(Path::new("/home/pippo/.ssh/config"))
            .key_storage(Box::new(MockSshKeyStorage::default()))
            .method(KeyMethod::new(
                MethodType::CryptClientServer,
                &[
                    "aes128-ctr".to_string(),
                    "aes192-ctr".to_string(),
                    "aes256-ctr".to_string(),
                    "aes128-cbc".to_string(),
                    "3des-cbc".to_string(),
                ],
            ));
        assert_eq!(opts.host.as_str(), "localhost");
        assert_eq!(opts.port.unwrap(), 22);
        assert_eq!(opts.username.as_deref().unwrap(), "foobar");
        assert_eq!(opts.password.as_deref().unwrap(), "qwerty123");
        assert_eq!(opts.connection_timeout.unwrap(), Duration::from_secs(10));
        assert_eq!(
            opts.config_file.as_deref().unwrap(),
            Path::new("/home/pippo/.ssh/config")
        );
        assert!(opts.key_storage.is_some());
        assert_eq!(opts.methods.len(), 1);
    }

    #[test]
    fn should_build_sftp_client() {
        let _: SftpFs = SshOpts::new("localhost").into();
    }

    #[test]
    fn should_build_scp_client() {
        let _: ScpFs = SshOpts::new("localhost").into();
    }
}
