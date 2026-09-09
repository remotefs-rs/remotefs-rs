//! Regression tests for buffered blocking streams.

use std::collections::VecDeque;
use std::future::poll_fn;
use std::io::{self, Cursor, Read, Seek, SeekFrom, Write};
use std::pin::Pin;
use std::sync::{Arc, Mutex, mpsc};
use std::task::{Context, Poll, Waker};

use futures_io::{AsyncRead, AsyncWrite};

use super::{UnblockRead, UnblockWrite};
use crate::fs::{
    AsyncRemoteRead, AsyncRemoteWrite, ReadStream, RemoteRead, RemoteResult, RemoteWrite,
    WriteStream,
};

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .max_blocking_threads(1)
        .build()
        .unwrap()
}

struct Reader {
    bytes: Cursor<Vec<u8>>,
    errors: VecDeque<io::ErrorKind>,
    gate: Option<mpsc::Receiver<()>>,
    reject_seek: bool,
}

impl Reader {
    fn new() -> Self {
        Self {
            bytes: Cursor::new(b"abcdef".to_vec()),
            errors: VecDeque::new(),
            gate: None,
            reject_seek: false,
        }
    }
}

impl Read for Reader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if let Some(gate) = self.gate.take() {
            gate.recv().map_err(io::Error::other)?;
        }
        if let Some(error) = self.errors.pop_front() {
            return Err(error.into());
        }
        self.bytes.read(buffer)
    }
}

impl RemoteRead for Reader {
    fn seekable(&self) -> bool {
        true
    }

    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        if self.reject_seek {
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        Seek::seek(&mut self.bytes, position)
    }
}

async fn read(reader: &mut UnblockRead, buffer: &mut [u8]) -> io::Result<usize> {
    poll_fn(|context| Pin::new(&mut *reader).poll_read(context, buffer)).await
}

async fn flush(writer: &mut UnblockWrite) -> io::Result<()> {
    poll_fn(|context| Pin::new(&mut *writer).poll_flush(context)).await
}

#[test]
fn interrupted_read_retries_without_truncating_copy() {
    runtime().block_on(async {
        let mut backend = Reader::new();
        backend.errors.push_back(io::ErrorKind::Interrupted);
        let mut reader = UnblockRead::new(ReadStream::new(backend));
        let mut output = crate::mock::async_io::AsyncCursor::new(Vec::new());
        assert_eq!(crate::io::copy(&mut reader, &mut output).await.unwrap(), 6);
        assert_eq!(output.into_inner(), b"abcdef");
        Box::new(reader).finish().await.unwrap();
    });
}

#[test]
fn read_recovers_after_reported_error() {
    runtime().block_on(async {
        let mut backend = Reader::new();
        backend.errors.push_back(io::ErrorKind::Other);
        let mut reader = UnblockRead::new(ReadStream::new(backend));
        let mut buffer = [0; 6];
        assert_eq!(
            read(&mut reader, &mut buffer).await.unwrap_err().kind(),
            io::ErrorKind::Other
        );
        assert_eq!(read(&mut reader, &mut buffer).await.unwrap(), 6);
        assert_eq!(&buffer, b"abcdef");
        Box::new(reader).finish().await.unwrap();
    });
}

#[test]
fn rejected_seeks_preserve_unread_bytes() {
    for (position, reject_seek, expected) in [
        (
            SeekFrom::Current(i64::MIN),
            false,
            io::ErrorKind::InvalidInput,
        ),
        (SeekFrom::Start(0), true, io::ErrorKind::PermissionDenied),
    ] {
        runtime().block_on(async {
            let (release, gate) = mpsc::channel();
            let mut backend = Reader::new();
            backend.gate = Some(gate);
            backend.reject_seek = reject_seek;
            let mut reader = UnblockRead::new(ReadStream::new(backend));
            let mut large = [0; 6];
            assert!(
                Pin::new(&mut reader)
                    .poll_read(&mut Context::from_waker(Waker::noop()), &mut large)
                    .is_pending()
            );
            release.send(()).unwrap();
            let mut first = [0; 1];
            assert_eq!(read(&mut reader, &mut first).await.unwrap(), 1);
            assert_eq!(&first, b"a");
            assert_eq!(reader.seek(position).await.unwrap_err().kind(), expected);
            let mut remaining = [0; 5];
            assert_eq!(read(&mut reader, &mut remaining).await.unwrap(), 5);
            assert_eq!(&remaining, b"bcdef");
            Box::new(reader).finish().await.unwrap();
        });
    }
}

