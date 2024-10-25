use argh::FromArgs;
use remotefs_smb::{SmbCredentials, SmbFs, SmbOptions};

#[derive(FromArgs, Debug)]
#[argh(subcommand, name = "smb")]
/// Mount a SMB share filesystem
pub struct SmbArgs {
    /// hostname of the SCP server
    #[argh(option)]
    address: String,
    /// port of the SCP server
    #[cfg(unix)]
    #[argh(option, default = "139")]
    port: u16,
    /// username to authenticate with
    #[argh(option)]
    username: Option<String>,
    /// password to authenticate with
    #[argh(option)]
    password: Option<String>,
    /// share to mount
    #[argh(option)]
    share: String,
    /// workgroup to authenticate with
    #[cfg(unix)]
    #[argh(option)]
    workgroup: Option<String>,
}

#[cfg(unix)]
impl From<SmbArgs> for SmbFs {
    fn from(args: SmbArgs) -> Self {
        let mut credentials = SmbCredentials::default()
            .server(format!("smb://{}:{}", args.address, args.port))
            .share(args.share);

        if let Some(username) = args.username {
            credentials = credentials.username(username);
        }
        if let Some(password) = args.password {
            credentials = credentials.password(password);
        }
        if let Some(workgroup) = args.workgroup {
            credentials = credentials.workgroup(workgroup);
        }

        SmbFs::try_new(
            credentials,
            SmbOptions::default()
                .one_share_per_server(true)
                .case_sensitive(false),
        )
        .expect("Failed to create SMB client")
    }
}

#[cfg(target_family = "windows")]
impl From<SmbArgs> for SmbFs {
    fn from(args: SmbArgs) -> Self {
        let mut credentials = SmbCredentials::new(args.address, args.share);

        if let Some(username) = args.username {
            credentials = credentials.username(username);
        }
        if let Some(password) = args.password {
            credentials = credentials.password(password);
        }

        SmbFs::new(credentials)
    }
}
