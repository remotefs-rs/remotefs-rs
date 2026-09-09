//! The blocking [`RemoteFs`] contract every protocol client implements.

use std::io::{self, Read, Write};
use std::path::Path;

use super::{
    Capabilities, ExecOutput, File, ReadOptions, ReadStream, RemoteError, SetMetadata, UnixPex,
    Welcome, WriteOptions, WriteStream,
};
use crate::RemoteResult;

/// The blocking contract for a protocol-backed remote file system.
pub trait RemoteFs: Send + Sync {
    /// Connects to the remote server and authenticates the client.
    ///
    /// # Errors
    ///
    /// Returns a connection or authentication error when setup fails.
    fn connect(&mut self) -> RemoteResult<Welcome>;

    /// Disconnects from the remote server.
    ///
    /// # Errors
    ///
    /// Returns [`crate::fs::RemoteErrorType::NotConnected`] when no connection exists.
    fn disconnect(&mut self) -> RemoteResult<()>;

    /// Returns the cached connection state without probing the transport.
    fn is_connected(&self) -> bool;

    /// Returns the operations natively supported by this client.
    fn capabilities(&self) -> Capabilities;

    /// Lists the direct children of an absolute directory path.
    ///
    /// # Errors
    ///
    /// Returns [`crate::fs::RemoteErrorType::InvalidPath`] for relative paths and the
    /// backend's lookup error when the directory cannot be listed.
    fn list_dir(&self, path: &Path) -> RemoteResult<Vec<File>>;

    /// Returns metadata for an absolute path without following symlinks.
    ///
    /// # Errors
    ///
    /// Returns [`crate::fs::RemoteErrorType::InvalidPath`] for relative paths and the
    /// backend's lookup error when the entry cannot be inspected.
    fn stat(&self, path: &Path) -> RemoteResult<File>;

    /// Reports whether an absolute path exists.
    ///
    /// A missing entry is `Ok(false)`; invalid paths and transport failures are
    /// returned as errors.
    fn exists(&self, path: &Path) -> RemoteResult<bool>;

    /// Changes the specified metadata fields of an absolute path.
    ///
    /// # Errors
    ///
    /// Returns [`crate::fs::RemoteErrorType::UnsupportedFeature`] when metadata changes
    /// are unavailable.
    fn set_metadata(&self, path: &Path, metadata: &SetMetadata) -> RemoteResult<()>;

    /// Creates a directory at an absolute path.
    ///
    /// The optional mode is ignored when POSIX permissions are unavailable.
    fn create_dir(&self, path: &Path, mode: Option<UnixPex>) -> RemoteResult<()>;

    /// Removes a file or symbolic link at an absolute path.
    fn remove_file(&self, path: &Path) -> RemoteResult<()>;

    /// Removes an empty directory at an absolute path.
    fn remove_dir(&self, path: &Path) -> RemoteResult<()>;

    /// Removes an entry and its descendants using depth-first traversal.
    ///
    /// Symlinks are removed as entries and are never followed. A failure during
    /// traversal may leave earlier deletions applied remotely.
    fn remove_dir_all(&self, path: &Path) -> RemoteResult<()> {
        crate::path::ensure_absolute(path)?;
        let entry = self.stat(path)?;
        if entry.is_dir() {
            for child in self.list_dir(entry.path())? {
                self.remove_dir_all(child.path())?;
            }
            self.remove_dir(entry.path())
        } else {
            self.remove_file(entry.path())
        }
    }

    /// Renames an absolute source path to an absolute destination path.
    fn rename(&self, src: &Path, dest: &Path) -> RemoteResult<()>;

    /// Copies an absolute source path to an absolute destination path.
    fn copy(&self, src: &Path, dest: &Path) -> RemoteResult<()>;

    /// Creates a symbolic link at `path` pointing to an absolute `target`.
    fn symlink(&self, path: &Path, target: &Path) -> RemoteResult<()>;

    /// Opens an absolute file path for a ranged read.
    fn open(&self, path: &Path, opts: &ReadOptions) -> RemoteResult<ReadStream>;

    /// Creates or truncates an absolute file path for writing.
    fn create(&self, path: &Path, opts: &WriteOptions) -> RemoteResult<WriteStream>;

    /// Opens or creates an absolute file path for appending.
    fn append(&self, path: &Path, opts: &WriteOptions) -> RemoteResult<WriteStream>;

