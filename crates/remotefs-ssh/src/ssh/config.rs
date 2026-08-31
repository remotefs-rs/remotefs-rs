//! ## Config
//!
//! implements configuration resolver for ssh

use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::time::Duration;

use remotefs::{RemoteError, RemoteErrorType, RemoteResult};
use ssh2_config::{DefaultAlgorithms, HostParams, ParseRule, SshConfig};

use super::SshOpts;

/// Ssh configuration params
#[derive(Clone)]
pub struct Config {
    pub params: HostParams,
    pub host: String,
    /// Host resolved from configuration
    pub resolved_host: String,
    /// Address is host:port
    pub address: String,
    pub port: u16,
    pub username: String,
    pub connection_timeout: Duration,
    pub connection_attempts: usize,
}

impl Config {
    // -- private

    /// Create `Config` from `HostParams` and `SshOpts`
    fn from_params(params: HostParams, opts: &SshOpts) -> Self {
        let resolved_host = Self::resolve_host(&params, opts);
        let port = Self::resolve_port(&params, opts);
        Config {
            host: opts.host.to_string(),
            address: Self::format_address(&resolved_host, port),
            resolved_host,
            port,
            username: Self::resolve_username(&params, opts),
            connection_timeout: Self::resolve_connection_timeout(&params, opts),
            connection_attempts: Self::resolve_connection_attempts(&params),
            params,
        }
    }

    /// Parse config at `p` and get params for `host`
    fn parse(p: &Path, host: &str, rules: ParseRule) -> RemoteResult<HostParams> {
        trace!("Parsing configuration at {}", p.display());
        let mut reader = BufReader::new(File::open(p).map_err(|e| {
            RemoteError::new_ex(
                RemoteErrorType::IoError,
                format!("Could not open configuration file: {e}"),
            )
        })?);
        SshConfig::default()
            .parse(&mut reader, rules)
            .map_err(|e| {
                RemoteError::new_ex(
                    RemoteErrorType::IoError,
                    format!("Could not parse configuration file: {e}"),
                )
            })
            .map(|x| x.query(host))
    }

    /// Given host params and ssh options, returns resolved remote host
    fn resolve_host(params: &HostParams, opts: &SshOpts) -> String {
        // Host should be overridden
        match params.host_name.as_deref() {
            Some(h) => h.to_string(),
            None => opts.host.to_string(),
        }
    }

    fn resolve_port(params: &HostParams, opts: &SshOpts) -> u16 {
        // Opts.port has priority
        match opts.port {
            None => params.port.unwrap_or(22),
            Some(p) => p,
        }
    }

    fn format_address(host: &str, port: u16) -> String {
        if host.contains(':') && !host.starts_with('[') {
            format!("[{host}]:{port}")
        } else {
            format!("{host}:{port}")
        }
    }

    /// Resolve username from opts and params.
    /// If defined in opts, get username in opts,
    /// if define in params and not in opts, get from params,
    /// otherwise empty string
    fn resolve_username(params: &HostParams, opts: &SshOpts) -> String {
        match opts.username.as_ref() {
            Some(u) => u.to_string(),
            None => params.user.as_deref().unwrap_or("").to_string(),
        }
    }

    /// Given host params, resolve connection timeout
    fn resolve_connection_timeout(params: &HostParams, opts: &SshOpts) -> Duration {
        match opts.connection_timeout {
            Some(t) => t,
            None => params
                .connect_timeout
                .unwrap_or_else(|| Duration::from_secs(30)),
        }
    }

    /// Given host params, resolve connection attempts.
    /// If `none`, gets 1
    fn resolve_connection_attempts(params: &HostParams) -> usize {
        params.connection_attempts.unwrap_or(1)
    }

    /// Resolve each configured jump host using the same SSH configuration file.
    pub fn proxy_jump_configs(&self, opts: &SshOpts) -> RemoteResult<Vec<Self>> {
        let Some(proxy_jumps) = self.params.proxy_jump.as_deref() else {
            return Ok(Vec::new());
        };
        if proxy_jumps.len() == 1 && proxy_jumps[0].eq_ignore_ascii_case("none") {
            return Ok(Vec::new());
        }

        proxy_jumps
            .iter()
            .map(|proxy_jump| {
                let spec = ProxyJumpSpec::parse(proxy_jump)?;
                let mut params = if let Some(config_file) = opts.config_file.as_deref() {
                    Self::parse(config_file, &spec.host, opts.parse_rules)?
                } else {
                    HostParams::new(&DefaultAlgorithms::default())
                };
                // A comma-separated ProxyJump list already describes the complete
                // chain. Do not recursively apply a jump host's own ProxyJump.
                params.proxy_jump = None;
                let resolved_host = params
                    .host_name
                    .clone()
                    .unwrap_or_else(|| spec.host.clone());
                let port = spec.port.or(params.port).unwrap_or(22);
                if port == 0 {
                    return Err(ProxyJumpSpec::invalid(proxy_jump));
                }
                let username = spec
                    .user
                    .clone()
                    .or_else(|| params.user.clone())
                    .unwrap_or_default();

                Ok(Self {
                    address: Self::format_address(&resolved_host, port),
                    connection_attempts: Self::resolve_connection_attempts(&params),
                    connection_timeout: Self::resolve_connection_timeout(&params, opts),
                    host: spec.host,
                    params,
                    port,
                    resolved_host,
                    username,
                })
            })
            .collect()
    }
}