#[derive(Default)]
struct Written {
    bytes: Cursor<Vec<u8>>,
    finished: bool,
}

struct Writer {
    written: Arc<Mutex<Written>>,
    gate: Option<mpsc::Receiver<()>>,
    errors: VecDeque<io::ErrorKind>,
    limit: usize,
}

impl Writer {
    fn new(written: &Arc<Mutex<Written>>) -> Self {
        Self {
            written: Arc::clone(written),
            gate: None,
            errors: VecDeque::new(),
            limit: usize::MAX,
        }
    }
}

impl Write for Writer {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        if let Some(gate) = self.gate.take() {
            gate.recv().map_err(io::Error::other)?;
        }
        if let Some(error) = self.errors.pop_front() {
            return Err(error.into());
        }
        self.written
            .lock()
            .unwrap()
            .bytes
            .write(&buffer[..buffer.len().min(self.limit)])
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl RemoteWrite for Writer {
    fn seekable(&self) -> bool {
        true
    }

    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        Seek::seek(&mut self.written.lock().unwrap().bytes, position)
    }

    fn finish(self: Box<Self>) -> RemoteResult<()> {
        self.written.lock().unwrap().finished = true;
        Ok(())
    }
}

#[test]
fn accepted_write_and_cancelled_next_write_never_acknowledge_wrong_buffer() {
    runtime().block_on(async {
        let written = Arc::new(Mutex::new(Written::default()));
        let (release, gate) = mpsc::channel();
        let mut backend = Writer::new(&written);
        backend.gate = Some(gate);
        let mut writer = UnblockWrite::new(WriteStream::new(backend));
        let mut context = Context::from_waker(Waker::noop());
        assert!(matches!(
            Pin::new(&mut writer).poll_write(&mut context, b"abcd"),
            Poll::Ready(Ok(4))
        ));
        assert!(
            Pin::new(&mut writer)
                .poll_write(&mut context, b"discard me")
                .is_pending()
        );
        release.send(()).unwrap();
        let accepted = poll_fn(|context| Pin::new(&mut writer).poll_write(context, b"X"))
            .await
            .unwrap();
        assert_eq!(accepted, 1);
        Box::new(writer).finish().await.unwrap();
        let state = written.lock().unwrap();
        assert_eq!(state.bytes.get_ref(), b"abcdX");
        assert!(state.finished);
    });
}

#[test]
fn buffered_writes_retry_interruptions_and_partial_writes_before_seek() {
    runtime().block_on(async {
        let written = Arc::new(Mutex::new(Written::default()));
        let mut backend = Writer::new(&written);
        backend.limit = 1;
        backend.errors.push_back(io::ErrorKind::Interrupted);
        let mut writer = UnblockWrite::new(WriteStream::new(backend));
        assert_eq!(
            poll_fn(|context| Pin::new(&mut writer).poll_write(context, b"abcd"))
                .await
                .unwrap(),
            4
        );
        assert_eq!(writer.seek(SeekFrom::Current(-1)).await.unwrap(), 3);
        assert_eq!(
            poll_fn(|context| Pin::new(&mut writer).poll_write(context, b"X"))
                .await
                .unwrap(),
            1
        );
        flush(&mut writer).await.unwrap();
        assert_eq!(written.lock().unwrap().bytes.get_ref(), b"abcX");
        Box::new(writer).finish().await.unwrap();
    });
}

