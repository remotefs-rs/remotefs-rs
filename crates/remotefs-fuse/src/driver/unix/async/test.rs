use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use futures_util::io::Cursor;
use pretty_assertions::assert_eq;
use remotefs::AsyncRemoteFs;
use remotefs::adapters::r#async::Unblock;
use remotefs::fs::{ReadOptions, UnixPex, WriteOptions};
use remotefs_memory::{Inode, MemoryFs, Node, Tree, node};
use tokio::sync::RwLock;

use super::{AsyncDriver, AsyncInner, HandleSlot};
use crate::MountOption;
use crate::driver::unix::state::RequestMeta;

type Remote = Unblock<MemoryFs>;

async fn setup() -> (AsyncDriver<Remote>, Arc<RwLock<Remote>>) {
    let gid = nix::unistd::getgid().as_raw();
    let uid = nix::unistd::getuid().as_raw();
    let tree = Tree::new(node!(
        PathBuf::from("/"),
        Inode::dir(uid, gid, UnixPex::from(0o755)),
    ));
    let fs = MemoryFs::new(tree)
        .with_get_gid(|| nix::unistd::getgid().as_raw())
        .with_get_uid(|| nix::unistd::getuid().as_raw());
    let mut remote = Unblock::new(fs);
    remote.connect().await.expect("connect");
    let remote = Arc::new(RwLock::new(remote));
    let driver = AsyncDriver::new(
        Arc::clone(&remote),
        vec![MountOption::AllowRoot, MountOption::RW],
        tokio::runtime::Handle::current(),
    );
    (driver, remote)
}

fn inner<T: AsyncRemoteFs + 'static>(driver: &AsyncDriver<T>) -> Arc<AsyncInner<T>> {
    Arc::clone(&driver.inner)
}

fn req() -> RequestMeta {
    RequestMeta {
        uid: nix::unistd::getuid().as_raw(),
        gid: nix::unistd::getgid().as_raw(),
        pid: 1,
    }
}

async fn make_file(remote: &Arc<RwLock<Remote>>, path: &Path, content: &[u8]) {
    let remote = remote.read().await;
    let mut dir = PathBuf::from("/");
    for stem in path.parent().expect("parent").iter().skip(1) {
        dir.push(stem);
        let _ = remote.create_dir(&dir, Some(UnixPex::from(0o755))).await;
    }
    remote
        .write_file(
            path,
            &WriteOptions::default().size_hint(content.len() as u64),
            &mut Cursor::new(content.to_vec()),
        )
        .await
        .expect("write_file");
}