struct ProxyJumpSpec {
    host: String,
    port: Option<u16>,
    user: Option<String>,
}

impl ProxyJumpSpec {
    fn parse(value: &str) -> RemoteResult<Self> {
        let value = value.strip_prefix("ssh://").unwrap_or(value);
        if value.is_empty() || value.contains(['/', '?', '#']) {
            return Err(Self::invalid(value));
        }
        let (user, host_and_port) = value
            .rsplit_once('@')
            .map_or((None, value), |(user, host)| (Some(user.to_string()), host));
        if user.as_deref() == Some("") {
            return Err(Self::invalid(value));
        }

        let (host, port) = if let Some(bracketed) = host_and_port.strip_prefix('[') {
            let Some(closing_bracket) = bracketed.find(']') else {
                return Err(Self::invalid(value));
            };
            let host = &bracketed[..closing_bracket];
            let suffix = &bracketed[closing_bracket + 1..];
            let port = match suffix.strip_prefix(':') {
                Some(port) => Some(Self::parse_port(value, port)?),
                None if suffix.is_empty() => None,
                None => return Err(Self::invalid(value)),
            };
            (host, port)
        } else if host_and_port.matches(':').count() == 1 {
            let (host, port) = host_and_port
                .rsplit_once(':')
                .expect("checked delimiter count");
            (host, Some(Self::parse_port(value, port)?))
        } else {
            (host_and_port, None)
        };
        if host.is_empty() {
            return Err(Self::invalid(value));
        }

        Ok(Self {
            host: host.to_string(),
            port,
            user,
        })
    }

    fn parse_port(value: &str, port: &str) -> RemoteResult<u16> {
        port.parse()
            .ok()
            .filter(|port| *port != 0)
            .ok_or_else(|| Self::invalid(value))
    }

    fn invalid(value: &str) -> RemoteError {
        RemoteError::new_ex(
            RemoteErrorType::BadAddress,
            format!("invalid ProxyJump destination '{value}'"),
        )
    }
}

impl TryFrom<&SshOpts> for Config {
    type Error = RemoteError;

    fn try_from(opts: &SshOpts) -> Result<Self, Self::Error> {
        if let Some(p) = opts.config_file.as_deref() {
            let params = Self::parse(p, opts.host.as_str(), opts.parse_rules)?;
            Ok(Self::from_params(params, opts))
        } else {
            let params = HostParams::new(&DefaultAlgorithms::default());
            Ok(Self::from_params(params, opts))
        }
    }
}

#[cfg(test)]
mod test {
    use std::io::Write as _;

    use pretty_assertions::{assert_eq, assert_ne};
    use tempfile::NamedTempFile;

    use super::*;
    use crate::mock::ssh as ssh_mock;

    #[test]
    fn should_init_config_from_default_ssh_opts() {
        let opts = SshOpts::new("192.168.1.1");
        let config = Config::try_from(&opts).ok().unwrap();
        assert_eq!(config.connection_attempts, 1);
        assert_eq!(config.connection_timeout, Duration::from_secs(30));
        assert_eq!(config.address.as_str(), "192.168.1.1:22");
        assert_eq!(config.host.as_str(), "192.168.1.1");
        assert_eq!(config.port, 22);
        assert!(config.username.is_empty());
        assert_eq!(
            config.params,
            HostParams::new(&DefaultAlgorithms::default())
        );
    }

