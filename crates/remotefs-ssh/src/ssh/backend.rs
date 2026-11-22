//! Defines the main trait for SSH Backends to be used with the clients and the backend implementations
//! to support different SSH libraries (e.g. libssh2, libssh)

#[cfg(any(feature = "libssh", feature = "libssh2"))]
mod forward;
#[cfg(any(feature = "libssh", feature = "libssh2", feature = "russh"))]
mod interface;
#[cfg(feature = "libssh2")]
mod keepalive;
#[cfg(any(feature = "libssh", feature = "libssh2", feature = "russh"))]
mod socket;

#[cfg(feature = "libssh")]
#[cfg_attr(docsrs, doc(cfg(feature = "libssh")))]
mod libssh;

#[cfg(feature = "libssh2")]
#[cfg_attr(docsrs, doc(cfg(feature = "libssh2")))]
mod libssh2;
#[cfg(feature = "russh")]
#[cfg_attr(docsrs, doc(cfg(feature = "russh")))]
mod russh;

use std::path::Path;

use remotefs::fs::{ReadOptions, ReadStream, SetMetadata, WriteStream};
use remotefs::{File, RemoteResult};

#[cfg(feature = "libssh")]
#[cfg_attr(docsrs, doc(cfg(feature = "libssh")))]
pub use self::libssh::LibSshSession;
#[cfg(feature = "libssh2")]
#[cfg_attr(docsrs, doc(cfg(feature = "libssh2")))]
pub use self::libssh2::LibSsh2Session;
#[cfg(feature = "russh")]
pub(crate) use self::russh::RusshSftp;
#[cfg(feature = "russh")]
#[cfg_attr(docsrs, doc(cfg(feature = "russh")))]
pub use self::russh::{NoCheckServerKey, RusshSession};
use crate::SshOpts;

#[cfg(any(feature = "libssh", feature = "libssh2", feature = "russh"))]
const MAX_FORWARD_CONNECTIONS: usize = 64;

/// SSH session trait for the blocking backends.
///
/// Every method takes `&self`: a session is shared by the client and by the
/// streams it hands out, so backends guard their protocol handle with a
/// `Mutex` where the underlying library is not `Sync`.
///
/// # Examples
///
/// ```rust,no_run
/// # #[cfg(feature = "libssh2")]
/// use remotefs_ssh::{LibSsh2Session, SshOpts, SshSession};
///
/// # #[cfg(feature = "libssh2")]
/// # fn main() -> remotefs::RemoteResult<()> {
/// let session = LibSsh2Session::connect(&SshOpts::new("127.0.0.1").username("user").password("pw"))?;
/// let (exit_code, stdout) = session.cmd("echo hello")?;
/// assert_eq!(exit_code, 0);
/// assert_eq!(stdout.trim(), "hello");
/// session.disconnect()
/// # }
/// # #[cfg(not(feature = "libssh2"))]
/// # fn main() {}
/// ```
pub trait SshSession: Send + Sync + Sized {
    type Sftp: Sftp;

    /// Connects to the SSH server and establishes a new [`SshSession`]
    fn connect(opts: &SshOpts) -> RemoteResult<Self>;

    /// Disconnect from the server
    fn disconnect(&self) -> RemoteResult<()>;

    /// Return the assigned ports for configured TCP `RemoteForward` listeners.
    ///
    /// Ports are returned in configuration order. Unix socket listeners are
    /// omitted. This reports the server-assigned port when a listener requested
    /// port `0`.
    fn remote_forward_ports(&self) -> &[u16] {
        &[]
    }

    /// Returns the SSH server banner, when the backend exposes one.
    fn banner(&self) -> RemoteResult<Option<String>>;

    /// Returns whether the session is authenticated and alive.
    fn authenticated(&self) -> RemoteResult<bool>;

    /// Executes a command on the server and returns its exit code and standard output.
    fn cmd<S>(&self, cmd: S) -> RemoteResult<(u32, String)>
    where
        S: AsRef<str>;

    /// Receives a file over SCP as an owned stream honoring `opts`.
    fn scp_recv(&self, path: &Path, opts: &ReadOptions) -> RemoteResult<ReadStream>;

    /// Sends a file of exactly `size` bytes over SCP.
    fn scp_send(
        &self,
        remote_path: &Path,
        mode: i32,
        size: u64,
        times: Option<(u64, u64)>,
    ) -> RemoteResult<WriteStream>;

    /// Returns a SFTP client
    fn sftp(&self) -> RemoteResult<Self::Sftp>;
}

/// SFTP provider for a [`SshSession`] implementation via the [`SshSession::sftp`] method.
pub trait Sftp: Send + Sync {
    /// Creates a new directory at `path` with the given `mode`.
    fn mkdir(&self, path: &Path, mode: i32) -> RemoteResult<()>;

    /// Opens `path` for reading, honoring `opts.offset` and `opts.length`.
    fn open_read(&self, path: &Path, opts: &ReadOptions) -> RemoteResult<ReadStream>;

    /// Open a file for write at the specified `path` with the given `flags`. If the file is created, set the mode.
    fn open_write(&self, path: &Path, flags: WriteMode, mode: i32) -> RemoteResult<WriteStream>;

    /// Lists the entries of `dirname` (without `.` and `..`).
    fn readdir(&self, dirname: &Path) -> RemoteResult<Vec<File>>;

    /// Renames a file from `src` to `dest`.
    fn rename(&self, src: &Path, dest: &Path) -> RemoteResult<()>;

    /// Removes a directory at `path`.
    fn rmdir(&self, path: &Path) -> RemoteResult<()>;

    /// Applies the requested metadata changes to `path`.
    fn set_metadata(&self, path: &Path, metadata: &SetMetadata) -> RemoteResult<()>;

    /// Returns the entry at `path` without following a final symlink.
    fn stat(&self, path: &Path) -> RemoteResult<File>;

    /// Creates a symlink at `path` pointing to `target`.
    fn symlink(&self, path: &Path, target: &Path) -> RemoteResult<()>;

    /// Deletes the file or symlink at `path`.
    fn unlink(&self, path: &Path) -> RemoteResult<()>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Open modes for reading and writing files.
pub enum WriteMode {
    /// Open or create and position at the end.
    Append,
    /// Create or truncate.
    Truncate,
}
