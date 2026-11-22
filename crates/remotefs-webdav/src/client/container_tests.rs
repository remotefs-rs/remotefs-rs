//! Tests that talk to the WebDAV container from `tests/docker-compose.yml`.

use std::path::{Path, PathBuf};

use futures::io::{AsyncReadExt as _, AsyncWriteExt as _, Cursor};
use pretty_assertions::assert_eq;
use remotefs::fs::{ReadOptions, SetMetadata, WriteOptions};
use remotefs::{AsyncRemoteFs, RemoteErrorType};
use serial_test::serial;

use super::{Auth, WebDAVFs};

const URL: &str = "http://localhost:3080";

struct Context {
    fs: WebDAVFs,
    root: PathBuf,
}

impl Context {
    async fn new() -> Self {
        crate::mock::logger();
        let mut fs = WebDAVFs::new(URL, Auth::basic("alice", "secret1234")).unwrap();
        fs.connect().await.expect("connect");
        assert!(fs.is_connected());
        let root = PathBuf::from(format!("/test-{id}", id = uuid::Uuid::new_v4()));
        fs.create_dir(&root, None).await.expect("create tempdir");
        Self { fs, root }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }

    async fn write(&self, name: &str, data: &[u8]) -> PathBuf {
        let path = self.path(name);
        let mut source = Cursor::new(data.to_vec());
        let opts = WriteOptions::default().size_hint(data.len() as u64);
        let written = self.fs.write_file(&path, &opts, &mut source).await.unwrap();
        assert_eq!(written, data.len() as u64);
        path
    }

    async fn read(&self, path: &Path, opts: &ReadOptions) -> Vec<u8> {
        let mut output = Vec::new();
        self.fs.read_file(path, opts, &mut output).await.unwrap();
        output
    }

    async fn finish(mut self) {
        self.fs
            .remove_dir_all(&self.root)
            .await
            .expect("remove tempdir");
        self.fs.disconnect().await.expect("disconnect");
        assert!(!self.fs.is_connected());
    }
}

#[tokio::test]
#[serial]
async fn connect_rejects_bad_credentials() {
    crate::mock::logger();
    let mut fs = WebDAVFs::new(URL, Auth::basic("alice", "wrong")).unwrap();
    assert_eq!(
        fs.connect().await.unwrap_err().kind(),
        RemoteErrorType::AuthenticationFailed
    );
    assert!(!fs.is_connected());
}

#[tokio::test]
#[serial]
async fn write_stat_and_read_a_file() {
    let context = Context::new().await;
    let path = context.write("a.txt", b"test data\n").await;
    let entry = context.fs.stat(&path).await.unwrap();
    assert_eq!(entry.name(), "a.txt");
    assert_eq!(entry.path(), path.as_path());
    assert!(entry.is_file());
    assert_eq!(entry.metadata().size, Some(10));
    assert_eq!(entry.metadata().mode, None);
    assert!(entry.metadata().modified.is_some());
    assert_eq!(
        context.read(&path, &ReadOptions::default()).await,
        b"test data\n"
    );
    context.finish().await;
}

#[tokio::test]
#[serial]
async fn ranged_reads_are_honored_by_the_server() {
    let context = Context::new().await;
    let path = context.write("range.txt", b"abcdef").await;
    assert_eq!(
        context
            .read(&path, &ReadOptions::default().offset(2).length(2))
            .await,
        b"cd"
    );
    assert_eq!(
        context.read(&path, &ReadOptions::default().offset(4)).await,
        b"ef"
    );
    assert!(
        context
            .read(&path, &ReadOptions::default().offset(100))
            .await
            .is_empty()
    );
    assert!(
        context
            .read(&path, &ReadOptions::default().offset(2).length(0))
            .await
            .is_empty()
    );
    context.finish().await;
}

#[tokio::test]
#[serial]
async fn owned_streams_finish_explicitly_and_dropped_uploads_vanish() {
    let context = Context::new().await;
    let path = context.path("stream.txt");
    let mut writer = context
        .fs
        .create(&path, &WriteOptions::default())
        .await
        .unwrap();
    writer.write_all(b"hello ").await.unwrap();
    writer.write_all(b"world").await.unwrap();
    writer.finish().await.unwrap();

    let mut reader = context
        .fs
        .open(&path, &ReadOptions::default())
        .await
        .unwrap();
    let mut output = String::new();
    reader.read_to_string(&mut output).await.unwrap();
    reader.finish().await.unwrap();
    assert_eq!(output, "hello world");

    let abandoned = context.path("abandoned.txt");
    let mut writer = context
        .fs
        .create(&abandoned, &WriteOptions::default())
        .await
        .unwrap();
    writer.write_all(b"never sent").await.unwrap();
    drop(writer);
    assert!(!context.fs.exists(&abandoned).await.unwrap());
    context.finish().await;
}

