//! The blocking [`RemoteFs`] trait every protocol client implements.

use std::io;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use log::{debug, trace};

use super::{
    File, Metadata, ReadStream, RemoteError, RemoteErrorType, UnixPex, Welcome, WriteStream,
};
use crate::RemoteResult;

/// The blocking contract a protocol client implements to expose a remote host.
///
/// A client implements the required methods — the ones only the protocol can
/// answer — and inherits the rest. The defaulted methods
/// ([`RemoteFs::remove_dir_all`], [`RemoteFs::append_file`],
/// [`RemoteFs::create_file`], [`RemoteFs::open_file`], [`RemoteFs::on_written`],
/// [`RemoteFs::on_read`], and, behind the `find` feature, `find` and
/// `iter_search`) are written on top of the required ones and are worth
/// overriding only when the protocol offers a faster path.
///
/// # Not every protocol can do everything
///
/// The trait is the union of what the protocols can do, not the intersection.
/// A method a protocol has no equivalent for returns
/// [`RemoteErrorType::UnsupportedFeature`] — S3 cannot [`RemoteFs::exec`], SCP
/// cannot [`RemoteFs::setstat`]. Treat that error as an answer, not a bug, and
/// fall back accordingly: when [`RemoteFs::create`] is unsupported, for instance,
/// [`RemoteFs::create_file`] transfers the whole file in one call instead.
///
/// # Connection state
///
/// Every method other than [`RemoteFs::connect`] and [`RemoteFs::is_connected`]
/// requires an established connection, and returns
/// [`RemoteErrorType::NotConnected`] without one. Connecting twice returns
/// [`RemoteErrorType::AlreadyConnected`].
///
/// # Object safety
///
/// The trait is object-safe on purpose: consumers keep clients as
/// `Box<dyn RemoteFs>` to choose a protocol at runtime. Do not add generic
/// methods to it.
///
/// # Examples
///
/// ```
/// use std::io::Cursor;
///
/// use remotefs::fs::Metadata;
/// use remotefs::{RemoteFs, RemoteResult};
///
/// /// Upload `content` to `path`, whatever the protocol underneath is.
/// fn upload<T>(client: &mut T, path: &str, content: Vec<u8>) -> RemoteResult<u64>
/// where
///     T: RemoteFs,
/// {
///     // SCP needs the size up front, so always set it.
///     let metadata = Metadata::default().size(content.len() as u64);
///
///     client.create_file(path.as_ref(), &metadata, Box::new(Cursor::new(content)))
/// }
/// ```
pub trait RemoteFs {
    /// Connect to the remote server and authenticate.
    ///
    /// On success returns the server's greeting, which carries a banner for the
    /// protocols that send one.
    ///
    /// # Errors
    ///
    /// Returns [`RemoteErrorType::AlreadyConnected`] when a connection is already
    /// established, [`RemoteErrorType::AuthenticationFailed`] when the server
    /// rejects the credentials, and [`RemoteErrorType::ConnectionError`] or
    /// [`RemoteErrorType::BadAddress`] when the transport cannot be set up.
    fn connect(&mut self) -> RemoteResult<Welcome>;

    /// Close the connection to the remote server.
    ///
    /// # Errors
    ///
    /// Returns [`RemoteErrorType::NotConnected`] when there is nothing to close,
    /// or [`RemoteErrorType::ProtocolError`] when the server refuses the
    /// shutdown.
    fn disconnect(&mut self) -> RemoteResult<()>;

    /// Return whether the client currently holds a connection.
    ///
    /// The receiver is `&mut self` because some protocols have to probe the
    /// transport to answer.
    fn is_connected(&mut self) -> bool;

    /// Return the current working directory on the remote host.
    ///
    /// # Errors
    ///
    /// Returns [`RemoteErrorType::NotConnected`] when not connected, or
    /// [`RemoteErrorType::ProtocolError`] when the server answers unexpectedly.
    fn pwd(&mut self) -> RemoteResult<PathBuf>;

    /// Change the working directory, returning the real path of the new one.
    ///
    /// # Errors
    ///
    /// Returns [`RemoteErrorType::NoSuchFileOrDirectory`] when `dir` does not
    /// exist, [`RemoteErrorType::PermissionDenied`] when it may not be entered, and
    /// [`RemoteErrorType::NotConnected`] when not connected.
    fn change_dir(&mut self, dir: &Path) -> RemoteResult<PathBuf>;

