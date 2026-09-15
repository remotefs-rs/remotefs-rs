use std::io::{Cursor, Read as _, Seek as _, SeekFrom};
use std::path::{Path, PathBuf};

use pretty_assertions::assert_eq;
use remotefs::RemoteFs;
use remotefs::fs::{Capabilities, ReadOptions, SetMetadata, UnixPex, WriteOptions};
use ssh2_config::ParseRule;

use super::*;
use crate::mock::ssh as ssh_mock;
use crate::ssh::container::OpensshServer;

type Client = SftpFs<crate::ssh::backend::LibSshSession>;

struct TestCtx {
    client: Client,
    root: PathBuf,
    #[expect(dead_code, reason = "keeps the container alive for the test")]
    container: OpensshServer,
}

impl TestCtx {
    fn path(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }
}

fn setup() -> TestCtx {
    crate::mock::logger();
    let container = OpensshServer::start();
    let config_file = ssh_mock::create_ssh_config(container.port());
    let mut client = SftpFs::libssh(
        SshOpts::new("sftp")
            .key_storage(Box::new(ssh_mock::MockSshKeyStorage::default()))
            .config_file(config_file.path(), ParseRule::ALLOW_UNKNOWN_FIELDS)
            .ssh_agent_identity(Some(crate::SshAgentIdentity::All)),
    );
    client.connect().expect("connect");
    let root = PathBuf::from(generate_tempdir());
    client
        .create_dir(&root, Some(UnixPex::from(0o775)))
        .expect("create scratch dir");
    TestCtx {
        client,
        root,
        container,
    }
}

fn finalize(ctx: TestCtx) {
    let TestCtx {
        mut client, root, ..
    } = ctx;
    client.remove_dir_all(&root).expect("remove scratch dir");
    client.disconnect().expect("disconnect");
}

fn write(client: &Client, path: &Path, data: &[u8]) {
    let count = client
        .write_file(
            path,
            &WriteOptions::default().size_hint(data.len() as u64),
            &mut Cursor::new(data),
        )
        .expect("write_file");
    assert_eq!(count, data.len() as u64);
}

fn read(client: &Client, path: &Path, opts: &ReadOptions) -> Vec<u8> {
    let mut output = Vec::new();
    client
        .read_file(path, opts, &mut output)
        .expect("read_file");
    output
}

fn generate_tempdir() -> String {
    use rand::distr::Alphanumeric;
    use rand::{RngExt, rng};
    let mut rng = rng();
    let name: String = std::iter::repeat(())
        .map(|()| rng.sample(Alphanumeric))
        .map(char::from)
        .take(8)
        .collect();
    format!("/tmp/temp_{name}")
}

#[test]
fn should_initialize_sftp_filesystem() {
    let client = SftpFs::libssh(SshOpts::new("127.0.0.1"));
    assert!(client.session.is_none());
    assert!(!client.is_connected());
    assert_eq!(client.capabilities(), super::SFTP_CAPABILITIES);
}

#[test]
fn should_append_and_copy_file() {
    let ctx = setup();
    let source = ctx.path("a.txt");
    write(&ctx.client, &source, b"test data\n");
    let appended = ctx
        .client
        .append_file(
            &source,
            &WriteOptions::default(),
            &mut Cursor::new(b"Hello, world!\n"),
        )
        .unwrap();
    assert_eq!(appended, 14);
    assert_eq!(ctx.client.stat(&source).unwrap().metadata().size, Some(24));
    let destination = ctx.path("b.txt");
    ctx.client.copy(&source, &destination).unwrap();
    assert_eq!(
        read(&ctx.client, &destination, &ReadOptions::default()),
        b"test data\nHello, world!\n"
    );
    finalize(ctx);
}

#[test]
fn should_honor_absolute_paths_and_ranges() {
    let ctx = setup();
    let path = ctx.path("range.txt");
    write(&ctx.client, &path, b"abcdef");
    assert_eq!(
        read(
            &ctx.client,
            &path,
            &ReadOptions::default().offset(2).length(2)
        ),
        b"cd"
    );
    assert_eq!(
        read(&ctx.client, &path, &ReadOptions::default().offset(4)),
        b"ef"
    );
    assert!(read(&ctx.client, &path, &ReadOptions::default().offset(100)).is_empty());
    let error = ctx.client.stat(Path::new("relative.txt")).unwrap_err();
    assert_eq!(error.kind(), remotefs::RemoteErrorType::InvalidPath);
    finalize(ctx);
}

#[test]
fn should_seek_and_finish_sftp_streams() {
    let ctx = setup();
    let path = ctx.path("stream.txt");
    write(&ctx.client, &path, b"abcdef");
    let mut stream = ctx.client.open(&path, &ReadOptions::default()).unwrap();
    assert!(stream.seekable());
    stream.seek(SeekFrom::Start(3)).unwrap();
    let mut output = Vec::new();
    stream.read_to_end(&mut output).unwrap();
    stream.finish().unwrap();
    assert_eq!(output, b"def");
    let mut writer = ctx
        .client
        .create(&ctx.path("new.txt"), &WriteOptions::default())
        .unwrap();
    assert!(writer.seekable());
    writer.write_all(b"hello").unwrap();
    writer.finish().unwrap();
    finalize(ctx);
}

#[test]
fn should_set_metadata_and_stat_symlink() {
    let ctx = setup();
    let target = ctx.path("target.txt");
    let link = ctx.path("link.txt");
    write(&ctx.client, &target, b"data");
    ctx.client
        .set_metadata(&target, &SetMetadata::default().mode(UnixPex::from(0o755)))
        .unwrap();
    ctx.client.symlink(&link, &target).unwrap();
    let entry = ctx.client.stat(&link).unwrap();
    assert!(entry.is_symlink());
    assert_eq!(entry.metadata().symlink.as_deref(), Some(target.as_path()));
    finalize(ctx);
}

#[test]
fn should_advertise_capabilities() {
    let ctx = setup();
    let caps = ctx.client.capabilities();
    assert!(caps.contains(Capabilities::APPEND));
    assert!(caps.contains(Capabilities::RANGE_READ));
    assert!(caps.contains(Capabilities::SEEK_READ | Capabilities::SEEK_WRITE));
    assert!(caps.contains(Capabilities::EXEC));
    finalize(ctx);
}

#[test]
fn should_return_not_connected_error() {
    let client = SftpFs::libssh(SshOpts::new("127.0.0.1"));
    assert_eq!(
        client.stat(Path::new("/tmp")).unwrap_err().kind(),
        remotefs::RemoteErrorType::NotConnected
    );
    assert_eq!(
        client.exec("echo 5").unwrap_err().kind(),
        remotefs::RemoteErrorType::NotConnected
    );
}

#[test]
fn should_be_sync() {
    fn is_sync<T: Sync>(_: T) {}
    let client = SftpFs::libssh(SshOpts::new("sftp"));
    is_sync(&client);
}

#[test]
fn should_be_send() {
    fn is_send<T: Send>(_: T) {}
    is_send(SftpFs::libssh(SshOpts::new("sftp")));
}
