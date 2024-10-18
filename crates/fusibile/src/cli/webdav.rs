use argh::FromArgs;
use remotefs_webdav::WebDAVFs;

#[derive(FromArgs, Debug)]
#[argh(subcommand, name = "webdav")]
/// Mount a WebDAV server filesystem
pub struct WebdavArgs {
    /// webDAV url
    #[argh(option)]
    url: String,
    /// webDAV username
    #[argh(option)]
    username: String,
    /// webDAV password
    #[argh(option)]
    password: String,
}

impl From<WebdavArgs> for WebDAVFs {
    fn from(args: WebdavArgs) -> Self {
        WebDAVFs::new(&args.url, &args.username, &args.password)
    }
}
