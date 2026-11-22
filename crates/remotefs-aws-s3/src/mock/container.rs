use std::borrow::Cow;
use std::time::Duration;

use testcontainers::core::{ContainerPort, WaitFor};
use testcontainers::{ContainerAsync, Image, ImageExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// Access key configured on the RustFS test container.
pub const ACCESS_KEY: &str = "rustfsadmin";
/// Secret key configured on the RustFS test container.
pub const SECRET_KEY: &str = "rustfsadmin";

/// S3 API port exposed by RustFS inside the container.
const S3_PORT: u16 = 9000;
/// Upper bound on the time RustFS may take to answer its health endpoint.
const READY_TIMEOUT: Duration = Duration::from_secs(60);
/// Delay between two health probes while waiting for RustFS.
const READY_POLL_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Debug, Clone)]
struct RustFsImage;

impl Image for RustFsImage {
    fn name(&self) -> &str {
        "rustfs/rustfs"
    }

    fn tag(&self) -> &str {
        "1.0.4"
    }

    fn ready_conditions(&self) -> Vec<WaitFor> {
        // The entrypoint prints this line right before exec'ing the server.
        // RustFS writes its own logs to a file, so actual readiness is
        // checked by polling the health endpoint in `RustFs::start`.
        vec![WaitFor::message_on_stdout("Starting:")]
    }

    fn expose_ports(&self) -> &[ContainerPort] {
        &[ContainerPort::Tcp(S3_PORT)]
    }

    fn env_vars(
        &self,
    ) -> impl IntoIterator<Item = (impl Into<Cow<'_, str>>, impl Into<Cow<'_, str>>)> {
        vec![
            ("RUSTFS_ACCESS_KEY", ACCESS_KEY),
            ("RUSTFS_SECRET_KEY", SECRET_KEY),
        ]
    }
}

/// RustFS S3-compatible server running in a test container.
pub struct RustFs {
    /// Held so the container stays up until the test context is dropped.
    _container: ContainerAsync<RustFsImage>,
    port: u16,
}

impl RustFs {
    /// Starts the container and waits until the S3 API answers its health check.
    pub async fn start() -> Self {
        use testcontainers::runners::AsyncRunner;
        let container = RustFsImage
            .with_mapped_port(0, ContainerPort::Tcp(S3_PORT))
            .start()
            .await
            .expect("Failed to start container");
        let port = container
            .get_host_port_ipv4(S3_PORT)
            .await
            .expect("Failed to get port");

        tokio::time::timeout(READY_TIMEOUT, wait_healthy(port))
            .await
            .expect("RustFS did not become healthy in time");

        Self {
            _container: container,
            port,
        }
    }

    /// Returns the host port mapped to the S3 API.
    pub fn port(&self) -> u16 {
        self.port
    }
}

/// Polls `GET /health` on the given host port until it returns `200`.
async fn wait_healthy(port: u16) {
    loop {
        if probe_health(port).await {
            return;
        }
        tokio::time::sleep(READY_POLL_INTERVAL).await;
    }
}

/// Sends a single HTTP/1.0 health request and reports whether it returned `200`.
async fn probe_health(port: u16) -> bool {
    let Ok(mut stream) = TcpStream::connect(("127.0.0.1", port)).await else {
        return false;
    };
    let request = b"GET /health HTTP/1.0\r\nHost: localhost\r\n\r\n";
    if stream.write_all(request).await.is_err() {
        return false;
    }
    let mut response = Vec::new();
    if stream.read_to_end(&mut response).await.is_err() {
        return false;
    }
    response.starts_with(b"HTTP/1.0 200") || response.starts_with(b"HTTP/1.1 200")
}
