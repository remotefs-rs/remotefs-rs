//! Options for the Rust-native SMB client.

use std::time::Duration;

use remotefs::{RemoteError, RemoteErrorType, RemoteResult};
use smb::connection::EncryptionMode;
use smb::{ClientConfig, ConnectionConfig};

use crate::SmbDialect;

/// Transport encryption policy for [`SmbFs`](super::SmbFs).
///
/// Variants mirror `PavaoSmbEncryptionLevel`.
///
/// # Examples
///
/// ```
/// use remotefs_smb::SmbEncryptionLevel;
///
/// assert_eq!(SmbEncryptionLevel::default(), SmbEncryptionLevel::Request);
/// ```
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum SmbEncryptionLevel {
    /// Never encrypt, even if the server asks for it.
    None,
    /// Encrypt when the server supports it (default).
    #[default]
    Request,
    /// Fail the connection unless the server encrypts.
    Require,
}

impl From<SmbEncryptionLevel> for EncryptionMode {
    fn from(value: SmbEncryptionLevel) -> Self {
        match value {
            SmbEncryptionLevel::None => Self::Disabled,
            SmbEncryptionLevel::Request => Self::Allowed,
            SmbEncryptionLevel::Require => Self::Required,
        }
    }
}

/// Connection options for [`SmbFs`](super::SmbFs).
///
/// # Examples
///
/// ```
/// use std::time::Duration;
///
/// use remotefs_smb::{SmbEncryptionLevel, SmbOptions};
///
/// let _options = SmbOptions::default()
///     .encryption_level(SmbEncryptionLevel::Require)
///     .compression(true)
///     .timeout(Duration::from_secs(5));
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmbOptions {
    encryption_level: SmbEncryptionLevel,
    compression: bool,
    dfs: bool,
    timeout: Option<Duration>,
}

impl Default for SmbOptions {
    fn default() -> Self {
        Self {
            encryption_level: SmbEncryptionLevel::Request,
            compression: false,
            dfs: true,
            timeout: None,
        }
    }
}

impl SmbOptions {
    /// Sets the encryption policy. Defaults to [`SmbEncryptionLevel::Request`].
    pub fn encryption_level(mut self, level: SmbEncryptionLevel) -> Self {
        self.encryption_level = level;
        self
    }

    /// Enables SMB 3.1.1 compression when the server supports it. Off by default.
    pub fn compression(mut self, enabled: bool) -> Self {
        self.compression = enabled;
        self
    }

    /// Enables DFS referral resolution. On by default.
    pub fn dfs(mut self, enabled: bool) -> Self {
        self.dfs = enabled;
        self
    }

    /// Sets the request timeout. Defaults to the `smb` crate's 10 seconds.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }
}

/// Converts the crate dialect into the `smb` crate dialect.
///
/// Returns `None` for [`SmbDialect::Nt1`], which the pure-Rust client cannot
/// speak.
pub(super) fn to_smb_dialect(dialect: SmbDialect) -> Option<smb::Dialect> {
    match dialect {
        SmbDialect::Nt1 => None,
        SmbDialect::Smb202 => Some(smb::Dialect::Smb0202),
        SmbDialect::Smb210 => Some(smb::Dialect::Smb021),
        SmbDialect::Smb300 => Some(smb::Dialect::Smb030),
        SmbDialect::Smb302 => Some(smb::Dialect::Smb0302),
        SmbDialect::Smb311 => Some(smb::Dialect::Smb0311),
    }
}

/// Assembles and validates the `smb::ClientConfig` for a connection.
pub(super) fn build_client_config(
    options: &SmbOptions,
    port: Option<u16>,
    min_dialect: SmbDialect,
    max_dialect: SmbDialect,
) -> RemoteResult<ClientConfig> {
    let min_dialect = to_smb_dialect(min_dialect).ok_or_else(nt1_unsupported)?;
    let max_dialect = to_smb_dialect(max_dialect).ok_or_else(nt1_unsupported)?;
    let connection = ConnectionConfig {
        port,
        timeout: options.timeout,
        min_dialect: Some(min_dialect),
        max_dialect: Some(max_dialect),
        encryption_mode: options.encryption_level.into(),
        compression_enabled: options.compression,
        ..ConnectionConfig::default()
    };
    connection
        .validate()
        .map_err(|e| RemoteError::new_ex(RemoteErrorType::BadAddress, e))?;
    Ok(ClientConfig {
        dfs: options.dfs,
        connection,
        ..ClientConfig::default()
    })
}

