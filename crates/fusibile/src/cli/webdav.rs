use clap::Args;
use remotefs_webdav::WebDAVFs;

/// Mount a WebDAV server filesystem
#[derive(Args, Debug)]
pub struct WebdavArgs {
    /// webDAV url
    #[arg(long)]
    url: String,
    /// webDAV username
    #[arg(long)]
    username: String,
    /// webDAV password
    #[arg(long)]
    password: String,
}

impl From<WebdavArgs> for WebDAVFs {
    fn from(args: WebdavArgs) -> Self {
        WebDAVFs::new(&args.username, &args.password, &args.url)
    }
}
