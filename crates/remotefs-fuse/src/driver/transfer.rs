//! Transfer helpers shared by the Unix and Windows drivers.
//!
//! Everything here is generic over [`RemoteFs`] and platform-neutral, so both
//! drivers stage writes, read ranges, truncate and append the same way.

use std::io::{Cursor, Read as _, Seek as _, SeekFrom};
use std::path::Path;

use remotefs::fs::{Capabilities, ReadOptions, UnixPex, WriteOptions, WriteStream};
use remotefs::{File, RemoteError, RemoteErrorType, RemoteFs, RemoteResult};

/// A write staged on an open handle, not yet persisted to the remote.
///
/// Staging lets many kernel `write` calls against one handle share a single
/// remote write (one streaming upload, or one buffered `write_file`), instead
/// of each call re-creating (and truncating) the remote file.
pub enum PendingWriteState {
    /// The remote streams writes; the stream stays open across writes.
    /// `next_offset` is the stream cursor, used to seek only on a
    /// non-sequential write.
    Stream {
        stream: WriteStream,
        next_offset: u64,
    },
    /// The remote cannot stream writes (or requires a size up front, like
    /// SCP); data is staged in memory and uploaded once, on finalization.
    Buffered(Vec<u8>),
}

impl std::fmt::Debug for PendingWriteState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Stream { next_offset, .. } => f
                .debug_struct("Stream")
                .field("next_offset", next_offset)
                .finish_non_exhaustive(),
            Self::Buffered(buffer) => f.debug_tuple("Buffered").field(&buffer.len()).finish(),
        }
    }
}

/// Write options carrying the mode of `file`, when it has one.
pub(crate) fn write_options(file: &File) -> WriteOptions {
    match file.metadata().mode {
        Some(mode) => WriteOptions::default().mode(mode),
        None => WriteOptions::default(),
    }
}