    /// Reads a remote file into a borrowed destination and finalizes its stream.
    ///
    /// # Errors
    ///
    /// Returns copy, flush, open, or finalization failures. If copying and
    /// finalization both fail, both causes are retained in the returned error.
    fn read_file(
        &self,
        path: &Path,
        opts: &ReadOptions,
        dest: &mut (dyn Write + Send),
    ) -> RemoteResult<u64> {
        crate::path::ensure_absolute(path)?;
        let mut stream = self.open(path, opts)?;
        let copied = io::copy(&mut stream, dest)
            .and_then(|count| dest.flush().map(|()| count))
            .map_err(RemoteError::from);
        let finished = stream.finish();
        super::stream::complete_transfer(copied, finished)
    }

    /// Writes a borrowed source to a remote file and finalizes its stream.
    ///
    /// Existing content is replaced.
    fn write_file(
        &self,
        path: &Path,
        opts: &WriteOptions,
        src: &mut (dyn Read + Send),
    ) -> RemoteResult<u64> {
        crate::path::ensure_absolute(path)?;
        let mut stream = self.create(path, opts)?;
        let copied = io::copy(src, &mut stream)
            .and_then(|count| stream.flush().map(|()| count))
            .map_err(RemoteError::from);
        let finished = stream.finish();
        super::stream::complete_transfer(copied, finished)
    }

    /// Appends a borrowed source to a remote file and finalizes its stream.
    fn append_file(
        &self,
        path: &Path,
        opts: &WriteOptions,
        src: &mut (dyn Read + Send),
    ) -> RemoteResult<u64> {
        crate::path::ensure_absolute(path)?;
        let mut stream = self.append(path, opts)?;
        let copied = io::copy(src, &mut stream)
            .and_then(|count| stream.flush().map(|()| count))
            .map_err(RemoteError::from);
        let finished = stream.finish();
        super::stream::complete_transfer(copied, finished)
    }

    /// Executes a command and returns its exit code and standard output.
    fn exec(&self, cmd: &str) -> RemoteResult<ExecOutput>;
}

#[cfg(test)]
mod test {
    use std::io::{Read, Write};

    use super::*;
    use crate::RemoteErrorType;
    use crate::mock::MockRemoteFs;

    #[test]
    fn should_be_able_to_create_trait_object() {
        let _: Box<dyn RemoteFs> = Box::new(MockRemoteFs::new());
    }

    #[test]
    fn write_file_finishes_before_returning() {
        let fs = MockRemoteFs::connected();
        let path = MockRemoteFs::path("upload");
        let mut input = io::Cursor::new(b"hello");
        let count = fs
            .write_file(&path, &WriteOptions::default(), &mut input)
            .unwrap();
        assert_eq!(count, 5);
        assert_eq!(fs.contents(&path), b"hello");
        assert_eq!(fs.finish_count(), 1);
        assert_eq!(fs.unfinished_count(), 0);
    }

    #[test]
    fn shared_and_boxed_clients_are_object_safe() {
        let _: Box<dyn RemoteFs> = Box::new(MockRemoteFs::new());
        let _: std::sync::Arc<dyn RemoteFs> = std::sync::Arc::new(MockRemoteFs::new());
        fn assert_client<T: RemoteFs>() {}
        assert_client::<Box<dyn RemoteFs>>();
    }

    #[test]
    fn create_truncates_and_append_preserves_existing_bytes() {
        let fs = MockRemoteFs::connected();
        let path = MockRemoteFs::path("file");
        fs.seed_file(&path, b"old");

        let mut writer = fs.create(&path, &WriteOptions::default()).unwrap();
        writer.write_all(b"new").unwrap();
        writer.finish().unwrap();
        assert_eq!(fs.contents(&path), b"new");

        let mut writer = fs.append(&path, &WriteOptions::default()).unwrap();
        writer.write_all(b"!").unwrap();
        writer.finish().unwrap();
        assert_eq!(fs.contents(&path), b"new!");
    }

    #[test]
    fn missing_parent_is_not_created_by_create() {
        let fs = MockRemoteFs::connected();
        let path = MockRemoteFs::path("missing/file");
        let error = fs.create(&path, &WriteOptions::default()).unwrap_err();
        assert_eq!(error.kind(), RemoteErrorType::NoSuchFileOrDirectory);
        assert!(!fs.exists(&path).unwrap());
    }

