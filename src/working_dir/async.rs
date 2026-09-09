//! Asynchronous working-directory filesystem wrapper.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

use futures_io::{AsyncRead, AsyncWrite};

use crate::fs::{
    AsyncReadStream, AsyncRemoteFs, AsyncWriteStream, Capabilities, ExecOutput, File, ReadOptions,
    RemoteError, RemoteErrorType, RemoteResult, SetMetadata, UnixPex, Welcome, WriteOptions,
};

/// An asynchronous filesystem wrapper that resolves relative paths against a cwd.
#[non_exhaustive]
pub struct AsyncWorkingDir<T> {
    inner: T,
    cwd: RwLock<PathBuf>,
}

impl<T> AsyncWorkingDir<T> {
    /// Wraps a filesystem with an absolute initial working directory.
    ///
    /// # Panics
    ///
    /// Panics when `cwd` is not absolute.
    pub fn new(inner: T, cwd: impl Into<PathBuf>) -> Self {
        let cwd = cwd.into();
        assert!(
            crate::path::ensure_absolute(&cwd).is_ok(),
            "working directory must be absolute"
        );
        Self {
            inner,
            cwd: RwLock::new(cwd),
        }
    }

    /// Returns a cloned snapshot of the current working directory.
    pub fn pwd(&self) -> PathBuf {
        match self.cwd.read() {
            Ok(cwd) => cwd.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    /// Consumes the wrapper and returns its filesystem.
    pub fn into_inner(self) -> T {
        let Self { inner, cwd } = self;
        let _ = match cwd.into_inner() {
            Ok(cwd) => cwd,
            Err(poisoned) => poisoned.into_inner(),
        };
        inner
    }
}

impl<T> fmt::Debug for AsyncWorkingDir<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AsyncWorkingDir")
            .field("cwd", &self.pwd())
            .finish_non_exhaustive()
    }
}

impl<T: AsyncRemoteFs> AsyncWorkingDir<T> {
    /// Changes the current working directory after verifying it remotely.
    ///
    /// The directory is changed only when the target exists and is a directory.
    ///
    /// # Errors
    ///
    /// Returns the backend error when the target cannot be inspected, or
    /// [`crate::fs::RemoteErrorType::BadFile`] when it is not a directory.
    pub async fn change_dir(&self, dir: &Path) -> RemoteResult<PathBuf> {
        let resolved = self.resolve(dir)?;
        let entry = self.inner.stat(&resolved).await?;
        if !entry.is_dir() {
            return Err(RemoteError::new(RemoteErrorType::BadFile));
        }
        let cwd = crate::path::ensure_absolute(&entry.path)?.to_path_buf();
        match self.cwd.write() {
            Ok(mut current) => *current = cwd.clone(),
            Err(poisoned) => *poisoned.into_inner() = cwd.clone(),
        }
        Ok(cwd)
    }

    fn resolve(&self, path: &Path) -> RemoteResult<PathBuf> {
        let cwd = self.pwd();
        crate::path::absolutize(&cwd, path)
    }

    fn resolve_pair(&self, first: &Path, second: &Path) -> RemoteResult<(PathBuf, PathBuf)> {
        let cwd = self.pwd();
        Ok((
            crate::path::absolutize(&cwd, first)?,
            crate::path::absolutize(&cwd, second)?,
        ))
    }
}

#[async_trait::async_trait]
impl<T: AsyncRemoteFs> AsyncRemoteFs for AsyncWorkingDir<T> {
    async fn connect(&mut self) -> RemoteResult<Welcome> {
        self.inner.connect().await
    }

    async fn disconnect(&mut self) -> RemoteResult<()> {
        self.inner.disconnect().await
    }

    fn is_connected(&self) -> bool {
        self.inner.is_connected()
    }

    fn capabilities(&self) -> Capabilities {
        self.inner.capabilities()
    }

    async fn list_dir(&self, path: &Path) -> RemoteResult<Vec<File>> {
        let path = self.resolve(path)?;
        self.inner.list_dir(&path).await
    }

    async fn stat(&self, path: &Path) -> RemoteResult<File> {
        let path = self.resolve(path)?;
        self.inner.stat(&path).await
    }

    async fn exists(&self, path: &Path) -> RemoteResult<bool> {
        let path = self.resolve(path)?;
        self.inner.exists(&path).await
    }

    async fn set_metadata(&self, path: &Path, metadata: &SetMetadata) -> RemoteResult<()> {
        let path = self.resolve(path)?;
        self.inner.set_metadata(&path, metadata).await
    }

    async fn create_dir(&self, path: &Path, mode: Option<UnixPex>) -> RemoteResult<()> {
        let path = self.resolve(path)?;
        self.inner.create_dir(&path, mode).await
    }

    async fn remove_file(&self, path: &Path) -> RemoteResult<()> {
        let path = self.resolve(path)?;
        self.inner.remove_file(&path).await
    }

    async fn remove_dir(&self, path: &Path) -> RemoteResult<()> {
        let path = self.resolve(path)?;
        self.inner.remove_dir(&path).await
    }

    async fn remove_dir_all(&self, path: &Path) -> RemoteResult<()> {
        let path = self.resolve(path)?;
        self.inner.remove_dir_all(&path).await
    }

    async fn rename(&self, src: &Path, dest: &Path) -> RemoteResult<()> {
        let (src, dest) = self.resolve_pair(src, dest)?;
        self.inner.rename(&src, &dest).await
    }

    async fn copy(&self, src: &Path, dest: &Path) -> RemoteResult<()> {
        let (src, dest) = self.resolve_pair(src, dest)?;
        self.inner.copy(&src, &dest).await
    }

    async fn symlink(&self, path: &Path, target: &Path) -> RemoteResult<()> {
        let (path, target) = self.resolve_pair(path, target)?;
        self.inner.symlink(&path, &target).await
    }

    async fn open(&self, path: &Path, opts: &ReadOptions) -> RemoteResult<AsyncReadStream> {
        let path = self.resolve(path)?;
        self.inner.open(&path, opts).await
    }

    async fn create(&self, path: &Path, opts: &WriteOptions) -> RemoteResult<AsyncWriteStream> {
        let path = self.resolve(path)?;
        self.inner.create(&path, opts).await
    }

    async fn append(&self, path: &Path, opts: &WriteOptions) -> RemoteResult<AsyncWriteStream> {
        let path = self.resolve(path)?;
        self.inner.append(&path, opts).await
    }

    async fn read_file(
        &self,
        path: &Path,
        opts: &ReadOptions,
        dest: &mut (dyn AsyncWrite + Send + Unpin),
    ) -> RemoteResult<u64> {
        let path = self.resolve(path)?;
        self.inner.read_file(&path, opts, dest).await
    }

    async fn write_file(
        &self,
        path: &Path,
        opts: &WriteOptions,
        src: &mut (dyn AsyncRead + Send + Unpin),
    ) -> RemoteResult<u64> {
        let path = self.resolve(path)?;
        self.inner.write_file(&path, opts, src).await
    }

    async fn append_file(
        &self,
        path: &Path,
        opts: &WriteOptions,
        src: &mut (dyn AsyncRead + Send + Unpin),
    ) -> RemoteResult<u64> {
        let path = self.resolve(path)?;
        self.inner.append_file(&path, opts, src).await
    }

    async fn exec(&self, cmd: &str) -> RemoteResult<ExecOutput> {
        self.inner.exec(cmd).await
    }
}