fn nt1_unsupported() -> RemoteError {
    RemoteError::new_ex(
        RemoteErrorType::BadAddress,
        "the SMB1/CIFS NT1 dialect is not supported by the Rust-native client",
    )
}

#[cfg(test)]
mod test {
    use std::time::Duration;

    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn should_convert_supported_dialects() {
        assert_eq!(
            to_smb_dialect(SmbDialect::Smb202),
            Some(smb::Dialect::Smb0202)
        );
        assert_eq!(
            to_smb_dialect(SmbDialect::Smb210),
            Some(smb::Dialect::Smb021)
        );
        assert_eq!(
            to_smb_dialect(SmbDialect::Smb300),
            Some(smb::Dialect::Smb030)
        );
        assert_eq!(
            to_smb_dialect(SmbDialect::Smb302),
            Some(smb::Dialect::Smb0302)
        );
        assert_eq!(
            to_smb_dialect(SmbDialect::Smb311),
            Some(smb::Dialect::Smb0311)
        );
    }

    #[test]
    fn should_not_convert_nt1() {
        assert_eq!(to_smb_dialect(SmbDialect::Nt1), None);
    }

    #[test]
    fn should_convert_encryption_levels() {
        assert_eq!(
            EncryptionMode::from(SmbEncryptionLevel::None),
            EncryptionMode::Disabled
        );
        assert_eq!(
            EncryptionMode::from(SmbEncryptionLevel::Request),
            EncryptionMode::Allowed
        );
        assert_eq!(
            EncryptionMode::from(SmbEncryptionLevel::Require),
            EncryptionMode::Required
        );
    }

    #[test]
    fn should_build_default_client_config() {
        let config = build_client_config(
            &SmbOptions::default(),
            Some(3445),
            SmbDialect::Smb202,
            SmbDialect::Smb311,
        )
        .unwrap();
        assert!(config.dfs);
        assert_eq!(config.connection.port, Some(3445));
        assert_eq!(config.connection.min_dialect, Some(smb::Dialect::Smb0202));
        assert_eq!(config.connection.max_dialect, Some(smb::Dialect::Smb0311));
        assert_eq!(config.connection.encryption_mode, EncryptionMode::Allowed);
        assert!(!config.connection.compression_enabled);
        assert_eq!(config.connection.timeout, None);
        assert!(config.connection.auth_methods.ntlm);
    }

    #[test]
    fn should_apply_options() {
        let options = SmbOptions::default()
            .encryption_level(SmbEncryptionLevel::Require)
            .compression(true)
            .dfs(false)
            .timeout(Duration::from_secs(3));
        let config =
            build_client_config(&options, None, SmbDialect::Smb300, SmbDialect::Smb311).unwrap();
        assert!(!config.dfs);
        assert_eq!(config.connection.port, None);
        assert_eq!(config.connection.encryption_mode, EncryptionMode::Required);
        assert!(config.connection.compression_enabled);
        assert_eq!(config.connection.timeout, Some(Duration::from_secs(3)));
    }

    #[test]
    fn should_reject_nt1_bounds() {
        let err = build_client_config(
            &SmbOptions::default(),
            None,
            SmbDialect::Nt1,
            SmbDialect::Smb311,
        )
        .unwrap_err();
        assert_eq!(err.kind, RemoteErrorType::BadAddress);
    }

    #[test]
    fn should_reject_inverted_bounds() {
        let err = build_client_config(
            &SmbOptions::default(),
            None,
            SmbDialect::Smb311,
            SmbDialect::Smb202,
        )
        .unwrap_err();
        assert_eq!(err.kind, RemoteErrorType::BadAddress);
    }
}
