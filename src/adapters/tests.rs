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

#[test]
fn unblock_block_on_round_trip_keeps_the_contract() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let blocking = BlockOn::new(MockRemoteFs::new(), runtime.handle().clone());
    runtime.block_on(async {
        use crate::r#async::Unblock;
        use crate::fs::AsyncRemoteFs;

        let mut fs = Unblock::new(blocking);
        AsyncRemoteFs::connect(&mut fs).await.unwrap();
        let path = MockRemoteFs::path("round-trip");
        let mut input = crate::mock::async_io::AsyncCursor::new(b"round trip".to_vec());
        AsyncRemoteFs::write_file(&fs, &path, &WriteOptions::default(), &mut input)
            .await
            .unwrap();
        let mut output = crate::mock::async_io::AsyncCursor::new(Vec::new());
        assert_eq!(
            AsyncRemoteFs::read_file(&fs, &path, &ReadOptions::default(), &mut output)
                .await
                .unwrap(),
            10
        );
        assert_eq!(output.into_inner(), b"round trip");
        AsyncRemoteFs::disconnect(&mut fs).await.unwrap();
        assert!(!AsyncRemoteFs::is_connected(&fs));
    });
}

#[test]
fn unblock_streams_offload_io_and_finish_once() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let client = MockRemoteFs::connected();
    let path = MockRemoteFs::path("stream-round-trip");
    client.seed_file(&path, b"stream data");

    runtime.block_on(async {
        use crate::r#async::Unblock;
        use crate::fs::AsyncRemoteFs;

        let fs = Unblock::new(client);
        let mut reader = AsyncRemoteFs::open(&fs, &path, &ReadOptions::default())
            .await
            .unwrap();
        let mut output = crate::mock::async_io::AsyncCursor::new(Vec::new());
        assert_eq!(crate::io::copy(&mut reader, &mut output).await.unwrap(), 11);
        crate::io::flush(&mut output).await.unwrap();
        reader.finish().await.unwrap();
        assert_eq!(output.into_inner(), b"stream data");

        let mut writer = AsyncRemoteFs::create(&fs, &path, &WriteOptions::default())
            .await
            .unwrap();
        let mut input = crate::mock::async_io::AsyncCursor::new(b"new data".to_vec());
        assert_eq!(crate::io::copy(&mut input, &mut writer).await.unwrap(), 8);
        crate::io::flush(&mut writer).await.unwrap();
        writer.finish().await.unwrap();

        let mut output = crate::mock::async_io::AsyncCursor::new(Vec::new());
        assert_eq!(
            AsyncRemoteFs::read_file(&fs, &path, &ReadOptions::default(), &mut output)
                .await
                .unwrap(),
            8
        );
        assert_eq!(output.into_inner(), b"new data");
    });
}

#[test]
fn unblock_streams_preserve_seek_and_current_position() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let client = MockRemoteFs::connected();
    let path = MockRemoteFs::path("seek-round-trip");
    client.seed_file(&path, b"abcdef");

    runtime.block_on(async {
        use std::io::SeekFrom;
        use std::pin::Pin;

        use crate::r#async::Unblock;
        use crate::fs::AsyncRemoteFs;

        let fs = Unblock::new(client);
        let mut reader = AsyncRemoteFs::open(&fs, &path, &ReadOptions::default())
            .await
            .unwrap();
        let position = std::future::poll_fn(|context| {
            futures_io::AsyncSeek::poll_seek(Pin::new(&mut reader), context, SeekFrom::Start(2))
        })
        .await
        .unwrap();
        assert_eq!(position, 2);
        let mut output = crate::mock::async_io::AsyncCursor::new(Vec::new());
        crate::io::copy(&mut reader, &mut output).await.unwrap();
        reader.finish().await.unwrap();
        assert_eq!(output.into_inner(), b"cdef");

        let mut writer = AsyncRemoteFs::create(&fs, &path, &WriteOptions::default())
            .await
            .unwrap();
        let _ = futures_io::AsyncWrite::poll_write(
            Pin::new(&mut writer),
            &mut std::task::Context::from_waker(std::task::Waker::noop()),
            b"abc",
        );
        let position = std::future::poll_fn(|context| {
            futures_io::AsyncSeek::poll_seek(Pin::new(&mut writer), context, SeekFrom::Start(1))
        })
        .await
        .unwrap();
        assert_eq!(position, 1);
        std::future::poll_fn(|context| {
            futures_io::AsyncWrite::poll_write(Pin::new(&mut writer), context, b"X")
        })
        .await
        .unwrap();
        crate::io::flush(&mut writer).await.unwrap();
        writer.finish().await.unwrap();

        let mut output = crate::mock::async_io::AsyncCursor::new(Vec::new());
        AsyncRemoteFs::read_file(&fs, &path, &ReadOptions::default(), &mut output)
            .await
            .unwrap();
        assert_eq!(output.into_inner(), b"aXc");
    });
}

#[test]
fn block_on_unblock_can_run_while_the_owner_drives_tokio() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let done = Arc::new(AtomicBool::new(false));
    let done_from_worker = done.clone();
    let path = MockRemoteFs::path("reverse-round-trip");
    let handle = runtime.handle().clone();
    let worker = std::thread::spawn(move || {
        let adapter = BlockOn::new(
            crate::r#async::Unblock::new(MockRemoteFs::connected()),
            handle,
        );
        let mut input = std::io::Cursor::new(b"reverse".to_vec());
        adapter
            .write_file(&path, &WriteOptions::default(), &mut input)
            .unwrap();
        let mut output = Vec::new();
        assert_eq!(
            adapter
                .read_file(&path, &ReadOptions::default(), &mut output)
                .unwrap(),
            7
        );
        assert_eq!(output, b"reverse");
        done_from_worker.store(true, Ordering::Release);
    });

    runtime.block_on(async {
        while !done.load(Ordering::Acquire) {
            tokio::task::yield_now().await;
        }
    });
    worker.join().unwrap();
}
