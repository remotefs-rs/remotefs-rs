use argh::FromArgs;
use remotefs_ssh::{ScpFs, SftpFs, SshOpts};

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
}

impl From<ScpArgs> for ScpFs {
    fn from(args: ScpArgs) -> Self {
        ScpFs::new(
            SshOpts::new(args.hostname)
                .port(args.port)
                .username(args.username)
                .password(args.password),
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
}

impl From<SftpArgs> for SftpFs {
    fn from(args: SftpArgs) -> Self {
        SftpFs::new(
            SshOpts::new(args.hostname)
                .port(args.port)
                .username(args.username)
                .password(args.password),
        )
    }
}