    /// List the entries of the directory at `path`.
    ///
    /// The listing is not recursive and the order is whatever the server sent.
    ///
    /// # Errors
    ///
    /// Returns [`RemoteErrorType::NoSuchFileOrDirectory`] when `path` does not
    /// exist, [`RemoteErrorType::PermissionDenied`] when it may not be read, and
    /// [`RemoteErrorType::NotConnected`] when not connected.
    fn list_dir(&mut self, path: &Path) -> RemoteResult<Vec<File>>;

    /// Return the entry at `path` with its metadata.
    ///
    /// # Errors
    ///
    /// Returns [`RemoteErrorType::NoSuchFileOrDirectory`] when `path` does not
    /// exist, [`RemoteErrorType::StatFailed`] when the server has the entry but
    /// will not describe it, and [`RemoteErrorType::NotConnected`] when not
    /// connected.
    fn stat(&mut self, path: &Path) -> RemoteResult<File>;

    /// Apply `metadata` to the entry at `path`.
    ///
    /// A client applies only the fields the protocol supports and the caller set;
    /// an empty [`Option`] means "leave it alone".
    ///
    /// # Errors
    ///
    /// Returns [`RemoteErrorType::UnsupportedFeature`] on a protocol with no
    /// notion of settable metadata, [`RemoteErrorType::NoSuchFileOrDirectory`]
    /// when `path` does not exist, [`RemoteErrorType::PermissionDenied`] when the change
    /// is not permitted, and [`RemoteErrorType::NotConnected`] when not
    /// connected.
    fn setstat(&mut self, path: &Path, metadata: Metadata) -> RemoteResult<()>;

    /// Return whether an entry exists at `path`.
    ///
    /// # Errors
    ///
    /// Returns [`RemoteErrorType::NotConnected`] when not connected, or
    /// [`RemoteErrorType::ProtocolError`] when the server answers in a way that
    /// is neither "yes" nor "no". A missing entry is `Ok(false)`, not an error.
    fn exists(&mut self, path: &Path) -> RemoteResult<bool>;

    /// Remove the file at `path`.
    ///
    /// # Errors
    ///
    /// Returns [`RemoteErrorType::NoSuchFileOrDirectory`] when `path` does not
    /// exist, [`RemoteErrorType::BadFile`] when it is not a file,
    /// [`RemoteErrorType::CouldNotRemoveFile`] when the server refuses, and
    /// [`RemoteErrorType::NotConnected`] when not connected.
    fn remove_file(&mut self, path: &Path) -> RemoteResult<()>;

    /// Remove the directory at `path`, which must be empty.
    ///
    /// Use [`RemoteFs::remove_dir_all`] to remove a directory with contents.
    ///
    /// # Errors
    ///
    /// Returns [`RemoteErrorType::DirectoryNotEmpty`] when the directory still
    /// has entries, [`RemoteErrorType::NoSuchFileOrDirectory`] when `path` does
    /// not exist, [`RemoteErrorType::PermissionDenied`] when the removal is not
    /// permitted, and [`RemoteErrorType::NotConnected`] when not connected.
    fn remove_dir(&mut self, path: &Path) -> RemoteResult<()>;

    /// Remove the entry at `path` and everything below it. **Use carefully!**
    ///
    /// A [`crate::fs::FileType::File`] at `path` is removed as well, since a
    /// directory is a file too. Symbolic links are removed, never followed.
    ///
    /// # Default implementation
    ///
    /// Walks the tree with [`RemoteFs::list_dir`] and removes what it finds with
    /// [`RemoteFs::remove_dir`] and [`RemoteFs::remove_file`], depth first.
    /// Override it when the protocol can delete a tree in one request.
    ///
    /// # Errors
    ///
    /// Returns [`RemoteErrorType::NotConnected`] when not connected, and
    /// whatever the underlying [`RemoteFs::stat`], [`RemoteFs::list_dir`],
    /// [`RemoteFs::remove_dir`] or [`RemoteFs::remove_file`] call reported. A
    /// failure partway through leaves the already-removed entries removed.
    fn remove_dir_all(&mut self, path: &Path) -> RemoteResult<()> {
        if self.is_connected() {
            let path = crate::utils::path::absolutize(&self.pwd()?, path);
            debug!("Removing {path}...", path = path.display());
            let entry = self.stat(path.as_path())?;
            if entry.is_dir() {
                // list dir
                debug!(
                    "{name} is a directory; removing all directory entries",
                    name = entry.name()
                );
                let directory_content = self.list_dir(entry.path())?;
                for entry in directory_content.iter() {
                    self.remove_dir_all(entry.path())?;
                }
                trace!(
                    "Removed all files in {path}; removing directory",
                    path = entry.path().display()
                );
                self.remove_dir(entry.path())
            } else {
                self.remove_file(entry.path())
            }
        } else {
            Err(RemoteError::new(RemoteErrorType::NotConnected))
        }
    }

