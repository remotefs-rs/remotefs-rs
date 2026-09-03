use std::path::{Path, PathBuf};
use std::sync::Arc;

use argh::FromArgs;
use remotefs_ssh::{
    NoCheckServerKey, RusshSession, ScpFs, SftpFs, SshAgentIdentity, SshConfigParseRule, SshOpts,
};

#[derive(FromArgs, Debug)]
#[argh(subcommand, name = "scp")]
/// Mount a SCP server filesystem
pub struct ScpArgs {
    /// hostname of the SCP server
    #[argh(option)]
    hostname: String,
    /// port of the SCP server
    #[argh(option, default = "22")]
    port: u16,
    /// username to authenticate with
    #[argh(option)]
    username: String,
    /// password to authenticate with
    #[argh(option)]
    password: String,
    /// path to the SSH config file
    #[argh(option, default = "default_ssh_config_path()")]
    config_file: std::path::PathBuf,
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

#[derive(FromArgs, Debug)]
#[argh(subcommand, name = "sftp")]
/// Mount a SFTP server filesystem
pub struct SftpArgs {
    /// hostname of the SCP server
    #[argh(option)]
    hostname: String,
    /// port of the SCP server
    #[argh(option, default = "22")]
    port: u16,
    /// username to authenticate with
    #[argh(option)]
    username: String,
    /// password to authenticate with
    #[argh(option)]
    password: String,
    /// path to the SSH config file
    #[argh(option, default = "default_ssh_config_path()")]
    config_file: std::path::PathBuf,
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
    use argh::FromArgs;

    use super::{ScpArgs, SftpArgs};

    #[test]
    fn ssh_config_defaults_to_platform_home_directory() {
        let expected = dirs::home_dir()
            .expect("the test platform should provide a home directory")
            .join(".ssh")
            .join("config");
        let scp_args = ScpArgs::from_args(
            &["scp"],
            &[
                "--hostname",
                "localhost",
                "--username",
                "user",
                "--password",
                "password",
            ],
        )
        .expect("valid SCP arguments should parse");
        let sftp_args = SftpArgs::from_args(
            &["sftp"],
            &[
                "--hostname",
                "localhost",
                "--username",
                "user",
                "--password",
                "password",
            ],
        )
        .expect("valid SFTP arguments should parse");

        assert_eq!(scp_args.config_file, expected);
        assert_eq!(sftp_args.config_file, expected);
    }
}
