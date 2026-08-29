use google_cloud_gax::error::rpc::Code;
use remotefs::{RemoteError, RemoteErrorType};

pub(crate) fn map_gcs_error(
    error: google_cloud_gax::error::Error,
    fallback: RemoteErrorType,
) -> RemoteError {
    let kind = match error.status().map(|status| status.code) {
        Some(Code::NotFound) => RemoteErrorType::NoSuchFileOrDirectory,
        Some(Code::AlreadyExists) => RemoteErrorType::DirectoryAlreadyExists,
        Some(Code::PermissionDenied) => RemoteErrorType::PexError,
        Some(Code::Unauthenticated) => RemoteErrorType::AuthenticationFailed,
        _ => fallback,
    };

    RemoteError::new_ex(kind, error)
}

#[cfg(test)]
mod tests {
    use google_cloud_gax::error::rpc::{Code, Status};
    use remotefs::RemoteErrorType;

    use super::map_gcs_error;

    fn service_error(code: Code) -> google_cloud_gax::error::Error {
        google_cloud_gax::error::Error::service(Status::default().set_code(code))
    }

    #[test]
    fn not_found_is_a_missing_remote_path() {
        let error = map_gcs_error(
            service_error(Code::NotFound),
            RemoteErrorType::ProtocolError,
        );
        assert_eq!(error.kind, RemoteErrorType::NoSuchFileOrDirectory);
    }

    #[test]
    fn already_exists_is_a_directory_conflict() {
        let error = map_gcs_error(
            service_error(Code::AlreadyExists),
            RemoteErrorType::ProtocolError,
        );
        assert_eq!(error.kind, RemoteErrorType::DirectoryAlreadyExists);
    }

    #[test]
    fn permission_denied_is_a_permissions_error() {
        let error = map_gcs_error(
            service_error(Code::PermissionDenied),
            RemoteErrorType::ProtocolError,
        );
        assert_eq!(error.kind, RemoteErrorType::PexError);
    }

    #[test]
    fn unauthenticated_is_an_authentication_error() {
        let error = map_gcs_error(
            service_error(Code::Unauthenticated),
            RemoteErrorType::ProtocolError,
        );
        assert_eq!(error.kind, RemoteErrorType::AuthenticationFailed);
    }
}
