//! Failure types shared by every remotefs client.
//!
//! A client maps protocol failures onto [`RemoteErrorType`] and retains the
//! original cause in [`RemoteError`] whenever one is available. Callers can
//! classify failures without losing protocol-specific diagnostic information.

use std::error::Error as StdError;
use std::{fmt, io};

use thiserror::Error;

/// The result of any fallible [`crate::RemoteFs`] operation.
pub type RemoteResult<T> = Result<T, RemoteError>;

/// A protocol-agnostic failure with an optional typed source error.
#[non_exhaustive]
#[derive(Debug)]
pub struct RemoteError {
    kind: RemoteErrorType,
    source: Option<Box<dyn StdError + Send + Sync>>,
}

impl RemoteError {
    /// Creates an error without a source.
    ///
    /// # Examples
    ///
    /// ```
    /// use remotefs::{RemoteError, RemoteErrorType};
    ///
    /// let error = RemoteError::new(RemoteErrorType::NotConnected);
    /// assert_eq!(error.kind(), RemoteErrorType::NotConnected);
    /// assert!(std::error::Error::source(&error).is_none());
    /// ```
    pub fn new(kind: RemoteErrorType) -> Self {
        Self { kind, source: None }
    }

    /// Creates an error while preserving a typed source.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::io;
    ///
    /// use remotefs::{RemoteError, RemoteErrorType};
    ///
    /// let error = RemoteError::with_source(
    ///     RemoteErrorType::IoError,
    ///     io::Error::new(io::ErrorKind::UnexpectedEof, "short read"),
    /// );
    /// assert_eq!(error.kind(), RemoteErrorType::IoError);
    /// assert_eq!(error.to_string(), "IO error (short read)");
    /// ```
    pub fn with_source<E>(kind: RemoteErrorType, source: E) -> Self
    where
        E: StdError + Send + Sync + 'static,
    {
        Self {
            kind,
            source: Some(Box::new(source)),
        }
    }

    /// Creates an error with human-readable detail as its source message.
    ///
    /// # Examples
    ///
    /// ```
    /// use remotefs::{RemoteError, RemoteErrorType};
    ///
    /// let error = RemoteError::with_message(RemoteErrorType::BadFile, "not regular");
    /// assert_eq!(error.to_string(), "bad file (not regular)");
    /// ```
    pub fn with_message(kind: RemoteErrorType, message: impl Into<String>) -> Self {
        Self::with_source(kind, Message(message.into()))
    }

    /// Returns the protocol-agnostic classification of this error.
    pub const fn kind(&self) -> RemoteErrorType {
        self.kind
    }
}

#[derive(Debug, Error)]
#[error("{0}")]
struct Message(String);

impl fmt::Display for RemoteError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.source {
            Some(source) => write!(formatter, "{} ({source})", self.kind),
            None => self.kind.fmt(formatter),
        }
    }
}

impl StdError for RemoteError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        self.source
            .as_ref()
            .map(|source| &**source as &(dyn StdError + 'static))
    }
}

impl From<io::Error> for RemoteError {
    fn from(error: io::Error) -> Self {
        let kind = match error.kind() {
            io::ErrorKind::NotFound => RemoteErrorType::NoSuchFileOrDirectory,
            io::ErrorKind::PermissionDenied => RemoteErrorType::PermissionDenied,
            io::ErrorKind::AlreadyExists => RemoteErrorType::AlreadyExists,
            io::ErrorKind::NotConnected => RemoteErrorType::NotConnected,
            io::ErrorKind::InvalidInput => RemoteErrorType::InvalidPath,
            io::ErrorKind::Unsupported => RemoteErrorType::UnsupportedFeature,
            io::ErrorKind::DirectoryNotEmpty => RemoteErrorType::DirectoryNotEmpty,
            io::ErrorKind::ConnectionRefused
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::ConnectionAborted => RemoteErrorType::ConnectionError,
            _ => RemoteErrorType::IoError,
        };
        Self::with_source(kind, error)
    }
}

