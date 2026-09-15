use std::io::{ErrorKind, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use pretty_assertions::assert_eq;
use remotefs::fs::{AsyncRemoteFs, Capabilities, ReadOptions, UnixPex, WriteOptions};
use remotefs::{RemoteErrorType, RemoteFs};
use ssh2_config::ParseRule;
use tokio::io::{AsyncReadExt as _, AsyncSeekExt as _, AsyncWriteExt as _};

use super::*;
use crate::mock::ssh as ssh_mock;
use crate::ssh::backend::NoCheckServerKey;
use crate::ssh::container::OpensshServer;
use crate::ssh::russh_scp::{BlockingRusshScpFs, RusshScpFs};

type Client = RusshScpFs<NoCheckServerKey>;

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
    let opts = SshOpts::new("scp")
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
    let client = Client::new(SshOpts::new("scp"));
    let error = runtime()
        .block_on(client.stat(Path::new("relative")))
        .unwrap_err();
    assert_eq!(error.kind(), RemoteErrorType::InvalidPath);
}

#[test]
fn should_copy_and_honor_scp_ranges() {
    let ctx = setup();
    let source = ctx.path("source.txt");
    let destination = ctx.path("destination.txt");
    ctx.runtime.block_on(async {
        write(&ctx.client, &source, b"abcdef").await;
        ctx.client.copy(&source, &destination).await.unwrap();
        assert_eq!(
            read(
                &ctx.client,
                &destination,
                &ReadOptions::default().offset(2).length(2),
            )
            .await,
            b"cd"
        );
    });
    finalize(ctx);
}

#[test]
fn should_require_size_and_reject_append() {
    let ctx = setup();
    ctx.runtime.block_on(async {
        let path = ctx.path("missing.txt");
        assert_eq!(
            ctx.client
                .create(&path, &WriteOptions::default())
                .await
                .unwrap_err()
                .kind(),
            RemoteErrorType::SizeRequired
        );
        assert_eq!(
            ctx.client
                .append(&path, &WriteOptions::default())
                .await
                .unwrap_err()
                .kind(),
            RemoteErrorType::UnsupportedFeature
        );

        let path = ctx.path("short.txt");
        let mut source = &b"short"[..];
        let error = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            ctx.client
                .write_file(&path, &WriteOptions::default().size_hint(10), &mut source),
        )
        .await
        .expect("short SCP upload must not hang")
        .unwrap_err();
        assert_eq!(error.kind(), RemoteErrorType::ProtocolError);
    });
    finalize(ctx);
}

#[test]
fn should_not_seek_scp_streams_and_finish_explicitly() {
    let ctx = setup();
    let path = ctx.path("stream.txt");
    ctx.runtime.block_on(async {
        write(&ctx.client, &path, b"abcdef").await;
        let stream = ctx
            .client
            .open(&path, &ReadOptions::default())
            .await
            .unwrap();
        assert!(!stream.seekable());
        let mut stream = stream.into_tokio();
        assert_eq!(
            stream.seek(SeekFrom::Start(1)).await.unwrap_err().kind(),
            ErrorKind::Unsupported
        );
        let mut output = Vec::new();
        stream.read_to_end(&mut output).await.unwrap();
        stream.into_inner().finish().await.unwrap();
        assert_eq!(output, b"abcdef");

        let mut writer = ctx
            .client
            .create(
                &ctx.path("new.txt"),
                &WriteOptions::default()
                    .size_hint(5)
                    .modified(UNIX_EPOCH + Duration::from_secs(1_700_000_000)),
            )
            .await
            .unwrap()
            .into_tokio();
        assert!(!writer.get_ref().seekable());
        writer.write_all(b"hello").await.unwrap();
        writer.flush().await.unwrap();
        writer.shutdown().await.unwrap();
        writer.into_inner().finish().await.unwrap();
        assert_eq!(
            ctx.client
                .stat(&ctx.path("new.txt"))
                .await
                .unwrap()
                .metadata()
                .modified
                .unwrap()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            1_700_000_000
        );
    });
    finalize(ctx);
}

#[test]
fn should_advertise_capabilities_and_stat_symlink() {
    let ctx = setup();
    let target = ctx.path("target.txt");
    let link = ctx.path("link.txt");
    ctx.runtime.block_on(async {
        write(&ctx.client, &target, b"data").await;
        ctx.client.symlink(&link, &target).await.unwrap();
        assert!(ctx.client.stat(&link).await.unwrap().is_symlink());
        let caps = ctx.client.capabilities();
        assert!(!caps.contains(Capabilities::APPEND | Capabilities::RANGE_READ));
        assert!(caps.contains(Capabilities::COPY | Capabilities::SYMLINK));

        ctx.client.remove_file(&target).await.unwrap();
        assert!(ctx.client.exists(&link).await.unwrap());
        ctx.client.remove_file(&link).await.unwrap();
    });
    finalize(ctx);
}

#[test]
fn should_remove_special_entries_recursively_and_classify_nonempty_dirs() {
    let ctx = setup();
    let directory = ctx.path("special");
    let fifo = directory.join("pipe");
    ctx.runtime.block_on(async {
        ctx.client.create_dir(&directory, None).await.unwrap();
        let output = ctx
            .client
            .exec(&format!("mkfifo {fifo}", fifo = fifo.display()))
            .await
            .unwrap();
        assert_eq!(output.exit_code, 0, "mkfifo failed: {}", output.stdout);
        assert_eq!(
            ctx.client.remove_dir(&directory).await.unwrap_err().kind(),
            RemoteErrorType::DirectoryNotEmpty
        );
        ctx.client.remove_dir_all(&directory).await.unwrap();
    });
    finalize(ctx);
}

#[test]
fn should_work_through_blocking_wrapper() {
    crate::mock::logger();
    let container = OpensshServer::start();
    let (opts, _config) = opts(container.port());
    let runtime = runtime();
    let mut client: BlockingRusshScpFs<NoCheckServerKey> =
        Client::new(opts).into_blocking(runtime.handle().clone());
    client.connect().unwrap();
    let root = PathBuf::from(generate_tempdir());
    client.create_dir(&root, None).unwrap();
    let path = root.join("blocking.txt");
    let mut source = std::io::Cursor::new(b"hello".to_vec());
    client
        .write_file(&path, &WriteOptions::default().size_hint(5), &mut source)
        .unwrap();
    let mut output = Vec::new();
    client
        .read_file(&path, &ReadOptions::default(), &mut output)
        .unwrap();
    assert_eq!(output, b"hello");
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
    let client = Client::new(SshOpts::new("scp"));
    is_send(&client);
    is_sync(&client);
    let _: Box<dyn remotefs::AsyncRemoteFs> = Box::new(client);
}
