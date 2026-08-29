#![cfg(feature = "with-containers")]

mod support;

use std::collections::BTreeSet;
use std::io::{Cursor, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use remotefs::RemoteFs;
use remotefs::fs::{Metadata, RemoteErrorType, UnixPex};
use remotefs_gcs::GoogleCloudStorageFs;
use support::TestContext;

#[test]
fn connect_establishes_the_client_once() {
    let mut context = TestContext::new();

    assert!(!context.client.is_connected());
    context
        .client
        .connect()
        .expect("failed to connect filesystem");
    assert!(context.client.is_connected());
    assert_eq!(
        context.client.connect().unwrap_err().kind,
        RemoteErrorType::AlreadyConnected
    );
}

#[test]
fn disconnect_closes_the_client_once() {
    let mut context = TestContext::connected();

    context.client.disconnect().expect("failed to disconnect");
    assert!(!context.client.is_connected());
    assert_eq!(
        context.client.disconnect().unwrap_err().kind,
        RemoteErrorType::NotConnected
    );
}

#[test]
fn is_connected_tracks_connection_state() {
    let mut context = TestContext::new();

    assert!(!context.client.is_connected());
    context.client.connect().expect("failed to connect");
    assert!(context.client.is_connected());
    context.client.disconnect().expect("failed to disconnect");
    assert!(!context.client.is_connected());
}

#[test]
fn pwd_returns_the_root_working_directory() {
    let mut context = TestContext::connected();
    assert_eq!(context.client.pwd().unwrap(), Path::new("/"));
}

#[test]
fn change_dir_resolves_directories_and_rejects_files() {
    let mut context = TestContext::connected();
    context
        .client
        .create_dir(Path::new("docs"), UnixPex::from(0o755))
        .unwrap();
    upload(&mut context.client, Path::new("file.txt"), b"file");

    assert_eq!(
        context.client.change_dir(Path::new("docs")).unwrap(),
        Path::new("/docs")
    );
    assert_eq!(
        context
            .client
            .change_dir(Path::new("/file.txt"))
            .unwrap_err()
            .kind,
        RemoteErrorType::BadFile
    );
}

#[test]
fn list_dir_returns_only_direct_children() {
    let mut context = TestContext::connected();
    context
        .client
        .create_dir(Path::new("docs"), UnixPex::from(0o755))
        .unwrap();
    upload(&mut context.client, Path::new("root.txt"), b"root");
    upload(&mut context.client, Path::new("docs/nested.txt"), b"nested");

    let entries = context.client.list_dir(Path::new("/")).unwrap();
    let names: BTreeSet<_> = entries.iter().map(remotefs::fs::File::name).collect();
    assert_eq!(
        names,
        BTreeSet::from([String::from("docs"), String::from("root.txt")])
    );
}

#[test]
fn stat_reports_root_directory_file_and_missing_path() {
    let mut context = TestContext::connected();
    upload(&mut context.client, Path::new("file.bin"), b"bytes");

    assert!(context.client.stat(Path::new("/")).unwrap().is_dir());
    let file = context.client.stat(Path::new("file.bin")).unwrap();
    assert!(file.is_file());
    assert_eq!(file.metadata().size, 5);
    assert_eq!(
        context.client.stat(Path::new("missing")).unwrap_err().kind,
        RemoteErrorType::NoSuchFileOrDirectory
    );
}

#[test]
fn exists_distinguishes_present_and_missing_paths() {
    let mut context = TestContext::connected();
    upload(&mut context.client, Path::new("present.txt"), b"present");

    assert!(context.client.exists(Path::new("present.txt")).unwrap());
    assert!(!context.client.exists(Path::new("missing.txt")).unwrap());
}

#[cfg(feature = "find")]
#[test]
fn find_recurses_and_filters_with_wildcards() {
    let mut context = TestContext::connected();
    context
        .client
        .create_dir(Path::new("docs"), UnixPex::from(0o755))
        .unwrap();
    upload(&mut context.client, Path::new("root.txt"), b"root");
    upload(&mut context.client, Path::new("docs/nested.txt"), b"nested");
    upload(&mut context.client, Path::new("docs/image.bin"), b"image");

    let paths: BTreeSet<_> = context
        .client
        .find("*.txt")
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

#[test]
fn create_dir_creates_a_directory_and_rejects_duplicates() {
    let mut context = TestContext::connected();

    context
        .client
        .create_dir(Path::new("docs"), UnixPex::from(0o755))
        .unwrap();
    assert!(context.client.stat(Path::new("docs")).unwrap().is_dir());
    assert_eq!(
        context
            .client
            .create_dir(Path::new("docs"), UnixPex::from(0o755))
            .unwrap_err()
            .kind,
        RemoteErrorType::DirectoryAlreadyExists
    );
}

#[test]
fn remove_file_deletes_only_files() {
    let mut context = TestContext::connected();
    upload(&mut context.client, Path::new("file.txt"), b"file");
    context
        .client
        .create_dir(Path::new("docs"), UnixPex::from(0o755))
        .unwrap();

    context.client.remove_file(Path::new("file.txt")).unwrap();
    assert!(!context.client.exists(Path::new("file.txt")).unwrap());
    assert_eq!(
        context
            .client
            .remove_file(Path::new("docs"))
            .unwrap_err()
            .kind,
        RemoteErrorType::BadFile
    );
}

#[test]
fn remove_dir_deletes_empty_directories_and_rejects_non_empty_ones() {
    let mut context = TestContext::connected();
    context
        .client
        .create_dir(Path::new("empty"), UnixPex::from(0o755))
        .unwrap();
    context
        .client
        .create_dir(Path::new("full"), UnixPex::from(0o755))
        .unwrap();
    upload(&mut context.client, Path::new("full/file.txt"), b"file");

    context.client.remove_dir(Path::new("empty")).unwrap();
    assert!(!context.client.exists(Path::new("empty")).unwrap());
    assert_eq!(
        context
            .client
            .remove_dir(Path::new("full"))
            .unwrap_err()
            .kind,
        RemoteErrorType::DirectoryNotEmpty
    );
}

#[test]
fn remove_dir_all_deletes_nested_content_and_the_marker() {
    let mut context = TestContext::connected();
    context
        .client
        .create_dir(Path::new("tree"), UnixPex::from(0o755))
        .unwrap();
    context
        .client
        .create_dir(Path::new("tree/nested"), UnixPex::from(0o755))
        .unwrap();
    upload(&mut context.client, Path::new("tree/root.txt"), b"root");
    upload(
        &mut context.client,
        Path::new("tree/nested/leaf.txt"),
        b"leaf",
    );

    context.client.remove_dir_all(Path::new("tree")).unwrap();
    assert!(!context.client.exists(Path::new("tree")).unwrap());
    assert!(
        !context
            .client
            .exists(Path::new("tree/nested/leaf.txt"))
            .unwrap()
    );
}

#[test]
fn create_file_uploads_small_and_multi_chunk_objects() {
    let mut context = TestContext::connected();
    let large = vec![7_u8; READ_CHUNK_SIZE + 13];

    upload(&mut context.client, Path::new("small.txt"), b"small");
    upload(&mut context.client, Path::new("large.bin"), &large);
    assert_eq!(
        context
            .client
            .stat(Path::new("small.txt"))
            .unwrap()
            .metadata()
            .size,
        5
    );
    assert_eq!(
        context
            .client
            .stat(Path::new("large.bin"))
            .unwrap()
            .metadata()
            .size,
        large.len() as u64
    );
}

#[test]
fn open_file_downloads_exact_multi_chunk_contents() {
    let mut context = TestContext::connected();
    let expected = vec![9_u8; READ_CHUNK_SIZE + 13];
    upload(&mut context.client, Path::new("large.bin"), &expected);

    assert_eq!(
        download(&mut context.client, Path::new("large.bin")),
        expected
    );
}

#[test]
fn copy_duplicates_a_file_and_preserves_the_source() {
    let mut context = TestContext::connected();
    upload(&mut context.client, Path::new("source.txt"), b"source");

    context
        .client
        .copy(Path::new("source.txt"), Path::new("copy.txt"))
        .unwrap();
    assert_eq!(
        download(&mut context.client, Path::new("source.txt")).as_slice(),
        b"source"
    );
    assert_eq!(
        download(&mut context.client, Path::new("copy.txt")).as_slice(),
        b"source"
    );
}

#[test]
fn mov_transfers_a_file_and_removes_the_source() {
    let mut context = TestContext::connected();
    upload(&mut context.client, Path::new("source.txt"), b"source");

    context
        .client
        .mov(Path::new("source.txt"), Path::new("moved.txt"))
        .unwrap();
    assert!(!context.client.exists(Path::new("source.txt")).unwrap());
    assert_eq!(
        download(&mut context.client, Path::new("moved.txt")).as_slice(),
        b"source"
    );
}

#[test]
fn setstat_reports_unsupported_feature() {
    let mut context = TestContext::connected();
    assert_eq!(
        context
            .client
            .setstat(Path::new("file"), Metadata::default())
            .unwrap_err()
            .kind,
        RemoteErrorType::UnsupportedFeature
    );
}

#[test]
fn symlink_reports_unsupported_feature() {
    let mut context = TestContext::connected();
    assert_eq!(
        context
            .client
            .symlink(Path::new("link"), Path::new("target"))
            .unwrap_err()
            .kind,
        RemoteErrorType::UnsupportedFeature
    );
}

#[test]
fn exec_reports_unsupported_feature() {
    let mut context = TestContext::connected();
    assert_eq!(
        context.client.exec("true").unwrap_err().kind,
        RemoteErrorType::UnsupportedFeature
    );
}

#[test]
fn append_reports_unsupported_feature() {
    let mut context = TestContext::connected();
    assert_eq!(
        context
            .client
            .append(Path::new("file"), &Metadata::default())
            .err()
            .expect("append unexpectedly returned a stream")
            .kind,
        RemoteErrorType::UnsupportedFeature
    );
}

#[test]
fn append_file_reports_unsupported_feature() {
    let mut context = TestContext::connected();
    assert_eq!(
        context
            .client
            .append_file(
                Path::new("file"),
                &metadata(b"bytes"),
                Box::new(Cursor::new(b"bytes".to_vec())),
            )
            .unwrap_err()
            .kind,
        RemoteErrorType::UnsupportedFeature
    );
}

#[test]
fn create_reports_unsupported_feature() {
    let mut context = TestContext::connected();
    assert_eq!(
        context
            .client
            .create(Path::new("file"), &Metadata::default())
            .err()
            .expect("create unexpectedly returned a stream")
            .kind,
        RemoteErrorType::UnsupportedFeature
    );
}

#[test]
fn open_reports_unsupported_feature() {
    let mut context = TestContext::connected();
    assert_eq!(
        context
            .client
            .open(Path::new("file"))
            .err()
            .expect("open unexpectedly returned a stream")
            .kind,
        RemoteErrorType::UnsupportedFeature
    );
}

fn metadata(bytes: &[u8]) -> Metadata {
    Metadata {
        size: bytes.len() as u64,
        ..Metadata::default()
    }
}

fn upload(client: &mut GoogleCloudStorageFs, path: &Path, bytes: &[u8]) {
    assert_eq!(
        client
            .create_file(
                path,
                &metadata(bytes),
                Box::new(Cursor::new(bytes.to_vec())),
            )
            .expect("failed to upload test object"),
        bytes.len() as u64
    );
}

fn download(client: &mut GoogleCloudStorageFs, path: &Path) -> Vec<u8> {
    let mut output = tempfile::tempfile().expect("failed to create output file");
    client
        .open_file(
            path,
            Box::new(output.try_clone().expect("failed to clone output file")),
        )
        .expect("failed to download test object");
    output
        .seek(SeekFrom::Start(0))
        .expect("failed to rewind output file");
    let mut bytes = Vec::new();
    output
        .read_to_end(&mut bytes)
        .expect("failed to read downloaded object");
    bytes
}

const READ_CHUNK_SIZE: usize = 256 * 1024;
