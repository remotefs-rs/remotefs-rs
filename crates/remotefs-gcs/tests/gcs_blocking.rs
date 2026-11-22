#![cfg(all(feature = "with-containers", feature = "tokio"))]

mod support;

use std::io::Cursor;
use std::path::Path;

use pretty_assertions::assert_eq;
use remotefs::RemoteFs;
use remotefs::fs::{ReadOptions, RemoteErrorType, WriteOptions};
use support::TestContext;
use support::container::GcsContainer;
use tokio::runtime::Runtime;

struct BlockingContext {
    client: Box<dyn RemoteFs>,
    container: Option<GcsContainer>,
    runtime: Runtime,
}

impl Drop for BlockingContext {
    fn drop(&mut self) {
        if let Some(container) = self.container.take() {
            self.runtime.block_on(container.remove());
        }
    }
}

fn connected() -> BlockingContext {
    support::logger();
    let runtime = Runtime::new().expect("failed to create runtime");
    let container = runtime.block_on(GcsContainer::start());
    let (client, _) = runtime.block_on(TestContext::client_for(&container));
    let mut client: Box<dyn RemoteFs> = Box::new(client.into_blocking(runtime.handle().clone()));
    client.connect().expect("failed to connect");
    BlockingContext {
        client,
        container: Some(container),
        runtime,
    }
}

#[test]
fn blocking_round_trip_writes_lists_and_reads() {
    let context = connected();
    assert!(context.client.is_connected());
    context.client.create_dir(Path::new("/docs"), None).unwrap();
    let mut source = Cursor::new(b"hello".to_vec());
    let written = context
        .client
        .write_file(
            Path::new("/docs/hello.txt"),
            &WriteOptions::default().size_hint(5),
            &mut source,
        )
        .unwrap();
    assert_eq!(written, 5);
    let names: Vec<_> = context
        .client
        .list_dir(Path::new("/docs"))
        .unwrap()
        .iter()
        .map(remotefs::File::name)
        .collect();
    assert_eq!(names, vec![String::from("hello.txt")]);
    let mut output = Vec::new();
    context
        .client
        .read_file(
            Path::new("/docs/hello.txt"),
            &ReadOptions::default().offset(1).length(3),
            &mut output,
        )
        .unwrap();
    assert_eq!(output, b"ell");
}

#[test]
fn blocking_streams_finish_explicitly() {
    let context = connected();
    let mut writer = context
        .client
        .create(Path::new("/stream.txt"), &WriteOptions::default())
        .unwrap();
    std::io::Write::write_all(&mut writer, b"streamed").unwrap();
    std::io::Write::flush(&mut writer).unwrap();
    writer.finish().unwrap();
    let mut reader = context
        .client
        .open(Path::new("/stream.txt"), &ReadOptions::default())
        .unwrap();
    let mut output = Vec::new();
    std::io::Read::read_to_end(&mut reader, &mut output).unwrap();
    reader.finish().unwrap();
    assert_eq!(output, b"streamed");
    assert_eq!(
        context
            .client
            .stat(Path::new("/missing"))
            .unwrap_err()
            .kind(),
        RemoteErrorType::NoSuchFileOrDirectory
    );
}
