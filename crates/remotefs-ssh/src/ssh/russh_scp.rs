//! Asynchronous SCP client over russh.

use std::path::Path;
use std::sync::{Mutex, PoisonError};
use std::time::SystemTime;

use remotefs::fs::{
    AsyncReadStream, AsyncRemoteFs, AsyncWriteStream, Capabilities, ExecOutput, ReadOptions,
    SetMetadata, UnixPex, WriteOptions,
};
use remotefs::path::ensure_absolute;
use remotefs::{File, RemoteError, RemoteErrorType, RemoteResult};
use russh::client::Handler;

use super::SshOpts;
use super::backend::RusshSession;
use super::scp::{SCP_CAPABILITIES, shell};

/// Blocking adapter for [`RusshScpFs`] created with [`RusshScpFs::into_blocking`].
pub type BlockingRusshScpFs<T> = remotefs::adapters::blocking::BlockOn<RusshScpFs<T>>;

/// Native asynchronous SCP client backed by [`russh`].
///
/// Every path passed to this client must be absolute. SCP uploads require a
/// `WriteOptions::size_hint`; SCP streams are not seekable and do not append.
pub struct RusshScpFs<T>
where
    T: Handler + Default + Send + 'static,
{
    session: Option<RusshSession<T>>,
    opts: SshOpts,
    stat_flavor: Mutex<Option<shell::StatFlavor>>,
}

