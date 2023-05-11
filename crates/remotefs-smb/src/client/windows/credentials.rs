#[derive(Debug, Default, Clone)]
pub struct SmbCredentials {
    pub(crate) server: String,
    pub(crate) share: String,
}

impl SmbCredentials {
    /// Construct SmbCredentials with the provided server
    pub fn server<S: AsRef<str>>(mut self, server: S) -> Self {
        self.server = server.as_ref().to_string();
        self
    }

    /// Construct SmbCredentials with the provided share
    pub fn share<S: AsRef<str>>(mut self, share: S) -> Self {
        self.share = share.as_ref().to_string();
        self
    }
}
