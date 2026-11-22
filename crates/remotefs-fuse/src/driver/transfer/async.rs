//! Asynchronous transfer helpers for [`AsyncRemoteFs`] clients.

use std::io::SeekFrom;
use std::path::Path;

use futures_util::io::{AsyncReadExt as _, AsyncSeekExt as _, AsyncWriteExt as _, Cursor};
use remotefs::fs::{
    AsyncReadStream, AsyncWriteStream, Capabilities, ReadOptions, UnixPex, WriteOptions,
};
use remotefs::{AsyncRemoteFs, File, RemoteError, RemoteErrorType, RemoteResult};

use super::write_options;

/// A write staged on an open handle against an async remote.
pub(crate) enum AsyncPendingWriteState {
    /// The remote streams writes; the stream stays open across writes.
    Stream {
        stream: AsyncWriteStream,
        next_offset: u64,
    },
    /// The remote cannot stream writes; data is staged and uploaded on finalization.
    Buffered(Vec<u8>),
}

impl std::fmt::Debug for AsyncPendingWriteState {
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

/// Fill `buffer` until full or EOF, retrying interrupted reads.
async fn read_fully(
    reader: &mut (impl futures_util::AsyncRead + Unpin),
    buffer: &mut [u8],
) -> std::io::Result<usize> {
    let mut filled = 0;
    while filled < buffer.len() {
        match reader.read(&mut buffer[filled..]).await {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(err) => return Err(err),
        }
    }
    Ok(filled)
}

fn complete_transfer<T>(operation: RemoteResult<T>, finished: RemoteResult<()>) -> RemoteResult<T> {
    match (operation, finished) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
        (Err(operation), Err(finish)) => Err(RemoteError::with_message(
            operation.kind(),
            format!("{operation}; transfer finalization also failed: {finish}"),
        )),
    }
}

async fn read_stream(mut stream: AsyncReadStream, buffer: &mut [u8]) -> RemoteResult<usize> {
    let read = read_fully(&mut stream, buffer)
        .await
        .map_err(RemoteError::from);
    let finished = stream.finish().await;
    complete_transfer(read, finished)
}

#[cfg(any(windows, test))]
async fn write_stream(mut stream: AsyncWriteStream, data: &[u8]) -> RemoteResult<u32> {
    let written = match stream.write_all(data).await {
        Ok(()) => stream
            .flush()
            .await
            .map(|()| data.len() as u32)
            .map_err(RemoteError::from),
        Err(error) => Err(RemoteError::from(error)),
    };
    let finished = stream.finish().await;
    complete_transfer(written, finished)
}

async fn read_existing_file<T: AsyncRemoteFs + ?Sized>(
    remote: &T,
    file: &File,
) -> RemoteResult<Vec<u8>> {
    let size = file.metadata().size.unwrap_or(0);
    if size == 0 {
        return Ok(Vec::new());
    }

    let mut sink = Cursor::new(Vec::new());
    remote
        .read_file(file.path(), &ReadOptions::default().length(size), &mut sink)
        .await?;
    Ok(sink.into_inner())
}

/// Read up to `buffer.len()` bytes of `path` at `offset`; `Ok(0)` at or past EOF.
pub(crate) async fn read_at<T: AsyncRemoteFs + ?Sized>(
    remote: &T,
    path: &Path,
    buffer: &mut [u8],
    offset: u64,
) -> RemoteResult<usize> {
    let opts = ReadOptions::default()
        .offset(offset)
        .length(buffer.len() as u64);
    if remote.capabilities().contains(Capabilities::STREAM_READ) {
        let stream = remote.open(path, &opts).await?;
        read_stream(stream, buffer).await
    } else {
        let mut sink = Cursor::new(Vec::with_capacity(buffer.len()));
        remote.read_file(path, &opts, &mut sink).await?;
        let data = sink.into_inner();
        let read = data.len().min(buffer.len());
        buffer[..read].copy_from_slice(&data[..read]);
        Ok(read)
    }
}