async fn read_all(remote: &Arc<RwLock<Remote>>, path: &Path) -> Vec<u8> {
    let mut out = Cursor::new(Vec::new());
    remote
        .read()
        .await
        .read_file(path, &ReadOptions::default(), &mut out)
        .await
        .expect("read_file");
    out.into_inner()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_should_resolve_inode_from_path_and_back() {
    let (driver, remote) = setup().await;
    make_file(&remote, Path::new("/tmp/test.txt"), b"hello world").await;
    let inner = inner(&driver);

    let (file, attrs) = inner
        .get_inode_from_path(Path::new("/tmp/test.txt"))
        .await
        .expect("stat");
    assert_eq!(file.path(), Path::new("/tmp/test.txt"));
    assert_eq!(attrs.size, 11);

    let (again, attrs_again) = inner.get_inode(attrs.ino.0).await.expect("stat by inode");
    assert_eq!(again, file);
    assert_eq!(attrs_again, attrs);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_should_lookup_name_under_parent() {
    let (driver, remote) = setup().await;
    make_file(&remote, Path::new("/home/user/a.txt"), b"").await;
    let inner = inner(&driver);
    let (_, parent) = inner
        .get_inode_from_path(Path::new("/home/user"))
        .await
        .expect("stat");
    let looked_up = inner
        .state()
        .lookup_name(parent.ino.0, OsStr::new("a.txt"))
        .expect("lookup");
    assert_eq!(looked_up, PathBuf::from("/home/user/a.txt"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_should_read_at_offset() {
    let (driver, remote) = setup().await;
    make_file(&remote, Path::new("/tmp/test.txt"), b"hello world").await;
    let mut buffer = [0u8; 5];
    let read = inner(&driver)
        .read_remote_file(Path::new("/tmp/test.txt"), &mut buffer, 6)
        .await
        .expect("read");
    assert_eq!(read, 5);
    assert_eq!(&buffer, b"world");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_should_stage_chunks_on_one_handle_and_finalize() {
    let (driver, remote) = setup().await;
    make_file(&remote, Path::new("/tmp/test.txt"), b"").await;
    let inner = inner(&driver);
    let (file, attrs) = inner
        .get_inode_from_path(Path::new("/tmp/test.txt"))
        .await
        .expect("stat");
    let fh = inner.open_handle(req().pid, attrs.ino.0, true, true);
    let slot = inner.slot(req().pid, fh).expect("slot");

    inner
        .write_to_handle(&slot, &file, b"hello ", 0)
        .await
        .expect("chunk 1");
    inner
        .write_to_handle(&slot, &file, b"world", 6)
        .await
        .expect("chunk 2");
    inner.finalize_pending_write(&slot).await.expect("finalize");

    assert_eq!(
        read_all(&remote, Path::new("/tmp/test.txt")).await,
        b"hello world"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_should_finalize_pending_write_for_fsync() {
    let (driver, remote) = setup().await;
    make_file(&remote, Path::new("/tmp/test.txt"), b"").await;
    let inner = inner(&driver);
    let (file, attrs) = inner
        .get_inode_from_path(Path::new("/tmp/test.txt"))
        .await
        .expect("stat");
    let fh = inner.open_handle(req().pid, attrs.ino.0, true, true);
    let slot = inner.slot(req().pid, fh).expect("slot");

    inner
        .write_to_handle(&slot, &file, b"durable", 0)
        .await
        .expect("write");
    inner.fsync_pending_write(&slot).await.expect("fsync");

    assert_eq!(
        read_all(&remote, Path::new("/tmp/test.txt")).await,
        b"durable"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_should_keep_write_order_across_tasks() {
    let (driver, remote) = setup().await;
    make_file(&remote, Path::new("/tmp/test.txt"), b"").await;
    let inner = inner(&driver);
    let (file, attrs) = inner
        .get_inode_from_path(Path::new("/tmp/test.txt"))
        .await
        .expect("stat");
    let fh = inner.open_handle(req().pid, attrs.ino.0, true, true);
    let slot: Arc<HandleSlot> = inner.slot(req().pid, fh).expect("slot");

    let chunks: Vec<(u64, Vec<u8>)> = (0..8u64)
        .map(|i| (i * 4, format!("{i:04}").into_bytes()))
        .collect();
    let tickets: Vec<u64> = chunks.iter().map(|_| slot.turnstile.ticket()).collect();
    let mut tasks = Vec::new();
    for ((offset, data), ticket) in chunks.into_iter().zip(tickets).rev() {
        let inner = Arc::clone(&inner);
        let slot = Arc::clone(&slot);
        let file = file.clone();
        tasks.push(tokio::spawn(async move {
            let _turn = slot.turnstile.wait(ticket).await;
            inner
                .write_to_handle(&slot, &file, &data, offset)
                .await
                .expect("write");
        }));
    }
    for task in tasks {
        task.await.expect("task");
    }
    inner.finalize_pending_write(&slot).await.expect("finalize");

    assert_eq!(
        read_all(&remote, Path::new("/tmp/test.txt")).await,
        b"00000001000200030004000500060007"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_should_truncate_via_transfer_helper() {
    let (driver, remote) = setup().await;
    make_file(&remote, Path::new("/tmp/test.txt"), b"hello world").await;
    let inner = inner(&driver);
    let (file, _) = inner
        .get_inode_from_path(Path::new("/tmp/test.txt"))
        .await
        .expect("stat");
    crate::driver::transfer::r#async::truncate_file(&*remote.read().await, &file, 5)
        .await
        .expect("truncate");
    assert_eq!(
        read_all(&remote, Path::new("/tmp/test.txt")).await,
        b"hello"
    );
}
