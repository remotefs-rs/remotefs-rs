//! Bounded duplex-pipe transfer workers.

use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::task::Poll;

use futures_io::{AsyncRead, AsyncWrite};
use tokio::runtime::Handle;
use tokio::task::{JoinHandle, spawn_blocking};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};
use tokio_util::io::SyncIoBridge;

use super::Shared;
use crate::fs::{ReadOptions, RemoteError, RemoteFs, RemoteResult, WriteOptions};

const PIPE_CAPACITY: usize = 8192;

pub(super) async fn read_file<T>(
    shared: Arc<Shared<T>>,
    path: &Path,
    opts: &ReadOptions,
    dest: &mut (dyn AsyncWrite + Send + Unpin),
) -> RemoteResult<u64>
where
    T: RemoteFs + 'static,
{
    let path = path.to_path_buf();
    let opts = opts.clone();
    let handle = Handle::current();
    let (local, worker_side) = tokio::io::duplex(PIPE_CAPACITY);

    let worker = spawn_io_worker(move || {
        let mut bridge = SyncIoBridge::new_with_handle(worker_side, handle);
        match shared.inner.read() {
            Ok(inner) => {
                let result = inner.read_file(&path, &opts, &mut bridge);
                super::refresh(&shared, &inner);
                result
            }
            Err(_) => Err(super::lock_error()),
        }
    });

    let pump = async move {
        let mut reader = local.compat();
        let copied = crate::io::copy(&mut reader, dest)
            .await
            .map_err(PumpFailure::from_io)?;
        crate::io::flush(dest).await.map_err(PumpFailure::from_io)?;
        Ok(copied)
    };

    join_transfer(pump, worker).await
}

pub(super) async fn write_file<T>(
    shared: Arc<Shared<T>>,
    path: &Path,
    opts: &WriteOptions,
    src: &mut (dyn AsyncRead + Send + Unpin),
    append: bool,
) -> RemoteResult<u64>
where
    T: RemoteFs + 'static,
{
    let path = path.to_path_buf();
    let opts = opts.clone();
    let handle = Handle::current();
    let (local, worker_side) = tokio::io::duplex(PIPE_CAPACITY);

    let worker = spawn_io_worker(move || {
        let mut bridge = SyncIoBridge::new_with_handle(worker_side, handle);
        match shared.inner.read() {
            Ok(inner) => {
                let result = if append {
                    inner.append_file(&path, &opts, &mut bridge)
                } else {
                    inner.write_file(&path, &opts, &mut bridge)
                };
                super::refresh(&shared, &inner);
                result
            }
            Err(_) => Err(super::lock_error()),
        }
    });

    let pump = async move {
        let mut writer = local.compat_write();
        let copied = crate::io::copy(src, &mut writer)
            .await
            .map_err(PumpFailure::from_io)?;
        close_writer(&mut writer)
            .await
            .map_err(PumpFailure::from_io)?;
        Ok(copied)
    };

    join_transfer(pump, worker).await
}

fn spawn_io_worker<F, R>(operation: F) -> JoinHandle<R>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    spawn_blocking(move || {
        std::thread::scope(|scope| {
            scope
                .spawn(operation)
                .join()
                .expect("remote transfer worker panicked")
        })
    })
}

async fn close_writer(writer: &mut (impl AsyncWrite + Unpin)) -> std::io::Result<()> {
    std::future::poll_fn(|context| Pin::new(&mut *writer).poll_close(context)).await
}

struct PumpFailure {
    error: RemoteError,
    consequential: bool,
}

impl PumpFailure {
    fn from_io(error: std::io::Error) -> Self {
        Self {
            consequential: error.kind() == std::io::ErrorKind::BrokenPipe,
            error: RemoteError::from(error),
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("{pump}; transfer worker also failed: {worker}")]
struct TransferPairFailure {
    #[source]
    pump: RemoteError,
    worker: RemoteError,
}

async fn join_transfer<'a, F>(pump: F, worker: JoinHandle<RemoteResult<u64>>) -> RemoteResult<u64>
where
    F: Future<Output = Result<u64, PumpFailure>> + 'a,
{
    let mut pump = Some(Box::pin(pump));
    let mut pump_result = None;
    let mut worker = Some(worker);
    let mut worker_result = None;

    std::future::poll_fn(|context| {
        if let Some(pump_future) = pump.as_mut() {
            match pump_future.as_mut().poll(context) {
                Poll::Pending => {}
                Poll::Ready(result) => {
                    pump = None;
                    pump_result = Some(result);
                }
            }
        }

        if let Some(worker_future) = worker.as_mut() {
            match Pin::new(worker_future).poll(context) {
                Poll::Pending => {}
                Poll::Ready(Ok(result)) => {
                    worker = None;
                    worker_result = Some(result);
                }
                Poll::Ready(Err(error)) => {
                    worker = None;
                    worker_result = Some(Err(RemoteError::with_source(
                        crate::fs::RemoteErrorType::ProtocolError,
                        error,
                    )));
                }
            }
        }

        if worker_result.as_ref().is_some_and(Result::is_err) {
            pump = None;
            let worker = worker_result.take().expect("worker result exists");
            let result = match (pump_result.take(), worker) {
                (Some(Err(pump)), Err(worker)) if !pump.consequential => {
                    Err(RemoteError::with_source(
                        pump.error.kind(),
                        TransferPairFailure {
                            pump: pump.error,
                            worker,
                        },
                    ))
                }
                (_, worker) => worker,
            };
            return Poll::Ready(result);
        }

        match (pump_result.as_ref(), worker_result.as_ref()) {
            (Some(Err(_)), _) => {
                pump = None;
                if worker_result.is_some() {
                    let result = match (pump_result.take(), worker_result.take()) {
                        (Some(Err(pump)), Some(Ok(_))) => Err(pump.error),
                        _ => unreachable!("transfer result state changed unexpectedly"),
                    };
                    Poll::Ready(result)
                } else {
                    Poll::Pending
                }
            }
            (Some(Ok(_)), Some(Ok(_))) => {
                let result = match (pump_result.take(), worker_result.take()) {
                    (Some(Ok(_)), Some(Ok(count))) => Ok(count),
                    _ => unreachable!("transfer result state changed unexpectedly"),
                };
                Poll::Ready(result)
            }
            _ => Poll::Pending,
        }
    })
    .await
}
