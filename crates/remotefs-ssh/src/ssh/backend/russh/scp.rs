//! SCP protocol implementation over russh channels.
//!
//! SCP is not a standalone protocol — it runs `scp -f` (recv) and `scp -t` (send)
//! over a normal SSH exec channel and uses a simple header + ACK wire format.

use std::io::Write;
use std::path::Path;
use std::sync::Arc;

use remotefs::{RemoteError, RemoteErrorType, RemoteResult};
use russh::client::Handler;
use tokio::runtime::Runtime;

use super::open_channel;

fn shell_escape_arg(value: &str) -> String {
    format!("'{}'", value.replace('\'', r#"'\''"#))
}

/// SCP recv: open a channel, exec `scp -f <path>`, handshake, and return
/// a synchronous reader that drains exactly `filesize` bytes.
pub(super) async fn recv<T>(
    session: &russh::client::Handle<T>,
    path: &Path,
    forward_agent: bool,
) -> RemoteResult<Box<dyn std::io::Read + Send>>
where
    T: Handler,
{
    debug!("Opening channel for scp recv");
    let mut channel = open_channel(session, forward_agent).await?;

    let cmd = format!("scp -f {}", shell_escape_arg(&path.to_string_lossy()));
    channel.exec(true, cmd.as_bytes()).await.map_err(|err| {
        RemoteError::new_ex(
            RemoteErrorType::ProtocolError,
            format!("Could not exec scp command: {err}"),
        )
    })?;

    // Send initial ACK (\0)
    debug!("Sending initial ACK");
    channel.data(&[0u8][..]).await.map_err(|err| {
        RemoteError::new_ex(
            RemoteErrorType::ProtocolError,
            format!("Could not write ACK to channel: {err}"),
        )
    })?;

    // Read the SCP header (e.g. "C0644 12345 filename\n")
    debug!("Reading SCP header");
    let mut header_buf = Vec::new();
    let mut initial_data = Vec::new();
    loop {
        match channel.wait().await {
            Some(russh::ChannelMsg::Data { data }) => {
                header_buf.extend_from_slice(&data);
                if let Some(header_end) = header_buf.iter().position(|byte| *byte == b'\n') {
                    initial_data.extend_from_slice(&header_buf[header_end + 1..]);
                    header_buf.truncate(header_end + 1);
                    break;
                }
            }
            Some(russh::ChannelMsg::Eof | russh::ChannelMsg::Close) => break,
            _ => {}
        }
    }

    let filesize = parse_header_filesize(&header_buf)?;
    debug!("File size: {filesize}");

    // Send OK
    debug!("Sending OK");
    channel.data(&[0u8][..]).await.map_err(|err| {
        RemoteError::new_ex(
            RemoteErrorType::ProtocolError,
            format!("Could not write ACK to channel: {err}"),
        )
    })?;

    // Collect all data bytes up to filesize
    let mut buf = Vec::with_capacity(filesize);
    buf.extend_from_slice(&initial_data);
    if buf.len() > filesize {
        buf.truncate(filesize);
    }
    while buf.len() < filesize {
        match channel.wait().await {
            Some(russh::ChannelMsg::Data { data }) => {
                buf.extend_from_slice(&data);
            }
            Some(russh::ChannelMsg::Eof | russh::ChannelMsg::Close) => break,
            _ => {}
        }
    }
    buf.truncate(filesize);

    let _ = channel.eof().await;

    Ok(Box::new(std::io::Cursor::new(buf)) as Box<dyn std::io::Read + Send>)
}

/// SCP send: open a channel, exec `scp -t <path>`, handshake, and return
/// a synchronous writer that forwards data into the channel.
pub(super) async fn send<T>(
    session: &russh::client::Handle<T>,
    remote_path: &Path,
    mode: i32,
    size: u64,
    runtime: Arc<Runtime>,
    forward_agent: bool,
) -> RemoteResult<Box<dyn Write + Send>>
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
        RemoteError::new_ex(
            RemoteErrorType::ProtocolError,
            format!("Could not exec scp command: {err}"),
        )
    })?;

    // Wait for initial ACK
    wait_for_ack(&mut channel).await?;

    let filename = remote_path
        .file_name()
        .map(|f| f.to_string_lossy())
        .ok_or_else(|| {
            RemoteError::new_ex(
                RemoteErrorType::ProtocolError,
                format!("Could not get file name: {remote_path:?}"),
            )
        })?;

    // Send file header: C<mode> <size> <filename>\n
    let header = format!("C{mode:04o} {size} {filename}\n", mode = mode & 0o7777);
    debug!("Sending SCP header: {header}");
    channel.data(header.as_bytes()).await.map_err(|err| {
        RemoteError::new_ex(
            RemoteErrorType::ProtocolError,
            format!("Could not write header to channel: {err}"),
        )
    })?;

    // Wait for ACK
    wait_for_ack(&mut channel).await?;

    let writer = SendChannel { channel, runtime };
    Ok(Box::new(writer) as Box<dyn Write + Send>)
}

