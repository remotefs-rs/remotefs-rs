//! Maps `suppaftp` failures onto typed [`RemoteError`] values.

use remotefs::{RemoteError, RemoteErrorType};
use suppaftp::{FtpError, Status};

/// Converts an FTP failure into a [`RemoteError`] while keeping `err` as the
/// typed error source.
///
/// Reply codes are classified as follows; every other failure is a
/// [`RemoteErrorType::ProtocolError`].
///
/// | failure                        | kind                    |
/// | ------------------------------ | ----------------------- |
/// | transport or TLS error         | `ConnectionError`       |
/// | `421`, `425`, `426`            | `ConnectionError`       |
/// | `530`                          | `AuthenticationFailed`  |
/// | `532`, `553`                   | `PermissionDenied`      |
/// | `550`                          | `NoSuchFileOrDirectory` |
/// | `500`, `501`, `502`, `504`     | `UnsupportedFeature`    |
/// | invalid address                | `BadAddress`            |
/// | data connection already open   | `ProtocolError`         |
pub(crate) fn ftp_error(err: FtpError) -> RemoteError {
    let kind = match &err {
        FtpError::ConnectionError(_) => RemoteErrorType::ConnectionError,
        #[cfg(any(
            feature = "native-tls",
            feature = "rustls-aws-lc-rs",
            feature = "rustls-ring"
        ))]
        FtpError::SecureError(_) => RemoteErrorType::ConnectionError,
        FtpError::InvalidAddress(_) => RemoteErrorType::BadAddress,
        FtpError::UnexpectedResponse(response) => status_kind(&response.status),
        FtpError::BadResponse | FtpError::DataConnectionAlreadyOpen => {
            RemoteErrorType::ProtocolError
        }
    };
    RemoteError::with_source(kind, err)
}

/// Classifies an FTP reply code the server used to refuse a command.
fn status_kind(status: &Status) -> RemoteErrorType {
    match status {
        Status::NotAvailable | Status::CannotOpenDataConnection | Status::TransferAborted => {
            RemoteErrorType::ConnectionError
        }
        Status::NotLoggedIn => RemoteErrorType::AuthenticationFailed,
        Status::StoringNeedAccount | Status::BadFilename => RemoteErrorType::PermissionDenied,
        Status::FileUnavailable => RemoteErrorType::NoSuchFileOrDirectory,
        Status::BadCommand
        | Status::BadArguments
        | Status::NotImplemented
        | Status::NotImplementedParameter => RemoteErrorType::UnsupportedFeature,
        _ => RemoteErrorType::ProtocolError,
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as _;
    use std::io;

    use pretty_assertions::assert_eq;
    use suppaftp::types::Response;

    use super::*;

    fn refused(status: Status) -> FtpError {
        FtpError::UnexpectedResponse(Response {
            status,
            body: b"refused".to_vec(),
        })
    }

    #[test]
    fn should_map_transport_failures_to_connection_error_and_keep_source() {
        let err = ftp_error(FtpError::ConnectionError(io::Error::new(
            io::ErrorKind::ConnectionReset,
            "reset",
        )));
        assert_eq!(err.kind(), RemoteErrorType::ConnectionError);
        assert!(err.source().unwrap().is::<FtpError>());
        assert!(err.to_string().contains("reset"));
    }

    #[test]
    fn should_map_reply_codes() {
        for (status, kind) in [
            (Status::NotAvailable, RemoteErrorType::ConnectionError),
            (
                Status::CannotOpenDataConnection,
                RemoteErrorType::ConnectionError,
            ),
            (Status::TransferAborted, RemoteErrorType::ConnectionError),
            (Status::NotLoggedIn, RemoteErrorType::AuthenticationFailed),
            (Status::BadFilename, RemoteErrorType::PermissionDenied),
            (
                Status::StoringNeedAccount,
                RemoteErrorType::PermissionDenied,
            ),
            (
                Status::FileUnavailable,
                RemoteErrorType::NoSuchFileOrDirectory,
            ),
            (Status::BadCommand, RemoteErrorType::UnsupportedFeature),
            (Status::NotImplemented, RemoteErrorType::UnsupportedFeature),
            (Status::BadArguments, RemoteErrorType::UnsupportedFeature),
            (
                Status::NotImplementedParameter,
                RemoteErrorType::UnsupportedFeature,
            ),
            (Status::BadSequence, RemoteErrorType::ProtocolError),
        ] {
            assert_eq!(ftp_error(refused(status)).kind(), kind, "status {status:?}");
        }
    }

    #[test]
    fn should_map_protocol_failures() {
        assert_eq!(
            ftp_error(FtpError::DataConnectionAlreadyOpen).kind(),
            RemoteErrorType::ProtocolError
        );
        assert_eq!(
            ftp_error(FtpError::BadResponse).kind(),
            RemoteErrorType::ProtocolError
        );
    }
}