#[tokio::test]
#[serial]
async fn create_in_a_missing_collection_fails_on_finish() {
    let context = Context::new().await;
    let path = context.path("missing/child.txt");
    let mut writer = context
        .fs
        .create(&path, &WriteOptions::default())
        .await
        .unwrap();
    writer.write_all(b"x").await.unwrap();
    assert_eq!(
        writer.finish().await.unwrap_err().kind(),
        // bytemark/webdav reports a missing PUT parent as 403 Forbidden.
        RemoteErrorType::PermissionDenied
    );
    context.finish().await;
}

#[tokio::test]
#[serial]
async fn list_dir_returns_children_only() {
    let context = Context::new().await;
    let file = context.write("a.txt", b"test data\n").await;
    let dir = context.path("sub");
    context.fs.create_dir(&dir, None).await.unwrap();
    let mut entries = context.fs.list_dir(&context.root).await.unwrap();
    entries.sort_by_key(|entry| entry.name());
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].path(), file.as_path());
    assert!(entries[0].is_file());
    assert_eq!(entries[0].metadata().size, Some(10));
    assert_eq!(entries[1].path(), dir.as_path());
    assert!(entries[1].is_dir());
    context.finish().await;
}

#[tokio::test]
#[serial]
async fn exists_distinguishes_present_and_missing_entries() {
    let context = Context::new().await;
    let path = context.write("a.txt", b"test data\n").await;
    assert!(context.fs.exists(&path).await.unwrap());
    assert!(!context.fs.exists(&context.path("b.txt")).await.unwrap());
    assert!(
        !context
            .fs
            .exists(Path::new("/tmp/ppppp/bhhrhu"))
            .await
            .unwrap()
    );
    assert!(context.fs.exists(&context.root).await.unwrap());
    context.finish().await;
}

#[tokio::test]
#[serial]
async fn directories_are_created_once_and_need_a_parent() {
    let context = Context::new().await;
    let dir = context.path("mydir");
    context.fs.create_dir(&dir, None).await.unwrap();
    assert!(context.fs.stat(&dir).await.unwrap().is_dir());
    assert_eq!(
        context.fs.create_dir(&dir, None).await.unwrap_err().kind(),
        RemoteErrorType::AlreadyExists
    );
    assert_eq!(
        context
            .fs
            .create_dir(&context.path("nope/child"), None)
            .await
            .unwrap_err()
            .kind(),
        RemoteErrorType::NoSuchFileOrDirectory
    );
    context.finish().await;
}

#[tokio::test]
#[serial]
async fn remove_operations_follow_the_contract() {
    let context = Context::new().await;
    let file = context.write("a.txt", b"test data\n").await;
    context.fs.remove_file(&file).await.unwrap();
    assert!(!context.fs.exists(&file).await.unwrap());

    let dir = context.path("tree");
    context.fs.create_dir(&dir, None).await.unwrap();
    context.write("tree/a.txt", b"test data\n").await;
    assert_eq!(
        context.fs.remove_dir(&dir).await.unwrap_err().kind(),
        RemoteErrorType::DirectoryNotEmpty
    );
    context.fs.remove_dir_all(&dir).await.unwrap();
    assert!(!context.fs.exists(&dir).await.unwrap());

    let empty = context.path("empty");
    context.fs.create_dir(&empty, None).await.unwrap();
    context.fs.remove_dir(&empty).await.unwrap();
    assert!(!context.fs.exists(&empty).await.unwrap());
    assert!(
        context
            .fs
            .remove_dir(&context.path("absent"))
            .await
            .is_err()
    );
    context.finish().await;
}

