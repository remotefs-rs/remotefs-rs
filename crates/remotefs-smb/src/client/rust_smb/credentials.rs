//! Credentials for the Rust-native SMB client.

use remotefs::{RemoteError, RemoteErrorType, RemoteResult};
use smb::UncPath;

/// Credentials used by [`SmbFs`](super::SmbFs) to reach an SMB share.
///
/// The builder accepts the same values as `PavaoSmbCredentials`: `server`
/// may carry the `smb://` scheme and a port, `share` may carry a leading
/// slash, and `workgroup` is folded into the logon name (`WORKGROUP\user`).
///
/// # Examples
///
/// ```
/// use remotefs_smb::SmbCredentials;
///
/// let _credentials = SmbCredentials::default()
///     .server("smb://localhost:3445")
///     .share("/temp")
///     .username("test")
///     .password("test")
///     .workgroup("pavao");
/// ```
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SmbCredentials {
    server: String,
    share: String,
    username: Option<String>,
    password: Option<String>,
    workgroup: Option<String>,
}

/// Credentials after parsing, ready for `smb::Client::share_connect`.
#[derive(Debug, Clone)]
pub(super) struct ResolvedCredentials {
    /// `\\host\share` with no path component.
    pub share: UncPath,
    /// Explicit TCP port, if the server string carried one.
    pub port: Option<u16>,
    /// Down-level logon name (`WORKGROUP\user`) or plain user name.
    pub logon_name: String,
    /// Password, empty when not provided.
    pub password: String,
}

impl SmbCredentials {
    /// Sets the server, as `smb://host[:port]`, `host[:port]`, or `\\host`.
    pub fn server<S: AsRef<str>>(mut self, server: S) -> Self {
        self.server = server.as_ref().to_string();
        self
    }

    /// Sets the share name; a leading slash is tolerated.
    pub fn share<S: AsRef<str>>(mut self, share: S) -> Self {
        self.share = share.as_ref().to_string();
        self
    }

    /// Sets the user name.
    pub fn username<S: AsRef<str>>(mut self, username: S) -> Self {
        self.username = Some(username.as_ref().to_string());
        self
    }

    /// Sets the password.
    pub fn password<S: AsRef<str>>(mut self, password: S) -> Self {
        self.password = Some(password.as_ref().to_string());
        self
    }

    /// Sets the workgroup (NetBIOS domain) used for NTLM authentication.
    pub fn workgroup<S: AsRef<str>>(mut self, workgroup: S) -> Self {
        self.workgroup = Some(workgroup.as_ref().to_string());
        self
    }

    /// Parses the credentials into the pieces the `smb` crate needs.
    pub(super) fn resolve(&self) -> RemoteResult<ResolvedCredentials> {
        let (host, port) = parse_server(&self.server)?;
        let share = self.share.trim_matches(['/', '\\']);
        if share.is_empty() {
            return Err(bad_address("share name is empty"));
        }
        let share = UncPath::new(&host)
            .and_then(|unc| unc.with_share(share))
            .map_err(bad_address)?;
        let username = self.username.clone().unwrap_or_default();
        let logon_name = match self.workgroup.as_deref() {
            Some(workgroup) if !workgroup.is_empty() && !username.is_empty() => {
                format!("{workgroup}\\{username}")
            }
            _ => username,
        };
        Ok(ResolvedCredentials {
            share,
            port,
            logon_name,
            password: self.password.clone().unwrap_or_default(),
        })
    }
}

/// Splits `smb://host[:port]` (scheme and slashes optional) into host and port.
pub(super) fn parse_server(server: &str) -> RemoteResult<(String, Option<u16>)> {
    let server = server.strip_prefix("smb://").unwrap_or(server);
    let server = server.trim_matches(['/', '\\']);
    if server.is_empty() {
        return Err(bad_address("server is empty"));
    }
    match server.rsplit_once(':') {
        Some((host, port)) if !host.is_empty() => {
            let port = port
                .parse::<u16>()
                .map_err(|_| bad_address(format!("invalid port '{port}'")))?;
            Ok((host.to_string(), Some(port)))
        }
        Some(_) => Err(bad_address("server host is empty")),
        None => Ok((server.to_string(), None)),
    }
}

fn bad_address<S: ToString>(msg: S) -> RemoteError {
    RemoteError::new_ex(RemoteErrorType::BadAddress, msg)
}

#[cfg(test)]
mod test {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn should_parse_server_with_scheme_and_port() {
        assert_eq!(
            parse_server("smb://localhost:3445").unwrap(),
            ("localhost".to_string(), Some(3445))
        );
    }

    #[test]
    fn should_parse_server_without_scheme_or_port() {
        assert_eq!(
            parse_server("fileserver").unwrap(),
            ("fileserver".to_string(), None)
        );
        assert_eq!(
            parse_server("//fileserver/").unwrap(),
            ("fileserver".to_string(), None)
        );
        assert_eq!(
            parse_server(r"\\fileserver").unwrap(),
            ("fileserver".to_string(), None)
        );
    }

    #[test]
    fn should_reject_empty_or_bad_server() {
        assert_eq!(
            parse_server("").unwrap_err().kind,
            RemoteErrorType::BadAddress
        );
        assert_eq!(
            parse_server("smb://").unwrap_err().kind,
            RemoteErrorType::BadAddress
        );
        assert_eq!(
            parse_server("host:notaport").unwrap_err().kind,
            RemoteErrorType::BadAddress
        );
    }

    #[test]
    fn should_resolve_credentials() {
        let resolved = SmbCredentials::default()
            .server("smb://localhost:3445")
            .share("/temp")
            .username("test")
            .password("secret")
            .workgroup("pavao")
            .resolve()
            .unwrap();
        assert_eq!(resolved.share.to_string(), r"\\localhost\temp");
        assert_eq!(resolved.port, Some(3445));
        assert_eq!(resolved.logon_name, r"pavao\test");
        assert_eq!(resolved.password, "secret");
    }

    #[test]
    fn should_resolve_anonymous_credentials_without_workgroup() {
        let resolved = SmbCredentials::default()
            .server("fileserver")
            .share("public")
            .resolve()
            .unwrap();
        assert_eq!(resolved.share.to_string(), r"\\fileserver\public");
        assert_eq!(resolved.port, None);
        assert_eq!(resolved.logon_name, "");
        assert_eq!(resolved.password, "");
    }

    #[test]
    fn should_reject_missing_share() {
        let err = SmbCredentials::default()
            .server("fileserver")
            .resolve()
            .unwrap_err();
        assert_eq!(err.kind, RemoteErrorType::BadAddress);
    }

    #[test]
    fn should_reject_share_with_path() {
        let err = SmbCredentials::default()
            .server("fileserver")
            .share("temp/sub")
            .resolve()
            .unwrap_err();
        assert_eq!(err.kind, RemoteErrorType::BadAddress);
    }
}
