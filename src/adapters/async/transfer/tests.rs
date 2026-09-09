use std::future::Future;
use std::io::{self, Read, Write};
use std::path::Path;
use std::pin::Pin;
use std::sync::{Arc, Mutex, mpsc};
use std::task::{Context, Poll, ready};
use std::time::Duration;

use futures_io::{AsyncRead, AsyncWrite};
use tokio::sync::oneshot;

use crate::r#async::Unblock;
use crate::fs::{
    AsyncRemoteFs, Capabilities, ExecOutput, File, ReadOptions, ReadStream, RemoteError,
    RemoteErrorType, RemoteFs, RemoteResult, SetMetadata, UnixPex, Welcome, WriteOptions,
    WriteStream,
};
use crate::mock::MockRemoteFs;
use crate::mock::async_io::AsyncCursor;

#[derive(Clone, Copy)]
enum Upload {
    Declared,
    Drain,
    Fail,
}

struct OneShot {
    upload: Upload,
    bytes: Vec<u8>,
    completed: Mutex<Option<oneshot::Sender<()>>>,
    open_gate: Option<Mutex<mpsc::Receiver<()>>>,
    dropped: Option<mpsc::Sender<std::thread::ThreadId>>,
}

impl OneShot {
    fn new(upload: Upload, bytes: Vec<u8>) -> (Self, oneshot::Receiver<()>) {
        let (sender, receiver) = oneshot::channel();
        (
            Self {
                upload,
                bytes,
                completed: Mutex::new(Some(sender)),
                open_gate: None,
                dropped: None,
            },
            receiver,
        )
    }

    fn completed(&self) {
        if let Some(sender) = self.completed.lock().unwrap().take() {
            let _ = sender.send(());
        }
    }
}

fn unsupported<T>() -> RemoteResult<T> {
    Err(RemoteError::new(RemoteErrorType::UnsupportedFeature))
}

impl RemoteFs for OneShot {
    fn connect(&mut self) -> RemoteResult<Welcome> {
        unsupported()
    }
    fn disconnect(&mut self) -> RemoteResult<()> {
        unsupported()
    }
    fn is_connected(&self) -> bool {
        true
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities::empty()
    }
    fn list_dir(&self, _: &Path) -> RemoteResult<Vec<File>> {
        unsupported()
    }
    fn stat(&self, _: &Path) -> RemoteResult<File> {
        unsupported()
    }
    fn exists(&self, _: &Path) -> RemoteResult<bool> {
        unsupported()
    }
    fn set_metadata(&self, _: &Path, _: &SetMetadata) -> RemoteResult<()> {
        unsupported()
    }
    fn create_dir(&self, _: &Path, _: Option<UnixPex>) -> RemoteResult<()> {
        unsupported()
    }
    fn remove_file(&self, _: &Path) -> RemoteResult<()> {
        unsupported()
    }
    fn remove_dir(&self, _: &Path) -> RemoteResult<()> {
        unsupported()
    }
    fn rename(&self, _: &Path, _: &Path) -> RemoteResult<()> {
        unsupported()
    }
    fn copy(&self, _: &Path, _: &Path) -> RemoteResult<()> {
        unsupported()
    }
    fn symlink(&self, _: &Path, _: &Path) -> RemoteResult<()> {
        unsupported()
    }
    fn open(&self, _: &Path, _: &ReadOptions) -> RemoteResult<ReadStream> {
        let Some(gate) = &self.open_gate else {
            return unsupported();
        };
        gate.lock().unwrap().recv().unwrap();
        Ok(ReadStream::new(DropStream(self.dropped.clone().unwrap())))
    }
    fn create(&self, _: &Path, _: &WriteOptions) -> RemoteResult<WriteStream> {
        let Some(gate) = &self.open_gate else {
            return unsupported();
        };
        gate.lock().unwrap().recv().unwrap();
        Ok(WriteStream::new(DropStream(self.dropped.clone().unwrap())))
    }
    fn append(&self, _: &Path, _: &WriteOptions) -> RemoteResult<WriteStream> {
        let Some(gate) = &self.open_gate else {
            return unsupported();
        };
        gate.lock().unwrap().recv().unwrap();
        Ok(WriteStream::new(DropStream(self.dropped.clone().unwrap())))
    }
    fn exec(&self, _: &str) -> RemoteResult<ExecOutput> {
        unsupported()
    }

    fn write_file(
        &self,
        _: &Path,
        opts: &WriteOptions,
        src: &mut (dyn Read + Send),
    ) -> RemoteResult<u64> {
        let result = match self.upload {
            Upload::Declared => io::copy(&mut src.take(opts.size_hint.unwrap()), &mut io::sink())
                .map_err(RemoteError::from),
            Upload::Drain => io::copy(src, &mut io::sink()).map_err(RemoteError::from),
            Upload::Fail => Err(RemoteError::with_message(
                RemoteErrorType::ProtocolError,
                "backend failed",
            )),
        };
        self.completed();
        result
    }

    fn append_file(
        &self,
        path: &Path,
        opts: &WriteOptions,
        src: &mut (dyn Read + Send),
    ) -> RemoteResult<u64> {
        RemoteFs::write_file(self, path, opts, src)
    }

