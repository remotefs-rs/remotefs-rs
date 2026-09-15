use std::path::Path;

use log::info;
use remotefs::AsyncRemoteFs;
use remotefs::fs::{ReadOptions, WriteOptions};
use remotefs_ssh::{NoCheckServerKey, RusshScpFs, SshOpts};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

const FILE_SIZE: usize = 2 * 1024 * 1024;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::builder()
        .filter_level(log::LevelFilter::Debug)
        .format_source_path(true)
        .format_line_number(true)
        .try_init()?;

    let host = std::env::args()
        .nth(1)
        .expect("Please provide the SSH host as the first argument. Syntax is user@hostname:port");
    let username = host
        .split('@')
        .next()
        .expect("Failed to parse username from host argument");
    let hostname = host
        .split('@')
        .nth(1)
        .and_then(|host| host.split(':').next())
        .expect("Failed to parse hostname from host argument");
    let port = host
        .split(':')
        .nth(1)
        .and_then(|port| port.parse::<u16>().ok())
        .unwrap_or(22);
    let password = rpassword::prompt_password("Password: ")?;

    let mut client: RusshScpFs<NoCheckServerKey> = RusshScpFs::new(
        SshOpts::new(hostname)
            .port(port)
            .username(username)
            .password(password),
    );
    client.connect().await?;

    for file in client.list_dir(Path::new("/tmp")).await? {
        info!("Found file: {file:?}");
    }

    let remote_file = Path::new("/tmp/remote_test_file.bin");
    let mut writer = client
        .create(
            remote_file,
            &WriteOptions::default().size_hint(FILE_SIZE as u64),
        )
        .await?
        .into_tokio();
    let chunk = vec![0x01; 64 * 1024];
    let mut written = 0;
    while written < FILE_SIZE {
        let length = (FILE_SIZE - written).min(chunk.len());
        writer.write_all(&chunk[..length]).await?;
        written += length;
    }
    writer.flush().await?;
    writer.into_inner().finish().await?;

    let mut reader = client
        .open(remote_file, &ReadOptions::default())
        .await?
        .into_tokio();
    let mut buffer = vec![0_u8; 64 * 1024];
    let mut remaining = FILE_SIZE;
    while remaining > 0 {
        let length = remaining.min(buffer.len());
        reader.read_exact(&mut buffer[..length]).await?;
        remaining -= length;
    }
    reader.into_inner().finish().await?;
    client.remove_file(remote_file).await?;
    client.disconnect().await?;
    Ok(())
}
