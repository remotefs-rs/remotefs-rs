//! Asynchronous SFTP client over russh.

use std::path::Path;

use remotefs::fs::{
    AsyncReadStream, AsyncRemoteFs, AsyncWriteStream, Capabilities, ExecOutput, ReadOptions,
    SetMetadata, UnixPex, WriteOptions,
};
use remotefs::path::ensure_absolute;
use remotefs::{File, RemoteError, RemoteErrorType, RemoteResult};
use russh::client::Handler;

use super::SshOpts;
use super::backend::{RusshSession, RusshSftp, WriteMode};
use super::sftp::SFTP_CAPABILITIES;

const DEFAULT_DIR_MODE: u32 = 0o755;
const DEFAULT_FILE_MODE: u32 = 0o644;

/// Blocking adapter for [`RusshSftpFs`] created with [`RusshSftpFs::into_blocking`].
pub type BlockingRusshSftpFs<T> = remotefs::adapters::blocking::BlockOn<RusshSftpFs<T>>;

/// Native asynchronous SFTP client backed by [`russh`].
///
/// Every path passed to this client must be absolute. Use [`Self::into_blocking`]
/// when an application needs the blocking [`remotefs::RemoteFs`] contract.
pub struct RusshSftpFs<T>
where
    T: Handler + Default + Send + 'static,
{
    session: Option<RusshSession<T>>,
    sftp: Option<RusshSftp>,
    opts: SshOpts,
}

impl<T> RusshSftpFs<T>
where
    T: Handler + Default + Send + 'static,
{
    /// Constructs a disconnected SFTP client.
    pub fn new(opts: SshOpts) -> Self {
        Self {
            session: None,
            sftp: None,
            opts,
        }
    }

    /// Returns ports assigned to configured TCP `RemoteForward` listeners.
    pub fn remote_forward_ports(&self) -> Vec<u16> {
        self.session
            .as_ref()
            .map(|session| session.remote_forward_ports().to_vec())
            .unwrap_or_default()
    }

    #[must_use]
    /// Converts this asynchronous client into a blocking adapter.
    ///
    /// The handle must belong to a multi-thread Tokio runtime because blocking
    /// calls wait for asynchronous operations from the calling thread.
    pub fn into_blocking(self, handle: tokio::runtime::Handle) -> BlockingRusshSftpFs<T> {
        assert_ne!(
            handle.runtime_flavor(),
            tokio::runtime::RuntimeFlavor::CurrentThread,
            "into_blocking requires a multi-thread Tokio runtime"
        );
        remotefs::adapters::blocking::BlockOn::new(self, handle)
    }

    fn session(&self) -> RemoteResult<&RusshSession<T>> {
        self.session
            .as_ref()
            .ok_or_else(|| RemoteError::new(RemoteErrorType::NotConnected))
    }

    fn sftp(&self) -> RemoteResult<&RusshSftp> {
        self.sftp
            .as_ref()
            .ok_or_else(|| RemoteError::new(RemoteErrorType::NotConnected))
    }

    fn mode(mode: Option<UnixPex>, default: u32) -> u32 {
        mode.map(u32::from).unwrap_or(default)
    }

    async fn copy_recursive(sftp: &RusshSftp, src: &Path, dest: &Path) -> RemoteResult<()> {
        let src_file = sftp.stat(src).await?;
        if src == dest || dest.starts_with(src) {
            return Err(RemoteError::with_message(
                RemoteErrorType::InvalidPath,
                "copy destination must not be the source or one of its descendants",
            ));
        }
        if src_file.is_dir() {
            sftp.mkdir(dest, Self::mode(src_file.metadata().mode, DEFAULT_DIR_MODE))
                .await?;
            for entry in sftp.readdir(src).await? {
                let name = entry.path().file_name().ok_or_else(|| {
                    RemoteError::with_message(
                        RemoteErrorType::BadFile,
                        format!(
                            "entry has no file name: {path}",
                            path = entry.path().display()
                        ),
                    )
                })?;
                Box::pin(Self::copy_recursive(sftp, entry.path(), &dest.join(name))).await?;
            }
            return Ok(());
        }

        if src_file.is_symlink() {
            let target = src_file.metadata().symlink.as_deref().ok_or_else(|| {
                RemoteError::with_message(
                    RemoteErrorType::BadFile,
                    format!(
                        "could not read symbolic link target: {path}",
                        path = src.display()
                    ),
                )
            })?;
            return sftp.symlink(target, dest).await;
        }

        let mut reader = sftp.open_read(src, &ReadOptions::default()).await?;
        let mut writer = sftp
            .open_write(
                dest,
                WriteMode::Truncate,
                Self::mode(src_file.metadata().mode, DEFAULT_FILE_MODE),
            )
            .await?;
        let mut reader_tokio = reader.into_tokio();
        let mut writer_tokio = writer.into_tokio();
        let copied = tokio::io::copy(&mut reader_tokio, &mut writer_tokio)
            .await
            .map_err(RemoteError::from);
        reader = reader_tokio.into_inner();
        writer = writer_tokio.into_inner();
        let finished_read = reader.finish().await;
        let finished_write = writer.finish().await;
        copied.and(finished_read).and(finished_write)
    }
}