    fn read_file(
        &self,
        _: &Path,
        _: &ReadOptions,
        dest: &mut (dyn Write + Send),
    ) -> RemoteResult<u64> {
        let result = dest
            .write_all(&self.bytes)
            .map(|()| self.bytes.len() as u64)
            .map_err(RemoteError::from);
        self.completed();
        result
    }
}

fn run(future: impl Future<Output = ()> + Send + 'static) {
    let (sender, receiver) = mpsc::channel();
    let thread = std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(future);
        sender.send(()).unwrap();
    });
    receiver
        .recv_timeout(Duration::from_secs(5))
        .expect("transfer hung");
    thread.join().unwrap();
}

struct PendingSource;

impl AsyncRead for PendingSource {
    fn poll_read(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        _: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        Poll::Pending
    }
}

struct BrokenSource;

impl AsyncRead for BrokenSource {
    fn poll_read(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        _: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        Poll::Ready(Err(io::Error::new(
            io::ErrorKind::BrokenPipe,
            "caller source failed",
        )))
    }
}

#[test]
fn upload_success_stops_pending_source_for_write_and_append() {
    run(async {
        for append in [false, true] {
            let (backend, _) = OneShot::new(Upload::Declared, Vec::new());
            let fs = Unblock::new(backend);
            let path = MockRemoteFs::path("empty");
            let opts = WriteOptions::default().size_hint(0);
            let result = if append {
                fs.append_file(&path, &opts, &mut PendingSource).await
            } else {
                fs.write_file(&path, &opts, &mut PendingSource).await
            };
            assert_eq!(result.unwrap(), 0);
        }
    });
}

#[test]
fn upload_success_ignores_only_pipe_broken_pipe() {
    run(async {
        let (backend, _) = OneShot::new(Upload::Declared, Vec::new());
        let fs = Unblock::new(backend);
        let mut source = AsyncCursor::new(vec![7; super::PIPE_CAPACITY * 4]);
        assert_eq!(
            fs.write_file(
                &MockRemoteFs::path("short"),
                &WriteOptions::default().size_hint(1),
                &mut source
            )
            .await
            .unwrap(),
            1
        );

        let (backend, _) = OneShot::new(Upload::Declared, Vec::new());
        let fs = Unblock::new(backend);
        let error = fs
            .write_file(
                &MockRemoteFs::path("error"),
                &WriteOptions::default().size_hint(0),
                &mut BrokenSource,
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("caller source failed"));
    });
}

#[test]
fn upload_worker_error_stops_pending_source_and_preserves_both_errors() {
    run(async {
        let (backend, _) = OneShot::new(Upload::Fail, Vec::new());
        let fs = Unblock::new(backend);
        let error = fs
            .write_file(
                &MockRemoteFs::path("failed"),
                &WriteOptions::default(),
                &mut PendingSource,
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("backend failed"));

        let (backend, _) = OneShot::new(Upload::Fail, Vec::new());
        let fs = Unblock::new(backend);
        let error = fs
            .write_file(
                &MockRemoteFs::path("failed"),
                &WriteOptions::default(),
                &mut BrokenSource,
            )
            .await
            .unwrap_err();
        let message = error.to_string();
        assert!(message.contains("caller source failed"));
        assert!(message.contains("backend failed"));
    });
}

struct DelayedDestination {
    completed: Option<oneshot::Receiver<()>>,
    bytes: Vec<u8>,
    flushed: bool,
}

impl AsyncWrite for DelayedDestination {
    fn poll_write(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        if let Some(completed) = self.completed.as_mut() {
            ready!(Pin::new(completed).poll(context)).unwrap();
            self.completed = None;
        }
        self.bytes.extend_from_slice(bytes);
        Poll::Ready(Ok(bytes.len()))
    }
    fn poll_flush(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.flushed = true;
        Poll::Ready(Ok(()))
    }
    fn poll_close(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.poll_flush(context)
    }
}

#[test]
fn download_drains_buffered_data_and_flushes_after_worker_success() {
    run(async {
        let bytes = vec![42; super::PIPE_CAPACITY];
        let (backend, completed) = OneShot::new(Upload::Drain, bytes.clone());
        let fs = Unblock::new(backend);
        let mut dest = DelayedDestination {
            completed: Some(completed),
            bytes: Vec::new(),
            flushed: false,
        };
        assert_eq!(
            fs.read_file(
                &MockRemoteFs::path("download"),
                &ReadOptions::default(),
                &mut dest
            )
            .await
            .unwrap(),
            bytes.len() as u64
        );
        assert_eq!(dest.bytes, bytes);
        assert!(dest.flushed);
    });
}

#[test]
fn cancelling_upload_releases_the_pipe_worker() {
    run(async {
        let (backend, completed) = OneShot::new(Upload::Drain, Vec::new());
        let fs = Arc::new(Unblock::new(backend));
        let task = tokio::spawn(async move {
            fs.write_file(
                &MockRemoteFs::path("cancelled"),
                &WriteOptions::default(),
                &mut PendingSource,
            )
            .await
        });
        tokio::task::yield_now().await;
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        completed.await.unwrap();
    });
}

