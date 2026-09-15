use std::io::SeekFrom;
use std::path::{Path, PathBuf};

use pretty_assertions::assert_eq;
use remotefs::fs::{AsyncRemoteFs, Capabilities, ReadOptions, UnixPex, WriteOptions};
use remotefs::{RemoteErrorType, RemoteFs};
use ssh2_config::ParseRule;
use tokio::io::{AsyncReadExt as _, AsyncSeekExt as _, AsyncWriteExt as _};

use super::*;
use crate::mock::ssh as ssh_mock;
use crate::ssh::backend::NoCheckServerKey;
use crate::ssh::container::OpensshServer;
use crate::ssh::russh_sftp::{BlockingRusshSftpFs, RusshSftpFs};

type Client = RusshSftpFs<NoCheckServerKey>;

struct TestCtx {
    runtime: tokio::runtime::Runtime,
    client: Client,
    root: PathBuf,
    container: OpensshServer,
}

impl TestCtx {
    fn path(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("tokio runtime")
}

fn opts(port: u16) -> (SshOpts, tempfile::NamedTempFile) {
    let config_file = ssh_mock::create_ssh_config(port);
    let opts = SshOpts::new("sftp")
        .key_storage(Box::new(ssh_mock::MockSshKeyStorage::default()))
        .config_file(config_file.path(), ParseRule::ALLOW_UNKNOWN_FIELDS)
        .ssh_agent_identity(Some(crate::SshAgentIdentity::All));
    (opts, config_file)
}

fn setup() -> TestCtx {
    crate::mock::logger();
    let container = OpensshServer::start();
    let (opts, _config) = opts(container.port());
    let runtime = runtime();
    let mut client = Client::new(opts);
    let root = PathBuf::from(generate_tempdir());
    runtime.block_on(async {
        client.connect().await.expect("connect");
        client
            .create_dir(&root, Some(UnixPex::from(0o775)))
            .await
            .expect("create scratch directory");
    });
    TestCtx {
        runtime,
        client,
        root,
        container,
    }
}

fn finalize(ctx: TestCtx) {
    let TestCtx {
        runtime,
        mut client,
        root,
        container,
    } = ctx;
    runtime.block_on(async {
        client
            .remove_dir_all(&root)
            .await
            .expect("remove scratch directory");
        client.disconnect().await.expect("disconnect");
    });
    drop(container);
}

async fn write(client: &Client, path: &Path, data: &[u8]) {
    let mut source = data;
    let written = client
        .write_file(
            path,
            &WriteOptions::default().size_hint(data.len() as u64),
            &mut source,
        )
        .await
        .expect("write_file");
    assert_eq!(written, data.len() as u64);
}

async fn read(client: &Client, path: &Path, opts: &ReadOptions) -> Vec<u8> {
    let mut output = Vec::new();
    client
        .read_file(path, opts, &mut output)
        .await
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
fn should_reject_relative_paths() {
    let client = Client::new(SshOpts::new("sftp"));
    let error = runtime()
        .block_on(client.stat(Path::new("relative")))
        .unwrap_err();
    assert_eq!(error.kind(), RemoteErrorType::InvalidPath);
}

#[test]
fn should_append_and_honor_ranges() {
    let ctx = setup();
    let path = ctx.path("range.txt");
    ctx.runtime.block_on(async {
        write(&ctx.client, &path, b"abcdef").await;
        let mut source = &b"ghi"[..];
        assert_eq!(
            ctx.client
                .append_file(&path, &WriteOptions::default(), &mut source)
                .await
                .unwrap(),
            3
        );
        assert_eq!(
            read(
                &ctx.client,
                &path,
                &ReadOptions::default().offset(2).length(3),
            )
            .await,
            b"cde"
        );
    });
    finalize(ctx);
}

#[test]
fn should_seek_and_finish_streams() {
    let ctx = setup();
    let path = ctx.path("stream.txt");
    ctx.runtime.block_on(async {
        write(&ctx.client, &path, b"abcdef").await;
        let stream = ctx
            .client
            .open(&path, &ReadOptions::default())
            .await
            .unwrap();
        assert!(stream.seekable());
        let mut stream = stream.into_tokio();
        stream.seek(SeekFrom::Start(3)).await.unwrap();
        let mut output = Vec::new();
        stream.read_to_end(&mut output).await.unwrap();
        stream.into_inner().finish().await.unwrap();
        assert_eq!(output, b"def");

        let mut writer = ctx
            .client
            .create(&ctx.path("new.txt"), &WriteOptions::default())
            .await
            .unwrap()
            .into_tokio();
        writer.write_all(b"hello").await.unwrap();
        writer.flush().await.unwrap();
        writer.shutdown().await.unwrap();
        writer.into_inner().finish().await.unwrap();
    });
    finalize(ctx);
}

#[test]
fn should_stat_symlink_and_advertise_capabilities() {
    let ctx = setup();
    let target = ctx.path("target.txt");
    let link = ctx.path("link.txt");
    ctx.runtime.block_on(async {
        write(&ctx.client, &target, b"data").await;
        ctx.client.symlink(&link, &target).await.unwrap();
        let entry = ctx.client.stat(&link).await.unwrap();
        assert!(entry.is_symlink());
        assert_eq!(entry.metadata().symlink.as_deref(), Some(target.as_path()));
        let caps = ctx.client.capabilities();
        assert!(caps.contains(Capabilities::APPEND | Capabilities::RANGE_READ));
        assert!(caps.contains(Capabilities::SEEK_READ | Capabilities::SEEK_WRITE));
    });
    finalize(ctx);
}

#[test]
fn should_copy_symlinks_and_reject_overlapping_destinations() {
    let ctx = setup();
    let source_dir = ctx.path("source");
    let target = source_dir.join("target.txt");
    let link = source_dir.join("link.txt");
    let copied_link = ctx.path("copied-link.txt");
    let nested_destination = source_dir.join("nested");
    ctx.runtime.block_on(async {
        ctx.client.create_dir(&source_dir, None).await.unwrap();
        write(&ctx.client, &target, b"data").await;
        ctx.client.symlink(&link, &target).await.unwrap();

        ctx.client.copy(&link, &copied_link).await.unwrap();
        let copied = ctx.client.stat(&copied_link).await.unwrap();
        assert!(copied.is_symlink());
        assert_eq!(copied.metadata().symlink.as_deref(), Some(target.as_path()));

        assert_eq!(
            ctx.client
                .copy(&source_dir, &source_dir)
                .await
                .unwrap_err()
                .kind(),
            RemoteErrorType::InvalidPath
        );
        assert_eq!(
            ctx.client
                .copy(&source_dir, &nested_destination)
                .await
                .unwrap_err()
                .kind(),
            RemoteErrorType::InvalidPath
        );
    });
    finalize(ctx);
}

#[test]
fn should_work_through_blocking_wrapper() {
    crate::mock::logger();
    let container = OpensshServer::start();
    let (opts, _config) = opts(container.port());
    let runtime = runtime();
    let mut client: BlockingRusshSftpFs<NoCheckServerKey> =
        Client::new(opts).into_blocking(runtime.handle().clone());
    client.connect().unwrap();
    let root = PathBuf::from(generate_tempdir());
    client.create_dir(&root, None).unwrap();
    let path = root.join("blocking.txt");
    let mut source = std::io::Cursor::new(b"hello".to_vec());
    assert_eq!(
        client
            .write_file(&path, &WriteOptions::default().size_hint(5), &mut source,)
            .unwrap(),
        5
    );
    let mut output = Vec::new();
    client
        .read_file(&path, &ReadOptions::default().offset(1), &mut output)
        .unwrap();
    assert_eq!(output, b"ello");
    client.remove_dir_all(&root).unwrap();
    client.disconnect().unwrap();
    drop(runtime);
    drop(container);
}

#[test]
#[should_panic(expected = "into_blocking requires a multi-thread Tokio runtime")]
fn should_reject_current_thread_blocking_wrapper() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let _ = Client::new(SshOpts::new("127.0.0.1")).into_blocking(runtime.handle().clone());
}

#[test]
fn should_be_send_and_sync() {
    fn is_send<T: Send>(_: &T) {}
    fn is_sync<T: Sync>(_: &T) {}
    let client = Client::new(SshOpts::new("sftp"));
    is_send(&client);
    is_sync(&client);
    let _: Box<dyn remotefs::AsyncRemoteFs> = Box::new(client);
}
