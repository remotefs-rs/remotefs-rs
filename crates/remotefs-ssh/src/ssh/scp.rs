//! ## SCP
//!
//! Blocking SCP client implementing [`RemoteFs`].
//!
//! Filesystem operations run POSIX shell commands over the SSH session and
//! transfers use the SCP protocol. SCP has no append and no native ranges.

use std::path::Path;
use std::sync::{Mutex, PoisonError};
use std::time::SystemTime;

use remotefs::File;
use remotefs::fs::{
    Capabilities, ExecOutput, ReadOptions, ReadStream, RemoteError, RemoteErrorType, RemoteFs,
    RemoteResult, SetMetadata, UnixPex, WriteOptions, WriteStream,
};
use remotefs::path::ensure_absolute;

use self::shell::StatFlavor;
use super::SshOpts;
use crate::SshSession;

pub(crate) mod shell;

/// Operations the SCP client performs natively.
pub const SCP_CAPABILITIES: Capabilities = Capabilities::STREAM_READ
    .union(Capabilities::STREAM_WRITE)
    .union(Capabilities::COPY)
    .union(Capabilities::SYMLINK)
    .union(Capabilities::SET_METADATA)
    .union(Capabilities::POSIX_MODE)
    .union(Capabilities::EXEC);

/// Blocking SCP filesystem client.
///
/// Every path must be absolute. `create` requires
/// [`WriteOptions::size_hint`] because SCP announces the file size up front;
/// `append` is unsupported.
///
/// # Examples
///
/// ```rust,no_run
/// # #[cfg(feature = "libssh2")]
/// use std::io::Cursor;
/// # #[cfg(feature = "libssh2")]
/// use std::path::Path;
///
/// # #[cfg(feature = "libssh2")]
/// use remotefs::RemoteFs;
/// # #[cfg(feature = "libssh2")]
/// use remotefs::fs::WriteOptions;
/// # #[cfg(feature = "libssh2")]
/// use remotefs_ssh::{ScpFs, SshOpts};
///
/// # #[cfg(feature = "libssh2")]
/// # fn main() -> remotefs::RemoteResult<()> {
/// let mut client = ScpFs::libssh2(SshOpts::new("127.0.0.1").username("user").password("pw"));
/// client.connect()?;
/// let data = b"hello";
/// client.write_file(
///     Path::new("/tmp/hello.txt"),
///     &WriteOptions::default().size_hint(data.len() as u64),
///     &mut Cursor::new(data),
/// )?;
/// client.disconnect()
/// # }
/// # #[cfg(not(feature = "libssh2"))]
/// # fn main() {}
/// ```
pub struct ScpFs<S>
where
    S: SshSession,
{
    session: Option<S>,
    opts: SshOpts,
    /// Cached `stat(1)` flavor for the remote host; probed lazily on first use.
    stat_flavor: Mutex<Option<StatFlavor>>,
}

#[cfg(feature = "libssh2")]
#[cfg_attr(docsrs, doc(cfg(feature = "libssh2")))]
impl ScpFs<super::backend::LibSsh2Session> {
    /// Constructs a new [`ScpFs`] instance with the `libssh2` backend.
    pub fn libssh2(opts: SshOpts) -> Self {
        Self::new(opts)
    }
}

#[cfg(feature = "libssh")]
#[cfg_attr(docsrs, doc(cfg(feature = "libssh")))]
impl ScpFs<super::backend::LibSshSession> {
    /// Constructs a new [`ScpFs`] instance with the `libssh` backend.
    pub fn libssh(opts: SshOpts) -> Self {
        Self::new(opts)
    }
}

