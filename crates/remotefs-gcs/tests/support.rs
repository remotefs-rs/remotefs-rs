#![cfg(feature = "with-containers")]

use std::path::Path;

use google_cloud_auth::credentials::anonymous::Builder as AnonymousBuilder;
use google_cloud_storage::client::StorageControl;
use google_cloud_storage::model::CreateBucketRequest;
use remotefs::AsyncRemoteFs;
use remotefs::fs::{ReadOptions, WriteOptions};
use remotefs_gcs::{GoogleCloudStorageCredentials, GoogleCloudStorageFs};

#[path = "support/container.rs"]
pub mod container;

use self::container::GcsContainer;

pub const TEST_BUCKET: &str = "remotefs-gcs-test";

#[derive(Debug)]
pub struct TestContext {
    pub client: GoogleCloudStorageFs,
    #[expect(dead_code, reason = "keeps the testbench alive for the test")]
    container: GcsContainer,
}

impl TestContext {
    /// Starts a testbench, creates the bucket, and returns an unconnected client.
    ///
    /// # Panics
    ///
    /// Panics if Docker, the testbench, or the test bucket cannot be started.
    pub async fn new() -> Self {
        logger();
        let container = GcsContainer::start().await;
        let (client, _) = Self::client_for(&container).await;
        let context = Self { client, container };
        let _ = &context.client;
        context
    }

    /// Like [`Self::new`] but with the client already connected.
    ///
    /// # Panics
    ///
    /// Panics if the testbench cannot be started or the client cannot connect.
    pub async fn connected() -> Self {
        let mut context = Self::new().await;
        context
            .client
            .connect()
            .await
            .expect("failed to connect filesystem");
        context
    }

    /// Builds the test bucket and an unconnected client against a running
    /// testbench. Returns the client plus its gRPC endpoint.
    ///
    /// # Panics
    ///
    /// Panics if the testbench endpoints, control client, or bucket cannot be
    /// initialized.
    pub async fn client_for(container: &GcsContainer) -> (GoogleCloudStorageFs, String) {
        let control_endpoint = container.endpoint().await;
        let storage_endpoint = container.http_endpoint().await;
        let control = StorageControl::builder()
            .with_endpoint(control_endpoint.clone())
            .with_credentials(AnonymousBuilder::new().build())
            .build()
            .await
            .expect("failed to build testbench control client");
        control
            .create_bucket()
            .with_request(CreateBucketRequest::new().set_parent("projects/test-project"))
            .set_bucket_id(TEST_BUCKET)
            .send()
            .await
            .expect("failed to create testbench bucket");
        let client = GoogleCloudStorageFs::with_credentials(
            TEST_BUCKET,
            GoogleCloudStorageCredentials::anonymous(),
        )
        .endpoint(storage_endpoint)
        .control_endpoint(control_endpoint.clone());
        (client, control_endpoint)
    }
}

/// Uploads bytes to a test object.
///
/// # Panics
///
/// Panics if the upload fails or reports a different byte count.
pub async fn upload(client: &GoogleCloudStorageFs, path: &Path, bytes: &[u8]) {
    let mut source = futures::io::Cursor::new(bytes.to_vec());
    let written = client
        .write_file(
            path,
            &WriteOptions::default().size_hint(bytes.len() as u64),
            &mut source,
        )
        .await
        .expect("failed to upload test object");
    assert_eq!(written, bytes.len() as u64);
}

/// Downloads a test object into memory.
///
/// # Panics
///
/// Panics if the download fails.
pub async fn download(client: &GoogleCloudStorageFs, path: &Path) -> Vec<u8> {
    let mut destination = futures::io::Cursor::new(Vec::new());
    client
        .read_file(path, &ReadOptions::default(), &mut destination)
        .await
        .expect("failed to download test object");
    destination.into_inner()
}

pub fn logger() {
    use std::sync::Once;

    static INIT: Once = Once::new();

    let _ = TestContext::new;
    let _ = TestContext::connected;
    let _ = upload;
    let _ = download;
    #[cfg(feature = "tokio")]
    let _ = GcsContainer::remove;

    INIT.call_once(|| {
        let _ = env_logger::builder()
            .filter_level(log::LevelFilter::Trace)
            .is_test(true)
            .format_line_number(true)
            .try_init();
    });
}