/// Fill `buffer` from `reader` until it is full or the reader hits EOF.
///
/// A single `read` may return fewer bytes than requested; FUSE substitutes
/// zeroes for short reads, so keep reading.
fn read_fully(reader: &mut impl std::io::Read, buffer: &mut [u8]) -> std::io::Result<usize> {
    let mut filled = 0;
    while filled < buffer.len() {
        match reader.read(&mut buffer[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(err) => return Err(err),
        }
    }
    Ok(filled)
}

fn read_existing_file<T: RemoteFs + ?Sized>(remote: &T, file: &File) -> RemoteResult<Vec<u8>> {
    let size = file.metadata().size.unwrap_or(0);
    if size == 0 {
        return Ok(Vec::new());
    }

    let mut sink = Cursor::new(Vec::new());
    remote.read_file(file.path(), &ReadOptions::default().length(size), &mut sink)?;
    Ok(sink.into_inner())
}

/// Read up to `buffer.len()` bytes of `path` starting at `offset`.
///
/// Streams when the remote supports it; otherwise runs a one-shot ranged
/// `read_file` into memory, bounded by `buffer.len()` through
/// [`ReadOptions::length`]. Reading at or past EOF yields `Ok(0)`.
pub(crate) fn read_at<T: RemoteFs + ?Sized>(
    remote: &T,
    path: &Path,
    buffer: &mut [u8],
    offset: u64,
) -> RemoteResult<usize> {
    let opts = ReadOptions::default()
        .offset(offset)
        .length(buffer.len() as u64);
    if remote.capabilities().contains(Capabilities::STREAM_READ) {
        let mut stream = remote.open(path, &opts)?;
        let read = read_fully(&mut stream, buffer)?;
        stream.finish()?;
        Ok(read)
    } else {
        let mut sink = Cursor::new(Vec::with_capacity(buffer.len()));
        remote.read_file(path, &opts, &mut sink)?;
        let data = sink.into_inner();
        let read = data.len().min(buffer.len());
        buffer[..read].copy_from_slice(&data[..read]);
        Ok(read)
    }
}

/// Open a pending write for `file`, streaming if the remote allows it.
pub(crate) fn start_pending_write<T: RemoteFs + ?Sized>(
    remote: &T,
    file: &File,
) -> RemoteResult<PendingWriteState> {
    if !remote.capabilities().contains(Capabilities::STREAM_WRITE) {
        log::debug!("{:?}: remote cannot stream writes; buffering", file.path());
        return Ok(PendingWriteState::Buffered(read_existing_file(
            remote, file,
        )?));
    }
    match remote.create(file.path(), &write_options(file)) {
        Ok(stream) => Ok(PendingWriteState::Stream {
            stream,
            next_offset: 0,
        }),
        Err(err)
            if matches!(
                err.kind(),
                RemoteErrorType::SizeRequired | RemoteErrorType::UnsupportedFeature
            ) =>
        {
            log::debug!("{:?}: {err}; buffering", file.path());
            Ok(PendingWriteState::Buffered(read_existing_file(
                remote, file,
            )?))
        }
        Err(err) => Err(err),
    }
}

/// Write `data` at `offset` into a pending write.
pub(crate) fn write_to_pending(
    state: &mut PendingWriteState,
    data: &[u8],
    offset: u64,
) -> RemoteResult<u32> {
    match state {
        PendingWriteState::Stream {
            stream,
            next_offset,
        } => {
            if *next_offset != offset {
                if !stream.seekable() {
                    return Err(RemoteError::with_message(
                        RemoteErrorType::UnsupportedFeature,
                        format!(
                            "non-sequential write at {offset} (stream is at {next_offset}) on a non-seekable stream"
                        ),
                    ));
                }
                stream.seek(SeekFrom::Start(offset))?;
            }
            std::io::Write::write_all(stream, data)?;
            *next_offset = offset + data.len() as u64;
        }
        PendingWriteState::Buffered(buffer) => {
            let end = offset as usize + data.len();
            if buffer.len() < end {
                buffer.resize(end, 0);
            }
            buffer[offset as usize..end].copy_from_slice(data);
        }
    }
    Ok(data.len() as u32)
}

/// Persist a pending write: finish the stream, or upload the buffer.
pub(crate) fn finalize_pending_write<T: RemoteFs + ?Sized>(
    remote: &T,
    file: &File,
    state: PendingWriteState,
) -> RemoteResult<()> {
    match state {
        PendingWriteState::Stream { stream, .. } => stream.finish(),
        PendingWriteState::Buffered(buffer) => {
            log::debug!(
                "uploading {} buffered bytes to {:?}",
                buffer.len(),
                file.path()
            );
            let opts = write_options(file).size_hint(buffer.len() as u64);
            remote
                .write_file(file.path(), &opts, &mut Cursor::new(buffer))
                .map(|_| ())
        }
    }
}

/// Create (or truncate to empty) the file at `path`.
pub(crate) fn create_empty_file<T: RemoteFs + ?Sized>(
    remote: &T,
    path: &Path,
    mode: Option<UnixPex>,
) -> RemoteResult<()> {
    let mut opts = WriteOptions::default().size_hint(0);
    if let Some(mode) = mode {
        opts = opts.mode(mode);
    }
    remote
        .write_file(path, &opts, &mut Cursor::new(Vec::<u8>::new()))
        .map(|_| ())
}

/// Resize `file` to `size` bytes, keeping its prefix and zero-filling any
/// extension. The zero padding is streamed, never allocated.
pub(crate) fn truncate_file<T: RemoteFs + ?Sized>(
    remote: &T,
    file: &File,
    size: u64,
) -> RemoteResult<()> {
    let current = file.metadata().size.unwrap_or(0);
    if current == size {
        return Ok(());
    }
    let keep = size.min(current);
    let mut prefix = Cursor::new(Vec::with_capacity(keep as usize));
    if keep > 0 {
        remote.read_file(
            file.path(),
            &ReadOptions::default().length(keep),
            &mut prefix,
        )?;
    }
    let kept = prefix.get_ref().len() as u64;
    prefix.set_position(0);
    let mut source = prefix.chain(std::io::repeat(0).take(size.saturating_sub(kept)));
    let opts = write_options(file).size_hint(size);
    remote
        .write_file(file.path(), &opts, &mut source)
        .map(|_| ())
}

/// Append `data` to `file`, streaming when possible.
#[cfg_attr(unix, allow(dead_code))]
pub(crate) fn append_data<T: RemoteFs + ?Sized>(
    remote: &T,
    file: &File,
    data: &[u8],
) -> RemoteResult<u32> {
    let caps = remote.capabilities();
    let opts = write_options(file).size_hint(data.len() as u64);
    if caps.contains(Capabilities::STREAM_WRITE) && caps.contains(Capabilities::APPEND) {
        match remote.append(file.path(), &opts) {
            Ok(mut stream) => {
                std::io::Write::write_all(&mut stream, data)?;
                stream.finish()?;
                return Ok(data.len() as u32);
            }
            Err(err)
                if matches!(
                    err.kind(),
                    RemoteErrorType::SizeRequired | RemoteErrorType::UnsupportedFeature
                ) =>
            {
                log::debug!("{:?}: {err}; falling back to append_file", file.path());
            }
            Err(err) => return Err(err),
        }
    }
    remote
        .append_file(file.path(), &opts, &mut Cursor::new(data.to_vec()))
        .map(|written| written as u32)
}

#[cfg(feature = "tokio")]
pub(crate) mod r#async;

#[cfg(test)]
mod test {
    use std::path::PathBuf;

    use pretty_assertions::assert_eq;
    use remotefs::fs::{ExecOutput, Metadata, ReadStream, SetMetadata};
    use remotefs_memory::{Inode, MemoryFs, Node, Tree, node};

    use super::*;

    /// A [`MemoryFs`] that advertises no streaming, so the one-shot fallbacks run.
    pub(crate) struct NoStreamFs(pub(crate) MemoryFs);

    impl RemoteFs for NoStreamFs {
        fn connect(&mut self) -> RemoteResult<()> {
            self.0.connect()
        }

        fn disconnect(&mut self) -> RemoteResult<()> {
            self.0.disconnect()
        }

        fn is_connected(&self) -> bool {
            self.0.is_connected()
        }

        fn capabilities(&self) -> Capabilities {
            Capabilities::APPEND | Capabilities::RANGE_READ | Capabilities::SET_METADATA
        }

        fn list_dir(&self, path: &Path) -> RemoteResult<Vec<File>> {
            self.0.list_dir(path)
        }

        fn stat(&self, path: &Path) -> RemoteResult<File> {
            self.0.stat(path)
        }

        fn exists(&self, path: &Path) -> RemoteResult<bool> {
            self.0.exists(path)
        }

        fn set_metadata(&self, path: &Path, metadata: &SetMetadata) -> RemoteResult<()> {
            self.0.set_metadata(path, metadata)
        }

        fn create_dir(&self, path: &Path, mode: Option<UnixPex>) -> RemoteResult<()> {
            self.0.create_dir(path, mode)
        }

        fn remove_file(&self, path: &Path) -> RemoteResult<()> {
            self.0.remove_file(path)
        }

        fn remove_dir(&self, path: &Path) -> RemoteResult<()> {
            self.0.remove_dir(path)
        }

        fn rename(&self, src: &Path, dest: &Path) -> RemoteResult<()> {
            self.0.rename(src, dest)
        }

        fn copy(&self, src: &Path, dest: &Path) -> RemoteResult<()> {
            self.0.copy(src, dest)
        }

        fn symlink(&self, path: &Path, target: &Path) -> RemoteResult<()> {
            self.0.symlink(path, target)
        }

        fn open(&self, _path: &Path, _opts: &ReadOptions) -> RemoteResult<ReadStream> {
            Err(RemoteError::new(RemoteErrorType::UnsupportedFeature))
        }

        fn create(&self, _path: &Path, _opts: &WriteOptions) -> RemoteResult<WriteStream> {
            Err(RemoteError::new(RemoteErrorType::UnsupportedFeature))
        }

        fn append(&self, _path: &Path, _opts: &WriteOptions) -> RemoteResult<WriteStream> {
            Err(RemoteError::new(RemoteErrorType::UnsupportedFeature))
        }

        fn read_file(
            &self,
            path: &Path,
            opts: &ReadOptions,
            dest: &mut (dyn std::io::Write + Send),
        ) -> RemoteResult<u64> {
            self.0.read_file(path, opts, dest)
        }

        fn write_file(
            &self,
            path: &Path,
            opts: &WriteOptions,
            src: &mut (dyn std::io::Read + Send),
        ) -> RemoteResult<u64> {
            self.0.write_file(path, opts, src)
        }

        fn append_file(
            &self,
            path: &Path,
            opts: &WriteOptions,
            src: &mut (dyn std::io::Read + Send),
        ) -> RemoteResult<u64> {
            self.0.append_file(path, opts, src)
        }

        fn exec(&self, cmd: &str) -> RemoteResult<ExecOutput> {
            self.0.exec(cmd)
        }
    }

    pub(crate) fn memory_fs() -> MemoryFs {
        let tree = Tree::new(node!(
            PathBuf::from("/"),
            Inode::dir(0, 0, UnixPex::from(0o755)),
        ));
        let mut fs = MemoryFs::new(tree);
        fs.connect().expect("connect");
        fs
    }

    pub(crate) fn no_stream_fs() -> NoStreamFs {
        NoStreamFs(memory_fs())
    }

    fn put(remote: &impl RemoteFs, path: &Path, content: &[u8]) -> File {
        remote
            .write_file(
                path,
                &WriteOptions::default().size_hint(content.len() as u64),
                &mut Cursor::new(content.to_vec()),
            )
            .expect("write_file");
        remote.stat(path).expect("stat")
    }

    fn get(remote: &impl RemoteFs, path: &Path) -> Vec<u8> {
        let mut out = Cursor::new(Vec::new());
        remote
            .read_file(path, &ReadOptions::default(), &mut out)
            .expect("read_file");
        out.into_inner()
    }

    #[test]
    fn test_should_read_at_offset_with_stream() {
        let remote = memory_fs();
        put(&remote, Path::new("/a.txt"), b"hello world");
        let mut buffer = [0_u8; 5];
        let read = read_at(&remote, Path::new("/a.txt"), &mut buffer, 6).expect("read_at");
        assert_eq!(read, 5);
        assert_eq!(&buffer, b"world");
    }

    #[test]
    fn test_should_read_at_offset_without_stream() {
        let remote = no_stream_fs();
        put(&remote, Path::new("/a.txt"), b"hello world");
        let mut buffer = [0_u8; 5];
        let read = read_at(&remote, Path::new("/a.txt"), &mut buffer, 6).expect("read_at");
        assert_eq!(read, 5);
        assert_eq!(&buffer, b"world");
    }

    #[test]
    fn test_should_read_zero_bytes_past_end_of_file() {
        let remote = memory_fs();
        put(&remote, Path::new("/a.txt"), b"hello world");
        let mut buffer = [0_u8; 5];
        let read = read_at(&remote, Path::new("/a.txt"), &mut buffer, 1_000_000)
            .expect("read_at past EOF must not fail");
        assert_eq!(read, 0);
    }

    #[test]
    fn test_should_stage_and_finalize_stream_write() {
        let remote = memory_fs();
        let file = put(&remote, Path::new("/a.txt"), b"");
        let mut state = start_pending_write(&remote, &file).expect("start");
        assert!(matches!(state, PendingWriteState::Stream { .. }));
        write_to_pending(&mut state, b"hello ", 0).expect("chunk 1");
        write_to_pending(&mut state, b"world", 6).expect("chunk 2");
        finalize_pending_write(&remote, &file, state).expect("finish");
        assert_eq!(get(&remote, Path::new("/a.txt")), b"hello world");
    }

    #[test]
    fn test_should_buffer_write_when_stream_unsupported() {
        let remote = no_stream_fs();
        let file = put(&remote, Path::new("/a.txt"), b"");
        let mut state = start_pending_write(&remote, &file).expect("start");
        assert!(matches!(state, PendingWriteState::Buffered(_)));
        write_to_pending(&mut state, b"world", 6).expect("chunk");
        finalize_pending_write(&remote, &file, state).expect("finish");
        let data = get(&remote, Path::new("/a.txt"));
        assert_eq!(&data[..6], &[0_u8; 6]);
        assert_eq!(&data[6..], b"world");
    }

    #[test]
    fn test_should_preserve_existing_contents_for_buffered_partial_write() {
        let remote = no_stream_fs();
        let file = put(&remote, Path::new("/a.txt"), b"abcdef");
        let mut state = start_pending_write(&remote, &file).expect("start");

        write_to_pending(&mut state, b"X", 2).expect("chunk");
        finalize_pending_write(&remote, &file, state).expect("finish");

        assert_eq!(get(&remote, Path::new("/a.txt")), b"abXdef");
    }

    #[test]
    fn test_should_create_empty_file() {
        let remote = memory_fs();
        create_empty_file(&remote, Path::new("/empty"), Some(UnixPex::from(0o644)))
            .expect("create_empty_file");
        let file = remote.stat(Path::new("/empty")).expect("stat");
        assert_eq!(file.metadata().size, Some(0));
        assert_eq!(file.metadata().mode, Some(UnixPex::from(0o644)));
    }

    #[test]
    fn test_should_truncate_file_to_zero() {
        let remote = memory_fs();
        let file = put(&remote, Path::new("/a.txt"), b"hello world");
        truncate_file(&remote, &file, 0).expect("truncate");
        assert_eq!(get(&remote, Path::new("/a.txt")), b"");
    }

    #[test]
    fn test_should_shrink_and_extend_file() {
        let remote = memory_fs();
        let file = put(&remote, Path::new("/a.txt"), b"hello world");
        truncate_file(&remote, &file, 5).expect("shrink");
        assert_eq!(get(&remote, Path::new("/a.txt")), b"hello");
        let file = remote.stat(Path::new("/a.txt")).expect("stat");
        truncate_file(&remote, &file, 8).expect("extend");
        assert_eq!(get(&remote, Path::new("/a.txt")), b"hello\0\0\0");
    }

    #[test]
    fn test_should_append_with_and_without_stream() {
        let remote = memory_fs();
        let file = put(&remote, Path::new("/a.txt"), b"hello");
        assert_eq!(append_data(&remote, &file, b" world").expect("append"), 6);
        assert_eq!(get(&remote, Path::new("/a.txt")), b"hello world");

        let remote = no_stream_fs();
        let file = put(&remote, Path::new("/a.txt"), b"hello");
        assert_eq!(append_data(&remote, &file, b" world").expect("append"), 6);
        assert_eq!(get(&remote, Path::new("/a.txt")), b"hello world");
    }

    #[test]
    fn test_write_options_carry_mode() {
        let file = File::new(
            PathBuf::from("/a"),
            Metadata::default().mode(UnixPex::from(0o600)),
        );
        assert_eq!(write_options(&file).mode, Some(UnixPex::from(0o600)));
        assert_eq!(write_options(&file).size_hint, None);
    }
}
