use std::borrow::Cow;
use std::time::Duration;

use testcontainers::core::{ContainerPort, WaitFor};
use testcontainers::{ContainerAsync, Image, ImageExt};

#[derive(Debug, Clone)]
struct MinioImage;

impl Image for MinioImage {
    fn name(&self) -> &str {
        "quay.io/minio/minio"
    }

    fn tag(&self) -> &str {
        "RELEASE.2025-09-07T16-13-09Z"
    }

    fn ready_conditions(&self) -> Vec<WaitFor> {
        vec![WaitFor::message_on_stderr("API:")]
    }

    fn expose_ports(&self) -> &[ContainerPort] {
        &[ContainerPort::Tcp(9000), ContainerPort::Tcp(9001)]
    }

    fn cmd(&self) -> impl IntoIterator<Item = impl Into<Cow<'_, str>>> {
        ["server", "/data"]
    }

    fn env_vars(
        &self,
    ) -> impl IntoIterator<Item = (impl Into<Cow<'_, str>>, impl Into<Cow<'_, str>>)> {
        vec![("MINIO_CONSOLE_ADDRESS", ":9001")]
    }
}

pub struct Minio {
    container: ContainerAsync<MinioImage>,
}

impl Minio {
    pub async fn start() -> Self {
        use testcontainers::runners::AsyncRunner;
        let container = MinioImage
            .with_mapped_port(0, ContainerPort::Tcp(9000))
            .start()
            .await
            .expect("Failed to start container");

        Self { container }
    }

    pub async fn port(&self) -> u16 {
        tokio::time::sleep(Duration::from_secs(5)).await;
        self.container
            .get_host_port_ipv6(9000)
            .await
            .expect("Failed to get port")
    }
}