    #[test]
    fn should_init_config_from_custom_opts() {
        let opts = SshOpts::new("192.168.1.1")
            .connection_timeout(Duration::from_secs(10))
            .port(2222)
            .username("omar");
        let config = Config::try_from(&opts).ok().unwrap();
        assert_eq!(config.connection_attempts, 1);
        assert_eq!(config.connection_timeout, Duration::from_secs(10));
        assert_eq!(config.host.as_str(), "192.168.1.1");
        assert_eq!(config.address.as_str(), "192.168.1.1:2222");
        assert_eq!(config.port, 2222);
        assert_eq!(config.username.as_str(), "omar");
        assert_eq!(
            config.params,
            HostParams::new(&DefaultAlgorithms::default())
        );
    }

    #[test]
    fn should_init_config_from_file() {
        let config_file = ssh_mock::create_ssh_config(2222);
        let opts = SshOpts::new("sftp").config_file(config_file.path(), ParseRule::STRICT);
        let config = Config::try_from(&opts).ok().unwrap();
        assert_eq!(config.connection_attempts, 3);
        assert_eq!(config.connection_timeout, Duration::from_secs(60));
        assert_eq!(config.host.as_str(), "sftp");
        assert_eq!(config.resolved_host.as_str(), "127.0.0.1");
        assert_eq!(config.address.as_str(), "127.0.0.1:2222");
        assert_eq!(config.port, 2222);
        assert_eq!(config.username.as_str(), "sftp");
        assert_ne!(
            config.params,
            HostParams::new(&DefaultAlgorithms::default())
        );
    }

    #[test]
    fn should_init_config_from_file_with_override() {
        let config_file = ssh_mock::create_ssh_config(2222);
        let opts = SshOpts::new("sftp")
            .config_file(config_file.path(), ParseRule::STRICT)
            .connection_timeout(Duration::from_secs(10))
            .port(2200)
            .username("omar");
        let config = Config::try_from(&opts).ok().unwrap();
        assert_eq!(config.connection_attempts, 3);
        assert_eq!(config.connection_timeout, Duration::from_secs(10));
        assert_eq!(config.host.as_str(), "sftp");
        assert_eq!(config.resolved_host.as_str(), "127.0.0.1");
        assert_eq!(config.address.as_str(), "127.0.0.1:2200");
        assert_eq!(config.port, 2200);
        assert_eq!(config.username.as_str(), "omar");
        assert_ne!(
            config.params,
            HostParams::new(&DefaultAlgorithms::default())
        );
    }

    #[test]
    fn should_format_ipv6_address() {
        let opts = SshOpts::new("2001:db8::1").port(2222);

        let config = Config::try_from(&opts).expect("failed to resolve SSH config");

        assert_eq!(config.address.as_str(), "[2001:db8::1]:2222");
        assert_eq!(config.resolved_host.as_str(), "2001:db8::1");
        assert_eq!(config.port, 2222);
    }

    #[test]
    fn should_parse_proxy_jump_destinations() {
        let alias = ProxyJumpSpec::parse("jump").expect("failed to parse host alias");
        assert_eq!(alias.host, "jump");
        assert_eq!(alias.port, None);
        assert_eq!(alias.user, None);

        let destination = ProxyJumpSpec::parse("alice@jump.example.com:2222")
            .expect("failed to parse jump destination");
        assert_eq!(destination.host, "jump.example.com");
        assert_eq!(destination.port, Some(2222));
        assert_eq!(destination.user.as_deref(), Some("alice"));

        let uri = ProxyJumpSpec::parse("ssh://alice@[2001:db8::1]:2200")
            .expect("failed to parse jump URI");
        assert_eq!(uri.host, "2001:db8::1");
        assert_eq!(uri.port, Some(2200));
        assert_eq!(uri.user.as_deref(), Some("alice"));
    }

    #[test]
    fn should_reject_invalid_proxy_jump_destinations() {
        assert!(ProxyJumpSpec::parse("").is_err());
        assert!(ProxyJumpSpec::parse("alice@").is_err());
        assert!(ProxyJumpSpec::parse("jump:0").is_err());
        assert!(ProxyJumpSpec::parse("jump:invalid").is_err());
        assert!(ProxyJumpSpec::parse("ssh://jump/path").is_err());
        assert!(ProxyJumpSpec::parse("[2001:db8::1").is_err());
    }

    #[test]
    fn should_reject_zero_port_from_proxy_jump_alias() {
        let mut config_file = NamedTempFile::new().expect("failed to create SSH config");
        writeln!(
            config_file,
            "Host target\n    ProxyJump jump\nHost jump\n    Port 0"
        )
        .expect("failed to write SSH config");
        let opts = SshOpts::new("target").config_file(config_file.path(), ParseRule::STRICT);
        let config = Config::try_from(&opts).expect("failed to parse destination config");

        assert!(config.proxy_jump_configs(&opts).is_err());
    }
}
