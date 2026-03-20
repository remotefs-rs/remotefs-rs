use std::io::{Read as _, Write as _};
use std::path::Path;
use std::sync::Arc;

use log::info;
use remotefs::RemoteFs as _;
use remotefs::fs::Metadata;
use remotefs_ssh::{NoCheckServerKey, RusshSession, SftpFs, SshOpts};

const FILE_SIZE: usize = 2 * 1024 * 1024;

fn main() -> Result<(), Box<dyn std::error::Error>> {
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
        .and_then(|h| h.split(':').next())
        .expect("Failed to parse hostname from host argument");
    let port = host
        .split(':')
        .nth(1)
        .and_then(|p| p.parse::<u16>().ok())
        .unwrap_or(22);

    let password = rpassword::prompt_password("Password: ")?;

    let runtime = Arc::new(
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?,
    );

    let mut sftp_fs: SftpFs<RusshSession<NoCheckServerKey>> = SftpFs::russh(
        SshOpts::new(hostname)
            .port(port)
            .username(username)
            .password(password),
        runtime,
    );
    sftp_fs.connect()?;

    // list files
    let files = sftp_fs.list_dir(&Path::new("/tmp"))?;
    for file in files {
        info!("Found file: {:?}", file);
    }

    // upload file to temp
    let remote_file = Path::new("/tmp/remote_test_file.bin");
    let mut writer = sftp_fs.create(remote_file, &Metadata::default().size(FILE_SIZE as u64))?;
    let mut bytes = 0;
    const CHUNK_SIZE: usize = 64 * 1024;
    let chunk = vec![0x01; CHUNK_SIZE];
    while bytes < FILE_SIZE {
        let chunk_size = std::cmp::min(CHUNK_SIZE, FILE_SIZE - bytes);
        writer.write_all(&chunk[..chunk_size])?;
        bytes += chunk_size;
    }
    info!("Uploaded file to {:?}", remote_file);

    // download file
    let mut reader = sftp_fs.open(remote_file)?;
    let mut bytes = 0;
    let mut buffer = vec![0u8; CHUNK_SIZE];
    while bytes < FILE_SIZE {
        let chunk_size = std::cmp::min(CHUNK_SIZE, FILE_SIZE - bytes);
        reader.read_exact(&mut buffer[..chunk_size])?;
        bytes += chunk_size;
    }
    info!("Downloaded file from {:?}", remote_file);

    // remove file
    sftp_fs.remove_file(remote_file)?;

    sftp_fs.disconnect()?;

    Ok(())
}
