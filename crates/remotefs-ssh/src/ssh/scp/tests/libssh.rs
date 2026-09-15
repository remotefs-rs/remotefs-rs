use std::io::{Cursor, ErrorKind, Seek as _, Write as _};
use std::path::{Path, PathBuf};

use pretty_assertions::assert_eq;
use remotefs::RemoteFs;
use remotefs::fs::{Capabilities, ReadOptions, SetMetadata, UnixPex, WriteOptions};
use ssh2_config::ParseRule;

use super::*;
use crate::mock::ssh as ssh_mock;
use crate::ssh::container::OpensshServer;

type Client = ScpFs<crate::ssh::backend::LibSshSession>;

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
    let mut client = ScpFs::libssh(
        SshOpts::new("scp")
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
fn should_initialize_scp_filesystem() {
    let client = ScpFs::libssh(SshOpts::new("127.0.0.1"));
    assert!(client.session.is_none());
    assert!(!client.is_connected());
    assert_eq!(client.capabilities(), super::SCP_CAPABILITIES);
}

#[test]
fn should_copy_and_read_with_ranges() {
    let ctx = setup();
    let source = ctx.path("a.txt");
    write(&ctx.client, &source, b"abcdef");
    let destination = ctx.path("b.txt");
    ctx.client.copy(&source, &destination).unwrap();
    assert_eq!(
        read(
            &ctx.client,
            &destination,
            &ReadOptions::default().offset(2).length(2)
        ),
        b"cd"
    );
    assert!(read(&ctx.client, &source, &ReadOptions::default().offset(100)).is_empty());
    finalize(ctx);
}

#[test]
fn should_require_size_and_reject_append() {
    let ctx = setup();
    let path = ctx.path("missing.txt");
    let error = ctx
        .client
        .create(&path, &WriteOptions::default())
        .unwrap_err();
    assert_eq!(error.kind(), remotefs::RemoteErrorType::SizeRequired);
    assert!(!ctx.client.exists(&path).unwrap());
    let error = ctx
        .client
        .append(&path, &WriteOptions::default())
        .unwrap_err();
    assert_eq!(error.kind(), remotefs::RemoteErrorType::UnsupportedFeature);
    assert!(!ctx.client.capabilities().contains(Capabilities::APPEND));
    finalize(ctx);
}

#[test]
fn should_not_seek_scp_streams() {
    let ctx = setup();
    let path = ctx.path("stream.txt");
    write(&ctx.client, &path, b"abcdef");
    let mut reader = ctx.client.open(&path, &ReadOptions::default()).unwrap();
    assert!(!reader.seekable());
    assert_eq!(
        reader.seek(std::io::SeekFrom::Start(1)).unwrap_err().kind(),
        ErrorKind::Unsupported
    );
    reader.finish().unwrap();
    let mut writer = ctx
        .client
        .create(
            &ctx.path("write.txt"),
            &WriteOptions::default().size_hint(1),
        )
        .unwrap();
    assert!(!writer.seekable());
    writer.write_all(b"x").unwrap();
    writer.finish().unwrap();
    finalize(ctx);
}

#[test]
fn should_stat_symlink_and_set_metadata() {
    let ctx = setup();
    let target = ctx.path("target.txt");
    let link = ctx.path("link.txt");
    write(&ctx.client, &target, b"data");
    ctx.client.symlink(&link, &target).unwrap();
    let entry = ctx.client.stat(&link).unwrap();
    assert!(entry.is_symlink());
    assert_eq!(entry.metadata().symlink.as_deref(), Some(target.as_path()));
    ctx.client
        .set_metadata(&target, &SetMetadata::default().mode(UnixPex::from(0o755)))
        .unwrap();
    finalize(ctx);
}

#[test]
fn should_advertise_capabilities() {
    let ctx = setup();
    let caps = ctx.client.capabilities();
    assert!(!caps.contains(Capabilities::APPEND));
    assert!(!caps.contains(Capabilities::RANGE_READ));
    assert!(caps.contains(Capabilities::EXEC | Capabilities::COPY | Capabilities::SYMLINK));
    finalize(ctx);
}

#[test]
fn should_be_sync() {
    fn is_sync<T: Sync>(_: T) {}
    let client = ScpFs::libssh(SshOpts::new("scp"));
    is_sync(&client);
}

#[test]
fn should_be_send() {
    fn is_send<T: Send>(_: T) {}
    is_send(ScpFs::libssh(SshOpts::new("scp")));
}