/// Wait for a single-byte SCP ACK (0x00 = OK).
///
/// Skips non-data channel messages (e.g. `WindowAdjusted`, `Eof`) that may
/// arrive before the actual ACK byte.
async fn wait_for_ack(channel: &mut russh::Channel<russh::client::Msg>) -> RemoteResult<()> {
    debug!("Waiting for channel acknowledgment");
    loop {
        match channel.wait().await {
            Some(russh::ChannelMsg::Data { data }) => {
                if data.first() == Some(&0) {
                    return Ok(());
                }
                return Err(RemoteError::new_ex(
                    RemoteErrorType::ProtocolError,
                    format!("Unexpected SCP ACK: {data:?}"),
                ));
            }
            Some(russh::ChannelMsg::Close) | None => {
                return Err(RemoteError::new_ex(
                    RemoteErrorType::ProtocolError,
                    "Channel closed before receiving SCP ACK",
                ));
            }
            Some(other) => {
                trace!("Skipping non-data channel message while waiting for ACK: {other:?}");
            }
        }
    }
}

/// Parse file size from an SCP header (`C<mode> <size> <filename>\n`).
fn parse_header_filesize(header: &[u8]) -> RemoteResult<usize> {
    let header_str = std::str::from_utf8(header).map_err(|e| {
        RemoteError::new_ex(
            RemoteErrorType::ProtocolError,
            format!("Could not parse SCP header: {e}"),
        )
    })?;
    let parts: Vec<&str> = header_str.split_whitespace().collect();
    if parts.len() < 3 {
        return Err(RemoteError::new_ex(
            RemoteErrorType::ProtocolError,
            "Invalid SCP header: not enough parts",
        ));
    }
    if !parts[0].starts_with('C') {
        return Err(RemoteError::new_ex(
            RemoteErrorType::ProtocolError,
            "Invalid SCP header: missing 'C'",
        ));
    }
    parts[1].parse::<usize>().map_err(|e| {
        RemoteError::new_ex(
            RemoteErrorType::ProtocolError,
            format!("Invalid file size in SCP header: {e}"),
        )
    })
}

/// Synchronous writer wrapping a russh channel for SCP send.
///
/// Stores the full `Arc<Runtime>` rather than just a `Handle` so that
/// `Runtime::block_on` drives IO and background tasks (including SSH
/// window-adjust processing) on a current-thread runtime.
struct SendChannel {
    channel: russh::Channel<russh::client::Msg>,
    runtime: Arc<Runtime>,
}

impl Write for SendChannel {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.runtime
            .block_on(self.channel.data(buf))
            .map(|()| buf.len())
            .map_err(std::io::Error::other)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Drop for SendChannel {
    fn drop(&mut self) {
        debug!("Dropping SCP send channel");
        if let Err(err) = self.runtime.block_on(self.channel.eof()) {
            debug!("Error sending EOF: {err}");
        }
    }
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
    fn should_parse_scp_header_with_payload_remainder_trimmed() {
        let header = b"C0644 5 hello.txt\nhello";
        let trimmed = &header[..header.iter().position(|byte| *byte == b'\n').unwrap() + 1];
        assert_eq!(parse_header_filesize(trimmed).unwrap(), 5);
    }
}
