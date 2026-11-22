#![cfg(feature = "with-containers")]

mod support;

use std::collections::BTreeSet;
use std::io;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::task::{Context, Poll};

use futures::io::{AsyncReadExt as _, AsyncWriteExt as _};
use pretty_assertions::assert_eq;
use remotefs::AsyncRemoteFs;
use remotefs::fs::{Capabilities, ReadOptions, RemoteErrorType, SetMetadata, WriteOptions};
use support::{TestContext, download, upload};

const READ_CHUNK_SIZE: usize = 256 * 1024;

struct ChunkedSource {
    bytes: Vec<u8>,
    offset: usize,
    chunk_size: usize,
}

struct FailingSource {
    emitted: bool,
}

impl futures::io::AsyncRead for FailingSource {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _context: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        if self.emitted {
            Poll::Ready(Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "source failed after emitting data",
            )))
        } else {
            let bytes = b"partial";
            buf[..bytes.len()].copy_from_slice(bytes);
            self.emitted = true;
            Poll::Ready(Ok(bytes.len()))
        }
    }
}

impl futures::io::AsyncRead for ChunkedSource {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _context: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        if self.offset == self.bytes.len() {
            return Poll::Ready(Ok(0));
        }
        let count = self
            .chunk_size
            .min(self.bytes.len() - self.offset)
            .min(buf.len());
        buf[..count].copy_from_slice(&self.bytes[self.offset..self.offset + count]);
        self.offset += count;
        Poll::Ready(Ok(count))
    }
}

#[tokio::test]
async fn connect_establishes_the_client_once() {
    let mut context = TestContext::new().await;
    assert!(!context.client.is_connected());
    context.client.connect().await.expect("failed to connect");
    assert!(context.client.is_connected());
    assert_eq!(
        context.client.connect().await.unwrap_err().kind(),
        RemoteErrorType::AlreadyConnected
    );
}

#[tokio::test]
async fn disconnect_closes_the_client_once() {
    let mut context = TestContext::connected().await;
    context
        .client
        .disconnect()
        .await
        .expect("failed to disconnect");
    assert!(!context.client.is_connected());
    assert_eq!(
        context.client.disconnect().await.unwrap_err().kind(),
        RemoteErrorType::NotConnected
    );
}

#[tokio::test]
async fn capabilities_match_the_supported_operations() {
    let context = TestContext::connected().await;
    let capabilities = context.client.capabilities();
    assert!(capabilities.contains(
        Capabilities::STREAM_READ
            | Capabilities::STREAM_WRITE
            | Capabilities::RANGE_READ
            | Capabilities::COPY
    ));
    assert!(!capabilities.contains(Capabilities::APPEND));
}

#[tokio::test]
async fn relative_paths_are_rejected() {
    let context = TestContext::connected().await;
    assert_eq!(
        context
            .client
            .stat(Path::new("docs"))
            .await
            .unwrap_err()
            .kind(),
        RemoteErrorType::InvalidPath
    );
    assert_eq!(
        context
            .client
            .create_dir(Path::new("docs"), None)
            .await
            .unwrap_err()
            .kind(),
        RemoteErrorType::InvalidPath
    );
}

#[tokio::test]
async fn list_dir_returns_only_direct_children() {
    let context = TestContext::connected().await;
    context
        .client
        .create_dir(Path::new("/docs"), None)
        .await
        .unwrap();
    upload(&context.client, Path::new("/root.txt"), b"root").await;
    upload(&context.client, Path::new("/docs/nested.txt"), b"nested").await;
    let entries = context.client.list_dir(Path::new("/")).await.unwrap();
    let names: BTreeSet<_> = entries.iter().map(remotefs::File::name).collect();
    assert_eq!(
        names,
        BTreeSet::from([String::from("docs"), String::from("root.txt")])
    );
    assert_eq!(
        context
            .client
            .list_dir(Path::new("/root.txt"))
            .await
            .unwrap_err()
            .kind(),
        RemoteErrorType::BadFile
    );
}

#[tokio::test]
async fn stat_reports_root_directory_file_and_missing_path() {
    let context = TestContext::connected().await;
    upload(&context.client, Path::new("/file.bin"), b"bytes").await;
    let root = context.client.stat(Path::new("/")).await.unwrap();
    assert!(root.is_dir());
    assert_eq!(root.metadata().size, None);
    let file = context.client.stat(Path::new("/file.bin")).await.unwrap();
    assert!(file.is_file());
    assert_eq!(file.metadata().size, Some(5));
    assert_eq!(
        context
            .client
            .stat(Path::new("/missing"))
            .await
            .unwrap_err()
            .kind(),
        RemoteErrorType::NoSuchFileOrDirectory
    );
}

