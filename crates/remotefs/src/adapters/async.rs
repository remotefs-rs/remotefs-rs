//! Asynchronous adapters for blocking filesystem clients.

mod stream;
mod transfer;

use std::fmt;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, RwLock};

use self::stream::{UnblockRead, UnblockWrite};
use crate::fs::{
    AsyncReadStream, AsyncRemoteFs, AsyncWriteStream, Capabilities, ExecOutput, File, ReadOptions,
    RemoteError, RemoteErrorType, RemoteResult, SetMetadata, UnixPex, WriteOptions,
};

/// An asynchronous view of a blocking filesystem client.
///
/// Blocking operations run on Tokio's blocking pool. Streamed writes acknowledge
/// bytes accepted into a bounded local buffer; `flush` and `finish` wait for the
/// worker and report write failures. Dropping a stream may leave accepted bytes
/// written remotely, and does not finalize its protocol transfer.
///
/// One-shot uploads stop polling the borrowed source when the backend completes,
/// including overrides that consume a declared length without waiting for EOF.
/// Downloads drain buffered bytes into the destination and flush it before
/// returning success. Dropping a transfer future closes the caller's pipe endpoint
/// and cancels the local pump. A running blocking worker can continue until its
/// current backend operation and cleanup return; cancellation cannot interrupt
/// arbitrary blocking I/O.
#[non_exhaustive]
pub struct Unblock<T> {
    shared: Arc<Shared<T>>,
}

struct Shared<T> {
    inner: RwLock<T>,
    connected: AtomicBool,
    capabilities: AtomicU32,
}

impl<T> Unblock<T> {
    /// Wraps a blocking client for use from a Tokio runtime.
    pub fn new(inner: T) -> Self
    where
        T: crate::fs::RemoteFs + 'static,
    {
        let connected = inner.is_connected();
        let capabilities = inner.capabilities().bits();
        Self {
            shared: Arc::new(Shared {
                inner: RwLock::new(inner),
                connected: AtomicBool::new(connected),
                capabilities: AtomicU32::new(capabilities),
            }),
        }
    }
}

impl<T> fmt::Debug for Unblock<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("Unblock").finish_non_exhaustive()
    }
}

impl<T: crate::fs::RemoteFs + 'static> Unblock<T> {
    fn spawn_read<R, F>(&self, operation: F) -> tokio::task::JoinHandle<RemoteResult<R>>
    where
        F: FnOnce(&T) -> RemoteResult<R> + Send + 'static,
        R: Send + 'static,
    {
        spawn_read(self.shared.clone(), operation)
    }

    fn spawn_mut<R, F>(&self, operation: F) -> tokio::task::JoinHandle<RemoteResult<R>>
    where
        F: FnOnce(&mut T) -> RemoteResult<R> + Send + 'static,
        R: Send + 'static,
    {
        spawn_mut(self.shared.clone(), operation)
    }
}

fn refresh<T: crate::fs::RemoteFs>(shared: &Shared<T>, inner: &T) {
    shared
        .connected
        .store(inner.is_connected(), Ordering::Release);
    shared
        .capabilities
        .store(inner.capabilities().bits(), Ordering::Release);
}

fn lock_error() -> RemoteError {
    RemoteError::with_message(
        RemoteErrorType::ProtocolError,
        "blocking client lock poisoned",
    )
}

fn spawn_read<T, R, F>(
    shared: Arc<Shared<T>>,
    operation: F,
) -> tokio::task::JoinHandle<RemoteResult<R>>
where
    T: crate::fs::RemoteFs + 'static,
    F: FnOnce(&T) -> RemoteResult<R> + Send + 'static,
    R: Send + 'static,
{
    tokio::task::spawn_blocking(move || match shared.inner.read() {
        Ok(inner) => {
            let result = operation(&inner);
            refresh(&shared, &inner);
            result
        }
        Err(_) => Err(lock_error()),
    })
}

fn spawn_mut<T, R, F>(
    shared: Arc<Shared<T>>,
    operation: F,
) -> tokio::task::JoinHandle<RemoteResult<R>>
where
    T: crate::fs::RemoteFs + 'static,
    F: FnOnce(&mut T) -> RemoteResult<R> + Send + 'static,
    R: Send + 'static,
{
    tokio::task::spawn_blocking(move || match shared.inner.write() {
        Ok(mut inner) => {
            let result = operation(&mut inner);
            refresh(&shared, &inner);
            result
        }
        Err(_) => Err(lock_error()),
    })
}

async fn await_job<R>(job: tokio::task::JoinHandle<RemoteResult<R>>) -> RemoteResult<R>
where
    R: Send + 'static,
{
    job.await
        .map_err(|error| RemoteError::with_source(RemoteErrorType::ProtocolError, error))?
}