    /// Create a directory at `path` with the given permissions.
    ///
    /// `mode` is ignored by protocols with no notion of POSIX permissions.
    ///
    /// # Errors
    ///
    /// Returns [`RemoteErrorType::AlreadyExists`] when something is
    /// already there, [`RemoteErrorType::PermissionDenied`] when the parent may not be
    /// written, and [`RemoteErrorType::NotConnected`] when not connected.
    fn create_dir(&mut self, path: &Path, mode: UnixPex) -> RemoteResult<()>;

    /// Create a symbolic link at `path` pointing at `target`.
    ///
    /// The link is created whether or not `target` exists.
    ///
    /// # Errors
    ///
    /// Returns [`RemoteErrorType::UnsupportedFeature`] on a protocol with no
    /// links, [`RemoteErrorType::FileCreateDenied`] when the server refuses, and
    /// [`RemoteErrorType::NotConnected`] when not connected.
    fn symlink(&mut self, path: &Path, target: &Path) -> RemoteResult<()>;

    /// Copy the entry at `src` to `dest` on the remote host.
    ///
    /// The copy happens server-side; nothing travels through the client.
    ///
    /// # Errors
    ///
    /// Returns [`RemoteErrorType::UnsupportedFeature`] on a protocol that cannot
    /// copy server-side, [`RemoteErrorType::NoSuchFileOrDirectory`] when `src`
    /// does not exist, [`RemoteErrorType::PermissionDenied`] when `dest` may not be
    /// written, and [`RemoteErrorType::NotConnected`] when not connected.
    fn copy(&mut self, src: &Path, dest: &Path) -> RemoteResult<()>;

    /// Move the entry at `src` to `dest` on the remote host.
    ///
    /// # Errors
    ///
    /// Returns [`RemoteErrorType::NoSuchFileOrDirectory`] when `src` does not
    /// exist, [`RemoteErrorType::PermissionDenied`] when `dest` may not be written, and
    /// [`RemoteErrorType::NotConnected`] when not connected.
    fn mov(&mut self, src: &Path, dest: &Path) -> RemoteResult<()>;

    /// Run `cmd` on the remote host, returning its exit code and stdout.
    ///
    /// # Errors
    ///
    /// Returns [`RemoteErrorType::UnsupportedFeature`] on a protocol with no
    /// shell, [`RemoteErrorType::PermissionDenied`] when execution is not permitted, and
    /// [`RemoteErrorType::NotConnected`] when not connected. A command that runs
    /// and fails is `Ok` with a non-zero exit code, not an error.
    fn exec(&mut self, cmd: &str) -> RemoteResult<(u32, String)>;

    /// Open the file at `path` for appending, creating it when absent.
    ///
    /// Hand the returned stream back to [`RemoteFs::on_written`] once the last
    /// byte is written; dropping it is not enough on every protocol.
    ///
    /// # ⚠️ Warning
    ///
    /// `metadata` should describe the local file being sent. Protocols such as
    /// SCP take [`Metadata::size`] as the transfer size and will truncate or hang
    /// if it is wrong.
    ///
    /// # Errors
    ///
    /// Returns [`RemoteErrorType::UnsupportedFeature`] on a protocol without
    /// streamed writes — use [`RemoteFs::append_file`] then —
    /// [`RemoteErrorType::CouldNotOpenFile`] when the server refuses, and
    /// [`RemoteErrorType::NotConnected`] when not connected.
    fn append(&mut self, path: &Path, metadata: &Metadata) -> RemoteResult<WriteStream>;

    /// Open the file at `path` for writing, truncating any existing content.
    ///
    /// Hand the returned stream back to [`RemoteFs::on_written`] once the last
    /// byte is written; dropping it is not enough on every protocol.
    ///
    /// # ⚠️ Warning
    ///
    /// `metadata` should describe the local file being sent. Protocols such as
    /// SCP take [`Metadata::size`] as the transfer size and will truncate or hang
    /// if it is wrong.
    ///
    /// # Errors
    ///
    /// Returns [`RemoteErrorType::UnsupportedFeature`] on a protocol without
    /// streamed writes — use [`RemoteFs::create_file`] then —
    /// [`RemoteErrorType::FileCreateDenied`] when the server refuses, and
    /// [`RemoteErrorType::NotConnected`] when not connected.
    fn create(&mut self, path: &Path, metadata: &Metadata) -> RemoteResult<WriteStream>;

