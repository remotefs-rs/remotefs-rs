//! SCP protocol implementation over russh channels.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use remotefs::fs::{AsyncReadStream, AsyncWriteStream, ReadOptions};
use remotefs::{RemoteError, RemoteErrorType, RemoteResult};
use russh::client::Handler;

use super::open_channel;

const MAX_SCP_HEADER_SIZE: usize = 64 * 1024;

fn shell_escape_arg(value: &str) -> String {
    format!("'{}'", value.replace('\'', r#"'\''"#))
}

pub(super) async fn recv<T>(
    session: &russh::client::Handle<T>,
    path: &Path,
    opts: &ReadOptions,
    forward_agent: bool,
) -> RemoteResult<AsyncReadStream>
where
    T: Handler,
{
    debug!("Opening channel for scp recv");
    let mut channel = open_channel(session, forward_agent).await?;
    let cmd = format!("scp -f {}", shell_escape_arg(&path.to_string_lossy()));
    channel.exec(true, cmd.as_bytes()).await.map_err(|err| {
        RemoteError::with_message(
            RemoteErrorType::ProtocolError,
            format!("Could not exec scp command: {err}"),
        )
    })?;
    channel.data(&[0_u8][..]).await.map_err(|err| {
        RemoteError::with_message(
            RemoteErrorType::ProtocolError,
            format!("Could not write ACK to channel: {err}"),
        )
    })?;

    let mut header_buf = Vec::new();
    let mut initial_data = Vec::new();
    loop {
        match channel.wait().await {
            Some(russh::ChannelMsg::Data { data }) => {
                header_buf.extend_from_slice(&data);
                if let Some(header_end) = header_buf.iter().position(|byte| *byte == b'\n') {
                    if header_end + 1 > MAX_SCP_HEADER_SIZE {
                        return Err(RemoteError::with_message(
                            RemoteErrorType::ProtocolError,
                            "SCP header exceeds the maximum allowed size",
                        ));
                    }
                    initial_data.extend_from_slice(&header_buf[header_end + 1..]);
                    header_buf.truncate(header_end + 1);
                    break;
                }
                if header_buf.len() > MAX_SCP_HEADER_SIZE {
                    return Err(RemoteError::with_message(
                        RemoteErrorType::ProtocolError,
                        "SCP header exceeds the maximum allowed size",
                    ));
                }
            }
            Some(russh::ChannelMsg::Eof | russh::ChannelMsg::Close) | None => {
                return Err(RemoteError::with_message(
                    RemoteErrorType::ProtocolError,
                    "SCP channel closed before the file header",
                ));
            }
            _ => {}
        }
    }
    let filesize = parse_header_filesize(&header_buf)?;
    channel.data(&[0_u8][..]).await.map_err(|err| {
        RemoteError::with_message(
            RemoteErrorType::ProtocolError,
            format!("Could not write ACK to channel: {err}"),
        )
    })?;
    Ok(AsyncReadStream::new(super::stream::RusshScpReader::new(
        channel,
        initial_data,
        filesize as u64,
        opts,
    )))
}

pub(super) async fn send<T>(
    session: &russh::client::Handle<T>,
    remote_path: &Path,
    mode: u32,
    size: u64,
    modified: Option<SystemTime>,
    forward_agent: bool,
) -> RemoteResult<AsyncWriteStream>
where
    T: Handler,
{
    debug!("Opening channel for scp send");
    let mut channel = open_channel(session, forward_agent).await?;
    let cmd = format!(
        "scp -t {}",
        shell_escape_arg(&remote_path.to_string_lossy())
    );
    channel.exec(true, cmd.as_bytes()).await.map_err(|err| {
        RemoteError::with_message(
            RemoteErrorType::ProtocolError,
            format!("Could not exec scp command: {err}"),
        )
    })?;
    wait_for_ack(&mut channel).await?;
    if let Some(modified) = modified {
        let seconds = modified.duration_since(UNIX_EPOCH).map_err(|err| {
            RemoteError::with_message(
                RemoteErrorType::InvalidPath,
                format!("Invalid SCP modification time: {err}"),
            )
        })?;
        let timestamp = format!("T{mtime} 0 {mtime} 0\n", mtime = seconds.as_secs());
        channel.data(timestamp.as_bytes()).await.map_err(|err| {
            RemoteError::with_message(
                RemoteErrorType::ProtocolError,
                format!("Could not write SCP timestamp: {err}"),
            )
        })?;
        wait_for_ack(&mut channel).await?;
    }
    let filename = remote_path
        .file_name()
        .map(|file| file.to_string_lossy())
        .ok_or_else(|| {
            RemoteError::with_message(
                RemoteErrorType::ProtocolError,
                format!("Could not get file name: {remote_path:?}"),
            )
        })?;
    let header = format!("C{mode:04o} {size} {filename}\n", mode = mode & 0o7777);
    channel.data(header.as_bytes()).await.map_err(|err| {
        RemoteError::with_message(
            RemoteErrorType::ProtocolError,
            format!("Could not write header to channel: {err}"),
        )
    })?;
    wait_for_ack(&mut channel).await?;
    Ok(AsyncWriteStream::new(super::stream::RusshScpWriter::new(
        channel, size,
    )))
}

pub(super) async fn wait_for_ack(
    channel: &mut russh::Channel<russh::client::Msg>,
) -> RemoteResult<()> {
    loop {
        match channel.wait().await {
            Some(russh::ChannelMsg::Data { data }) => {
                if data.first() == Some(&0) {
                    return Ok(());
                }
                return Err(RemoteError::with_message(
                    RemoteErrorType::ProtocolError,
                    format!("Unexpected SCP ACK: {data:?}"),
                ));
            }
            Some(russh::ChannelMsg::Close) | None => {
                return Err(RemoteError::with_message(
                    RemoteErrorType::ProtocolError,
                    "Channel closed before receiving SCP ACK",
                ));
            }
            Some(other) => {
                trace!("Skipping non-data channel message while waiting for ACK: {other:?}")
            }
        }
    }
}

fn parse_header_filesize(header: &[u8]) -> RemoteResult<usize> {
    let header_str = std::str::from_utf8(header).map_err(|err| {
        RemoteError::with_message(
            RemoteErrorType::ProtocolError,
            format!("Could not parse SCP header: {err}"),
        )
    })?;
    let parts: Vec<&str> = header_str.split_whitespace().collect();
    if parts.len() < 3 {
        return Err(RemoteError::with_message(
            RemoteErrorType::ProtocolError,
            "Invalid SCP header: not enough parts",
        ));
    }
    if !parts[0].starts_with('C') {
        return Err(RemoteError::with_message(
            RemoteErrorType::ProtocolError,
            "Invalid SCP header: missing 'C'",
        ));
    }
    parts[1].parse::<usize>().map_err(|err| {
        RemoteError::with_message(
            RemoteErrorType::ProtocolError,
            format!("Invalid file size in SCP header: {err}"),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::{parse_header_filesize, shell_escape_arg};

    #[test]
    fn should_escape_shell_argument_for_scp() {
        assert_eq!(shell_escape_arg("/tmp/a b"), "'/tmp/a b'");
        assert_eq!(shell_escape_arg("/tmp/it's.txt"), r#"'/tmp/it'\''s.txt'"#);
    }

    #[test]
    fn should_parse_scp_header_filesize() {
        assert_eq!(parse_header_filesize(b"C0644 42 file.txt\n").unwrap(), 42);
        assert!(parse_header_filesize(b"bad\n").is_err());
    }
}
