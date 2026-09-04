use clap::Args;
use remotefs_ftp::FtpFs;

/// Mount an FTP server filesystem
#[derive(Args)]
pub struct FtpArgs {
    /// FTP server hostname
    #[arg(long)]
    hostname: String,
    /// FTP server port
    #[arg(long, default_value_t = 21)]
    port: u16,
    /// FTP server username
    #[arg(long, default_value = "anonymous")]
    username: String,
    /// FTP server password
    #[arg(long)]
    password: Option<String>,
    /// use FTPS (FTP over TLS)
    #[arg(long)]
    secure: bool,
    /// active mode; default passive
    #[arg(long)]
    active: bool,
}

impl std::fmt::Debug for FtpArgs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FtpArgs")
            .field("hostname", &self.hostname)
            .field("port", &self.port)
            .field("username", &self.username)
            .field("password", &self.password.as_ref().map(|_| "[REDACTED]"))
            .field("secure", &self.secure)
            .field("active", &self.active)
            .finish()
    }
}

impl From<FtpArgs> for FtpFs {
    fn from(args: FtpArgs) -> Self {
        let mut ftp = FtpFs::new(args.hostname, args.port).username(args.username);

        if let Some(password) = args.password {
            ftp = ftp.password(password);
        }

        ftp = if args.active {
            ftp.active_mode()
        } else {
            ftp.passive_mode()
        };

        if args.secure {
            ftp.secure(true, true)
        } else {
            ftp
        }
    }
}

#[cfg(test)]
mod tests {
    use super::FtpArgs;

    #[test]
    fn debug_should_redact_password() {
        let secret = "super-secret-password";

        let args = FtpArgs {
            hostname: "localhost".to_string(),
            port: 21,
            username: "anonymous".to_string(),
            password: Some(secret.to_string()),
            secure: false,
            active: false,
        };
        let rendered = format!("{args:?}");
        assert!(!rendered.contains(secret));
        assert!(rendered.contains("[REDACTED]"));
    }
}
