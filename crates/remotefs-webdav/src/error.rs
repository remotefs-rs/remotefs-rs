//! Mapping of `dav-xml-client` failures onto [`remotefs::RemoteError`].

use dav_xml_client::Error;
use dav_xml_client::transport::TransportErrorKind;
use http::StatusCode;
use remotefs::{RemoteError, RemoteErrorType};

/// Converts a client failure into a [`RemoteError`] and keeps it as the
/// typed source so callers can still inspect the HTTP status or body.
pub(crate) fn map_error(error: Error) -> RemoteError {
    let kind = match &error {
        Error::Transport(transport) => match transport.kind() {
            TransportErrorKind::Io => RemoteErrorType::IoError,
            _ => RemoteErrorType::ConnectionError,
        },
        Error::Status { status, .. } => status_kind(*status),
        Error::Multistatus(multistatus) => multistatus
            .failures()
            .next()
            .map_or(RemoteErrorType::ProtocolError, |(_, status)| {
                status_kind(status.code)
            }),
        Error::InvalidUrl(_) => RemoteErrorType::InvalidPath,
        _ => RemoteErrorType::ProtocolError,
    };
    RemoteError::with_source(kind, error)
}

/// Maps an HTTP status onto the closest protocol-agnostic error kind.
pub(crate) fn status_kind(status: StatusCode) -> RemoteErrorType {
    match status {
        StatusCode::UNAUTHORIZED => RemoteErrorType::AuthenticationFailed,
        StatusCode::FORBIDDEN | StatusCode::LOCKED => RemoteErrorType::PermissionDenied,
        StatusCode::NOT_FOUND | StatusCode::GONE | StatusCode::CONFLICT => {
            RemoteErrorType::NoSuchFileOrDirectory
        }
        StatusCode::METHOD_NOT_ALLOWED | StatusCode::PRECONDITION_FAILED => {
            RemoteErrorType::AlreadyExists
        }
        StatusCode::INSUFFICIENT_STORAGE => RemoteErrorType::IoError,
        _ => RemoteErrorType::ProtocolError,
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as _;

    use dav_xml_client::Error;
    use dav_xml_client::dav_xml::FromXml;
    use dav_xml_client::dav_xml::elements::Multistatus;
    use dav_xml_client::transport::{TransportError, TransportErrorKind};
    use http::StatusCode;
    use pretty_assertions::assert_eq;
    use remotefs::RemoteErrorType;

    use super::{map_error, status_kind};

    fn status(code: StatusCode) -> Error {
        Error::Status {
            status: code,
            error: None,
            body: Vec::new(),
        }
    }

    #[test]
    fn http_statuses_map_to_remote_kinds() {
        let cases = [
            (
                StatusCode::UNAUTHORIZED,
                RemoteErrorType::AuthenticationFailed,
            ),
            (StatusCode::FORBIDDEN, RemoteErrorType::PermissionDenied),
            (StatusCode::LOCKED, RemoteErrorType::PermissionDenied),
            (
                StatusCode::NOT_FOUND,
                RemoteErrorType::NoSuchFileOrDirectory,
            ),
            (StatusCode::GONE, RemoteErrorType::NoSuchFileOrDirectory),
            (StatusCode::CONFLICT, RemoteErrorType::NoSuchFileOrDirectory),
            (
                StatusCode::METHOD_NOT_ALLOWED,
                RemoteErrorType::AlreadyExists,
            ),
            (
                StatusCode::PRECONDITION_FAILED,
                RemoteErrorType::AlreadyExists,
            ),
            (StatusCode::INSUFFICIENT_STORAGE, RemoteErrorType::IoError),
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                RemoteErrorType::ProtocolError,
            ),
            (
                StatusCode::MOVED_PERMANENTLY,
                RemoteErrorType::ProtocolError,
            ),
        ];
        for (code, expected) in cases {
            assert_eq!(status_kind(code), expected, "status: {code}");
            assert_eq!(map_error(status(code)).kind(), expected, "status: {code}");
        }
    }

    #[test]
    fn transport_failures_are_connection_or_io_errors() {
        let connect = Error::Transport(TransportError::new(TransportErrorKind::Connect, "refused"));
        assert_eq!(map_error(connect).kind(), RemoteErrorType::ConnectionError);
        let timeout = Error::Transport(TransportError::new(TransportErrorKind::Timeout, "slow"));
        assert_eq!(map_error(timeout).kind(), RemoteErrorType::ConnectionError);
        let io = Error::Transport(TransportError::new(TransportErrorKind::Io, "reset"));
        assert_eq!(map_error(io).kind(), RemoteErrorType::IoError);
    }

    #[test]
    fn invalid_urls_are_invalid_paths_and_the_rest_is_protocol() {
        let invalid = "http://exa mple.com".parse::<http::Uri>().unwrap_err();
        assert_eq!(
            map_error(Error::InvalidUrl(invalid)).kind(),
            RemoteErrorType::InvalidPath
        );
        assert_eq!(
            map_error(Error::MissingLockToken).kind(),
            RemoteErrorType::ProtocolError
        );
    }

    #[test]
    fn the_client_error_is_kept_as_source() {
        let mapped = map_error(status(StatusCode::FORBIDDEN));
        let source = mapped.source().expect("source is kept");
        assert!(source.is::<Error>());
        assert_eq!(
            mapped.to_string(),
            "not enough permissions (server returned 403 Forbidden)"
        );
    }

    #[test]
    fn multistatus_failures_use_their_first_http_status() {
        let multistatus = Multistatus::from_xml(
            br#"<D:multistatus xmlns:D="DAV:"><D:response><D:href>/f</D:href><D:status>HTTP/1.1 423 Locked</D:status></D:response></D:multistatus>"#.to_vec(),
        )
        .unwrap();
        assert_eq!(
            map_error(Error::Multistatus(multistatus)).kind(),
            RemoteErrorType::PermissionDenied
        );
    }
}
