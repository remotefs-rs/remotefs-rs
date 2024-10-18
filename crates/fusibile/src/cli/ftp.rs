use argh::FromArgs;
use remotefs_ftp::FtpFs;

#[derive(FromArgs, Debug)]
#[argh(subcommand, name = "ftp")]
/// Mount an FTP server filesystem
pub struct FtpArgs {
    /// FTP server hostname
    #[argh(option)]
    hostname: String,
    /// FTP server port
    #[argh(option, default = "21")]
    port: u16,
    /// FTP server username
    #[argh(option, default = "String::from(\"anonymous\")")]
    username: String,
    /// FTP server password
    #[argh(option)]
    password: Option<String>,
    /// use FTPS (FTP over TLS)
    #[argh(switch)]
    secure: bool,
    /// active mode; default passive
    #[argh(switch)]
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
            ftp.secure()
        } else {
            ftp
        }
    }
}