#[async_trait::async_trait]
impl<T: crate::fs::RemoteFs + 'static> AsyncRemoteFs for Unblock<T> {
    async fn connect(&mut self) -> RemoteResult<()> {
        await_job(self.spawn_mut(crate::fs::RemoteFs::connect)).await
    }

    async fn disconnect(&mut self) -> RemoteResult<()> {
        await_job(self.spawn_mut(crate::fs::RemoteFs::disconnect)).await
    }

    fn is_connected(&self) -> bool {
        self.shared.connected.load(Ordering::Acquire)
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities::from_bits(self.shared.capabilities.load(Ordering::Acquire))
    }

    async fn list_dir(&self, path: &Path) -> RemoteResult<Vec<File>> {
        let path = path.to_path_buf();
        await_job(self.spawn_read(move |inner| inner.list_dir(&path))).await
    }

    async fn stat(&self, path: &Path) -> RemoteResult<File> {
        let path = path.to_path_buf();
        await_job(self.spawn_read(move |inner| inner.stat(&path))).await
    }

    async fn exists(&self, path: &Path) -> RemoteResult<bool> {
        let path = path.to_path_buf();
        await_job(self.spawn_read(move |inner| inner.exists(&path))).await
    }

    async fn set_metadata(&self, path: &Path, metadata: &SetMetadata) -> RemoteResult<()> {
        let path = path.to_path_buf();
        let metadata = metadata.clone();
        await_job(self.spawn_read(move |inner| inner.set_metadata(&path, &metadata))).await
    }

    async fn create_dir(&self, path: &Path, mode: Option<UnixPex>) -> RemoteResult<()> {
        let path = path.to_path_buf();
        await_job(self.spawn_read(move |inner| inner.create_dir(&path, mode))).await
    }

    async fn remove_file(&self, path: &Path) -> RemoteResult<()> {
        let path = path.to_path_buf();
        await_job(self.spawn_read(move |inner| inner.remove_file(&path))).await
    }

    async fn remove_dir(&self, path: &Path) -> RemoteResult<()> {
        let path = path.to_path_buf();
        await_job(self.spawn_read(move |inner| inner.remove_dir(&path))).await
    }

    async fn remove_dir_all(&self, path: &Path) -> RemoteResult<()> {
        let path = path.to_path_buf();
        await_job(self.spawn_read(move |inner| inner.remove_dir_all(&path))).await
    }

    async fn rename(&self, src: &Path, dest: &Path) -> RemoteResult<()> {
        let src = src.to_path_buf();
        let dest = dest.to_path_buf();
        await_job(self.spawn_read(move |inner| inner.rename(&src, &dest))).await
    }

    async fn copy(&self, src: &Path, dest: &Path) -> RemoteResult<()> {
        let src = src.to_path_buf();
        let dest = dest.to_path_buf();
        await_job(self.spawn_read(move |inner| inner.copy(&src, &dest))).await
    }

    async fn symlink(&self, path: &Path, target: &Path) -> RemoteResult<()> {
        let path = path.to_path_buf();
        let target = target.to_path_buf();
        await_job(self.spawn_read(move |inner| inner.symlink(&path, &target))).await
    }

    async fn open(&self, path: &Path, opts: &ReadOptions) -> RemoteResult<AsyncReadStream> {
        let path = path.to_path_buf();
        let opts = opts.clone();
        await_job(self.spawn_read(move |inner| {
            let stream = inner.open(&path, &opts)?;
            Ok(AsyncReadStream::new(UnblockRead::new(stream)))
        }))
        .await
    }

    async fn create(&self, path: &Path, opts: &WriteOptions) -> RemoteResult<AsyncWriteStream> {
        let path = path.to_path_buf();
        let opts = opts.clone();
        await_job(self.spawn_read(move |inner| {
            let stream = inner.create(&path, &opts)?;
            Ok(AsyncWriteStream::new(UnblockWrite::new(stream)))
        }))
        .await
    }

    async fn append(&self, path: &Path, opts: &WriteOptions) -> RemoteResult<AsyncWriteStream> {
        let path = path.to_path_buf();
        let opts = opts.clone();
        await_job(self.spawn_read(move |inner| {
            let stream = inner.append(&path, &opts)?;
            Ok(AsyncWriteStream::new(UnblockWrite::new(stream)))
        }))
        .await
    }

    async fn read_file(
        &self,
        path: &Path,
        opts: &ReadOptions,
        dest: &mut (dyn futures_io::AsyncWrite + Send + Unpin),
    ) -> RemoteResult<u64> {
        self::transfer::read_file(self.shared.clone(), path, opts, dest).await
    }

    async fn write_file(
        &self,
        path: &Path,
        opts: &WriteOptions,
        src: &mut (dyn futures_io::AsyncRead + Send + Unpin),
    ) -> RemoteResult<u64> {
        self::transfer::write_file(self.shared.clone(), path, opts, src, false).await
    }

    async fn append_file(
        &self,
        path: &Path,
        opts: &WriteOptions,
        src: &mut (dyn futures_io::AsyncRead + Send + Unpin),
    ) -> RemoteResult<u64> {
        self::transfer::write_file(self.shared.clone(), path, opts, src, true).await
    }

    async fn exec(&self, cmd: &str) -> RemoteResult<ExecOutput> {
        let cmd = cmd.to_owned();
        await_job(self.spawn_read(move |inner| inner.exec(&cmd))).await
    }
}