impl From<RemoteError> for io::Error {
    fn from(error: RemoteError) -> Self {
        let kind = match error.kind {
            RemoteErrorType::NoSuchFileOrDirectory => io::ErrorKind::NotFound,
            RemoteErrorType::PermissionDenied | RemoteErrorType::AuthenticationFailed => {
                io::ErrorKind::PermissionDenied
            }
            RemoteErrorType::AlreadyExists => io::ErrorKind::AlreadyExists,
            RemoteErrorType::NotConnected => io::ErrorKind::NotConnected,
            RemoteErrorType::InvalidPath
            | RemoteErrorType::SizeRequired
            | RemoteErrorType::BadAddress => io::ErrorKind::InvalidInput,
            RemoteErrorType::UnsupportedFeature => io::ErrorKind::Unsupported,
            RemoteErrorType::DirectoryNotEmpty => io::ErrorKind::DirectoryNotEmpty,
            RemoteErrorType::ConnectionError => io::ErrorKind::ConnectionAborted,
            RemoteErrorType::ProtocolError => io::ErrorKind::InvalidData,
            _ => io::ErrorKind::Other,
        };
        io::Error::new(kind, error)
    }
}

/// The protocol-agnostic classification of a [`RemoteError`].
#[non_exhaustive]
#[derive(Error, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RemoteErrorType {
    /// A connection was requested while one was already established.
    #[error("already connected")]
    AlreadyConnected,
    /// The connection is not established.
    #[error("not connected yet")]
    NotConnected,
    /// The server rejected the supplied credentials.
    #[error("authentication failed")]
    AuthenticationFailed,
    /// The host address could not be parsed.
    #[error("bad address syntax")]
    BadAddress,
    /// The transport could not be established or was lost.
    #[error("connection error")]
    ConnectionError,
    /// An entry already exists at the requested path.
    #[error("already exists")]
    AlreadyExists,
    /// The entry exists but is not usable for the requested operation.
    #[error("bad file")]
    BadFile,
    /// The file could not be opened.
    #[error("failed to open file")]
    CouldNotOpenFile,
    /// The file could not be removed.
    #[error("failed to remove file")]
    CouldNotRemoveFile,
    /// A directory was removed while it still had entries.
    #[error("directory is not empty")]
    DirectoryNotEmpty,
    /// The server refused to create a file.
    #[error("failed to create file")]
    FileCreateDenied,
    /// An input or output operation failed without a more specific category.
    #[error("IO error")]
    IoError,
    /// No entry exists at the requested path.
    #[error("no such file or directory")]
    NoSuchFileOrDirectory,
    /// The server refused a metadata or permission operation.
    #[error("not enough permissions")]
    PermissionDenied,
    /// A path is relative or otherwise malformed for the operation.
    #[error("invalid path")]
    InvalidPath,
    /// A protocol requires a transfer size before opening it.
    #[error("size required")]
    SizeRequired,
    /// The metadata of an entry could not be read.
    #[error("could not stat file")]
    StatFailed,
    /// The server answered in a way the protocol does not allow.
    #[error("protocol error")]
    ProtocolError,
    /// The protocol has no equivalent for the requested operation.
    #[error("unsupported feature")]
    UnsupportedFeature,
}