#[test]
fn buffered_write_errors_are_reported_by_flush_and_finish() {
    for finish in [false, true] {
        for zero in [false, true] {
            runtime().block_on(async {
                let written = Arc::new(Mutex::new(Written::default()));
                let mut backend = Writer::new(&written);
                if zero {
                    backend.limit = 0;
                } else {
                    backend.errors.push_back(io::ErrorKind::PermissionDenied);
                }
                let mut writer = UnblockWrite::new(WriteStream::new(backend));
                assert_eq!(
                    poll_fn(|context| Pin::new(&mut writer).poll_write(context, b"abc"))
                        .await
                        .unwrap(),
                    3
                );
                if finish {
                    assert!(Box::new(writer).finish().await.is_err());
                } else {
                    let expected = if zero {
                        io::ErrorKind::WriteZero
                    } else {
                        io::ErrorKind::PermissionDenied
                    };
                    assert_eq!(flush(&mut writer).await.unwrap_err().kind(), expected);
                    assert!(Box::new(writer).finish().await.is_err());
                }
                assert!(written.lock().unwrap().finished);
            });
        }
    }
}

#[test]
fn stream_operations_reuse_tokio_blocking_pool_thread() {
    let runtime = runtime();
    runtime.block_on(async {
        let first = super::spawn_io_worker(|| std::thread::current().id())
            .await
            .unwrap();
        for _ in 0..8 {
            assert_eq!(
                super::spawn_io_worker(|| std::thread::current().id())
                    .await
                    .unwrap(),
                first
            );
        }
    });
}

#[test]
fn unblock_can_drive_block_on_streams_on_the_same_runtime() {
    use crate::blocking::BlockOn;
    use crate::fs::{ReadOptions, RemoteFs, WriteOptions};
    use crate::mock::MockRemoteFs;

    let runtime = runtime();
    let client = MockRemoteFs::connected();
    let path = MockRemoteFs::path("nested-streams");
    client.seed_file(&path, b"abcdef");
    let blocking = BlockOn::new(client, runtime.handle().clone());
    let reader = RemoteFs::open(&blocking, &path, &ReadOptions::default()).unwrap();
    runtime.block_on(async {
        let mut reader = UnblockRead::new(reader);
        let mut buffer = [0; 6];
        assert_eq!(read(&mut reader, &mut buffer).await.unwrap(), 6);
        assert_eq!(&buffer, b"abcdef");
        assert_eq!(reader.seek(SeekFrom::Start(1)).await.unwrap(), 1);
        Box::new(reader).finish().await.unwrap();
    });
    let writer = RemoteFs::create(&blocking, &path, &WriteOptions::default()).unwrap();
    runtime.block_on(async {
        let mut writer = UnblockWrite::new(writer);
        assert_eq!(
            poll_fn(|context| Pin::new(&mut writer).poll_write(context, b"abc"))
                .await
                .unwrap(),
            3
        );
        assert_eq!(writer.seek(SeekFrom::Start(1)).await.unwrap(), 1);
        assert_eq!(
            poll_fn(|context| Pin::new(&mut writer).poll_write(context, b"X"))
                .await
                .unwrap(),
            1
        );
        flush(&mut writer).await.unwrap();
        Box::new(writer).finish().await.unwrap();
    });
    let client = blocking.into_inner();
    assert_eq!(client.contents(&path), b"aXc");
    assert_eq!(client.finish_count(), 2);
}

#[test]
fn writes_larger_than_the_buffer_are_drained_before_finish() {
    runtime().block_on(async {
        let written = Arc::new(Mutex::new(Written::default()));
        let mut backend = Writer::new(&written);
        backend.limit = 97;
        let mut writer = UnblockWrite::new(WriteStream::new(backend));
        let input = (0..25000)
            .map(|index| (index % 251) as u8)
            .collect::<Vec<_>>();
        let mut offset = 0;
        while offset < input.len() {
            let accepted =
                poll_fn(|context| Pin::new(&mut writer).poll_write(context, &input[offset..]))
                    .await
                    .unwrap();
            assert!(accepted > 0 && accepted <= input.len() - offset);
            offset += accepted;
        }
        Box::new(writer).finish().await.unwrap();
        assert_eq!(written.lock().unwrap().bytes.get_ref(), &input);
    });
}

struct DroppedReader {
    reader: Reader,
    dropped: mpsc::Sender<std::thread::ThreadId>,
    finalized: Arc<std::sync::atomic::AtomicBool>,
}

impl Read for DroppedReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.reader.read(buffer)
    }
}

