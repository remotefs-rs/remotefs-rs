//! One SFTP v3 request for operands the native bindings cannot represent.
//! Uses the SFTP subsystem so restricted accounts need no shell access.

use std::io::{self, Read, Write};
use std::path::Path;

use remotefs::{RemoteError, RemoteErrorType, RemoteResult};

const INIT: u8 = 1;
const VERSION: u8 = 2;
const SYMLINK: u8 = 20;
const STATUS: u8 = 101;
const REQUEST_ID: u32 = 1;

fn read_packet(reader: &mut impl Read) -> io::Result<Vec<u8>> {
    let mut size = [0; 4];
    reader.read_exact(&mut size)?;
    let size = u32::from_be_bytes(size);
    // The two expected replies contain version extensions or a status message.
    if size > 256 * 1024 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "oversized SFTP reply",
        ));
    }
    let mut packet = vec![0; size as usize];
    reader.read_exact(&mut packet)?;
    Ok(packet)
}

fn status_string<'a>(bytes: &mut &'a [u8]) -> Option<&'a [u8]> {
    let (length, rest) = bytes.split_first_chunk::<4>()?;
    let (value, rest) = rest.split_at_checked(u32::from_be_bytes(*length) as usize)?;
    *bytes = rest;
    Some(value)
}

pub(super) fn request(
    mut reader: impl Read,
    mut writer: impl Write,
    first: &Path,
    second: &Path,
) -> RemoteResult<()> {
    writer.write_all(&[0, 0, 0, 5, INIT, 0, 0, 0, 3])?;
    writer.flush()?;
    if !read_packet(&mut reader)?.starts_with(&[VERSION, 0, 0, 0, 3]) {
        return Err(RemoteError::with_message(
            RemoteErrorType::ProtocolError,
            "expected SFTP v3",
        ));
    }

    let mut packet = vec![SYMLINK];
    packet.extend(REQUEST_ID.to_be_bytes());
    for operand in [first, second] {
        let bytes = operand.as_os_str().as_encoded_bytes();
        let length = u32::try_from(bytes.len())
            .map_err(|err| RemoteError::with_source(RemoteErrorType::InvalidPath, err))?;
        packet.extend(length.to_be_bytes());
        packet.extend(bytes);
    }
    let length = u32::try_from(packet.len())
        .map_err(|err| RemoteError::with_source(RemoteErrorType::InvalidPath, err))?;
    writer.write_all(&length.to_be_bytes())?;
    writer.write_all(&packet)?;
    writer.flush()?;
    let reply = read_packet(&mut reader)?;
    let [STATUS, 0, 0, 0, 1, a, b, c, d, tail @ ..] = reply.as_slice() else {
        return Err(RemoteError::with_message(
            RemoteErrorType::ProtocolError,
            "expected SFTP symlink status",
        ));
    };
    let mut fields = tail;
    let message = status_string(&mut fields);
    let language = status_string(&mut fields);
    let (Some(message), Some(_), true) = (message, language, fields.is_empty()) else {
        return Err(RemoteError::with_message(
            RemoteErrorType::ProtocolError,
            "invalid SFTP status fields",
        ));
    };
    match u32::from_be_bytes([*a, *b, *c, *d]) {
        0 => Ok(()),
        status => Err(RemoteError::with_message(
            super::sftp_error_kind(status, RemoteErrorType::FileCreateDenied),
            format!(
                "SFTP symlink returned status {status}: {}",
                String::from_utf8_lossy(message)
            ),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(version: u32, id: u32, status: u32) -> Vec<u8> {
        let mut bytes = vec![0, 0, 0, 5, VERSION];
        bytes.extend(version.to_be_bytes());
        bytes.extend([0, 0, 0, 17, STATUS]);
        bytes.extend(id.to_be_bytes());
        bytes.extend(status.to_be_bytes());
        bytes.extend([0; 8]); // Empty message and language tag.
        bytes
    }

    fn send(bytes: &[u8]) -> RemoteResult<()> {
        request(bytes, Vec::new(), Path::new("target"), Path::new("/link"))
    }

    #[test]
    fn should_encode_symlink_operands_in_server_order() {
        use crate::{SftpSymlinkOrder, SshOpts};

        let link = Path::new("/link");
        let target = Path::new("../missing");
        let link_first = b"\0\0\0\x05/link\0\0\0\x0a../missing";
        let target_first = b"\0\0\0\x0a../missing\0\0\0\x05/link";
        for (banner, configured, expected) in [
            ("SSH-2.0-libssh_0.12", None, link_first),
            ("SSH-2.0-OpenSSH_10.2", None, target_first),
            ("SSH-2.0-other", None, target_first),
            (
                "SSH-2.0-OpenSSH_10.2",
                Some(SftpSymlinkOrder::LinkFirst),
                link_first,
            ),
            (
                "SSH-2.0-libssh_0.12",
                Some(SftpSymlinkOrder::TargetFirst),
                target_first,
            ),
        ] {
            let mut opts = SshOpts::new("localhost");
            if let Some(configured) = configured {
                opts = opts.sftp_symlink_order(configured);
            }
            let detected =
                SftpSymlinkOrder::from_banner(banner.as_bytes(), SftpSymlinkOrder::TargetFirst);
            let (first, second) = opts.symlink_order(detected).operands(link, target);
            let mut packet = Vec::new();
            request(response(3, 1, 0).as_slice(), &mut packet, first, second).unwrap();
            // Skip the INIT frame and the SYMLINK frame header and request ID.
            assert_eq!(&packet[18..], expected);
        }
    }

    #[test]
    fn should_preserve_sftp_status() {
        for (code, kind) in [
            (2, RemoteErrorType::NoSuchFileOrDirectory),
            (3, RemoteErrorType::PermissionDenied),
            (4, RemoteErrorType::FileCreateDenied),
            (10, RemoteErrorType::NoSuchFileOrDirectory),
            (11, RemoteErrorType::AlreadyExists),
            (18, RemoteErrorType::DirectoryNotEmpty),
        ] {
            assert_eq!(send(&response(3, 1, code)).unwrap_err().kind(), kind);
        }
    }

    #[test]
    fn should_enforce_the_packet_size_limit() {
        let limit: u32 = 256 * 1024;
        let mut frame = limit.to_be_bytes().to_vec();
        frame.resize(4 + limit as usize, 0);
        assert_eq!(
            read_packet(&mut frame.as_slice()).unwrap().len(),
            limit as usize
        );
        let oversized = (limit + 1).to_be_bytes();
        let err = read_packet(&mut oversized.as_slice()).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn should_reject_invalid_sftp_replies() {
        for bytes in [response(4, 1, 0), response(3, 2, 0)] {
            assert_eq!(
                send(&bytes).unwrap_err().kind(),
                RemoteErrorType::ProtocolError
            );
        }
        let mut truncated = response(3, 1, 0);
        truncated.pop();
        let err = send(&truncated).unwrap_err();
        assert_eq!(err.kind(), RemoteErrorType::IoError);
        assert_eq!(
            std::error::Error::source(&err)
                .unwrap()
                .downcast_ref::<io::Error>()
                .unwrap()
                .kind(),
            io::ErrorKind::UnexpectedEof
        );
        // A complete frame may also end midway through a required status field.
        truncated[12] -= 1;
        assert_eq!(
            send(&truncated).unwrap_err().kind(),
            RemoteErrorType::ProtocolError
        );
    }
}