#[tokio::test]
async fn exists_distinguishes_present_and_missing_paths() {
    let context = TestContext::connected().await;
    upload(&context.client, Path::new("/present.txt"), b"present").await;
    assert!(
        context
            .client
            .exists(Path::new("/present.txt"))
            .await
            .unwrap()
    );
    assert!(
        !context
            .client
            .exists(Path::new("/missing.txt"))
            .await
            .unwrap()
    );
}

#[cfg(feature = "find")]
#[tokio::test]
async fn find_recurses_from_an_explicit_root() {
    let context = TestContext::connected().await;
    context
        .client
        .create_dir(Path::new("/docs"), None)
        .await
        .unwrap();
    upload(&context.client, Path::new("/root.txt"), b"root").await;
    upload(&context.client, Path::new("/docs/nested.txt"), b"nested").await;
    upload(&context.client, Path::new("/docs/image.bin"), b"image").await;
    let paths: BTreeSet<_> = remotefs::find_async(&context.client, Path::new("/"), "*.txt")
        .await
        .unwrap()
        .into_iter()
        .map(|file| file.path().to_path_buf())
        .collect();
    assert_eq!(
        paths,
        BTreeSet::from([
            PathBuf::from("/root.txt"),
            PathBuf::from("/docs/nested.txt")
        ])
    );
}

#[tokio::test]
async fn create_dir_creates_a_directory_and_rejects_duplicates() {
    let context = TestContext::connected().await;
    context
        .client
        .create_dir(Path::new("/docs"), None)
        .await
        .unwrap();
    assert!(
        context
            .client
            .stat(Path::new("/docs"))
            .await
            .unwrap()
            .is_dir()
    );
    assert_eq!(
        context
            .client
            .create_dir(Path::new("/docs"), None)
            .await
            .unwrap_err()
            .kind(),
        RemoteErrorType::AlreadyExists
    );
    assert_eq!(
        context
            .client
            .create_dir(Path::new("/"), None)
            .await
            .unwrap_err()
            .kind(),
        RemoteErrorType::InvalidPath
    );
}

#[tokio::test]
async fn create_rejects_directory_destinations() {
    let context = TestContext::connected().await;
    context
        .client
        .create_dir(Path::new("/docs"), None)
        .await
        .unwrap();
    assert_eq!(
        context
            .client
            .create(Path::new("/docs"), &WriteOptions::default())
            .await
            .unwrap_err()
            .kind(),
        RemoteErrorType::BadFile
    );

    upload(&context.client, Path::new("/tree/leaf"), b"leaf").await;
    let Err(error) = context
        .client
        .create(Path::new("/tree"), &WriteOptions::default())
        .await
    else {
        panic!("a directory destination must be rejected");
    };
    assert_eq!(error.kind(), RemoteErrorType::BadFile, "{error}");

    upload(&context.client, Path::new("/file"), b"file").await;
    assert_eq!(
        context
            .client
            .create(Path::new("/file/child"), &WriteOptions::default())
            .await
            .unwrap_err()
            .kind(),
        RemoteErrorType::BadFile
    );
    assert_eq!(
        context
            .client
            .create_dir(Path::new("/file/directory"), None)
            .await
            .unwrap_err()
            .kind(),
        RemoteErrorType::BadFile
    );
}

#[tokio::test]
async fn remove_file_deletes_only_files() {
    let context = TestContext::connected().await;
    upload(&context.client, Path::new("/file.txt"), b"file").await;
    context
        .client
        .create_dir(Path::new("/docs"), None)
        .await
        .unwrap();
    context
        .client
        .remove_file(Path::new("/file.txt"))
        .await
        .unwrap();
    assert!(!context.client.exists(Path::new("/file.txt")).await.unwrap());
    assert_eq!(
        context
            .client
            .remove_file(Path::new("/docs"))
            .await
            .unwrap_err()
            .kind(),
        RemoteErrorType::BadFile
    );
}

#[tokio::test]
async fn remove_dir_deletes_empty_directories_and_rejects_non_empty_ones() {
    let context = TestContext::connected().await;
    context
        .client
        .create_dir(Path::new("/empty"), None)
        .await
        .unwrap();
    context
        .client
        .create_dir(Path::new("/full"), None)
        .await
        .unwrap();
    upload(&context.client, Path::new("/full/file.txt"), b"file").await;
    context
        .client
        .remove_dir(Path::new("/empty"))
        .await
        .unwrap();
    assert!(!context.client.exists(Path::new("/empty")).await.unwrap());
    assert_eq!(
        context
            .client
            .remove_dir(Path::new("/full"))
            .await
            .unwrap_err()
            .kind(),
        RemoteErrorType::DirectoryNotEmpty
    );
}

