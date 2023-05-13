#[derive(Debug, Default, Clone)]
pub struct SmbCredentials {
    pub(crate) server: String,
    pub(crate) share: String,
    pub(crate) username: Option<String>,
    pub(crate) password: Option<String>,
}

impl SmbCredentials {
    pub fn new<S: AsRef<str>>(server: S, share: S) -> Self {
        Self {
            server: server.as_ref().to_string(),
            share: share.as_ref().to_string(),
            ..Default::default()
        }
    }

    /// Construct SmbCredentials with the provided username
    pub fn username<S: AsRef<str>>(mut self, username: S) -> Self {
        self.username = Some(username.as_ref().to_string());
        self
    }

    /// Construct SmbCredentials with the provided password
    pub fn password<S: AsRef<str>>(mut self, password: S) -> Self {
        self.password = Some(password.as_ref().to_string());
        self
    }
}

#[cfg(test)]
mod test {

    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn should_init_credentials() {
        let credentials = SmbCredentials::new("localhost", "temp");
        assert_eq!(&credentials.server, "localhost");
        assert_eq!(&credentials.share, "temp");
        assert!(credentials.username.is_none());
        assert!(credentials.password.is_none());
    }

    #[test]
    fn should_construct_credentials() {
        let credentials = SmbCredentials::new("localhost", "temp")
            .username("test")
            .password("foobar");
        assert_eq!(&credentials.server, "localhost");
        assert_eq!(&credentials.share, "temp");
        assert_eq!(credentials.username.as_deref().unwrap(), "test");
        assert_eq!(credentials.password.as_deref().unwrap(), "foobar");
    }
}