    /// Open the file at `path` for reading.
    ///
    /// Hand the returned stream back to [`RemoteFs::on_read`] once the last byte
    /// is read; dropping it is not enough on every protocol.
    ///
    /// # Errors
    ///
    /// Returns [`RemoteErrorType::UnsupportedFeature`] on a protocol without
    /// streamed reads — use [`RemoteFs::open_file`] then —
    /// [`RemoteErrorType::NoSuchFileOrDirectory`] when `path` does not exist,
    /// [`RemoteErrorType::CouldNotOpenFile`] when the server refuses, and
    /// [`RemoteErrorType::NotConnected`] when not connected.
    fn open(&mut self, path: &Path) -> RemoteResult<ReadStream>;

    /// Finalize a write started by [`RemoteFs::create`] or [`RemoteFs::append`].
    ///
    /// Call this every time a streamed write ends. FTP, for one, only counts the
    /// transfer once the data connection is closed and the final reply read.
    ///
    /// # Default implementation
    ///
    /// Returns [`Ok`]. Override it only when the protocol has something to do.
    ///
    /// # Errors
    ///
    /// Returns [`RemoteErrorType::ProtocolError`] when the server rejects the
    /// transfer at the very end, so a write is not durable until this returns
    /// [`Ok`].
    fn on_written(&mut self, _writable: WriteStream) -> RemoteResult<()> {
        Ok(())
    }

    /// Finalize a read started by [`RemoteFs::open`].
    ///
    /// Call this every time a streamed read ends, for the same reason as
    /// [`RemoteFs::on_written`].
    ///
    /// # Default implementation
    ///
    /// Returns [`Ok`]. Override it only when the protocol has something to do.
    ///
    /// # Errors
    ///
    /// Returns [`RemoteErrorType::ProtocolError`] when the server reports the
    /// transfer was incomplete.
    fn on_read(&mut self, _readable: ReadStream) -> RemoteResult<()> {
        Ok(())
    }

    /// Append everything `reader` yields to `path`, returning the bytes written.
    ///
    /// This is the one-shot form of [`RemoteFs::append`]: reach for it when
    /// [`RemoteFs::append`] answered [`RemoteErrorType::UnsupportedFeature`], or
    /// when you do not want to drive the stream yourself.
    ///
    /// # Default implementation
    ///
    /// Opens the file with [`RemoteFs::append`], copies the reader into it, and
    /// finalizes with [`RemoteFs::on_written`]. Override it on a protocol with no
    /// streamed writes at all.
    ///
    /// # Errors
    ///
    /// Returns [`RemoteErrorType::NotConnected`] when not connected,
    /// [`RemoteErrorType::ProtocolError`] when the copy fails, and whatever
    /// [`RemoteFs::append`] reported.
    fn append_file(
        &mut self,
        path: &Path,
        metadata: &Metadata,
        mut reader: Box<dyn Read + Send>,
    ) -> RemoteResult<u64> {
        if self.is_connected() {
            trace!("Opened remote file");
            let mut stream = self.append(path, metadata)?;
            let sz = io::copy(&mut reader, &mut stream)
                .map_err(|e| RemoteError::with_source(RemoteErrorType::ProtocolError, e))?;
            self.on_written(stream)?;
            trace!("Written {sz} bytes to destination");
            Ok(sz)
        } else {
            Err(RemoteError::new(RemoteErrorType::NotConnected))
        }
    }

    /// Write everything `reader` yields to `path`, returning the bytes written.
    ///
    /// This is the one-shot form of [`RemoteFs::create`]: reach for it when
    /// [`RemoteFs::create`] answered [`RemoteErrorType::UnsupportedFeature`], or
    /// when you do not want to drive the stream yourself. Any existing content at
    /// `path` is replaced.
    ///
    /// # Default implementation
    ///
    /// Opens the file with [`RemoteFs::create`], copies the reader into it, and
    /// finalizes with [`RemoteFs::on_written`]. Override it on a protocol with no
    /// streamed writes at all.
    ///
    /// # Errors
    ///
    /// Returns [`RemoteErrorType::NotConnected`] when not connected,
    /// [`RemoteErrorType::ProtocolError`] when the copy fails, and whatever
    /// [`RemoteFs::create`] reported.
    fn create_file(
        &mut self,
        path: &Path,
        metadata: &Metadata,
        mut reader: Box<dyn Read + Send>,
    ) -> RemoteResult<u64> {
        if self.is_connected() {
            let mut stream = self.create(path, metadata)?;
            trace!("Opened remote file");
            let sz = io::copy(&mut reader, &mut stream)
                .map_err(|e| RemoteError::with_source(RemoteErrorType::ProtocolError, e))?;
            self.on_written(stream)?;
            trace!("Written {sz} bytes to destination");
            Ok(sz)
        } else {
            Err(RemoteError::new(RemoteErrorType::NotConnected))
        }
    }

