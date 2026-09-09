use std::io;
use std::path::Path;
use std::pin::Pin;

use super::BlockOn;
use crate::fs::{
    AsyncReadStream, AsyncRemoteFs, AsyncWriteStream, Capabilities, ExecOutput, File, ReadOptions,
    RemoteError, RemoteErrorType, RemoteFs, RemoteResult, SetMetadata, UnixPex, Welcome,
    WriteOptions,
};
use crate::mock::MockRemoteFs;

struct BufferedDownload {
    fail: bool,
    empty: bool,
    drain: bool,
}

fn unsupported<T>() -> RemoteResult<T> {
    Err(RemoteError::new(RemoteErrorType::UnsupportedFeature))
}

#[async_trait::async_trait]
impl AsyncRemoteFs for BufferedDownload {
    async fn connect(&mut self) -> RemoteResult<Welcome> {
        unsupported()
    }
    async fn disconnect(&mut self) -> RemoteResult<()> {
        unsupported()
    }
    fn is_connected(&self) -> bool {
        true
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities::empty()
    }
    async fn list_dir(&self, _: &Path) -> RemoteResult<Vec<File>> {
        unsupported()
    }
    async fn stat(&self, _: &Path) -> RemoteResult<File> {
        unsupported()
    }
    async fn exists(&self, _: &Path) -> RemoteResult<bool> {
        unsupported()
    }
    async fn set_metadata(&self, _: &Path, _: &SetMetadata) -> RemoteResult<()> {
        unsupported()
    }
    async fn create_dir(&self, _: &Path, _: Option<UnixPex>) -> RemoteResult<()> {
        unsupported()
    }
    async fn remove_file(&self, _: &Path) -> RemoteResult<()> {
        unsupported()
    }
    async fn remove_dir(&self, _: &Path) -> RemoteResult<()> {
        unsupported()
    }
    async fn rename(&self, _: &Path, _: &Path) -> RemoteResult<()> {
        unsupported()
    }
    async fn copy(&self, _: &Path, _: &Path) -> RemoteResult<()> {
        unsupported()
    }
    async fn symlink(&self, _: &Path, _: &Path) -> RemoteResult<()> {
        unsupported()
    }
    async fn open(&self, _: &Path, _: &ReadOptions) -> RemoteResult<AsyncReadStream> {
        unsupported()
    }
    async fn create(&self, _: &Path, _: &WriteOptions) -> RemoteResult<AsyncWriteStream> {
        unsupported()
    }
    async fn append(&self, _: &Path, _: &WriteOptions) -> RemoteResult<AsyncWriteStream> {
        unsupported()
    }
    async fn exec(&self, _: &str) -> RemoteResult<ExecOutput> {
        unsupported()
    }

    async fn read_file(
        &self,
        _: &Path,
        _: &ReadOptions,
        dest: &mut (dyn futures_io::AsyncWrite + Send + Unpin),
    ) -> RemoteResult<u64> {
        if self.empty {
            return Ok(0);
        }
        let count =
            std::future::poll_fn(|context| Pin::new(&mut *dest).poll_write(context, b"data"))
                .await
                .map_err(RemoteError::from)?;
        if self.drain {
            std::future::poll_fn(|context| Pin::new(&mut *dest).poll_write(context, &[]))
                .await
                .map_err(RemoteError::from)?;
        }
        if self.fail {
            Err(RemoteError::with_message(
                RemoteErrorType::ProtocolError,
                "backend failed after accepting write",
            ))
        } else {
            Ok(count as u64)
        }
    }
}

struct FailedDestination;

impl io::Write for FailedDestination {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        Err(io::Error::other("borrowed destination failed"))
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn one_shot_override_cannot_hide_a_deferred_destination_error() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    for fail in [false, true] {
        let fs = BlockOn::new(
            BufferedDownload {
                fail,
                empty: false,
                drain: false,
            },
            runtime.handle().clone(),
        );
        let error = fs
            .read_file(
                &MockRemoteFs::path("buffered"),
                &ReadOptions::default(),
                &mut FailedDestination,
            )
            .unwrap_err();
        let message = error.to_string();
        assert!(message.contains("borrowed destination failed"));
        if fail {
            assert!(message.contains("backend failed after accepting write"));
        }
    }
}

#[derive(Default)]
struct FlushDestination {
    bytes: Vec<u8>,
    flushed: bool,
}
impl io::Write for FlushDestination {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        self.flushed = true;
        Ok(())
    }
}

#[test]
fn one_shot_override_flushes_even_after_draining_its_buffer_or_writing_nothing() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    for empty in [false, true] {
        let fs = BlockOn::new(
            BufferedDownload {
                fail: false,
                empty,
                drain: true,
            },
            runtime.handle().clone(),
        );
        let mut dest = FlushDestination::default();
        let count = fs
            .read_file(
                &MockRemoteFs::path("buffered"),
                &ReadOptions::default(),
                &mut dest,
            )
            .unwrap();
        assert_eq!(count, if empty { 0 } else { 4 });
        assert!(dest.flushed);
    }
}