/// Open a pending write for `file`, streaming if the remote allows it.
pub(crate) async fn start_pending_write<T: AsyncRemoteFs + ?Sized>(
    remote: &T,
    file: &File,
) -> RemoteResult<AsyncPendingWriteState> {
    if !remote.capabilities().contains(Capabilities::STREAM_WRITE) {
        log::debug!("{:?}: remote cannot stream writes; buffering", file.path());
        return Ok(AsyncPendingWriteState::Buffered(
            read_existing_file(remote, file).await?,
        ));
    }
    match remote.create(file.path(), &write_options(file)).await {
        Ok(stream) => Ok(AsyncPendingWriteState::Stream {
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
            Ok(AsyncPendingWriteState::Buffered(
                read_existing_file(remote, file).await?,
            ))
        }
        Err(err) => Err(err),
    }
}

/// Write `data` at `offset` into a pending write.
pub(crate) async fn write_to_pending(
    state: &mut AsyncPendingWriteState,
    data: &[u8],
    offset: u64,
) -> RemoteResult<u32> {
    match state {
        AsyncPendingWriteState::Stream {
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
                stream.seek(SeekFrom::Start(offset)).await?;
            }
            stream.write_all(data).await?;
            *next_offset = offset + data.len() as u64;
        }
        AsyncPendingWriteState::Buffered(buffer) => {
            let end = offset as usize + data.len();
            if buffer.len() < end {
                buffer.resize(end, 0);
            }
            buffer[offset as usize..end].copy_from_slice(data);
        }
    }
    Ok(data.len() as u32)
}

/// Persist a pending write: flush and finish the stream, or upload the buffer.
pub(crate) async fn finalize_pending_write<T: AsyncRemoteFs + ?Sized>(
    remote: &T,
    file: &File,
    state: AsyncPendingWriteState,
) -> RemoteResult<()> {
    match state {
        AsyncPendingWriteState::Stream { mut stream, .. } => {
            let flushed = stream.flush().await.map_err(RemoteError::from);
            let finished = stream.finish().await;
            complete_transfer(flushed, finished)
        }
        AsyncPendingWriteState::Buffered(buffer) => {
            log::debug!(
                "uploading {} buffered bytes to {:?}",
                buffer.len(),
                file.path()
            );
            let opts = write_options(file).size_hint(buffer.len() as u64);
            remote
                .write_file(file.path(), &opts, &mut Cursor::new(buffer))
                .await
                .map(|_| ())
        }
    }
}

/// Create (or truncate to empty) the file at `path`.
pub(crate) async fn create_empty_file<T: AsyncRemoteFs + ?Sized>(
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
        .await
        .map(|_| ())
}

/// Resize `file` to `size` bytes; the zero padding is streamed, never allocated.
pub(crate) async fn truncate_file<T: AsyncRemoteFs + ?Sized>(
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
        remote
            .read_file(
                file.path(),
                &ReadOptions::default().length(keep),
                &mut prefix,
            )
            .await?;
    }
    let kept = prefix.get_ref().len() as u64;
    prefix.set_position(0);
    let mut source = prefix.chain(futures_util::io::repeat(0).take(size.saturating_sub(kept)));
    let opts = write_options(file).size_hint(size);
    remote
        .write_file(file.path(), &opts, &mut source)
        .await
        .map(|_| ())
}

