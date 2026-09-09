//! Blocking adapters for asynchronous filesystem clients.

mod stream;

use std::fmt;
use std::path::Path;

use self::stream::{BlockingRead, BlockingWrite, BorrowedRead, BorrowedWrite};
use crate::fs::{
    AsyncRemoteFs, Capabilities, ExecOutput, File, ReadOptions, ReadStream, RemoteFs, RemoteResult,
    SetMetadata, UnixPex, Welcome, WriteOptions, WriteStream,
};

/// A blocking view of an asynchronous filesystem client.
///
/// Each operation uses the supplied Tokio handle. Calling an operation from an
/// asynchronous execution context panics, matching [`tokio::runtime::Handle::block_on`].
/// Use `spawn_blocking` when a blocking operation must be initiated by async code.
pub struct BlockOn<T> {
    inner: T,
    handle: tokio::runtime::Handle,
}

impl<T> BlockOn<T> {
    /// Wraps an asynchronous client with a caller-owned Tokio runtime handle.
    pub fn new(inner: T, handle: tokio::runtime::Handle) -> Self {
        Self { inner, handle }
    }

    /// Consumes the adapter and returns its asynchronous client.
    pub fn into_inner(self) -> T {
        self.inner
    }
}

impl<T> fmt::Debug for BlockOn<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("BlockOn").finish_non_exhaustive()
    }
}

impl<T: AsyncRemoteFs> RemoteFs for BlockOn<T> {
    fn connect(&mut self) -> RemoteResult<Welcome> {
        self.handle.block_on(self.inner.connect())
    }

    fn disconnect(&mut self) -> RemoteResult<()> {
        self.handle.block_on(self.inner.disconnect())
    }

    fn is_connected(&self) -> bool {
        self.inner.is_connected()
    }

    fn capabilities(&self) -> Capabilities {
        self.inner.capabilities()
    }

    fn list_dir(&self, path: &Path) -> RemoteResult<Vec<File>> {
        self.handle.block_on(self.inner.list_dir(path))
    }

    fn stat(&self, path: &Path) -> RemoteResult<File> {
        self.handle.block_on(self.inner.stat(path))
    }

    fn exists(&self, path: &Path) -> RemoteResult<bool> {
        self.handle.block_on(self.inner.exists(path))
    }

    fn set_metadata(&self, path: &Path, metadata: &SetMetadata) -> RemoteResult<()> {
        self.handle
            .block_on(self.inner.set_metadata(path, metadata))
    }

    fn create_dir(&self, path: &Path, mode: Option<UnixPex>) -> RemoteResult<()> {
        self.handle.block_on(self.inner.create_dir(path, mode))
    }

    fn remove_file(&self, path: &Path) -> RemoteResult<()> {
        self.handle.block_on(self.inner.remove_file(path))
    }

    fn remove_dir(&self, path: &Path) -> RemoteResult<()> {
        self.handle.block_on(self.inner.remove_dir(path))
    }

    fn remove_dir_all(&self, path: &Path) -> RemoteResult<()> {
        self.handle.block_on(self.inner.remove_dir_all(path))
    }

    fn rename(&self, src: &Path, dest: &Path) -> RemoteResult<()> {
        self.handle.block_on(self.inner.rename(src, dest))
    }

    fn copy(&self, src: &Path, dest: &Path) -> RemoteResult<()> {
        self.handle.block_on(self.inner.copy(src, dest))
    }

    fn symlink(&self, path: &Path, target: &Path) -> RemoteResult<()> {
        self.handle.block_on(self.inner.symlink(path, target))
    }

    fn open(&self, path: &Path, opts: &ReadOptions) -> RemoteResult<ReadStream> {
        let stream = self.handle.block_on(self.inner.open(path, opts))?;
        Ok(ReadStream::new(BlockingRead::new(stream, &self.handle)))
    }

    fn create(&self, path: &Path, opts: &WriteOptions) -> RemoteResult<WriteStream> {
        let stream = self.handle.block_on(self.inner.create(path, opts))?;
        Ok(WriteStream::new(BlockingWrite::new(stream, &self.handle)))
    }

    fn append(&self, path: &Path, opts: &WriteOptions) -> RemoteResult<WriteStream> {
        let stream = self.handle.block_on(self.inner.append(path, opts))?;
        Ok(WriteStream::new(BlockingWrite::new(stream, &self.handle)))
    }

    fn read_file(
        &self,
        path: &Path,
        opts: &ReadOptions,
        dest: &mut (dyn std::io::Write + Send),
    ) -> RemoteResult<u64> {
        let mut dest = BorrowedWrite::new(dest);
        self.handle
            .block_on(self.inner.read_file(path, opts, &mut dest))
    }

    fn write_file(
        &self,
        path: &Path,
        opts: &WriteOptions,
        src: &mut (dyn std::io::Read + Send),
    ) -> RemoteResult<u64> {
        let mut src = BorrowedRead::new(src);
        self.handle
            .block_on(self.inner.write_file(path, opts, &mut src))
    }

    fn append_file(
        &self,
        path: &Path,
        opts: &WriteOptions,
        src: &mut (dyn std::io::Read + Send),
    ) -> RemoteResult<u64> {
        let mut src = BorrowedRead::new(src);
        self.handle
            .block_on(self.inner.append_file(path, opts, &mut src))
    }

    fn exec(&self, cmd: &str) -> RemoteResult<ExecOutput> {
        self.handle.block_on(self.inner.exec(cmd))
    }
}
