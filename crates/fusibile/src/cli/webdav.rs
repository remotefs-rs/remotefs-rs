use clap::Args;
use remotefs_webdav::{Auth, WebDAVFs};

/// Mount a WebDAV server filesystem
#[derive(Args)]
pub struct WebdavArgs {
    /// webDAV url
    #[arg(long)]
    url: String,
    /// basic auth username
    #[arg(long, requires = "password", conflicts_with = "bearer_token")]
    username: Option<String>,
    /// basic auth password
    #[arg(long, requires = "username", conflicts_with = "bearer_token")]
    password: Option<String>,
    /// bearer token for authentication
    #[arg(long, conflicts_with_all = ["username", "password"])]
    bearer_token: Option<String>,
}

impl std::fmt::Debug for WebdavArgs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebdavArgs")
            .field("url", &self.url)
            .field("username", &self.username)
            .field("password", &"[REDACTED]")
            .field("bearer_token", &"[REDACTED]")
            .finish()
    }
}

impl From<WebdavArgs> for WebDAVFs {
    fn from(args: WebdavArgs) -> Self {
        let auth = match (&args.username, &args.password, &args.bearer_token) {
            (Some(username), Some(password), None) => Auth::basic(username, password),
            (None, None, Some(token)) => Auth::bearer(token),
            _ => Auth::None,
        };

        WebDAVFs::new(&args.url, auth)
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
            username: Some("user".to_string()),
            password: Some(secret.to_string()),
            bearer_token: None,
        };
        let rendered = format!("{args:?}");
        assert!(!rendered.contains(secret));
        assert!(rendered.contains("[REDACTED]"));
    }
}
