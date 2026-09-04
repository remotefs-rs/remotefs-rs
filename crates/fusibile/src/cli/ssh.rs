use std::path::{Path, PathBuf};
use std::sync::Arc;

use clap::Args;
use remotefs_ssh::{
    NoCheckServerKey, RusshSession, ScpFs, SftpFs, SshAgentIdentity, SshConfigParseRule, SshOpts,
};

/// Mount a SCP server filesystem
#[derive(Args)]
pub struct ScpArgs {
    /// hostname of the SCP server
    #[arg(long)]
    hostname: String,
    /// port of the SCP server
    #[arg(long, default_value_t = 22)]
    port: u16,
    /// username to authenticate with
    #[arg(long)]
    username: String,
    /// password to authenticate with
    #[arg(long)]
    password: String,
    /// path to the SSH config file
    #[arg(long, default_value_os_t = default_ssh_config_path())]
    config_file: std::path::PathBuf,
}

impl std::fmt::Debug for ScpArgs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScpArgs")
            .field("hostname", &self.hostname)
            .field("port", &self.port)
            .field("username", &self.username)
            .field("password", &"[REDACTED]")
            .field("config_file", &self.config_file)
            .finish()
    }
}

impl From<ScpArgs> for ScpFs<RusshSession<NoCheckServerKey>> {
    fn from(args: ScpArgs) -> Self {
        let rt = Arc::new(
            tokio::runtime::Builder::new_current_thread()
                .worker_threads(1)
                .enable_all()
                .build()
                .expect("Unable to create tokio runtime"),
        );

        ScpFs::russh(
            build_ssh_opts(
                &args.hostname,
                args.port,
                &args.username,
                &args.password,
                &args.config_file,
            ),
            rt,
        )
    }
}

/// Mount a SFTP server filesystem
#[derive(Args)]
pub struct SftpArgs {
    /// hostname of the SCP server
    #[arg(long)]
    hostname: String,
    /// port of the SCP server
    #[arg(long, default_value_t = 22)]
    port: u16,
    /// username to authenticate with
    #[arg(long)]
    username: String,
    /// password to authenticate with
    #[arg(long)]
    password: String,
    /// path to the SSH config file
    #[arg(long, default_value_os_t = default_ssh_config_path())]
    config_file: std::path::PathBuf,
}

impl std::fmt::Debug for SftpArgs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SftpArgs")
            .field("hostname", &self.hostname)
            .field("port", &self.port)
            .field("username", &self.username)
            .field("password", &"[REDACTED]")
            .field("config_file", &self.config_file)
            .finish()
    }
}

impl From<SftpArgs> for SftpFs<RusshSession<NoCheckServerKey>> {
    fn from(args: SftpArgs) -> Self {
        let rt = Arc::new(
            tokio::runtime::Builder::new_current_thread()
                .worker_threads(1)
                .enable_all()
                .build()
                .expect("Unable to create tokio runtime"),
        );

        SftpFs::russh(
            build_ssh_opts(
                &args.hostname,
                args.port,
                &args.username,
                &args.password,
                &args.config_file,
            ),
            rt,
        )
    }
}

fn build_ssh_opts(
    hostname: &str,
    port: u16,
    username: &str,
    password: &str,
    ssh_config_path: &Path,
) -> SshOpts {
    SshOpts::new(hostname)
        .port(port)
        .username(username)
        .password(password)
        .ssh_agent_identity(Some(SshAgentIdentity::All))
        .config_file(ssh_config_path, SshConfigParseRule::ALLOW_UNKNOWN_FIELDS)
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

    use super::{ScpArgs, SftpArgs};

    #[test]
    fn ssh_config_defaults_to_platform_home_directory() {
        let expected = dirs::home_dir()
            .expect("the test platform should provide a home directory")
            .join(".ssh")
            .join("config");

        let scp_matches = ScpArgs::augment_args(Command::new("scp"))
            .try_get_matches_from([
                "scp",
                "--hostname",
                "localhost",
                "--username",
                "user",
                "--password",
                "password",
            ])
            .expect("valid SCP arguments should parse");
        let scp_args =
            ScpArgs::from_arg_matches(&scp_matches).expect("valid SCP arguments should parse");

        let sftp_matches = SftpArgs::augment_args(Command::new("sftp"))
            .try_get_matches_from([
                "sftp",
                "--hostname",
                "localhost",
                "--username",
                "user",
                "--password",
                "password",
            ])
            .expect("valid SFTP arguments should parse");
        let sftp_args =
            SftpArgs::from_arg_matches(&sftp_matches).expect("valid SFTP arguments should parse");

        assert_eq!(scp_args.config_file, expected);
        assert_eq!(sftp_args.config_file, expected);
    }

    #[test]
    fn debug_should_redact_password() {
        let secret = "super-secret-password";

        let scp_args = ScpArgs {
            hostname: "localhost".to_string(),
            port: 22,
            username: "user".to_string(),
            password: secret.to_string(),
            config_file: PathBuf::from("/dev/null"),
        };
        let rendered = format!("{scp_args:?}");
        assert!(!rendered.contains(secret));
        assert!(rendered.contains("[REDACTED]"));

        let sftp_args = SftpArgs {
            hostname: "localhost".to_string(),
            port: 22,
            username: "user".to_string(),
            password: secret.to_string(),
            config_file: PathBuf::from("/dev/null"),
        };
        let rendered = format!("{sftp_args:?}");
        assert!(!rendered.contains(secret));
        assert!(rendered.contains("[REDACTED]"));
    }
}