#[tokio::test]
#[serial]
async fn rename_and_copy_move_bytes_between_paths() {
    let context = Context::new().await;
    let src = context.write("a.txt", b"test data\n").await;
    let renamed = context.path("b.txt");
    context.fs.rename(&src, &renamed).await.unwrap();
    assert!(!context.fs.exists(&src).await.unwrap());
    assert_eq!(
        context.read(&renamed, &ReadOptions::default()).await,
        b"test data\n"
    );

    let copied = context.path("c.txt");
    context.fs.copy(&renamed, &copied).await.unwrap();
    assert!(context.fs.exists(&renamed).await.unwrap());
    assert_eq!(
        context.read(&copied, &ReadOptions::default()).await,
        b"test data\n"
    );

    let dir = context.path("dir");
    context.fs.create_dir(&dir, None).await.unwrap();
    context.write("dir/inner.txt", b"inner\n").await;
    let dir_copy = context.path("dir-copy");
    context.fs.copy(&dir, &dir_copy).await.unwrap();
    assert!(
        context
            .fs
            .exists(&dir_copy.join("inner.txt"))
            .await
            .unwrap()
    );
    let dir_moved = context.path("dir-moved");
    context.fs.rename(&dir_copy, &dir_moved).await.unwrap();
    assert!(!context.fs.exists(&dir_copy).await.unwrap());
    assert!(context.fs.stat(&dir_moved).await.unwrap().is_dir());
    context.finish().await;
}

#[tokio::test]
#[serial]
async fn unsupported_operations_are_reported() {
    let context = Context::new().await;
    let path = context.write("a.sh", b"echo 5\n").await;
    let mut source = Cursor::new(b"more".to_vec());
    assert_eq!(
        context
            .fs
            .append_file(&path, &WriteOptions::default(), &mut source)
            .await
            .unwrap_err()
            .kind(),
        RemoteErrorType::UnsupportedFeature
    );
    assert_eq!(
        context
            .fs
            .set_metadata(&path, &SetMetadata::default().uid(1000))
            .await
            .unwrap_err()
            .kind(),
        RemoteErrorType::UnsupportedFeature
    );
    assert_eq!(
        context
            .fs
            .symlink(&context.path("b.sh"), &path)
            .await
            .unwrap_err()
            .kind(),
        RemoteErrorType::UnsupportedFeature
    );
    assert_eq!(
        context.fs.exec("echo 5").await.unwrap_err().kind(),
        RemoteErrorType::UnsupportedFeature
    );
    context.finish().await;
}

#[cfg(feature = "find")]
#[tokio::test]
#[serial]
async fn find_async_walks_from_an_explicit_root() {
    let context = Context::new().await;
    context.write("a.log", b"a\n").await;
    context
        .fs
        .create_dir(&context.path("sub"), None)
        .await
        .unwrap();
    context.write("sub/b.log", b"b\n").await;
    context.write("sub/c.txt", b"c\n").await;
    let mut found: Vec<_> = remotefs::find_async(&context.fs, &context.root, "*.log")
        .await
        .unwrap()
        .into_iter()
        .map(|entry| entry.name())
        .collect();
    found.sort();
    assert_eq!(found, ["a.log", "b.log"]);
    context.finish().await;
}

#[cfg(feature = "tokio")]
#[test]
#[serial]
fn blocking_wrapper_round_trips_against_the_container() {
    use std::io::Cursor;

    use remotefs::RemoteFs;

    crate::mock::logger();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let mut client: Box<dyn RemoteFs> = Box::new(
        WebDAVFs::new(URL, Auth::basic("alice", "secret1234"))
            .unwrap()
            .into_blocking(runtime.handle().clone()),
    );
    client.connect().unwrap();
    let root = PathBuf::from(format!("/test-{id}", id = uuid::Uuid::new_v4()));
    client.create_dir(&root, None).unwrap();
    let path = root.join("hello.txt");
    let mut source = Cursor::new(b"hello".to_vec());
    let written = client
        .write_file(&path, &WriteOptions::default().size_hint(5), &mut source)
        .unwrap();
    assert_eq!(written, 5);
    let names: Vec<_> = client
        .list_dir(&root)
        .unwrap()
        .into_iter()
        .map(|entry| entry.name())
        .collect();
    assert_eq!(names, ["hello.txt"]);
    let mut output = Vec::new();
    client
        .read_file(
            &path,
            &ReadOptions::default().offset(1).length(3),
            &mut output,
        )
        .unwrap();
    assert_eq!(output, b"ell");
    client.remove_dir_all(&root).unwrap();
    client.disconnect().unwrap();
}