impl<S> ScpFs<S>
where
    S: SshSession,
{
    fn new(opts: SshOpts) -> Self {
        Self {
            session: None,
            opts,
            stat_flavor: Mutex::new(None),
        }
    }

    /// Returns the ports assigned to configured TCP `RemoteForward` listeners.
    ///
    /// Empty when disconnected.
    pub fn remote_forward_ports(&self) -> Vec<u16> {
        self.session
            .as_ref()
            .map(|session| session.remote_forward_ports().to_vec())
            .unwrap_or_default()
    }

    /// Returns the server banner, when the backend exposes one.
    ///
    /// # Errors
    ///
    /// Returns [`RemoteErrorType::NotConnected`] when disconnected.
    pub fn banner(&self) -> RemoteResult<Option<String>> {
        self.session()?.banner()
    }

    fn session(&self) -> RemoteResult<&S> {
        self.session
            .as_ref()
            .ok_or_else(|| RemoteError::new(RemoteErrorType::NotConnected))
    }

    /// Runs `cmd` and returns `(exit_code, stdout)`, mapping transport errors to `ProtocolError`.
    fn run(&self, cmd: &str) -> RemoteResult<(u32, String)> {
        self.session()?.cmd(cmd)
    }

    /// Runs `cmd` and returns whether it exited with `0`.
    fn succeeds(&self, cmd: &str) -> RemoteResult<bool> {
        self.run(cmd).map(|(rc, _)| rc == 0)
    }

    /// Runs `cmd` and maps a non-zero exit code to `kind`.
    fn assert_ok(&self, cmd: &str, kind: RemoteErrorType) -> RemoteResult<()> {
        match self.run(cmd)? {
            (0, _) => Ok(()),
            (rc, output) => Err(RemoteError::with_message(
                kind,
                format!("command exited with {rc}: {output}", output = output.trim()),
            )),
        }
    }

    fn stat_flavor(&self) -> StatFlavor {
        let mut cached = self
            .stat_flavor
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(flavor) = *cached {
            return flavor;
        }
        let gnu = self.succeeds(shell::STAT_PROBE_GNU).unwrap_or(false);
        let bsd = !gnu && self.succeeds(shell::STAT_PROBE_BSD).unwrap_or(false);
        let flavor = shell::stat_flavor(gnu, bsd);
        trace!("Detected remote stat flavor: {flavor:?}");
        *cached = Some(flavor);
        flavor
    }

    fn mtime_epoch(&self, path: &Path) -> Option<SystemTime> {
        let cmd = shell::mtime_command(self.stat_flavor(), path)?;
        match self.run(&cmd) {
            Ok((0, output)) => shell::parse_mtime(&output),
            _ => None,
        }
    }

    fn apply_mtimes(&self, dir: &Path, entries: &mut [File]) {
        let names: Vec<String> = entries.iter().map(File::name).collect();
        let Some(cmd) = shell::mtimes_command(self.stat_flavor(), dir, &names) else {
            return;
        };
        let mtimes = match self.run(&cmd) {
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

    fn require_exists(&self, path: &Path) -> RemoteResult<()> {
        if self.exists(path)? {
            Ok(())
        } else {
            Err(RemoteError::new(RemoteErrorType::NoSuchFileOrDirectory))
        }
    }
}

impl<S> RemoteFs for ScpFs<S>
where
    S: SshSession,
{
    fn connect(&mut self) -> RemoteResult<()> {
        if self.session.is_some() {
            return Err(RemoteError::new(RemoteErrorType::AlreadyConnected));
        }
        debug!("Initializing SCP connection...");
        let session = S::connect(&self.opts)?;
        info!("Connection established");
        self.session = Some(session);
        Ok(())
    }

    fn disconnect(&mut self) -> RemoteResult<()> {
        debug!("Disconnecting from remote...");
        let session = self
            .session
            .take()
            .ok_or_else(|| RemoteError::new(RemoteErrorType::NotConnected))?;
        *self
            .stat_flavor
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = None;
        session.disconnect()
    }

    fn is_connected(&self) -> bool {
        self.session
            .as_ref()
            .is_some_and(|session| session.authenticated().unwrap_or(false))
    }

    fn capabilities(&self) -> Capabilities {
        SCP_CAPABILITIES
    }

    fn list_dir(&self, path: &Path) -> RemoteResult<Vec<File>> {
        ensure_absolute(path)?;
        self.require_exists(path)?;
        debug!("Getting file entries in {path}", path = path.display());
        let (rc, output) = self.run(&shell::list_command(path))?;
        if rc != 0 {
            return Err(RemoteError::with_message(
                RemoteErrorType::ProtocolError,
                format!("Failed to list directory: {output}"),
            ));
        }
        let mut entries = shell::parse_listing(path, &output);
        self.apply_mtimes(path, &mut entries);
        debug!("Found {count} valid file entries", count = entries.len());
        Ok(entries)
    }

    fn stat(&self, path: &Path) -> RemoteResult<File> {
        ensure_absolute(path)?;
        debug!("Stat {path}", path = path.display());
        let is_dir = self.succeeds(&shell::is_dir_command(path))?;
        let (rc, line) = self.run(&shell::stat_command(path, is_dir))?;
        if rc != 0 {
            return Err(RemoteError::with_message(
                RemoteErrorType::NoSuchFileOrDirectory,
                format!("Failed to stat file: {line}", line = line.trim()),
            ));
        }
        let parent = path.parent().unwrap_or(path);
        let mut entry = shell::parse_ls_line(parent, line.trim())
            .ok_or_else(|| RemoteError::new(RemoteErrorType::NoSuchFileOrDirectory))?;
        if let Some(mtime) = self.mtime_epoch(path) {
            entry.metadata.modified = Some(mtime);
        }
        Ok(entry)
    }

    fn exists(&self, path: &Path) -> RemoteResult<bool> {
        ensure_absolute(path)?;
        self.succeeds(&shell::exists_command(path))
    }

    fn set_metadata(&self, path: &Path, metadata: &SetMetadata) -> RemoteResult<()> {
        ensure_absolute(path)?;
        self.require_exists(path)?;
        debug!("Setting attributes for {path}", path = path.display());
        for cmd in shell::set_metadata_commands(path, metadata) {
            self.assert_ok(&cmd, RemoteErrorType::StatFailed)?;
        }
        Ok(())
    }

    fn create_dir(&self, path: &Path, mode: Option<UnixPex>) -> RemoteResult<()> {
        ensure_absolute(path)?;
        if self.exists(path)? {
            return Err(RemoteError::new(RemoteErrorType::AlreadyExists));
        }
        debug!("Creating directory at {path}", path = path.display());
        self.assert_ok(
            &shell::mkdir_command(path, mode),
            RemoteErrorType::FileCreateDenied,
        )
    }

    fn remove_file(&self, path: &Path) -> RemoteResult<()> {
        ensure_absolute(path)?;
        self.require_exists(path)?;
        debug!("Removing file {path}", path = path.display());
        self.assert_ok(
            &shell::remove_file_command(path),
            RemoteErrorType::CouldNotRemoveFile,
        )
    }

    fn remove_dir(&self, path: &Path) -> RemoteResult<()> {
        ensure_absolute(path)?;
        self.require_exists(path)?;
        debug!("Removing directory {path}", path = path.display());
        self.assert_ok(
            &shell::remove_dir_command(path),
            RemoteErrorType::DirectoryNotEmpty,
        )
    }

    fn remove_dir_all(&self, path: &Path) -> RemoteResult<()> {
        ensure_absolute(path)?;
        self.require_exists(path)?;
        debug!(
            "Removing directory {path} recursively",
            path = path.display()
        );
        self.assert_ok(
            &shell::remove_dir_all_command(path),
            RemoteErrorType::CouldNotRemoveFile,
        )
    }

    fn rename(&self, src: &Path, dest: &Path) -> RemoteResult<()> {
        ensure_absolute(src)?;
        ensure_absolute(dest)?;
        self.require_exists(src)?;
        debug!(
            "Renaming {src} to {dest}",
            src = src.display(),
            dest = dest.display()
        );
        self.assert_ok(
            &shell::rename_command(src, dest),
            RemoteErrorType::FileCreateDenied,
        )
    }

    fn copy(&self, src: &Path, dest: &Path) -> RemoteResult<()> {
        ensure_absolute(src)?;
        ensure_absolute(dest)?;
        self.require_exists(src)?;
        debug!(
            "Copying {src} to {dest}",
            src = src.display(),
            dest = dest.display()
        );
        self.assert_ok(
            &shell::copy_command(src, dest),
            RemoteErrorType::FileCreateDenied,
        )
    }

    fn symlink(&self, path: &Path, target: &Path) -> RemoteResult<()> {
        ensure_absolute(path)?;
        ensure_absolute(target)?;
        self.require_exists(target)?;
        if self.exists(path)? {
            return Err(RemoteError::new(RemoteErrorType::AlreadyExists));
        }
        debug!(
            "Creating a symlink at {path} pointing at {target}",
            path = path.display(),
            target = target.display()
        );
        self.assert_ok(
            &shell::symlink_command(path, target),
            RemoteErrorType::FileCreateDenied,
        )
    }

    fn open(&self, path: &Path, opts: &ReadOptions) -> RemoteResult<ReadStream> {
        ensure_absolute(path)?;
        self.require_exists(path)?;
        debug!(
            "Opening file {path} for read ({opts:?})",
            path = path.display()
        );
        self.session()?.scp_recv(path, opts)
    }

    fn create(&self, path: &Path, opts: &WriteOptions) -> RemoteResult<WriteStream> {
        ensure_absolute(path)?;
        let size = opts
            .size_hint
            .ok_or_else(|| RemoteError::new(RemoteErrorType::SizeRequired))?;
        let mode = opts.mode.map(u32::from).unwrap_or(shell::DEFAULT_FILE_MODE) as i32;
        let epoch = |time: Option<SystemTime>| {
            time.and_then(|time| time.duration_since(SystemTime::UNIX_EPOCH).ok())
                .map(|duration| duration.as_secs())
        };
        let modified = epoch(opts.modified);
        let times = modified.map(|modified| (modified, modified));
        debug!(
            "Creating file {path} (mode {mode:o}, size {size}, mtime {modified:?})",
            path = path.display()
        );
        self.session()?.scp_send(path, mode, size, times)
    }

    fn append(&self, path: &Path, _opts: &WriteOptions) -> RemoteResult<WriteStream> {
        ensure_absolute(path)?;
        Err(RemoteError::new(RemoteErrorType::UnsupportedFeature))
    }

    fn exec(&self, cmd: &str) -> RemoteResult<ExecOutput> {
        debug!(r#"Executing command "{cmd}""#);
        let (exit_code, stdout) = self.run(cmd)?;
        Ok(ExecOutput::new(exit_code, stdout))
    }
}

#[cfg(test)]
mod tests;