impl<T> RusshScpFs<T>
where
    T: Handler + Default + Send + 'static,
{
    /// Constructs a disconnected SCP client.
    pub fn new(opts: SshOpts) -> Self {
        Self {
            session: None,
            opts,
            stat_flavor: Mutex::new(None),
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
    pub fn into_blocking(self, handle: tokio::runtime::Handle) -> BlockingRusshScpFs<T> {
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

    async fn run(&self, cmd: &str) -> RemoteResult<(u32, String)> {
        self.session()?.cmd(cmd).await
    }

    async fn succeeds(&self, cmd: &str) -> RemoteResult<bool> {
        self.run(cmd).await.map(|(rc, _)| rc == 0)
    }

    async fn assert_ok(&self, cmd: &str, kind: RemoteErrorType) -> RemoteResult<()> {
        match self.run(cmd).await? {
            (0, _) => Ok(()),
            (rc, output) => Err(RemoteError::with_message(
                kind,
                format!("command exited with {rc}: {output}", output = output.trim()),
            )),
        }
    }

    async fn stat_flavor(&self) -> shell::StatFlavor {
        if let Some(flavor) = *self
            .stat_flavor
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
        {
            return flavor;
        }
        let gnu = self.succeeds(shell::STAT_PROBE_GNU).await.unwrap_or(false);
        let bsd = !gnu && self.succeeds(shell::STAT_PROBE_BSD).await.unwrap_or(false);
        let flavor = shell::stat_flavor(gnu, bsd);
        *self
            .stat_flavor
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(flavor);
        flavor
    }

    async fn mtime_epoch(&self, path: &Path) -> Option<SystemTime> {
        let cmd = shell::mtime_command(self.stat_flavor().await, path)?;
        match self.run(&cmd).await {
            Ok((0, output)) => shell::parse_mtime(&output),
            _ => None,
        }
    }

    async fn apply_mtimes(&self, dir: &Path, entries: &mut [File]) {
        let names: Vec<String> = entries.iter().map(File::name).collect();
        let Some(cmd) = shell::mtimes_command(self.stat_flavor().await, dir, &names) else {
            return;
        };
        let mtimes = match self.run(&cmd).await {
            Ok((_, output)) => shell::parse_mtimes(&output),
            Err(err) => {
                warn!("Batched stat failed, falling back to ls timestamps: {err}");
                return;
            }
        };
        for (entry, name) in entries.iter_mut().zip(names) {
            if let Some(mtime) = mtimes.get(&name) {
                entry.metadata.modified = Some(*mtime);
            }
        }
    }

    async fn require_exists(&self, path: &Path) -> RemoteResult<()> {
        if self.exists(path).await? {
            Ok(())
        } else {
            Err(RemoteError::new(RemoteErrorType::NoSuchFileOrDirectory))
        }
    }
}

#[remotefs::async_trait]
impl<T> AsyncRemoteFs for RusshScpFs<T>
where
    T: Handler + Default + Send + 'static,
{
    async fn connect(&mut self) -> RemoteResult<()> {
        if self.session.is_some() {
            return Err(RemoteError::new(RemoteErrorType::AlreadyConnected));
        }
        self.session = Some(RusshSession::<T>::connect(&self.opts).await?);
        Ok(())
    }

    async fn disconnect(&mut self) -> RemoteResult<()> {
        let session = self
            .session
            .take()
            .ok_or_else(|| RemoteError::new(RemoteErrorType::NotConnected))?;
        *self
            .stat_flavor
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = None;
        session.disconnect().await
    }

    fn is_connected(&self) -> bool {
        self.session.as_ref().is_some_and(RusshSession::is_alive)
    }

    fn capabilities(&self) -> Capabilities {
        SCP_CAPABILITIES
    }

    async fn list_dir(&self, path: &Path) -> RemoteResult<Vec<File>> {
        ensure_absolute(path)?;
        self.require_exists(path).await?;
        let (rc, output) = self.run(&shell::list_command(path)).await?;
        if rc != 0 {
            return Err(RemoteError::with_message(
                RemoteErrorType::ProtocolError,
                format!("Failed to list directory: {output}"),
            ));
        }
        let mut entries = shell::parse_listing(path, &output);
        self.apply_mtimes(path, &mut entries).await;
        Ok(entries)
    }

    async fn stat(&self, path: &Path) -> RemoteResult<File> {
        ensure_absolute(path)?;
        let is_dir = self.succeeds(&shell::is_dir_command(path)).await?;
        let (rc, line) = self.run(&shell::stat_command(path, is_dir)).await?;
        if rc != 0 {
            return Err(RemoteError::with_message(
                RemoteErrorType::NoSuchFileOrDirectory,
                format!("Failed to stat file: {line}", line = line.trim()),
            ));
        }
        let parent = path.parent().unwrap_or(path);
        let mut entry = shell::parse_ls_line(parent, line.trim())
            .ok_or_else(|| RemoteError::new(RemoteErrorType::NoSuchFileOrDirectory))?;
        if let Some(mtime) = self.mtime_epoch(path).await {
            entry.metadata.modified = Some(mtime);
        }
        Ok(entry)
    }

    async fn exists(&self, path: &Path) -> RemoteResult<bool> {
        ensure_absolute(path)?;
        self.succeeds(&shell::exists_command(path)).await
    }

    async fn set_metadata(&self, path: &Path, metadata: &SetMetadata) -> RemoteResult<()> {
        ensure_absolute(path)?;
        self.require_exists(path).await?;
        for cmd in shell::set_metadata_commands(path, metadata) {
            self.assert_ok(&cmd, RemoteErrorType::StatFailed).await?;
        }
        Ok(())
    }

    async fn create_dir(&self, path: &Path, mode: Option<UnixPex>) -> RemoteResult<()> {
        ensure_absolute(path)?;
        if self.exists(path).await? {
            return Err(RemoteError::new(RemoteErrorType::AlreadyExists));
        }
        self.assert_ok(
            &shell::mkdir_command(path, mode),
            RemoteErrorType::FileCreateDenied,
        )
        .await
    }

    async fn remove_file(&self, path: &Path) -> RemoteResult<()> {
        ensure_absolute(path)?;
        self.require_exists(path).await?;
        self.assert_ok(
            &shell::remove_file_command(path),
            RemoteErrorType::CouldNotRemoveFile,
        )
        .await
    }

    async fn remove_dir(&self, path: &Path) -> RemoteResult<()> {
        ensure_absolute(path)?;
        self.require_exists(path).await?;
        self.assert_ok(
            &shell::remove_dir_command(path),
            RemoteErrorType::DirectoryNotEmpty,
        )
        .await
    }

    async fn remove_dir_all(&self, path: &Path) -> RemoteResult<()> {
        ensure_absolute(path)?;
        self.require_exists(path).await?;
        self.assert_ok(
            &shell::remove_dir_all_command(path),
            RemoteErrorType::CouldNotRemoveFile,
        )
        .await
    }

    async fn rename(&self, src: &Path, dest: &Path) -> RemoteResult<()> {
        ensure_absolute(src)?;
        ensure_absolute(dest)?;
        self.assert_ok(
            &shell::rename_command(src, dest),
            RemoteErrorType::FileCreateDenied,
        )
        .await
    }

    async fn copy(&self, src: &Path, dest: &Path) -> RemoteResult<()> {
        ensure_absolute(src)?;
        ensure_absolute(dest)?;
        self.assert_ok(
            &shell::copy_command(src, dest),
            RemoteErrorType::FileCreateDenied,
        )
        .await
    }

    async fn symlink(&self, path: &Path, target: &Path) -> RemoteResult<()> {
        ensure_absolute(path)?;
        ensure_absolute(target)?;
        self.require_exists(target).await?;
        self.assert_ok(
            &shell::symlink_command(path, target),
            RemoteErrorType::FileCreateDenied,
        )
        .await
    }

    async fn open(&self, path: &Path, opts: &ReadOptions) -> RemoteResult<AsyncReadStream> {
        ensure_absolute(path)?;
        self.session()?.scp_recv(path, opts).await
    }

    async fn create(&self, path: &Path, opts: &WriteOptions) -> RemoteResult<AsyncWriteStream> {
        ensure_absolute(path)?;
        let size = opts
            .size_hint
            .ok_or_else(|| RemoteError::new(RemoteErrorType::SizeRequired))?;
        self.session()?
            .scp_send(
                path,
                opts.mode.map(u32::from).unwrap_or(shell::DEFAULT_FILE_MODE),
                size,
                opts.modified,
            )
            .await
    }

    async fn append(&self, path: &Path, _opts: &WriteOptions) -> RemoteResult<AsyncWriteStream> {
        ensure_absolute(path)?;
        Err(RemoteError::new(RemoteErrorType::UnsupportedFeature))
    }

    async fn exec(&self, cmd: &str) -> RemoteResult<ExecOutput> {
        let (exit_code, stdout) = self.run(cmd).await?;
        Ok(ExecOutput::new(exit_code, stdout))
    }
}