    #[test]
    fn required_size_hint_fails_before_opening_a_stream() {
        let fs = MockRemoteFs::connected();
        fs.require_size_hint(true);
        let path = MockRemoteFs::path("sized");
        let error = fs.create(&path, &WriteOptions::default()).unwrap_err();
        assert_eq!(error.kind(), RemoteErrorType::SizeRequired);
        assert_eq!(fs.finish_count(), 0);
        assert_eq!(fs.unfinished_count(), 0);
        assert!(!fs.exists(&path).unwrap());
    }

    #[test]
    fn range_options_handle_zero_and_beyond_eof() {
        let fs = MockRemoteFs::connected();
        let path = MockRemoteFs::path("range");
        fs.seed_file(&path, b"abcdef");

        let mut output = Vec::new();
        fs.read_file(
            &path,
            &ReadOptions::default().offset(2).length(0),
            &mut output,
        )
        .unwrap();
        assert!(output.is_empty());

        let mut output = Vec::new();
        fs.read_file(&path, &ReadOptions::default().offset(100), &mut output)
            .unwrap();
        assert!(output.is_empty());

        let mut output = Vec::new();
        fs.read_file(
            &path,
            &ReadOptions::default().offset(2).length(2),
            &mut output,
        )
        .unwrap();
        assert_eq!(output, b"cd");
        assert_eq!(fs.finish_count(), 3);
    }

    #[test]
    fn failing_finish_is_returned_and_does_not_commit_staged_data() {
        let fs = MockRemoteFs::connected();
        fs.fail_finish(Some(RemoteErrorType::ProtocolError));
        let path = MockRemoteFs::path("failed");
        let mut input = io::Cursor::new(b"data");
        let error = fs
            .write_file(&path, &WriteOptions::default(), &mut input)
            .unwrap_err();
        assert_eq!(error.kind(), RemoteErrorType::ProtocolError);
        assert_eq!(fs.finish_count(), 1);
        assert_eq!(fs.unfinished_count(), 0);
        assert!(!fs.exists(&path).unwrap());
    }

    #[test]
    fn recursive_removal_is_depth_first_and_does_not_follow_links() {
        let fs = MockRemoteFs::connected();
        let root = MockRemoteFs::path("tree");
        let nested = root.join("nested");
        let file = nested.join("file");
        let cycle = nested.join("cycle");
        fs.seed_dir(&root);
        fs.seed_dir(&nested);
        fs.seed_file(&file, b"data");
        fs.seed_symlink(&cycle, &root);
        fs.remove_dir_all(&root).unwrap();
        assert!(!fs.exists(&root).unwrap());
        assert!(!fs.exists(&file).unwrap());
    }

    #[test]
    fn paths_must_be_absolute_and_connection_is_required() {
        let fs = MockRemoteFs::new();
        assert_eq!(
            fs.stat(Path::new("relative")).unwrap_err().kind(),
            RemoteErrorType::InvalidPath
        );
        let path = MockRemoteFs::path("file");
        assert_eq!(
            fs.stat(&path).unwrap_err().kind(),
            RemoteErrorType::NotConnected
        );
        let mut fs = fs;
        fs.connect().unwrap();
        assert_eq!(
            fs.connect().unwrap_err().kind(),
            RemoteErrorType::AlreadyConnected
        );
        fs.disconnect().unwrap();
        assert!(!fs.is_connected());
    }

    #[test]
    fn capabilities_control_stream_and_seek_independently() {
        let fs = MockRemoteFs::connected();
        let path = MockRemoteFs::path("capabilities");
        fs.seed_file(&path, b"abcdef");
        fs.set_capabilities(Capabilities::empty());
        assert_eq!(
            fs.open(&path, &ReadOptions::default()).unwrap_err().kind(),
            RemoteErrorType::UnsupportedFeature
        );

        fs.set_capabilities(Capabilities::STREAM_READ | Capabilities::RANGE_READ);
        let stream = fs.open(&path, &ReadOptions::default().offset(2)).unwrap();
        assert!(!stream.seekable());
        let mut output = Vec::new();
        let mut stream = stream;
        stream.read_to_end(&mut output).unwrap();
        stream.finish().unwrap();
        assert_eq!(output, b"cdef");
    }
}
