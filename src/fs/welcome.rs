//! ## Welcome
//!
//! welcome data type

/// Structure holding all data related to a successful connection and authentication
/// on remote host.
#[derive(Debug, Default, Clone)]
pub struct Welcome {
    /// Welcome message / banner
    pub banner: Option<String>,
}

impl Welcome {
    /// Set welcome message or banner
    pub fn banner(mut self, banner: Option<String>) -> Self {
        self.banner = banner;
        self
    }
}

#[cfg(test)]
mod test {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn should_create_welcome_type() {
        let welcome = Welcome::default();
        assert!(welcome.banner.is_none());
        let welcome = Welcome::default().banner(Some("Hello, world!".to_string()));
        assert_eq!(welcome.banner.as_deref().unwrap(), "Hello, world!");
    }
}
