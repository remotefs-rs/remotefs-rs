//! Failure types shared by every remotefs client.
//!
//! A client never invents its own error type. It maps whatever the protocol
//! reported onto one of the [`RemoteErrorType`] variants, optionally attaching
//! the original message, and returns the pair as a [`RemoteError`]. Callers can
//! therefore match on the kind of failure without knowing which protocol
//! produced it, and still surface the protocol's own wording to a user.
//!
//! [`RemoteErrorType`] is deliberately small and closed. Adding a variant is a
//! breaking change for every crate in the family, so a protocol-specific failure
//! that does not fit an existing variant belongs in the message, not in a new
//! variant.

use std::error::Error as StdError;
use std::fmt;

use thiserror::Error;

/// The result of any fallible [`crate::RemoteFs`] operation.
pub type RemoteResult<T> = Result<T, RemoteError>;

/// A failure reported by a remote file system, with optional detail.
///
/// The [`kind`](RemoteError::kind) classifies the failure in protocol-agnostic
/// terms, so callers can react to it. The [`msg`](RemoteError::msg) carries
/// whatever the protocol said, so a user can be told what actually went wrong.
///
/// # Examples
///
/// ```
/// use remotefs::{RemoteError, RemoteErrorType};
///
/// let err = RemoteError::new_ex(RemoteErrorType::StatFailed, "permission denied");
///
/// assert_eq!(err.kind, RemoteErrorType::StatFailed);
/// assert_eq!(err.to_string(), "could not stat file (permission denied)");
/// ```
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct RemoteError {
    /// Protocol-agnostic classification of the failure.
    pub kind: RemoteErrorType,
    /// The detail reported by the protocol, when there is one.
    pub msg: Option<String>,
}

/// The protocol-agnostic classification of a [`RemoteError`].
///
/// Every variant renders as a short lower-case sentence through [`fmt::Display`],
/// which [`RemoteError`] embeds in its own message.
#[derive(Error, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RemoteErrorType {
    /// A connection was requested while one was already established.
    #[error("already connected")]
    AlreadyConnected,
    /// The server rejected the supplied credentials.
    #[error("authentication failed")]
    AuthenticationFailed,
    /// The host address could not be parsed.
    #[error("bad address syntax")]
    BadAddress,
    /// The transport could not be established or was lost.
    #[error("connection error")]
    ConnectionError,
    /// The TLS or SSH layer failed to negotiate or verify.
    #[error("SSL error")]
    SslError,
    /// The metadata of an entry could not be read.
    #[error("could not stat file")]
    StatFailed,
    /// The entry exists but is not usable for the requested operation.
    #[error("bad file")]
    BadFile,
    /// A directory was created at a path that already holds one.
    #[error("directory already exists")]
    DirectoryAlreadyExists,
    /// A directory was removed while it still had entries.
    #[error("directory is not empty")]
    DirectoryNotEmpty,
    /// The server refused to create the file.
    #[error("failed to create file")]
    FileCreateDenied,
    /// The file exists but could not be opened.
    #[error("failed to open file")]
    CouldNotOpenFile,
    /// The file exists but could not be removed.
    #[error("failed to remove file")]
    CouldNotRemoveFile,
    /// A read or write on the underlying stream failed.
    #[error("IO error")]
    IoError,
    /// No entry exists at the requested path.
    #[error("no such file or directory")]
    NoSuchFileOrDirectory,
    /// The credentials in use are not entitled to the operation.
    #[error("not enough permissions")]
    PexError,
    /// The server answered in a way the protocol does not allow.
    #[error("protocol error")]
    ProtocolError,
    /// An operation was requested before connecting.
    #[error("not connected yet")]
    NotConnected,
    /// The protocol has no equivalent for the requested operation.
    #[error("unsupported feature")]
    UnsupportedFeature,
}

