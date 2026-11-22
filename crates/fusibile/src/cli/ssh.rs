use std::path::PathBuf;
use std::time::Duration;

use clap::Args;
use remotefs_ssh::{
    NoCheckServerKey, RusshScpFs, RusshSftpFs, SshAgentIdentity, SshConfigParseRule, SshOpts,
};

/// The SCP client `fusibile` mounts.
pub type ScpClient = RusshScpFs<NoCheckServerKey>;
/// The SFTP client `fusibile` mounts.
pub type SftpClient = RusshSftpFs<NoCheckServerKey>;

/// Mount a SSH server filesystem
#[derive(Args)]
pub struct SshArgs {
    /// hostname of the SSH server
    #[arg(long)]
    hostname: String,
    /// port of the SSH server
    #[arg(long)]
    port: Option<u16>,
    /// username to authenticate with
    #[arg(long)]
    username: Option<String>,
    /// password to authenticate with
    #[arg(long)]
    password: Option<String>,
    /// path to the SSH config file
    #[arg(long, default_value_os_t = default_ssh_config_path())]
    ssh_config: PathBuf,
    /// connection timeout (seconds)
    #[arg(long)]
    timeout: Option<u64>,
}

impl std::fmt::Debug for SshArgs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SshArgs")
            .field("hostname", &self.hostname)
            .field("port", &self.port)
            .field("username", &self.username)
            .field("password", &"[REDACTED]")
            .field("ssh_config", &self.ssh_config)
            .field("timeout", &self.timeout)
            .finish()
    }
}

impl TryFrom<SshArgs> for ScpClient {
    type Error = anyhow::Error;

    fn try_from(args: SshArgs) -> Result<Self, Self::Error> {
        Ok(RusshScpFs::new(build_ssh_opts(args)?))
    }
}

impl TryFrom<SshArgs> for SftpClient {
    type Error = anyhow::Error;

    fn try_from(args: SshArgs) -> Result<Self, Self::Error> {
        Ok(RusshSftpFs::new(build_ssh_opts(args)?))
    }
}

fn build_ssh_opts(
    SshArgs {
        hostname,
        port,
        username,
        password,
        ssh_config: ssh_config_path,
        timeout,
    }: SshArgs,
) -> anyhow::Result<SshOpts> {
    let is_ssh_config_path_default = ssh_config_path == default_ssh_config_path().as_path();
    let ssh_config_path_exists = ssh_config_path.exists();
    if !is_ssh_config_path_default && !ssh_config_path_exists {
        anyhow::bail!(
            "SSH config file does not exist at path: {path}",
            path = ssh_config_path.display()
        );
    }

    let mut opts = SshOpts::new(hostname).ssh_agent_identity(Some(SshAgentIdentity::All));
    if let Some(port) = port {
        log::debug!("port argument is specified; setting port to {port}");
        opts = opts.port(port);
    }
    if let Some(username) = username {
        log::debug!("username argument is specified; setting username to {username}");
        opts = opts.username(username);
    }
    if let Some(password) = password {
        log::debug!("password argument is specified; setting password");
        opts = opts.password(password);
    }
    if let Some(timeout) = timeout {
        log::debug!("timeout argument is specified; setting timeout to {timeout}");
        opts = opts.connection_timeout(Duration::from_secs(timeout));
    }

    if ssh_config_path_exists {
        Ok(opts.config_file(ssh_config_path, SshConfigParseRule::ALLOW_UNKNOWN_FIELDS))
    } else {
        log::debug!(
            "SSH config file does not exist at path: {path}, using default options",
            path = ssh_config_path.display()
        );
        Ok(opts)
    }
}

fn default_ssh_config_path() -> PathBuf {
    dirs::home_dir().map_or_else(
        || PathBuf::from(".ssh").join("config"),
        |home| home.join(".ssh").join("config"),
    )
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use clap::{Args, Command, FromArgMatches};

    use super::SshArgs;

    #[test]
    fn ssh_config_defaults_to_platform_home_directory() {
        let expected = dirs::home_dir()
            .expect("the test platform should provide a home directory")
            .join(".ssh")
            .join("config");

        let scp_matches = SshArgs::augment_args(Command::new("scp"))
            .try_get_matches_from([
                "scp",
                "--hostname",
                "localhost",
                "--username",
                "user",
                "--password",
                "password",
                "--timeout",
                "30",
            ])
            .expect("valid SCP arguments should parse");
        let scp_args =
            SshArgs::from_arg_matches(&scp_matches).expect("valid SCP arguments should parse");

        let sftp_matches = SshArgs::augment_args(Command::new("sftp"))
            .try_get_matches_from([
                "sftp",
                "--hostname",
                "localhost",
                "--username",
                "user",
                "--password",
                "password",
                "--timeout",
                "30",
            ])
            .expect("valid SFTP arguments should parse");
        let sftp_args =
            SshArgs::from_arg_matches(&sftp_matches).expect("valid SFTP arguments should parse");

        assert_eq!(scp_args.ssh_config, expected);
        assert_eq!(scp_args.timeout, Some(30));
        assert_eq!(sftp_args.ssh_config, expected);
        assert_eq!(sftp_args.timeout, Some(30));
    }

    #[test]
    fn debug_should_redact_password() {
        let secret = "super-secret-password";

        let args = SshArgs {
            hostname: "localhost".to_string(),
            port: Some(22),
            username: Some("user".to_string()),
            password: Some(secret.to_string()),
            ssh_config: PathBuf::from("/dev/null"),
            timeout: Some(30),
        };
        let rendered = format!("{args:?}");
        assert!(!rendered.contains(secret));
        assert!(rendered.contains("[REDACTED]"));
    }
}
