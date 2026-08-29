#![cfg(feature = "with-containers")]

mod support;

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use google_cloud_auth::credentials::anonymous::Builder as AnonymousBuilder;
use google_cloud_storage::client::StorageControl;
use google_cloud_storage::model::CreateBucketRequest;
use remotefs::RemoteFs;
use remotefs::fs::{Metadata, RemoteErrorType};
use remotefs_gcs::{GoogleCloudStorageCredentials, GoogleCloudStorageFs};
use support::container::GcsContainer;
use tokio::runtime::Runtime;

const READ_CHUNK_SIZE: usize = 256 * 1024;

fn unique_name(prefix: &str) -> String {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is before Unix epoch")
        .as_nanos();
    format!("{prefix}-{timestamp}")
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "covers the complete filesystem contract"
)]
#[ignore = "fake-gcs-server HTTP mode does not provide the gRPC transport used by StorageControl"]
fn fake_gcs_supports_the_filesystem_contract() {
    support::logger();
    let container = GcsContainer::start();
    let endpoint = container.endpoint();
    let bucket = unique_name("remotefs-gcs-test");
    let runtime = Arc::new(Runtime::new().expect("failed to create Tokio runtime"));

    let control = runtime
        .block_on(
            StorageControl::builder()
                .with_endpoint(endpoint.clone())
                .with_credentials(AnonymousBuilder::new().build())
                .build(),
        )
        .expect("failed to build control client");
    runtime
        .block_on(
            control
                .create_bucket()
                .with_request(CreateBucketRequest::new().set_parent("projects/_"))
                .set_bucket_id(bucket.clone())
                .send(),
        )
        .expect("failed to create fake GCS bucket");

    let mut client = GoogleCloudStorageFs::with_credentials(
        bucket,
        GoogleCloudStorageCredentials::anonymous(),
        &runtime,
    )
    .endpoint(endpoint);
    client.connect().expect("failed to connect filesystem");

    assert_eq!(client.pwd().unwrap(), Path::new("/"));
    assert!(client.stat(Path::new("/")).unwrap().is_dir());
    assert!(client.list_dir(Path::new("/")).unwrap().is_empty());

    client
        .create_dir(Path::new("docs"), remotefs::fs::UnixPex::from(0o755))
        .unwrap();
    assert_eq!(
        client.change_dir(Path::new("docs")).unwrap(),
        Path::new("/docs")
    );

    let small = b"hello";
    let small_metadata = Metadata {
        size: small.len() as u64,
        ..Metadata::default()
    };
    assert_eq!(
        client
            .create_file(
                Path::new("small.txt"),
                &small_metadata,
                Box::new(std::io::Cursor::new(small.to_vec())),
            )
            .unwrap(),
        small.len() as u64
    );

    let large = vec![7_u8; READ_CHUNK_SIZE + 13];
    let large_metadata = Metadata {
        size: large.len() as u64,
        ..Metadata::default()
    };
    client
        .create_file(
            Path::new("large.bin"),
            &large_metadata,
            Box::new(std::io::Cursor::new(large.clone())),
        )
        .unwrap();

    assert_eq!(
        client.stat(Path::new("small.txt")).unwrap().metadata().size,
        5
    );
    let entries = client.list_dir(Path::new(".")).unwrap();
    assert_eq!(entries.len(), 2);

    let mut output = tempfile::tempfile().unwrap();
    assert_eq!(
        client
            .open_file(
                Path::new("small.txt"),
                Box::new(output.try_clone().unwrap())
            )
            .unwrap(),
        small.len() as u64
    );
    output.seek(SeekFrom::Start(0)).unwrap();
    let mut downloaded = Vec::new();
    output.read_to_end(&mut downloaded).unwrap();
    assert_eq!(downloaded, small);

    client
        .copy(Path::new("small.txt"), Path::new("copied.txt"))
        .unwrap();
    client
        .mov(Path::new("copied.txt"), Path::new("moved.txt"))
        .unwrap();
    assert!(!client.exists(Path::new("copied.txt")).unwrap());
    assert!(client.exists(Path::new("moved.txt")).unwrap());

    assert_eq!(
        client.remove_dir(Path::new(".")).unwrap_err().kind,
        RemoteErrorType::DirectoryNotEmpty
    );
    client.remove_file(Path::new("large.bin")).unwrap();
    client.remove_dir_all(Path::new(".")).unwrap();
    assert!(!client.exists(Path::new("small.txt")).unwrap());
    assert!(!client.exists(Path::new("moved.txt")).unwrap());

    client.disconnect().unwrap();
    assert_eq!(
        client.pwd().unwrap_err().kind,
        RemoteErrorType::NotConnected
    );
}