#[remotefs::async_trait]
impl<T> AsyncRemoteFs for RusshSftpFs<T>
where
    T: Handler + Default + Send + 'static,
{
    async fn connect(&mut self) -> RemoteResult<()> {
        if self.session.is_some() {
            return Err(RemoteError::new(RemoteErrorType::AlreadyConnected));
        }
        let session = RusshSession::<T>::connect(&self.opts).await?;
        let sftp = session.sftp().await?;
        self.session = Some(session);
        self.sftp = Some(sftp);
        Ok(())
    }

    async fn disconnect(&mut self) -> RemoteResult<()> {
        let session = self
            .session
            .take()
            .ok_or_else(|| RemoteError::new(RemoteErrorType::NotConnected))?;
        self.sftp = None;
        session.disconnect().await
    }

    fn is_connected(&self) -> bool {
        self.session.as_ref().is_some_and(RusshSession::is_alive)
    }

    fn capabilities(&self) -> Capabilities {
        SFTP_CAPABILITIES
    }

    async fn list_dir(&self, path: &Path) -> RemoteResult<Vec<File>> {
        ensure_absolute(path)?;
        self.sftp()?.readdir(path).await
    }

    async fn stat(&self, path: &Path) -> RemoteResult<File> {
        ensure_absolute(path)?;
        self.sftp()?.stat(path).await
    }

    async fn exists(&self, path: &Path) -> RemoteResult<bool> {
        match self.stat(path).await {
            Ok(_) => Ok(true),
            Err(err) if err.kind() == RemoteErrorType::NoSuchFileOrDirectory => Ok(false),
            Err(err) => Err(err),
        }
    }

    async fn set_metadata(&self, path: &Path, metadata: &SetMetadata) -> RemoteResult<()> {
        ensure_absolute(path)?;
        self.sftp()?.set_metadata(path, metadata).await
    }

    async fn create_dir(&self, path: &Path, mode: Option<UnixPex>) -> RemoteResult<()> {
        ensure_absolute(path)?;
        if self.exists(path).await? {
            return Err(RemoteError::new(RemoteErrorType::AlreadyExists));
        }
        self.sftp()?
            .mkdir(path, Self::mode(mode, DEFAULT_DIR_MODE))
            .await
    }

    async fn remove_file(&self, path: &Path) -> RemoteResult<()> {
        ensure_absolute(path)?;
        self.sftp()?.unlink(path).await
    }

    async fn remove_dir(&self, path: &Path) -> RemoteResult<()> {
        ensure_absolute(path)?;
        self.sftp()?.rmdir(path).await
    }

    async fn rename(&self, src: &Path, dest: &Path) -> RemoteResult<()> {
        ensure_absolute(src)?;
        ensure_absolute(dest)?;
        self.sftp()?.rename(src, dest).await
    }

    async fn copy(&self, src: &Path, dest: &Path) -> RemoteResult<()> {
        ensure_absolute(src)?;
        ensure_absolute(dest)?;
        Self::copy_recursive(self.sftp()?, src, dest).await
    }

    async fn symlink(&self, path: &Path, target: &Path) -> RemoteResult<()> {
        ensure_absolute(path)?;
        ensure_absolute(target)?;
        if !self.exists(target).await? {
            return Err(RemoteError::new(RemoteErrorType::NoSuchFileOrDirectory));
        }
        self.sftp()?.symlink(target, path).await
    }

    async fn open(&self, path: &Path, opts: &ReadOptions) -> RemoteResult<AsyncReadStream> {
        ensure_absolute(path)?;
        self.sftp()?.open_read(path, opts).await
    }

    async fn create(&self, path: &Path, opts: &WriteOptions) -> RemoteResult<AsyncWriteStream> {
        ensure_absolute(path)?;
        self.sftp()?
            .open_write(
                path,
                WriteMode::Truncate,
                Self::mode(opts.mode, DEFAULT_FILE_MODE),
            )
            .await
    }

    async fn append(&self, path: &Path, opts: &WriteOptions) -> RemoteResult<AsyncWriteStream> {
        ensure_absolute(path)?;
        self.sftp()?
            .open_write(
                path,
                WriteMode::Append,
                Self::mode(opts.mode, DEFAULT_FILE_MODE),
            )
            .await
    }

    async fn exec(&self, cmd: &str) -> RemoteResult<ExecOutput> {
        let (exit_code, stdout) = self.session()?.cmd(cmd).await?;
        Ok(ExecOutput::new(exit_code, stdout))
    }
}
