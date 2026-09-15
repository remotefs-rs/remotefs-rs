//! Maps Google Cloud SDK failures onto [`RemoteError`].

use google_cloud_gax::error::Error;
use google_cloud_gax::error::rpc::Code;
use remotefs::{RemoteError, RemoteErrorType};

const HTTP_RANGE_NOT_SATISFIABLE: u16 = 416;
const HTTP_NOT_FOUND: u16 = 404;

/// Classifies a Google Cloud SDK error while retaining it as the source.
pub(crate) fn map_gcs_error(error: Error, fallback: RemoteErrorType) -> RemoteError {
    map_gcs_error_with_precondition(error, fallback, false)
}

/// Classifies a Google Cloud SDK object-creation failure.
pub(crate) fn map_gcs_create_error(error: Error, fallback: RemoteErrorType) -> RemoteError {
    map_gcs_error_with_precondition(error, fallback, true)
}

fn map_gcs_error_with_precondition(
    error: Error,
    fallback: RemoteErrorType,
    failed_precondition_is_already_exists: bool,
) -> RemoteError {
    let kind = if is_range_not_satisfiable(&error) {
        fallback
    } else {
        match (
            error.status().map(|status| status.code),
            error.http_status_code(),
        ) {
            (_, Some(HTTP_NOT_FOUND)) | (Some(Code::NotFound), _) => {
                RemoteErrorType::NoSuchFileOrDirectory
            }
            (Some(Code::AlreadyExists), _) => RemoteErrorType::AlreadyExists,
            (Some(Code::FailedPrecondition), _) if failed_precondition_is_already_exists => {
                RemoteErrorType::AlreadyExists
            }
            (Some(Code::PermissionDenied), _) => RemoteErrorType::PermissionDenied,
            (Some(Code::Unauthenticated), _) => RemoteErrorType::AuthenticationFailed,
            (Some(Code::Unavailable | Code::DeadlineExceeded), _) => {
                RemoteErrorType::ConnectionError
            }
            _ => fallback,
        }
    };

    RemoteError::with_source(kind, error)
}

/// Returns whether the error reports a requested byte range beyond the object.
pub(crate) fn is_range_not_satisfiable(error: &Error) -> bool {
    error.http_status_code() == Some(HTTP_RANGE_NOT_SATISFIABLE)
        || error
            .status()
            .is_some_and(|status| status.code == Code::OutOfRange)
}

#[cfg(test)]
mod tests {
    use google_cloud_gax::error::rpc::Status;
    use pretty_assertions::assert_eq;

    use super::*;

    fn service_error(code: Code) -> Error {
        Error::service(Status::default().set_code(code))
    }

    #[test]
    fn grpc_codes_map_to_remote_error_kinds() {
        let cases = [
            (Code::NotFound, RemoteErrorType::NoSuchFileOrDirectory),
            (Code::AlreadyExists, RemoteErrorType::AlreadyExists),
            (Code::FailedPrecondition, RemoteErrorType::ProtocolError),
            (Code::PermissionDenied, RemoteErrorType::PermissionDenied),
            (Code::Unauthenticated, RemoteErrorType::AuthenticationFailed),
            (Code::Unavailable, RemoteErrorType::ConnectionError),
            (Code::DeadlineExceeded, RemoteErrorType::ConnectionError),
            (Code::Internal, RemoteErrorType::ProtocolError),
        ];
        for (code, expected) in cases {
            let error = map_gcs_error(service_error(code), RemoteErrorType::ProtocolError);
            assert_eq!(error.kind(), expected, "code: {code:?}");
            assert!(std::error::Error::source(&error).is_some());
        }
    }

    #[test]
    fn create_errors_map_failed_precondition_to_already_exists() {
        let error = map_gcs_create_error(
            service_error(Code::FailedPrecondition),
            RemoteErrorType::FileCreateDenied,
        );
        assert_eq!(error.kind(), RemoteErrorType::AlreadyExists);
        assert!(std::error::Error::source(&error).is_some());
    }

    #[test]
    fn out_of_range_is_range_not_satisfiable() {
        assert!(is_range_not_satisfiable(&service_error(Code::OutOfRange)));
        assert!(!is_range_not_satisfiable(&service_error(Code::NotFound)));
    }
}
