use argh::FromArgs;
#[cfg(unix)]
use remotefs_smb::{
    PavaoSmbCredentials as SmbCredentials, PavaoSmbFs as SmbFs, PavaoSmbOptions as SmbOptions,
};
#[cfg(windows)]
use remotefs_smb::{WNetSmbCredentials as SmbCredentials, WNetSmbFs as SmbFs};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg(unix)]
pub enum SmbDialect {
    /// Automatically negotiate the dialect to use
    Auto,
    /// Use NT1 (SMB1) dialect
    Nt1,
    /// Use SMB2 dialect
    Smb2,
    /// Use SMB3 dialect
    Smb3,
}

#[cfg(unix)]
impl std::str::FromStr for SmbDialect {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "auto" => Ok(SmbDialect::Auto),
            "smb1" | "nt1" => Ok(SmbDialect::Nt1),
            "smb2" => Ok(SmbDialect::Smb2),
            "smb3" => Ok(SmbDialect::Smb3),
            _ => Err(format!("Invalid SMB dialect: {s}")),
        }
    }
}

#[cfg(unix)]
impl SmbDialect {
    pub fn min_max_dialect(&self) -> (remotefs_smb::SmbDialect, remotefs_smb::SmbDialect) {
        match self {
            SmbDialect::Auto => (
                remotefs_smb::SmbDialect::Smb202,
                remotefs_smb::SmbDialect::Smb311,
            ),
            SmbDialect::Nt1 => (remotefs_smb::SmbDialect::Nt1, remotefs_smb::SmbDialect::Nt1),
            SmbDialect::Smb2 => (
                remotefs_smb::SmbDialect::Smb202,
                remotefs_smb::SmbDialect::Smb210,
            ),
            SmbDialect::Smb3 => (
                remotefs_smb::SmbDialect::Smb300,
                remotefs_smb::SmbDialect::Smb311,
            ),
        }
    }
}

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
    #[cfg(unix)]
    /// SMB dialect to use (auto, smb1, smb2, smb3)
    #[argh(option, default = "SmbDialect::Auto")]
    dialect: SmbDialect,
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

        let (dialect_min, dialect_max) = args.dialect.min_max_dialect();
        SmbFs::try_new_with_dialect(
            credentials,
            SmbOptions::default()
                .one_share_per_server(true)
                .case_sensitive(false),
            dialect_min,
            dialect_max,
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
