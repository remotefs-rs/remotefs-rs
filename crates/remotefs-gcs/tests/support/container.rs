#![cfg(feature = "with-containers")]

use testcontainers::core::wait::HttpWaitStrategy;
use testcontainers::core::{ContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};

const GCS_GRPC_PORT: ContainerPort = ContainerPort::Tcp(8888);
const GCS_HTTP_PORT: ContainerPort = ContainerPort::Tcp(9000);
const GCS_IMAGE: &str = "gcr.io/cloud-devrel-public-resources/storage-testbench";
const GCS_TAG: &str = concat!(
    "latest@sha256:",
    "600fa5c3cfc8be26435c38591cc094fb4ef648f760ffabf77f93237b1ebee027"
);
// The testbench assumes every multipart media part has a content type, while
// the Google Rust SDK omits that header for streamed parts.
const GCS_COMMAND: [&str; 2] = [
    "-c",
    concat!(
        "sed -i 's|headers\\[content_type_key\\]|headers.get(content_type_key, ",
        "b\"application/octet-stream\")|' ",
        "/opt/storage-testbench/testbench/common.py && ",
        "exec python3 testbench_run.py 0.0.0.0 9000 10"
    ),
];

#[derive(Debug)]
/// A running Google Cloud Storage testbench.
pub struct GcsContainer {
    container: ContainerAsync<GenericImage>,
}

impl GcsContainer {
    /// Starts a Google Cloud Storage testbench in Docker.
    ///
    /// # Panics
    ///
    /// Panics if Docker cannot start the container or the server port cannot
    /// be mapped.
    #[must_use]
    pub async fn start() -> Self {
        let wait_for = WaitFor::http(
            HttpWaitStrategy::new("/start_grpc?port=8888")
                .with_port(GCS_HTTP_PORT)
                .with_expected_status_code(200_u16),
        );
        let container = GenericImage::new(GCS_IMAGE, GCS_TAG)
            .with_exposed_port(GCS_HTTP_PORT)
            .with_exposed_port(GCS_GRPC_PORT)
            .with_wait_for(wait_for)
            .with_entrypoint("sh")
            .with_cmd(GCS_COMMAND)
            .start()
            .await
            .expect("failed to start GCS testbench");
        Self { container }
    }

    /// Returns the host endpoint for the running server.
    ///
    /// # Panics
    ///
    /// Panics if the exposed server port cannot be mapped.
    #[must_use]
    pub async fn endpoint(&self) -> String {
        let port = self
            .container
            .get_host_port_ipv4(GCS_GRPC_PORT)
            .await
            .expect("failed to map GCS testbench gRPC port");
        format!("http://127.0.0.1:{port}")
    }

    /// Returns the host endpoint for the HTTP object-data server.
    ///
    /// # Panics
    ///
    /// Panics if the exposed HTTP port cannot be mapped.
    #[must_use]
    pub async fn http_endpoint(&self) -> String {
        let port = self
            .container
            .get_host_port_ipv4(GCS_HTTP_PORT)
            .await
            .expect("failed to map GCS testbench HTTP port");
        format!("http://127.0.0.1:{port}")
    }

    /// Removes the container while an async runtime is active.
    #[cfg(feature = "tokio")]
    ///
    /// # Panics
    ///
    /// Panics if Docker cannot remove the testbench.
    pub async fn remove(self) {
        self.container
            .rm()
            .await
            .expect("failed to remove GCS testbench");
    }
}
