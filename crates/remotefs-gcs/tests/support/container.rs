#![cfg(feature = "with-containers")]

use testcontainers::core::wait::HttpWaitStrategy;
use testcontainers::core::{ContainerPort, WaitFor};
use testcontainers::runners::SyncRunner;
use testcontainers::{Container, GenericImage, ImageExt};

const GCS_IMAGE: &str = "fsouza/fake-gcs-server";
const GCS_TAG: &str = "1.54.0";
const GCS_PORT: ContainerPort = ContainerPort::Tcp(4443);

#[derive(Debug)]
/// A running fake Google Cloud Storage server.
pub struct GcsContainer {
    container: Container<GenericImage>,
}

impl GcsContainer {
    /// Starts a fake Google Cloud Storage server in Docker.
    ///
    /// # Panics
    ///
    /// Panics if Docker cannot start the container or the server port cannot
    /// be mapped.
    #[must_use]
    pub fn start() -> Self {
        let wait_for = WaitFor::http(
            HttpWaitStrategy::new("/_internal/healthcheck")
                .with_port(GCS_PORT)
                .with_expected_status_code(200_u16),
        );
        let container = GenericImage::new(GCS_IMAGE, GCS_TAG)
            .with_exposed_port(GCS_PORT)
            .with_wait_for(wait_for)
            .with_cmd(["-scheme", "http"])
            .start()
            .expect("failed to start fake GCS server");
        Self { container }
    }

    /// Returns the host endpoint for the running server.
    ///
    /// # Panics
    ///
    /// Panics if the exposed server port cannot be mapped.
    #[must_use]
    pub fn endpoint(&self) -> String {
        let port = self
            .container
            .get_host_port_ipv4(GCS_PORT)
            .expect("failed to map fake GCS server port");
        format!("http://127.0.0.1:{port}")
    }
}
