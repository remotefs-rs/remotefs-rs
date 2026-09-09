//! The greeting a server may send once a connection is established.

/// The outcome of a successful connection and authentication.
///
/// Returned by [`crate::RemoteFs::connect`]. Some protocols — FTP and SSH among
/// them — greet a client with a banner that is worth showing to a user; the ones
/// that do not simply leave [`Welcome::banner`] empty.
///
/// The type exists so that a protocol can grow a second piece of connection
/// metadata without breaking [`crate::RemoteFs::connect`]'s signature.
///
/// # Examples
///
/// ```
/// use remotefs::fs::Welcome;
///
/// let welcome = Welcome::default().banner(Some("Hello, world!".to_string()));
///
/// assert_eq!(welcome.banner.as_deref(), Some("Hello, world!"));
/// ```
#[non_exhaustive]
#[derive(Debug, Default, Clone)]
pub struct Welcome {
    /// The welcome message or banner sent by the server, when there is one.
    pub banner: Option<String>,
}

impl Welcome {
    /// Set the welcome message or banner, consuming and returning `self`.
    ///
    /// # Examples
    ///
    /// ```
    /// use remotefs::fs::Welcome;
    ///
    /// assert!(Welcome::default().banner(None).banner.is_none());
    /// ```
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
