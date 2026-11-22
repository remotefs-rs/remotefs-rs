use clap::Args;
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

/// Mount a SMB share filesystem
#[derive(Args)]
pub struct SmbArgs {
    /// hostname of the SCP server
    #[arg(long)]
    address: String,
    /// port of the SCP server
    #[cfg(unix)]
    #[arg(long, default_value_t = 139)]
    port: u16,
    /// username to authenticate with
    #[arg(long)]
    username: Option<String>,
    /// password to authenticate with
    #[arg(long)]
    password: Option<String>,
    /// share to mount
    #[arg(long)]
    share: String,
    /// workgroup to authenticate with
    #[cfg(unix)]
    #[arg(long)]
    workgroup: Option<String>,
    #[cfg(unix)]
    /// SMB dialect to use (auto, smb1, smb2, smb3)
    #[arg(long, default_value = "auto")]
    dialect: SmbDialect,
}

impl std::fmt::Debug for SmbArgs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut debug_struct = f.debug_struct("SmbArgs");
        debug_struct.field("address", &self.address);
        #[cfg(unix)]
        debug_struct.field("port", &self.port);
        debug_struct
            .field("username", &self.username)
            .field("password", &self.password.as_ref().map(|_| "[REDACTED]"))
            .field("share", &self.share);
        #[cfg(unix)]
        debug_struct
            .field("workgroup", &self.workgroup)
            .field("dialect", &self.dialect);
        debug_struct.finish()
    }
}

#[cfg(unix)]
impl TryFrom<SmbArgs> for SmbFs {
    type Error = anyhow::Error;

    fn try_from(args: SmbArgs) -> Result<Self, Self::Error> {
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
        .map_err(anyhow::Error::from)
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

#[cfg(all(test, unix))]
mod tests {
    use super::{SmbArgs, SmbDialect};

    #[test]
    fn debug_should_redact_password() {
        let secret = "super-secret-password";

        let args = SmbArgs {
            address: "localhost".to_string(),
            port: 139,
            username: Some("user".to_string()),
            password: Some(secret.to_string()),
            share: "share".to_string(),
            workgroup: None,
            dialect: SmbDialect::Auto,
        };
        let rendered = format!("{args:?}");
        assert!(!rendered.contains(secret));
        assert!(rendered.contains("[REDACTED]"));
    }
}
