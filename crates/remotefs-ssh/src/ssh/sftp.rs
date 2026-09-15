//! ## SFTP
//!
//! Blocking SFTP client implementing [`RemoteFs`].

use std::io::{self, Write as _};
use std::path::Path;

use remotefs::File;
use remotefs::fs::{
    Capabilities, ExecOutput, ReadOptions, ReadStream, RemoteError, RemoteErrorType, RemoteFs,
    RemoteResult, SetMetadata, UnixPex, WriteOptions, WriteStream,
};
use remotefs::path::ensure_absolute;

use super::SshOpts;
use crate::SshSession;
use crate::ssh::backend::{Sftp as _, WriteMode};

/// Mode applied to directories created without an explicit mode.
const DEFAULT_DIR_MODE: i32 = 0o755;
/// Mode applied to files created without an explicit mode.
const DEFAULT_FILE_MODE: i32 = 0o644;

/// Operations the SFTP client performs natively.
pub const SFTP_CAPABILITIES: Capabilities = Capabilities::STREAM_READ
    .union(Capabilities::STREAM_WRITE)
    .union(Capabilities::APPEND)
    .union(Capabilities::RANGE_READ)
    .union(Capabilities::SEEK_READ)
    .union(Capabilities::SEEK_WRITE)
    .union(Capabilities::COPY)
    .union(Capabilities::SYMLINK)
    .union(Capabilities::SET_METADATA)
    .union(Capabilities::POSIX_MODE)
    .union(Capabilities::EXEC);

/// Blocking SFTP filesystem client.
///
/// The client is generic over the SSH backend ([`crate::LibSsh2Session`] or
/// [`crate::LibSshSession`]). Every path must be absolute.
///
/// # Examples
///
/// ```rust,no_run
/// # #[cfg(feature = "libssh2")]
/// use std::path::Path;
///
/// # #[cfg(feature = "libssh2")]
/// use remotefs::RemoteFs;
/// # #[cfg(feature = "libssh2")]
/// use remotefs::fs::ReadOptions;
/// # #[cfg(feature = "libssh2")]
/// use remotefs_ssh::{SftpFs, SshOpts};
///
/// # #[cfg(feature = "libssh2")]
/// # fn main() -> remotefs::RemoteResult<()> {
/// let mut client = SftpFs::libssh2(SshOpts::new("127.0.0.1").username("user").password("pw"));
/// client.connect()?;
/// let mut output = Vec::new();
/// client.read_file(Path::new("/etc/hostname"), &ReadOptions::default(), &mut output)?;
/// client.disconnect()
/// # }
/// # #[cfg(not(feature = "libssh2"))]
/// # fn main() {}
/// ```
pub struct SftpFs<S>
where
    S: SshSession,
{
    session: Option<S>,
    sftp: Option<S::Sftp>,
    opts: SshOpts,
}

#[cfg(feature = "libssh2")]
#[cfg_attr(docsrs, doc(cfg(feature = "libssh2")))]
impl SftpFs<super::backend::LibSsh2Session> {
    /// Constructs a new [`SftpFs`] instance with the `libssh2` backend.
    pub fn libssh2(opts: SshOpts) -> Self {
        Self::new(opts)
    }
}

#[cfg(feature = "libssh")]
#[cfg_attr(docsrs, doc(cfg(feature = "libssh")))]
impl SftpFs<super::backend::LibSshSession> {
    /// Constructs a new [`SftpFs`] instance with the `libssh` backend.
    pub fn libssh(opts: SshOpts) -> Self {
        Self::new(opts)
    }
}

