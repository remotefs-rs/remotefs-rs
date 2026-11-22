//! The asynchronous [`AsyncRemoteFs`] contract every protocol client implements.

use std::path::Path;

use futures_io::{AsyncRead, AsyncWrite};

use super::stream::r#async::{AsyncReadStream, AsyncWriteStream};
use super::{Capabilities, ExecOutput, File, ReadOptions, SetMetadata, UnixPex, WriteOptions};
use crate::RemoteResult;

/// The asynchronous contract for a protocol-backed remote file system.
#[async_trait::async_trait]
pub trait AsyncRemoteFs: Send + Sync {
    /// Connects to the remote server and authenticates the client.
    async fn connect(&mut self) -> RemoteResult<()>;

    /// Disconnects from the remote server.
    async fn disconnect(&mut self) -> RemoteResult<()>;

    /// Returns the cached connection state without probing the transport.
    fn is_connected(&self) -> bool;

    /// Returns the operations natively supported by this client.
    fn capabilities(&self) -> Capabilities;

    /// Lists the direct children of an absolute directory path.
    async fn list_dir(&self, path: &Path) -> RemoteResult<Vec<File>>;

    /// Returns metadata for an absolute path without following symlinks.
    async fn stat(&self, path: &Path) -> RemoteResult<File>;

    /// Reports whether an absolute path exists.
    async fn exists(&self, path: &Path) -> RemoteResult<bool>;

    /// Changes the specified metadata fields of an absolute path.
    async fn set_metadata(&self, path: &Path, metadata: &SetMetadata) -> RemoteResult<()>;

    /// Creates a directory at an absolute path.
    async fn create_dir(&self, path: &Path, mode: Option<UnixPex>) -> RemoteResult<()>;

    /// Removes a file or symbolic link at an absolute path.
    async fn remove_file(&self, path: &Path) -> RemoteResult<()>;

    /// Removes an empty directory at an absolute path.
    async fn remove_dir(&self, path: &Path) -> RemoteResult<()>;

    /// Removes an entry and its descendants using depth-first traversal.
    ///
    /// Symlinks are removed as entries and are never followed. A failure during
    /// traversal may leave earlier deletions applied remotely.
    async fn remove_dir_all(&self, path: &Path) -> RemoteResult<()> {
        crate::path::ensure_absolute(path)?;
        let entry = self.stat(path).await?;
        if entry.is_dir() {
            for child in self.list_dir(entry.path()).await? {
                self.remove_dir_all(child.path()).await?;
            }
            self.remove_dir(entry.path()).await
        } else {
            self.remove_file(entry.path()).await
        }
    }

    /// Renames an absolute source path to an absolute destination path.
    async fn rename(&self, src: &Path, dest: &Path) -> RemoteResult<()>;

    /// Copies an absolute source path to an absolute destination path.
    async fn copy(&self, src: &Path, dest: &Path) -> RemoteResult<()>;

    /// Creates a symbolic link at `path` pointing to an absolute `target`.
    async fn symlink(&self, path: &Path, target: &Path) -> RemoteResult<()>;

    /// Opens an absolute file path for a ranged read.
    async fn open(&self, path: &Path, opts: &ReadOptions) -> RemoteResult<AsyncReadStream>;

    /// Creates or truncates an absolute file path for writing.
    async fn create(&self, path: &Path, opts: &WriteOptions) -> RemoteResult<AsyncWriteStream>;

    /// Opens or creates an absolute file path for appending.
    async fn append(&self, path: &Path, opts: &WriteOptions) -> RemoteResult<AsyncWriteStream>;

    /// Reads a remote file into a borrowed destination and finalizes its stream.
    ///
    /// # Errors
    ///
    /// Returns copy, flush, open, or finalization failures. If copying and
    /// finalization both fail, both causes are retained in the returned error.
    async fn read_file(
        &self,
        path: &Path,
        opts: &ReadOptions,
        dest: &mut (dyn AsyncWrite + Send + Unpin),
    ) -> RemoteResult<u64> {
        crate::path::ensure_absolute(path)?;
        let mut stream = self.open(path, opts).await?;
        let copied = match crate::io::copy(&mut stream, dest).await {
            Ok(count) => crate::io::flush(dest).await.map(|()| count),
            Err(error) => Err(error),
        }
        .map_err(crate::RemoteError::from);
        let finished = stream.finish().await;
        super::stream::complete_transfer(copied, finished)
    }

    /// Writes a borrowed source to a remote file and finalizes its stream.
    ///
    /// Existing content is replaced.
    async fn write_file(
        &self,
        path: &Path,
        opts: &WriteOptions,
        src: &mut (dyn AsyncRead + Send + Unpin),
    ) -> RemoteResult<u64> {
        crate::path::ensure_absolute(path)?;
        let mut stream = self.create(path, opts).await?;
        let copied = match crate::io::copy(src, &mut stream).await {
            Ok(count) => crate::io::flush(&mut stream).await.map(|()| count),
            Err(error) => Err(error),
        }
        .map_err(crate::RemoteError::from);
        let finished = stream.finish().await;
        super::stream::complete_transfer(copied, finished)
    }

    /// Appends a borrowed source to a remote file and finalizes its stream.
    async fn append_file(
        &self,
        path: &Path,
        opts: &WriteOptions,
        src: &mut (dyn AsyncRead + Send + Unpin),
    ) -> RemoteResult<u64> {
        crate::path::ensure_absolute(path)?;
        let mut stream = self.append(path, opts).await?;
        let copied = match crate::io::copy(src, &mut stream).await {
            Ok(count) => crate::io::flush(&mut stream).await.map(|()| count),
            Err(error) => Err(error),
        }
        .map_err(crate::RemoteError::from);
        let finished = stream.finish().await;
        super::stream::complete_transfer(copied, finished)
    }

    /// Executes a command and returns its exit code and standard output.
    async fn exec(&self, cmd: &str) -> RemoteResult<ExecOutput>;
}
