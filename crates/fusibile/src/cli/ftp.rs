use clap::Args;
use remotefs_ftp::FtpFs;

/// Mount an FTP server filesystem
#[derive(Args, Debug)]
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
