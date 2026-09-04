use clap::Args;
use remotefs_webdav::WebDAVFs;

/// Mount a WebDAV server filesystem
#[derive(Args)]
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

impl std::fmt::Debug for WebdavArgs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebdavArgs")
            .field("url", &self.url)
            .field("username", &self.username)
            .field("password", &"[REDACTED]")
            .finish()
    }
}

impl From<WebdavArgs> for WebDAVFs {
    fn from(args: WebdavArgs) -> Self {
        WebDAVFs::new(&args.username, &args.password, &args.url)
    }
}

#[cfg(test)]
mod tests {
    use super::WebdavArgs;

    #[test]
    fn debug_should_redact_password() {
        let secret = "super-secret-password";

        let args = WebdavArgs {
            url: "https://example.com".to_string(),
            username: "user".to_string(),
            password: secret.to_string(),
        };
        let rendered = format!("{args:?}");
        assert!(!rendered.contains(secret));
        assert!(rendered.contains("[REDACTED]"));
    }
}