impl RemoteRead for DroppedReader {
    fn seekable(&self) -> bool {
        true
    }

    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        if let Some(gate) = self.reader.gate.take() {
            gate.recv().map_err(io::Error::other)?;
        }
        Seek::seek(&mut self.reader.bytes, position)
    }

    fn finish(self: Box<Self>) -> RemoteResult<()> {
        self.finalized
            .store(true, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
}

impl Drop for DroppedReader {
    fn drop(&mut self) {
        let _ = self.dropped.send(std::thread::current().id());
    }
}

#[test]
fn dropping_pending_read_cleans_up_off_runtime_without_finalizing() {
    let runtime = runtime();
    let (release, gate) = mpsc::channel();
    let (dropped, observed) = mpsc::channel();
    let finalized = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut backend = Reader::new();
    backend.gate = Some(gate);
    let backend = DroppedReader {
        reader: backend,
        dropped,
        finalized: Arc::clone(&finalized),
    };
    runtime.block_on(async {
        let mut reader = UnblockRead::new(ReadStream::new(backend));
        let mut buffer = [0; 6];
        assert!(
            Pin::new(&mut reader)
                .poll_read(&mut Context::from_waker(Waker::noop()), &mut buffer)
                .is_pending()
        );
        drop(reader);
    });
    release.send(()).unwrap();
    let dropped_on = observed
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    assert_ne!(dropped_on, std::thread::current().id());
    assert!(!finalized.load(std::sync::atomic::Ordering::SeqCst));
}

#[test]
fn dropping_completed_read_seek_cleans_up_off_runtime() {
    let runtime = runtime();
    let (release, gate) = mpsc::channel();
    let (dropped, observed) = mpsc::channel();
    let mut backend = Reader::new();
    backend.gate = Some(gate);
    let backend = DroppedReader {
        reader: backend,
        dropped,
        finalized: Arc::new(std::sync::atomic::AtomicBool::new(false)),
    };
    runtime.block_on(async {
        let mut reader = UnblockRead::new(ReadStream::new(backend));
        let mut seeking = Box::pin(reader.seek(SeekFrom::Start(0)));
        assert!(
            std::future::Future::poll(seeking.as_mut(), &mut Context::from_waker(Waker::noop()))
                .is_pending()
        );
        release.send(()).unwrap();
        // A single pool worker must finish the seek before executing this barrier.
        super::spawn_io_worker(|| ()).await.unwrap();
        drop(seeking);
        drop(reader);
    });
    assert_ne!(
        observed
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap(),
        std::thread::current().id()
    );
}

struct DroppedWriter {
    writer: Writer,
    dropped: mpsc::Sender<std::thread::ThreadId>,
}

impl Write for DroppedWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.writer.write(buffer)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.writer.flush()
    }
}

impl RemoteWrite for DroppedWriter {
    fn seekable(&self) -> bool {
        true
    }
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        if let Some(gate) = self.writer.gate.take() {
            gate.recv().map_err(io::Error::other)?;
        }
        RemoteWrite::seek(&mut self.writer, position)
    }
}

impl Drop for DroppedWriter {
    fn drop(&mut self) {
        let _ = self.dropped.send(std::thread::current().id());
    }
}

#[test]
fn dropping_completed_write_seek_cleans_up_off_runtime() {
    let runtime = runtime();
    let (release, gate) = mpsc::channel();
    let (dropped, observed) = mpsc::channel();
    let written = Arc::new(Mutex::new(Written::default()));
    let mut backend = Writer::new(&written);
    backend.gate = Some(gate);
    let backend = DroppedWriter {
        writer: backend,
        dropped,
    };
    runtime.block_on(async {
        let mut writer = UnblockWrite::new(WriteStream::new(backend));
        let mut seeking = Box::pin(writer.seek(SeekFrom::Start(0)));
        assert!(
            std::future::Future::poll(seeking.as_mut(), &mut Context::from_waker(Waker::noop()))
                .is_pending()
        );
        release.send(()).unwrap();
        super::spawn_io_worker(|| ()).await.unwrap();
        drop(seeking);
        drop(writer);
    });
    assert_ne!(
        observed
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap(),
        std::thread::current().id()
    );
}