    /// Read `src` into `dest`, returning the bytes written.
    ///
    /// This is the one-shot form of [`RemoteFs::open`]: reach for it when
    /// [`RemoteFs::open`] answered [`RemoteErrorType::UnsupportedFeature`], or
    /// when you do not want to drive the stream yourself.
    ///
    /// # Default implementation
    ///
    /// Opens the file with [`RemoteFs::open`], copies it into `dest`, and
    /// finalizes with [`RemoteFs::on_read`]. Override it on a protocol with no
    /// streamed reads at all.
    ///
    /// # Errors
    ///
    /// Returns [`RemoteErrorType::NotConnected`] when not connected,
    /// [`RemoteErrorType::ProtocolError`] when the copy fails, and whatever
    /// [`RemoteFs::open`] reported.
    fn open_file(&mut self, src: &Path, mut dest: Box<dyn Write + Send>) -> RemoteResult<u64> {
        if self.is_connected() {
            let mut stream = self.open(src)?;
            trace!("File opened");
            let sz = io::copy(&mut stream, &mut dest)
                .map_err(|e| RemoteError::with_source(RemoteErrorType::ProtocolError, e))?;
            self.on_read(stream)?;
            trace!("Copied {sz} bytes to destination");
            Ok(sz)
        } else {
            Err(RemoteError::new(RemoteErrorType::NotConnected))
        }
    }

    /// Search the working directory and below for names matching `search`.
    ///
    /// `search` is a wildcard pattern where `?` matches one character and `*`
    /// matches any run of them. A matching directory is returned *and* descended
    /// into.
    ///
    /// # Default implementation
    ///
    /// Walks the tree with [`RemoteFs::list_dir`] through
    /// [`RemoteFs::iter_search`]. On a large tree this is one request per
    /// directory; override it when the protocol can search server-side.
    ///
    /// # Errors
    ///
    /// Returns [`RemoteErrorType::NotConnected`] when not connected, and
    /// whatever [`RemoteFs::pwd`] or [`RemoteFs::list_dir`] reported.
    #[cfg(feature = "find")]
    fn find(&mut self, search: &str) -> RemoteResult<Vec<File>> {
        use wildmatch::WildMatch;

        match self.is_connected() {
            true => {
                // Starting from current directory, iter dir
                match self.pwd() {
                    Ok(p) => self.iter_search(p.as_path(), &WildMatch::new(search)),
                    Err(err) => Err(err),
                }
            }
            false => Err(RemoteError::new(RemoteErrorType::NotConnected)),
        }
    }

    /// Recursively collect the entries under `dir` matching `filter`.
    ///
    /// # ⚠️ Warning
    ///
    /// This is the engine behind [`RemoteFs::find`] and is not meant to be called
    /// from outside; treat it as private. Do not re-implement it unless the
    /// protocol offers a faster way to walk a tree.
    ///
    /// # Errors
    ///
    /// Returns whatever [`RemoteFs::list_dir`] reported for `dir` or for any
    /// directory below it.
    #[cfg(feature = "find")]
    fn iter_search(
        &mut self,
        dir: &Path,
        filter: &wildmatch::WildMatch,
    ) -> RemoteResult<Vec<File>> {
        let mut drained: Vec<File> = Vec::new();
        // Scan directory
        match self.list_dir(dir) {
            Ok(entries) => {
                /* For each entry:
                - if is dir: call iter_search with `dir`
                    - push `iter_search` result to `drained`
                - if is file: check if it matches `filter`
                    - if it matches `filter`: push to to filter
                */
                for entry in entries.into_iter() {
                    if entry.is_dir() {
                        // If directory name, matches wildcard, push it to drained
                        if filter.matches(entry.name().as_str()) {
                            drained.push(entry.clone());
                        }
                        drained.append(&mut self.iter_search(entry.path(), filter)?);
                    } else if filter.matches(entry.name().as_str()) {
                        drained.push(entry);
                    }
                }
                Ok(drained)
            }
            Err(err) => Err(err),
        }
    }
}

#[cfg(test)]
mod test {

    use super::*;
    use crate::mock::MockRemoteFs;

    #[test]
    fn should_be_able_to_create_trait_object() {
        let _: Box<dyn RemoteFs> = Box::new(MockRemoteFs {});
    }
}
