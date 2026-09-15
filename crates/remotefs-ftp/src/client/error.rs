//! Maps `suppaftp` failures onto typed [`RemoteError`] values.

use std::error::Error as _;
use std::fmt;

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
            feature = "rustls-ring",
            feature = "tokio-native-tls",
            feature = "tokio-rustls-aws-lc-rs",
            feature = "tokio-rustls-ring"
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

/// Returns whether a failed data-command setup may have left replies unsynchronized.
pub(crate) fn transfer_setup_requires_reconnect(err: &FtpError) -> bool {
    match err {
        FtpError::ConnectionError(_) | FtpError::BadResponse => true,
        #[cfg(any(
            feature = "native-tls",
            feature = "rustls-aws-lc-rs",
            feature = "rustls-ring",
            feature = "tokio-native-tls",
            feature = "tokio-rustls-aws-lc-rs",
            feature = "tokio-rustls-ring"
        ))]
        FtpError::SecureError(_) => true,
        // A 421 closes the service. Other server replies have been consumed,
        // including the preliminary refusal path in `data_command_with_response`.
        FtpError::UnexpectedResponse(response) => response.status == Status::NotAvailable,
        FtpError::InvalidAddress(_) | FtpError::DataConnectionAlreadyOpen => true,
    }
}

/// Returns whether a mapped error means the control channel may be out of sync.
pub(crate) fn remote_error_requires_reconnect(err: &RemoteError) -> bool {
    let mut source = err.source();
    while let Some(error) = source {
        if let Some(ftp_error) = error.downcast_ref::<FtpError>() {
            return transfer_setup_requires_reconnect(ftp_error);
        }
        source = error.source();
    }
    err.kind() == RemoteErrorType::ConnectionError
}

/// Returns whether an FTP reply used the ambiguous 550 refusal code.
pub(crate) fn is_file_unavailable(err: &FtpError) -> bool {
    matches!(
        err,
        FtpError::UnexpectedResponse(response) if response.status == Status::FileUnavailable
    )
}

/// Returns whether a create or rename refusal can indicate a missing parent.
pub(crate) fn is_creation_refusal(err: &FtpError) -> bool {
    matches!(
        err,
        FtpError::UnexpectedResponse(response)
            if matches!(response.status, Status::FileUnavailable | Status::BadFilename)
    )
}

/// Returns whether an error preserves an ambiguous FTP 550 refusal.
pub(crate) fn is_ambiguous_path_refusal(err: &RemoteError) -> bool {
    let mut source = err.source();
    while let Some(error) = source {
        if let Some(FtpError::UnexpectedResponse(response)) = error.downcast_ref::<FtpError>() {
            return response.status == Status::FileUnavailable;
        }
        source = error.source();
    }
    false
}

/// Preserves an ambiguous 550 refusal when probing cannot prove absence.
pub(crate) fn ambiguous_path_permission(err: RemoteError) -> RemoteError {
    RemoteError::with_source(RemoteErrorType::PermissionDenied, err)
}

/// Retains both failures from a raw LIST data read and its completion reply.
#[derive(Debug)]
pub(crate) struct ListCleanupFailure {
    pub(crate) read: RemoteError,
    pub(crate) finish: RemoteError,
}

impl fmt::Display for ListCleanupFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "LIST read failed: {}; completion failed: {}",
            self.read, self.finish
        )
    }
}

impl std::error::Error for ListCleanupFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.finish)
    }
}

#[cfg(test)]
mod tests {
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
