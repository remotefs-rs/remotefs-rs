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

// -- modules
// mod scp;
mod commons;
mod sftp;
// -- export
// pub use scp::ScpFileTransfer;
pub use sftp::SftpFs;
pub use ssh2::MethodType;

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
/// used to build and configure SCP/SFTP client
pub struct SshOpts {
    /// Address or hostname of the remote ssh server
    address: String,
    /// Port of the remote ssh server
    port: u16,
    /// Username to authenticate with
    username: String,
    /// Password to authenticate or to decrypt RSA key
    password: Option<String>,
    /// SSH configuration file. If provided will be parsed on connect.
    config_file: Option<PathBuf>,
    /// Key storage
    key_storage: Option<Box<dyn SshKeyStorage>>,
    /// Preferred key exchange methods
    methods: Vec<KeyMethod>,
}

impl SshOpts {
    /// Initialize SshOpts.
    /// You must define the address or hostname of the remote server, the port number the server is listening to
    /// and the username you're going to use to authenticate
    pub fn new<S: AsRef<str>>(address: S, port: u16, username: S) -> Self {
        Self {
            address: address.as_ref().to_string(),
            port,
            username: username.as_ref().to_string(),
            password: None,
            config_file: None,
            key_storage: None,
            methods: Vec::default(),
        }
    }

    /// Set password to authenticate with
    pub fn password<S: AsRef<str>>(mut self, password: S) -> Self {
        self.password = Some(password.as_ref().to_string());
        self
    }

    /// Set SSH configuration file to read
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

impl Into<SftpFs> for SshOpts {
    fn into(self) -> SftpFs {
        SftpFs::new(self)
    }
}

// TODO: impl for ScpFs

#[cfg(test)]
mod test {

    use super::*;
    use crate::mock::ssh::MockSshKeyStorage;

    use pretty_assertions::assert_eq;

    #[test]
    fn should_create_key_method() {
        let key_method = KeyMethod::new(
            MethodType::CryptCs,
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
        let opts = SshOpts::new("localhost", 22, "foobar");
        assert_eq!(opts.address.as_str(), "localhost");
        assert_eq!(opts.port, 22);
        assert_eq!(opts.username.as_str(), "foobar");
        assert!(opts.password.is_none());
        assert!(opts.config_file.is_none());
        assert!(opts.key_storage.is_none());
        assert!(opts.methods.is_empty());
    }

    #[test]
    fn should_build_ssh_opts() {
        let opts = SshOpts::new("localhost", 22, "foobar")
            .password("qwerty123")
            .config_file(Path::new("/home/pippo/.ssh/config"))
            .key_storage(Box::new(MockSshKeyStorage::default()))
            .method(KeyMethod::new(
                MethodType::CryptCs,
                &[
                    "aes128-ctr".to_string(),
                    "aes192-ctr".to_string(),
                    "aes256-ctr".to_string(),
                    "aes128-cbc".to_string(),
                    "3des-cbc".to_string(),
                ],
            ));
        assert_eq!(opts.address.as_str(), "localhost");
        assert_eq!(opts.port, 22);
        assert_eq!(opts.username.as_str(), "foobar");
        assert_eq!(opts.password.as_deref().unwrap(), "qwerty123");
        assert_eq!(
            opts.config_file.as_deref().unwrap(),
            Path::new("/home/pippo/.ssh/config")
        );
        assert!(opts.key_storage.is_some());
        assert_eq!(opts.methods.len(), 1);
    }

    #[test]
    fn should_build_sftp_client() {
        let _: SftpFs = SshOpts::new("localhost", 22, "foobar").into();
    }
}