impl RemoteError {
    /// Build an error of the given kind, with no detail.
    ///
    /// # Examples
    ///
    /// ```
    /// use remotefs::{RemoteError, RemoteErrorType};
    ///
    /// let err = RemoteError::new(RemoteErrorType::NotConnected);
    ///
    /// assert!(err.msg.is_none());
    /// assert_eq!(err.to_string(), "not connected yet");
    /// ```
    pub fn new(kind: RemoteErrorType) -> RemoteError {
        RemoteError { kind, msg: None }
    }

    /// Build an error of the given kind, carrying the protocol's own detail.
    ///
    /// Use this whenever the protocol said something more precise than the kind
    /// does; `msg` is rendered in parentheses after the kind.
    ///
    /// # Examples
    ///
    /// ```
    /// use remotefs::{RemoteError, RemoteErrorType};
    ///
    /// let err = RemoteError::new_ex(RemoteErrorType::ConnectionError, "connection reset");
    ///
    /// assert_eq!(err.to_string(), "connection error (connection reset)");
    /// ```
    pub fn new_ex<S: ToString>(kind: RemoteErrorType, msg: S) -> RemoteError {
        let mut err: RemoteError = RemoteError::new(kind);
        err.msg = Some(msg.to_string());
        err
    }
}

impl fmt::Display for RemoteError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match &self.msg {
            Some(msg) => write!(f, "{} ({})", self.kind, msg),
            None => write!(f, "{}", self.kind),
        }
    }
}

impl StdError for RemoteError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        Some(&self.kind)
    }
}

#[cfg(test)]
mod test {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn should_format_errors() {
        let err: RemoteError = RemoteError::new_ex(
            RemoteErrorType::NoSuchFileOrDirectory,
            String::from("non va una mazza"),
        );
        assert_eq!(*err.msg.as_ref().unwrap(), String::from("non va una mazza"));
        assert_eq!(
            err.to_string(),
            String::from("no such file or directory (non va una mazza)")
        );
        assert_eq!(
            RemoteError::new(RemoteErrorType::AlreadyConnected).to_string(),
            String::from("already connected")
        );
        assert_eq!(
            format!(
                "{}",
                RemoteError::new(RemoteErrorType::AuthenticationFailed)
            ),
            String::from("authentication failed")
        );
        assert_eq!(
            RemoteError::new(RemoteErrorType::BadAddress).to_string(),
            String::from("bad address syntax")
        );
        assert_eq!(
            RemoteError::new(RemoteErrorType::ConnectionError).to_string(),
            String::from("connection error")
        );
        assert_eq!(
            RemoteError::new(RemoteErrorType::StatFailed).to_string(),
            String::from("could not stat file")
        );
        assert_eq!(
            RemoteError::new(RemoteErrorType::FileCreateDenied).to_string(),
            String::from("failed to create file")
        );
        assert_eq!(
            format!(
                "{}",
                RemoteError::new(RemoteErrorType::NoSuchFileOrDirectory)
            ),
            String::from("no such file or directory")
        );
        assert_eq!(
            RemoteError::new(RemoteErrorType::PexError).to_string(),
            String::from("not enough permissions")
        );
        assert_eq!(
            RemoteError::new(RemoteErrorType::ProtocolError).to_string(),
            String::from("protocol error")
        );
        assert_eq!(
            RemoteError::new(RemoteErrorType::SslError).to_string(),
            String::from("SSL error")
        );
        assert_eq!(
            RemoteError::new(RemoteErrorType::NotConnected).to_string(),
            String::from("not connected yet")
        );
        assert_eq!(
            RemoteError::new(RemoteErrorType::UnsupportedFeature).to_string(),
            String::from("unsupported feature")
        );
        let err = RemoteError::new(RemoteErrorType::UnsupportedFeature);
        assert_eq!(err.kind, RemoteErrorType::UnsupportedFeature);
    }

    #[test]
    fn should_report_error_cause() {
        let error = RemoteError::new(RemoteErrorType::UnsupportedFeature);
        assert!(error.source().is_some());
    }
}