impl<S> SftpFs<S>
where
    S: SshSession,
{
    fn new(opts: SshOpts) -> Self {
        Self {
            session: None,
            sftp: None,
            opts,
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

    fn sftp(&self) -> RemoteResult<&S::Sftp> {
        self.sftp
            .as_ref()
            .ok_or_else(|| RemoteError::new(RemoteErrorType::NotConnected))
    }

    fn mode(mode: Option<UnixPex>, default: i32) -> i32 {
        mode.map(|mode| u32::from(mode) as i32).unwrap_or(default)
    }

    /// Recursively copies a file or directory using only SFTP operations.
    fn copy_recursive(sftp: &S::Sftp, src: &Path, dest: &Path) -> RemoteResult<()> {
        let src_file = sftp.stat(src)?;
        if src_file.is_dir() {
            sftp.mkdir(dest, Self::mode(src_file.metadata().mode, DEFAULT_DIR_MODE))?;
            for entry in sftp.readdir(src)? {
                let name = entry.path().file_name().ok_or_else(|| {
                    RemoteError::with_message(
                        RemoteErrorType::BadFile,
                        format!(
                            "entry has no file name: {path}",
                            path = entry.path().display()
                        ),
                    )
                })?;
                Self::copy_recursive(sftp, entry.path(), &dest.join(name))?;
            }
            return Ok(());
        }
        let mut reader = sftp.open_read(src, &ReadOptions::default())?;
        let mut writer = sftp.open_write(
            dest,
            WriteMode::Truncate,
            Self::mode(src_file.metadata().mode, DEFAULT_FILE_MODE),
        )?;
        let copied = io::copy(&mut reader, &mut writer)
            .and_then(|_| writer.flush())
            .map_err(RemoteError::from);
        let finished_read = reader.finish();
        let finished_write = writer.finish();
        copied.and(finished_read).and(finished_write)
    }
}

impl<S> RemoteFs for SftpFs<S>
where
    S: SshSession,
{
    fn connect(&mut self) -> RemoteResult<()> {
        if self.session.is_some() {
            return Err(RemoteError::new(RemoteErrorType::AlreadyConnected));
        }
        debug!("Initializing SFTP connection...");
        let session = S::connect(&self.opts)?;
        debug!("Getting SFTP client...");
        let sftp = session.sftp().map_err(|err| {
            error!("Could not get sftp client: {err}");
            err
        })?;
        info!("Connection established");
        self.session = Some(session);
        self.sftp = Some(sftp);
        Ok(())
    }

    fn disconnect(&mut self) -> RemoteResult<()> {
        debug!("Disconnecting from remote...");
        let session = self
            .session
            .take()
            .ok_or_else(|| RemoteError::new(RemoteErrorType::NotConnected))?;
        self.sftp = None;
        session.disconnect()
    }

    fn is_connected(&self) -> bool {
        self.session
            .as_ref()
            .is_some_and(|session| session.authenticated().unwrap_or(false))
    }

    fn capabilities(&self) -> Capabilities {
        SFTP_CAPABILITIES
    }

    fn list_dir(&self, path: &Path) -> RemoteResult<Vec<File>> {
        ensure_absolute(path)?;
        debug!("Reading directory content of {path}", path = path.display());
        self.sftp()?.readdir(path)
    }

    fn stat(&self, path: &Path) -> RemoteResult<File> {
        ensure_absolute(path)?;
        debug!("Collecting metadata for {path}", path = path.display());
        self.sftp()?.stat(path)
    }

    fn exists(&self, path: &Path) -> RemoteResult<bool> {
        match self.stat(path) {
            Ok(_) => Ok(true),
            Err(err) if err.kind() == RemoteErrorType::NoSuchFileOrDirectory => Ok(false),
            Err(err) => Err(err),
        }
    }

    fn set_metadata(&self, path: &Path, metadata: &SetMetadata) -> RemoteResult<()> {
        ensure_absolute(path)?;
        debug!("Setting metadata for {path}", path = path.display());
        self.sftp()?.set_metadata(path, metadata)
    }

    fn create_dir(&self, path: &Path, mode: Option<UnixPex>) -> RemoteResult<()> {
        ensure_absolute(path)?;
        let sftp = self.sftp()?;
        if self.exists(path)? {
            return Err(RemoteError::new(RemoteErrorType::AlreadyExists));
        }
        let mode = Self::mode(mode, DEFAULT_DIR_MODE);
        debug!(
            "Creating directory {path} (mode: {mode:o})",
            path = path.display()
        );
        sftp.mkdir(path, mode)
    }

    fn remove_file(&self, path: &Path) -> RemoteResult<()> {
        ensure_absolute(path)?;
        debug!("Removing file {path}", path = path.display());
        self.sftp()?.unlink(path)
    }

    fn remove_dir(&self, path: &Path) -> RemoteResult<()> {
        ensure_absolute(path)?;
        debug!("Removing directory {path}", path = path.display());
        self.sftp()?.rmdir(path)
    }

    fn rename(&self, src: &Path, dest: &Path) -> RemoteResult<()> {
        ensure_absolute(src)?;
        ensure_absolute(dest)?;
        debug!(
            "Renaming {src} to {dest}",
            src = src.display(),
            dest = dest.display()
        );
        self.sftp()?.rename(src, dest)
    }

    fn copy(&self, src: &Path, dest: &Path) -> RemoteResult<()> {
        ensure_absolute(src)?;
        ensure_absolute(dest)?;
        debug!(
            "Copying {src} to {dest}",
            src = src.display(),
            dest = dest.display()
        );
        Self::copy_recursive(self.sftp()?, src, dest)
    }

    fn symlink(&self, path: &Path, target: &Path) -> RemoteResult<()> {
        ensure_absolute(path)?;
        ensure_absolute(target)?;
        let sftp = self.sftp()?;
        debug!(
            "Creating symlink at {path} pointing to {target}",
            path = path.display(),
            target = target.display()
        );
        if !self.exists(target)? {
            return Err(RemoteError::new(RemoteErrorType::NoSuchFileOrDirectory));
        }
        sftp.symlink(target, path)
    }

    fn open(&self, path: &Path, opts: &ReadOptions) -> RemoteResult<ReadStream> {
        ensure_absolute(path)?;
        debug!("Opening file at {path} ({opts:?})", path = path.display());
        self.sftp()?.open_read(path, opts)
    }

    fn create(&self, path: &Path, opts: &WriteOptions) -> RemoteResult<WriteStream> {
        ensure_absolute(path)?;
        debug!("Creating file at {path}", path = path.display());
        self.sftp()?.open_write(
            path,
            WriteMode::Truncate,
            Self::mode(opts.mode, DEFAULT_FILE_MODE),
        )
    }

    fn append(&self, path: &Path, opts: &WriteOptions) -> RemoteResult<WriteStream> {
        ensure_absolute(path)?;
        debug!(
            "Opening file at {path} for appending",
            path = path.display()
        );
        self.sftp()?.open_write(
            path,
            WriteMode::Append,
            Self::mode(opts.mode, DEFAULT_FILE_MODE),
        )
    }

    fn exec(&self, cmd: &str) -> RemoteResult<ExecOutput> {
        debug!(r#"Executing command "{cmd}""#);
        let (exit_code, stdout) = self.session()?.cmd(cmd)?;
        Ok(ExecOutput::new(exit_code, stdout))
    }
}

#[cfg(test)]
mod tests;
