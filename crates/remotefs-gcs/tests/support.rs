#![cfg(feature = "with-containers")]

use std::sync::Arc;

use google_cloud_auth::credentials::anonymous::Builder as AnonymousBuilder;
use google_cloud_storage::client::StorageControl;
use google_cloud_storage::model::CreateBucketRequest;
use remotefs::RemoteFs;
use remotefs_gcs::{GoogleCloudStorageCredentials, GoogleCloudStorageFs};
use tokio::runtime::Runtime;

#[path = "support/container.rs"]
pub mod container;

use self::container::GcsContainer;

const TEST_BUCKET: &str = "remotefs-gcs-test";

#[derive(Debug)]
pub struct TestContext {
    pub client: GoogleCloudStorageFs,
    #[expect(dead_code, reason = "keeps the testbench alive for the test")]
    container: GcsContainer,
}

impl TestContext {
    /// Creates an unconnected test context with an isolated bucket.
    ///
    /// # Panics
    ///
    /// Panics if Docker, the Tokio runtime, the testbench client, or the
    /// bucket cannot be initialized.
    #[must_use]
    pub fn new() -> Self {
        logger();
        let container = GcsContainer::start();
        let control_endpoint = container.endpoint();
        let storage_endpoint = container.http_endpoint();
        let runtime = Arc::new(Runtime::new().expect("failed to create integration-test runtime"));
        let control = runtime
            .block_on(
                StorageControl::builder()
                    .with_endpoint(control_endpoint.clone())
                    .with_credentials(AnonymousBuilder::new().build())
                    .build(),
            )
            .expect("failed to build testbench control client");
        runtime
            .block_on(
                control
                    .create_bucket()
                    .with_request(CreateBucketRequest::new().set_parent("projects/test-project"))
                    .set_bucket_id(TEST_BUCKET)
                    .send(),
            )
            .expect("failed to create testbench bucket");
        let client = GoogleCloudStorageFs::with_credentials(
            TEST_BUCKET,
            GoogleCloudStorageCredentials::anonymous(),
            &runtime,
        )
        .endpoint(storage_endpoint)
        .control_endpoint(control_endpoint);

        Self { client, container }
    }

    /// Creates a test context with a connected filesystem client.
    ///
    /// # Panics
    ///
    /// Panics if [`Self::new`] cannot initialize the context or the client
    /// cannot connect to the testbench.
    #[must_use]
    pub fn connected() -> Self {
        let mut context = Self::new();
        context
            .client
            .connect()
            .expect("failed to connect filesystem");
        context
    }
}

impl Default for TestContext {
    fn default() -> Self {
        Self::new()
    }
}

pub fn logger() {
    use std::sync::Once;

    static INIT: Once = Once::new();

    INIT.call_once(|| {
        let _ = env_logger::builder()
            .filter_level(log::LevelFilter::Trace)
            .is_test(true)
            .format_line_number(true)
            .try_init();
    });
}