#[cfg(test)]
mod test {
    use std::error::Error;
    use std::io;

    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn should_format_errors() {
        let error =
            RemoteError::with_message(RemoteErrorType::NoSuchFileOrDirectory, "non va una mazza");
        assert_eq!(
            error.to_string(),
            "no such file or directory (non va una mazza)"
        );
        assert_eq!(
            RemoteError::new(RemoteErrorType::AlreadyConnected).to_string(),
            "already connected"
        );
        assert_eq!(
            RemoteError::new(RemoteErrorType::AuthenticationFailed).to_string(),
            "authentication failed"
        );
        assert_eq!(
            RemoteError::new(RemoteErrorType::BadAddress).to_string(),
            "bad address syntax"
        );
        assert_eq!(
            RemoteError::new(RemoteErrorType::ConnectionError).to_string(),
            "connection error"
        );
        assert_eq!(
            RemoteError::new(RemoteErrorType::StatFailed).to_string(),
            "could not stat file"
        );
        assert_eq!(
            RemoteError::new(RemoteErrorType::FileCreateDenied).to_string(),
            "failed to create file"
        );
        assert_eq!(
            RemoteError::new(RemoteErrorType::NoSuchFileOrDirectory).to_string(),
            "no such file or directory"
        );
        assert_eq!(
            RemoteError::new(RemoteErrorType::PermissionDenied).to_string(),
            "not enough permissions"
        );
        assert_eq!(
            RemoteError::new(RemoteErrorType::ProtocolError).to_string(),
            "protocol error"
        );
        assert_eq!(
            RemoteError::new(RemoteErrorType::NotConnected).to_string(),
            "not connected yet"
        );
        assert_eq!(
            RemoteError::new(RemoteErrorType::UnsupportedFeature).to_string(),
            "unsupported feature"
        );
    }

    #[test]
    fn should_report_error_cause() {
        let error = RemoteError::new(RemoteErrorType::UnsupportedFeature);
        assert!(error.source().is_none());
    }

    #[test]
    fn io_error_round_trip_preserves_original_source() {
        let original = io::Error::new(io::ErrorKind::PermissionDenied, "denied");
        let remote = RemoteError::from(original);
        assert_eq!(remote.kind(), RemoteErrorType::PermissionDenied);
        assert!(remote.source().unwrap().is::<io::Error>());

        let outer = io::Error::from(remote);
        assert_eq!(outer.kind(), io::ErrorKind::PermissionDenied);
        let remote = outer
            .get_ref()
            .unwrap()
            .downcast_ref::<RemoteError>()
            .unwrap();
        assert_eq!(remote.source().unwrap().to_string(), "denied");
    }

    #[test]
    fn remote_error_displays_kind_and_source() {
        assert_eq!(
            RemoteError::new(RemoteErrorType::InvalidPath).to_string(),
            "invalid path"
        );
        assert_eq!(
            RemoteError::with_message(RemoteErrorType::IoError, "denied").to_string(),
            "IO error (denied)"
        );
        assert_eq!(
            RemoteError::with_source(
                RemoteErrorType::ConnectionError,
                io::Error::new(io::ErrorKind::ConnectionReset, "reset"),
            )
            .to_string(),
            "connection error (reset)"
        );
    }

    #[test]
    fn io_error_kinds_map_to_remote_error_types() {
        let cases = [
            (
                io::ErrorKind::NotFound,
                RemoteErrorType::NoSuchFileOrDirectory,
            ),
            (
                io::ErrorKind::PermissionDenied,
                RemoteErrorType::PermissionDenied,
            ),
            (io::ErrorKind::AlreadyExists, RemoteErrorType::AlreadyExists),
            (io::ErrorKind::NotConnected, RemoteErrorType::NotConnected),
            (io::ErrorKind::InvalidInput, RemoteErrorType::InvalidPath),
            (
                io::ErrorKind::Unsupported,
                RemoteErrorType::UnsupportedFeature,
            ),
            (
                io::ErrorKind::DirectoryNotEmpty,
                RemoteErrorType::DirectoryNotEmpty,
            ),
            (
                io::ErrorKind::ConnectionRefused,
                RemoteErrorType::ConnectionError,
            ),
            (
                io::ErrorKind::ConnectionReset,
                RemoteErrorType::ConnectionError,
            ),
            (
                io::ErrorKind::ConnectionAborted,
                RemoteErrorType::ConnectionError,
            ),
            (io::ErrorKind::Other, RemoteErrorType::IoError),
        ];

        for (kind, expected) in cases {
            assert_eq!(RemoteError::from(io::Error::from(kind)).kind(), expected);
        }
    }
}
