use crate::blocking::BlockOn;
use crate::fs::{AsyncReadStream, AsyncWriteStream, ReadOptions, RemoteFs, WriteOptions};
use crate::mock::MockRemoteFs;

#[test]
fn block_on_preserves_stream_finalization() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let client = MockRemoteFs::connected();
    let adapter = BlockOn::new(client, runtime.handle().clone());
    let path = MockRemoteFs::path("bridged");
    let mut source = std::io::Cursor::new(b"bridge");
    adapter
        .write_file(&path, &WriteOptions::default(), &mut source)
        .unwrap();
    let client = adapter.into_inner();
    assert_eq!(client.contents(&path), b"bridge");
    assert_eq!(client.finish_count(), 1);
}

#[test]
fn boxed_async_client_can_be_spawned() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    runtime.block_on(async {
        use crate::fs::AsyncRemoteFs;

        let mut fs: Box<dyn AsyncRemoteFs> = Box::new(MockRemoteFs::new());
        tokio::spawn(async move { AsyncRemoteFs::connect(&mut fs).await })
            .await
            .unwrap()
            .unwrap();
    });
}

#[test]
fn tokio_stream_helpers_round_trip_and_finish() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    runtime.block_on(async {
        let mut reader = AsyncReadStream::from_tokio(tokio::io::empty());
        let mut buffer = [0_u8; 1];
        let count = std::future::poll_fn(|context| {
            futures_io::AsyncRead::poll_read(std::pin::Pin::new(&mut reader), context, &mut buffer)
        })
        .await
        .unwrap();
        assert_eq!(count, 0);
        reader.finish().await.unwrap();

        let mut writer = AsyncWriteStream::from_tokio(tokio::io::sink());
        let count = std::future::poll_fn(|context| {
            futures_io::AsyncWrite::poll_write(std::pin::Pin::new(&mut writer), context, b"data")
        })
        .await
        .unwrap();
        assert_eq!(count, 4);
        std::future::poll_fn(|context| {
            futures_io::AsyncWrite::poll_flush(std::pin::Pin::new(&mut writer), context)
        })
        .await
        .unwrap();
        writer.finish().await.unwrap();
    });
}

#[test]
fn tokio_compat_preserves_remote_stream_finalization() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let client = MockRemoteFs::connected();
    let path = MockRemoteFs::path("compat");
    runtime.block_on(async {
        use crate::fs::AsyncRemoteFs;

        let mut source = crate::mock::async_io::AsyncCursor::new(b"data".to_vec());
        AsyncRemoteFs::write_file(&client, &path, &WriteOptions::default(), &mut source)
            .await
            .unwrap();
    });
    let stream = runtime.block_on(async {
        use crate::fs::AsyncRemoteFs;

        AsyncRemoteFs::open(&client, &path, &ReadOptions::default())
            .await
            .unwrap()
    });
    let mut compat = stream.into_tokio();
    let mut output = Vec::new();
    runtime
        .block_on(async { tokio::io::AsyncReadExt::read_to_end(&mut compat, &mut output).await })
        .unwrap();
    assert_eq!(output, b"data");
    let stream = compat.into_inner();
    runtime.block_on(stream.finish()).unwrap();
    assert_eq!(client.finish_count(), 2);
}

#[test]
fn block_on_reports_nonseekable_streams() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let client = MockRemoteFs::connected();
    let path = MockRemoteFs::path("nonseekable");
    client.seed_file(&path, b"data");
    client.set_capabilities(crate::fs::Capabilities::STREAM_READ);
    let adapter = BlockOn::new(client, runtime.handle().clone());
    let mut stream = adapter.open(&path, &ReadOptions::default()).unwrap();
    assert!(!stream.seekable());
    assert_eq!(
        std::io::Seek::seek(&mut stream, std::io::SeekFrom::Start(0))
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::Unsupported
    );
    stream.finish().unwrap();
}

#[test]
fn block_on_preserves_stream_seeking() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let client = MockRemoteFs::connected();
    let path = MockRemoteFs::path("seekable");
    client.seed_file(&path, b"abcdef");
    client.set_capabilities(
        crate::fs::Capabilities::STREAM_READ | crate::fs::Capabilities::SEEK_READ,
    );
    let adapter = BlockOn::new(client, runtime.handle().clone());
    let mut stream = adapter.open(&path, &ReadOptions::default()).unwrap();
    assert_eq!(
        std::io::Seek::seek(&mut stream, std::io::SeekFrom::Start(2)).unwrap(),
        2
    );
    let mut output = Vec::new();
    std::io::Read::read_to_end(&mut stream, &mut output).unwrap();
    assert_eq!(output, b"cdef");
    stream.finish().unwrap();
}

#[test]
#[should_panic]
fn block_on_panics_inside_async_context() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let adapter = BlockOn::new(MockRemoteFs::connected(), runtime.handle().clone());
    runtime.block_on(async {
        let _ = adapter.capabilities();
        adapter.stat(&MockRemoteFs::path("missing")).unwrap();
    });
}