struct PrefixThenPending(bool);

impl AsyncRead for PrefixThenPending {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
        buffer: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        if self.0 {
            return Poll::Pending;
        }
        self.0 = true;
        buffer[..4].copy_from_slice(b"data");
        Poll::Ready(Ok(4))
    }
}

#[test]
fn upload_completes_after_declared_bytes_without_source_eof() {
    run(async {
        let (backend, _) = OneShot::new(Upload::Declared, Vec::new());
        let fs = Unblock::new(backend);
        assert_eq!(
            fs.write_file(
                &MockRemoteFs::path("sized"),
                &WriteOptions::default().size_hint(4),
                &mut PrefixThenPending(false)
            )
            .await
            .unwrap(),
            4
        );
    });
}

struct BrokenDestination;

impl AsyncWrite for BrokenDestination {
    fn poll_write(self: Pin<&mut Self>, _: &mut Context<'_>, _: &[u8]) -> Poll<io::Result<usize>> {
        Poll::Ready(Err(io::Error::new(
            io::ErrorKind::BrokenPipe,
            "caller destination failed",
        )))
    }
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
    fn poll_close(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.poll_flush(context)
    }
}

#[test]
fn download_preserves_caller_broken_pipe_and_worker_failure() {
    run(async {
        let (backend, completed) = OneShot::new(Upload::Drain, vec![1; super::PIPE_CAPACITY * 4]);
        let fs = Unblock::new(backend);
        let error = fs
            .read_file(
                &MockRemoteFs::path("failed"),
                &ReadOptions::default(),
                &mut BrokenDestination,
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("caller destination failed"));
        assert!(error.to_string().contains("transfer worker also failed"));
        completed.await.unwrap();
    });
}

#[test]
fn cancelling_download_releases_the_pipe_worker() {
    run(async {
        let (backend, completed) = OneShot::new(Upload::Drain, vec![1; super::PIPE_CAPACITY * 4]);
        let fs = Unblock::new(backend);
        let (_sender, pending) = oneshot::channel();
        let mut dest = DelayedDestination {
            completed: Some(pending),
            bytes: Vec::new(),
            flushed: false,
        };
        let path = MockRemoteFs::path("cancelled");
        let opts = ReadOptions::default();
        let mut transfer = Box::pin(fs.read_file(&path, &opts, &mut dest));
        std::future::poll_fn(|context| {
            assert!(transfer.as_mut().poll(context).is_pending());
            Poll::Ready(())
        })
        .await;
        drop(transfer);
        completed.await.unwrap();
    });
}

struct DropStream(mpsc::Sender<std::thread::ThreadId>);

impl Drop for DropStream {
    fn drop(&mut self) {
        self.0.send(std::thread::current().id()).unwrap();
    }
}
impl Read for DropStream {
    fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
        Ok(0)
    }
}
impl Write for DropStream {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        Ok(buffer.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
impl crate::fs::RemoteRead for DropStream {
    fn finish(self: Box<Self>) -> RemoteResult<()> {
        Ok(())
    }
}
impl crate::fs::RemoteWrite for DropStream {
    fn finish(self: Box<Self>) -> RemoteResult<()> {
        Ok(())
    }
}

struct NotifyReady(mpsc::Sender<()>);
impl std::task::Wake for NotifyReady {
    fn wake(self: Arc<Self>) {
        let _ = self.0.send(());
    }
}

#[test]
fn cancelling_completed_open_create_append_cleans_up_off_runtime() {
    run(async {
        for operation in ["open", "create", "append"] {
            let (mut backend, _) = OneShot::new(Upload::Drain, Vec::new());
            let (release, gate) = mpsc::channel();
            let (dropped, cleanup) = mpsc::channel();
            backend.open_gate = Some(Mutex::new(gate));
            backend.dropped = Some(dropped);
            let fs = Unblock::new(backend);
            let path = MockRemoteFs::path("cancelled-open");
            let read_opts = ReadOptions::default();
            let write_opts = WriteOptions::default();
            let mut future: Pin<Box<dyn Future<Output = ()> + Send + '_>> = match operation {
                "open" => Box::pin(async {
                    let _ = fs.open(&path, &read_opts).await.unwrap();
                }),
                "create" => Box::pin(async {
                    let _ = fs.create(&path, &write_opts).await.unwrap();
                }),
                _ => Box::pin(async {
                    let _ = fs.append(&path, &write_opts).await.unwrap();
                }),
            };
            let (ready_sender, ready_receiver) = mpsc::channel();
            let waker = std::task::Waker::from(Arc::new(NotifyReady(ready_sender)));
            assert!(
                future
                    .as_mut()
                    .poll(&mut Context::from_waker(&waker))
                    .is_pending()
            );
            release.send(()).unwrap();
            ready_receiver.recv_timeout(Duration::from_secs(2)).unwrap();
            drop(future);
            let worker = cleanup.recv_timeout(Duration::from_secs(2)).unwrap();
            assert_ne!(worker, std::thread::current().id());
        }
    });
}