#[tokio::test]
async fn remove_dir_all_deletes_nested_content_and_the_marker() {
    let context = TestContext::connected().await;
    context
        .client
        .create_dir(Path::new("/tree"), None)
        .await
        .unwrap();
    context
        .client
        .create_dir(Path::new("/tree/nested"), None)
        .await
        .unwrap();
    upload(&context.client, Path::new("/tree/root.txt"), b"root").await;
    upload(&context.client, Path::new("/tree/nested/leaf.txt"), b"leaf").await;
    context
        .client
        .remove_dir_all(Path::new("/tree"))
        .await
        .unwrap();
    assert!(!context.client.exists(Path::new("/tree")).await.unwrap());
    assert!(
        !context
            .client
            .exists(Path::new("/tree/nested/leaf.txt"))
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn write_file_uploads_small_and_multi_chunk_objects() {
    let context = TestContext::connected().await;
    let large = vec![7_u8; READ_CHUNK_SIZE + 13];
    upload(&context.client, Path::new("/small.txt"), b"small").await;
    upload(&context.client, Path::new("/large.bin"), &large).await;
    assert_eq!(
        context
            .client
            .stat(Path::new("/small.txt"))
            .await
            .unwrap()
            .metadata()
            .size,
        Some(5)
    );
    assert_eq!(
        context
            .client
            .stat(Path::new("/large.bin"))
            .await
            .unwrap()
            .metadata()
            .size,
        Some(large.len() as u64)
    );
    assert_eq!(
        download(&context.client, Path::new("/large.bin")).await,
        large
    );
}

#[tokio::test]
async fn write_file_without_a_size_hint_uploads_the_whole_source() {
    let context = TestContext::connected().await;
    let mut source = futures::io::Cursor::new(b"no hint".to_vec());
    let written = context
        .client
        .write_file(
            Path::new("/nohint.txt"),
            &WriteOptions::default(),
            &mut source,
        )
        .await
        .unwrap();
    assert_eq!(written, 7);
    assert_eq!(
        download(&context.client, Path::new("/nohint.txt")).await,
        b"no hint"
    );
}

#[tokio::test]
async fn write_file_without_a_size_hint_accepts_more_than_the_channel_capacity() {
    let context = TestContext::connected().await;
    let expected = b"abcdefghijklmnopqrstuvwxyz".to_vec();
    let mut source = ChunkedSource {
        bytes: expected.clone(),
        offset: 0,
        chunk_size: 1,
    };
    let written = context
        .client
        .write_file(
            Path::new("/many-chunks.txt"),
            &WriteOptions::default(),
            &mut source,
        )
        .await
        .unwrap();
    assert_eq!(written, expected.len() as u64);
    assert_eq!(
        download(&context.client, Path::new("/many-chunks.txt")).await,
        expected
    );
}

#[tokio::test]
async fn size_hint_mismatch_fails_on_finish() {
    let context = TestContext::connected().await;
    let mut stream = context
        .client
        .create(
            Path::new("/short.txt"),
            &WriteOptions::default().size_hint(10),
        )
        .await
        .unwrap();
    stream.write_all(b"abc").await.unwrap();
    assert_eq!(
        stream.close().await.unwrap_err().kind(),
        io::ErrorKind::InvalidData
    );
    let error = stream.finish().await.unwrap_err();
    assert_eq!(error.kind(), RemoteErrorType::ProtocolError);
    assert!(
        !context
            .client
            .exists(Path::new("/short.txt"))
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn failed_source_does_not_commit_a_partial_object() {
    let context = TestContext::connected().await;
    upload(&context.client, Path::new("/stable.txt"), b"stable").await;
    let mut source = FailingSource { emitted: false };
    assert_eq!(
        context
            .client
            .write_file(
                Path::new("/stable.txt"),
                &WriteOptions::default(),
                &mut source,
            )
            .await
            .unwrap_err()
            .kind(),
        RemoteErrorType::IoError
    );
    assert_eq!(
        download(&context.client, Path::new("/stable.txt")).await,
        b"stable"
    );
}

#[tokio::test]
async fn a_dropped_write_stream_does_not_create_the_object() {
    let context = TestContext::connected().await;
    let mut stream = context
        .client
        .create(Path::new("/abandoned.txt"), &WriteOptions::default())
        .await
        .unwrap();
    stream.write_all(b"partial").await.unwrap();
    drop(stream);
    assert!(
        !context
            .client
            .exists(Path::new("/abandoned.txt"))
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn open_streams_exact_multi_chunk_contents() {
    let context = TestContext::connected().await;
    let expected = vec![9_u8; READ_CHUNK_SIZE + 13];
    upload(&context.client, Path::new("/large.bin"), &expected).await;
    let mut stream = context
        .client
        .open(Path::new("/large.bin"), &ReadOptions::default())
        .await
        .unwrap();
    assert!(!stream.seekable());
    let mut output = Vec::new();
    stream.read_to_end(&mut output).await.unwrap();
    stream.finish().await.unwrap();
    assert_eq!(output, expected);
}

#[tokio::test]
async fn ranged_reads_honor_offset_length_zero_and_beyond_eof() {
    let context = TestContext::connected().await;
    upload(&context.client, Path::new("/range.txt"), b"abcdef").await;
    let read = |opts: ReadOptions| {
        let client = &context.client;
        async move {
            let mut output = futures::io::Cursor::new(Vec::new());
            client
                .read_file(Path::new("/range.txt"), &opts, &mut output)
                .await
                .unwrap();
            output.into_inner()
        }
    };
    assert_eq!(read(ReadOptions::default().offset(2)).await, b"cdef");
    assert_eq!(
        read(ReadOptions::default().offset(2).length(2)).await,
        b"cd"
    );
    assert_eq!(read(ReadOptions::default().length(3)).await, b"abc");
    assert_eq!(read(ReadOptions::default().offset(2).length(0)).await, b"");
    assert_eq!(read(ReadOptions::default().offset(100)).await, b"");
    assert_eq!(
        read(ReadOptions::default().offset(100).length(5)).await,
        b""
    );
    assert_eq!(
        context
            .client
            .open(Path::new("/missing.txt"), &ReadOptions::default().length(0),)
            .await
            .unwrap_err()
            .kind(),
        RemoteErrorType::NoSuchFileOrDirectory
    );
    assert_eq!(
        context
            .client
            .open(Path::new("/missing.txt"), &ReadOptions::default())
            .await
            .unwrap_err()
            .kind(),
        RemoteErrorType::NoSuchFileOrDirectory
    );
}

#[tokio::test]
async fn copy_duplicates_a_file_and_preserves_the_source() {
    let context = TestContext::connected().await;
    upload(&context.client, Path::new("/source.txt"), b"source").await;
    context
        .client
        .copy(Path::new("/source.txt"), Path::new("/copy.txt"))
        .await
        .unwrap();
    assert_eq!(
        download(&context.client, Path::new("/source.txt")).await,
        b"source"
    );
    assert_eq!(
        download(&context.client, Path::new("/copy.txt")).await,
        b"source"
    );
}

#[tokio::test]
async fn rename_transfers_a_file_and_removes_the_source() {
    let context = TestContext::connected().await;
    upload(&context.client, Path::new("/source.txt"), b"source").await;
    context
        .client
        .rename(Path::new("/source.txt"), Path::new("/moved.txt"))
        .await
        .unwrap();
    assert!(
        !context
            .client
            .exists(Path::new("/source.txt"))
            .await
            .unwrap()
    );
    assert_eq!(
        download(&context.client, Path::new("/moved.txt")).await,
        b"source"
    );
}

#[tokio::test]
async fn rename_of_a_file_to_itself_is_a_noop() {
    let context = TestContext::connected().await;
    upload(&context.client, Path::new("/same.txt"), b"same").await;
    context
        .client
        .rename(Path::new("/same.txt"), Path::new("/same.txt"))
        .await
        .unwrap();
    assert_eq!(
        download(&context.client, Path::new("/same.txt")).await,
        b"same"
    );
}

#[tokio::test]
async fn unsupported_operations_report_unsupported_feature() {
    let context = TestContext::connected().await;
    let path = Path::new("/file");
    assert_eq!(
        context
            .client
            .set_metadata(path, &SetMetadata::default())
            .await
            .unwrap_err()
            .kind(),
        RemoteErrorType::UnsupportedFeature
    );
    assert_eq!(
        context
            .client
            .symlink(Path::new("/link"), path)
            .await
            .unwrap_err()
            .kind(),
        RemoteErrorType::UnsupportedFeature
    );
    assert_eq!(
        context.client.exec("true").await.unwrap_err().kind(),
        RemoteErrorType::UnsupportedFeature
    );
    assert_eq!(
        context
            .client
            .append(path, &WriteOptions::default())
            .await
            .unwrap_err()
            .kind(),
        RemoteErrorType::UnsupportedFeature
    );
    let mut source = futures::io::Cursor::new(b"bytes".to_vec());
    assert_eq!(
        context
            .client
            .append_file(path, &WriteOptions::default(), &mut source)
            .await
            .unwrap_err()
            .kind(),
        RemoteErrorType::UnsupportedFeature
    );
}