/// Append `data` to `file`, streaming when possible.
#[cfg(any(windows, test))]
pub(crate) async fn append_data<T: AsyncRemoteFs + ?Sized>(
    remote: &T,
    file: &File,
    data: &[u8],
) -> RemoteResult<u32> {
    let caps = remote.capabilities();
    let opts = write_options(file).size_hint(data.len() as u64);
    if caps.contains(Capabilities::STREAM_WRITE) && caps.contains(Capabilities::APPEND) {
        match remote.append(file.path(), &opts).await {
            Ok(stream) => return write_stream(stream, data).await,
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
        .await
        .map(|written| written as u32)
}

#[cfg(test)]
mod test {
    use std::pin::Pin;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use futures_util::io::{AsyncRead, AsyncWrite};
    use pretty_assertions::assert_eq;
    use remotefs::adapters::r#async::Unblock;
    use remotefs::fs::{AsyncRemoteRead, AsyncRemoteWrite, Metadata};
    use remotefs_memory::MemoryFs;

    use super::*;
    use crate::driver::transfer::test::{NoStreamFs, memory_fs, no_stream_fs};

    fn async_fs() -> Unblock<MemoryFs> {
        Unblock::new(memory_fs())
    }

    fn async_no_stream_fs() -> Unblock<NoStreamFs> {
        Unblock::new(no_stream_fs())
    }

    async fn put(remote: &impl AsyncRemoteFs, path: &Path, content: &[u8]) -> File {
        remote
            .write_file(
                path,
                &WriteOptions::default().size_hint(content.len() as u64),
                &mut Cursor::new(content.to_vec()),
            )
            .await
            .expect("write_file");
        remote.stat(path).await.expect("stat")
    }

    async fn get(remote: &impl AsyncRemoteFs, path: &Path) -> Vec<u8> {
        let mut out = Cursor::new(Vec::new());
        remote
            .read_file(path, &ReadOptions::default(), &mut out)
            .await
            .expect("read_file");
        out.into_inner()
    }

    #[tokio::test]
    async fn test_should_read_at_offset_with_stream() {
        let remote = async_fs();
        put(&remote, Path::new("/a.txt"), b"hello world").await;
        let mut buffer = [0u8; 5];
        let read = read_at(&remote, Path::new("/a.txt"), &mut buffer, 6)
            .await
            .expect("read_at");
        assert_eq!(read, 5);
        assert_eq!(&buffer, b"world");
    }

    #[tokio::test]
    async fn test_should_read_at_offset_without_stream() {
        let remote = async_no_stream_fs();
        put(&remote, Path::new("/a.txt"), b"hello world").await;
        let mut buffer = [0u8; 5];
        let read = read_at(&remote, Path::new("/a.txt"), &mut buffer, 6)
            .await
            .expect("read_at");
        assert_eq!(read, 5);
        assert_eq!(&buffer, b"world");
    }

    #[tokio::test]
    async fn test_should_read_zero_bytes_past_end_of_file() {
        let remote = async_fs();
        put(&remote, Path::new("/a.txt"), b"hello world").await;
        let mut buffer = [0u8; 5];
        let read = read_at(&remote, Path::new("/a.txt"), &mut buffer, 1_000_000)
            .await
            .expect("read_at past EOF must not fail");
        assert_eq!(read, 0);
    }

    #[tokio::test]
    async fn test_should_stage_and_finalize_stream_write() {
        let remote = async_fs();
        let file = put(&remote, Path::new("/a.txt"), b"").await;
        let mut state = start_pending_write(&remote, &file).await.expect("start");
        assert!(matches!(state, AsyncPendingWriteState::Stream { .. }));
        write_to_pending(&mut state, b"hello ", 0)
            .await
            .expect("chunk 1");
        write_to_pending(&mut state, b"world", 6)
            .await
            .expect("chunk 2");
        finalize_pending_write(&remote, &file, state)
            .await
            .expect("finish");
        assert_eq!(get(&remote, Path::new("/a.txt")).await, b"hello world");
    }

    #[tokio::test]
    async fn test_should_buffer_write_when_stream_unsupported() {
        let remote = async_no_stream_fs();
        let file = put(&remote, Path::new("/a.txt"), b"").await;
        let mut state = start_pending_write(&remote, &file).await.expect("start");
        assert!(matches!(state, AsyncPendingWriteState::Buffered(_)));
        write_to_pending(&mut state, b"world", 6)
            .await
            .expect("chunk");
        finalize_pending_write(&remote, &file, state)
            .await
            .expect("finish");
        let data = get(&remote, Path::new("/a.txt")).await;
        assert_eq!(&data[..6], &[0u8; 6]);
        assert_eq!(&data[6..], b"world");
    }

    #[tokio::test]
    async fn test_should_preserve_existing_contents_for_buffered_partial_write() {
        let remote = async_no_stream_fs();
        let file = put(&remote, Path::new("/a.txt"), b"abcdef").await;
        let mut state = start_pending_write(&remote, &file).await.expect("start");

        write_to_pending(&mut state, b"X", 2).await.expect("chunk");
        finalize_pending_write(&remote, &file, state)
            .await
            .expect("finish");

        assert_eq!(get(&remote, Path::new("/a.txt")).await, b"abXdef");
    }

    #[tokio::test]
    async fn test_should_finish_read_stream_when_read_fails() {
        let finished = Arc::new(AtomicBool::new(false));
        let stream = AsyncReadStream::new(FailingRead {
            finished: Arc::clone(&finished),
        });
        let mut buffer = [0_u8; 1];

        assert!(read_stream(stream, &mut buffer).await.is_err());
        assert!(finished.load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn test_should_finish_write_stream_when_write_fails() {
        let finished = Arc::new(AtomicBool::new(false));
        let stream = remotefs::fs::AsyncWriteStream::new(FailingWrite {
            finished: Arc::clone(&finished),
        });

        assert!(write_stream(stream, b"data").await.is_err());
        assert!(finished.load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn test_should_create_empty_file() {
        let remote = async_fs();
        create_empty_file(&remote, Path::new("/empty"), Some(UnixPex::from(0o644)))
            .await
            .expect("create_empty_file");
        let file = remote.stat(Path::new("/empty")).await.expect("stat");
        assert_eq!(file.metadata().size, Some(0));
        assert_eq!(file.metadata().mode, Some(UnixPex::from(0o644)));
    }

    #[tokio::test]
    async fn test_should_shrink_and_extend_file() {
        let remote = async_fs();
        let file = put(&remote, Path::new("/a.txt"), b"hello world").await;
        truncate_file(&remote, &file, 5).await.expect("shrink");
        assert_eq!(get(&remote, Path::new("/a.txt")).await, b"hello");
        let file = remote.stat(Path::new("/a.txt")).await.expect("stat");
        truncate_file(&remote, &file, 8).await.expect("extend");
        assert_eq!(get(&remote, Path::new("/a.txt")).await, b"hello\0\0\0");
        truncate_file(&remote, &file, 0).await.expect("truncate");
        assert_eq!(get(&remote, Path::new("/a.txt")).await, b"");
    }

    #[tokio::test]
    async fn test_should_append_with_and_without_stream() {
        let remote = async_fs();
        let file = put(&remote, Path::new("/a.txt"), b"hello").await;
        assert_eq!(
            append_data(&remote, &file, b" world")
                .await
                .expect("append"),
            6
        );
        assert_eq!(get(&remote, Path::new("/a.txt")).await, b"hello world");

        let remote = async_no_stream_fs();
        let file = put(&remote, Path::new("/a.txt"), b"hello").await;
        assert_eq!(
            append_data(&remote, &file, b" world")
                .await
                .expect("append"),
            6
        );
        assert_eq!(get(&remote, Path::new("/a.txt")).await, b"hello world");
    }

    #[test]
    fn test_pending_state_debug_hides_stream() {
        let state = AsyncPendingWriteState::Buffered(vec![1, 2, 3]);
        assert_eq!(format!("{state:?}"), "Buffered(3)");
        let _ = File::new("/x", Metadata::default());
    }

    #[derive(Debug)]
    struct FailingRead {
        finished: Arc<AtomicBool>,
    }

    impl AsyncRead for FailingRead {
        fn poll_read(
            self: Pin<&mut Self>,
            _context: &mut std::task::Context<'_>,
            _buffer: &mut [u8],
        ) -> std::task::Poll<std::io::Result<usize>> {
            std::task::Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into()))
        }
    }

    #[async_trait::async_trait]
    impl AsyncRemoteRead for FailingRead {
        async fn finish(self: Box<Self>) -> RemoteResult<()> {
            self.finished.store(true, Ordering::Release);
            Ok(())
        }
    }

    #[derive(Debug)]
    struct FailingWrite {
        finished: Arc<AtomicBool>,
    }

    impl AsyncWrite for FailingWrite {
        fn poll_write(
            self: Pin<&mut Self>,
            _context: &mut std::task::Context<'_>,
            _buffer: &[u8],
        ) -> std::task::Poll<std::io::Result<usize>> {
            std::task::Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into()))
        }

        fn poll_flush(
            self: Pin<&mut Self>,
            _context: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Ok(()))
        }

        fn poll_close(
            self: Pin<&mut Self>,
            _context: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Ok(()))
        }
    }

    #[async_trait::async_trait]
    impl AsyncRemoteWrite for FailingWrite {
        async fn finish(self: Box<Self>) -> RemoteResult<()> {
            self.finished.store(true, Ordering::Release);
            Ok(())
        }
    }
}
