//! ## Errors
//!
//! errors types

/**
 * MIT License
 *
 * remotefs - Copyright (c) 2021 Christian Visintin
 *
 * Permission is hereby granted, free of charge, to any person obtaining a copy
 * of this software and associated documentation files (the "Software"), to deal
 * in the Software without restriction, including without limitation the rights
 * to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
 * copies of the Software, and to permit persons to whom the Software is
 * furnished to do so, subject to the following conditions:
 *
 * The above copyright notice and this permission notice shall be included in all
 * copies or substantial portions of the Software.
 *
 * THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
 * IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
 * FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
 * AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
 * LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
 * OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
 * SOFTWARE.
 */
use std::fmt;
use thiserror::Error;

/// Result type returned by a `FileTransfer` implementation
pub type RemoteResult<T> = Result<T, RemoteError>;

/// RemoteError defines the possible errors available for a file transfer
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct RemoteError {
    pub code: RemoteErrorType,
    pub msg: Option<String>,
}

/// RemoteErrorType defines the possible errors available for a file transfer
#[derive(Error, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RemoteErrorType {
    #[error("already connected")]
    AlreadyConnected,
    #[error("authentication failed")]
    AuthenticationFailed,
    #[error("bad address syntax")]
    BadAddress,
    #[error("connection error")]
    ConnectionError,
    #[error("SSL error")]
    SslError,
    #[error("could not stat file")]
    StatFailed,
    #[error("bad file")]
    BadFile,
    #[error("directory already exists")]
    DirectoryAlreadyExists,
    #[error("failed to create file")]
    FileCreateDenied,
    #[error("failed to open file")]
    CouldNotOpenFile,
    #[error("failed to remove file")]
    CouldNotRemoveFile,
    #[error("IO error")]
    IoError,
    #[error("no such file or directory")]
    NoSuchFileOrDirectory,
    #[error("not enough permissions")]
    PexError,
    #[error("protocol error")]
    ProtocolError,
    #[error("not connected yet")]
    NotConnected,
    #[error("unsupported feature")]
    UnsupportedFeature,
}

impl RemoteError {
    /// Instantiates a new RemoteError
    pub fn new(code: RemoteErrorType) -> RemoteError {
        RemoteError { code, msg: None }
    }

    /// Instantiates a new RemoteError with message
    pub fn new_ex<S: ToString>(code: RemoteErrorType, msg: S) -> RemoteError {
        let mut err: RemoteError = RemoteError::new(code);
        err.msg = Some(msg.to_string());
        err
    }
}

impl fmt::Display for RemoteError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match &self.msg {
            Some(msg) => write!(f, "{} ({})", self.code, msg),
            None => write!(f, "{}", self.code),
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    use pretty_assertions::assert_eq;

    #[test]
    fn should_format_errors() {
        let err: RemoteError = RemoteError::new_ex(
            RemoteErrorType::NoSuchFileOrDirectory,
            String::from("non va una mazza"),
        );
        assert_eq!(*err.msg.as_ref().unwrap(), String::from("non va una mazza"));
        assert_eq!(
            format!("{}", err),
            String::from("no such file or directory (non va una mazza)")
        );
        assert_eq!(
            format!("{}", RemoteError::new(RemoteErrorType::AlreadyConnected)),
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
            format!("{}", RemoteError::new(RemoteErrorType::BadAddress)),
            String::from("bad address syntax")
        );
        assert_eq!(
            format!("{}", RemoteError::new(RemoteErrorType::ConnectionError)),
            String::from("connection error")
        );
        assert_eq!(
            format!("{}", RemoteError::new(RemoteErrorType::StatFailed)),
            String::from("could not stat file")
        );
        assert_eq!(
            format!("{}", RemoteError::new(RemoteErrorType::FileCreateDenied)),
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
            format!("{}", RemoteError::new(RemoteErrorType::PexError)),
            String::from("not enough permissions")
        );
        assert_eq!(
            format!("{}", RemoteError::new(RemoteErrorType::ProtocolError)),
            String::from("protocol error")
        );
        assert_eq!(
            format!("{}", RemoteError::new(RemoteErrorType::SslError)),
            String::from("SSL error")
        );
        assert_eq!(
            format!("{}", RemoteError::new(RemoteErrorType::NotConnected)),
            String::from("not connected yet")
        );
        assert_eq!(
            format!("{}", RemoteError::new(RemoteErrorType::UnsupportedFeature)),
            String::from("unsupported feature")
        );
        let err = RemoteError::new(RemoteErrorType::UnsupportedFeature);
        assert_eq!(err.code, RemoteErrorType::UnsupportedFeature);
    }
}
