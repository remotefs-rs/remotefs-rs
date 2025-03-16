use std::borrow::Cow;
use std::time::Duration;

use testcontainers::core::{ContainerPort, WaitFor};
use testcontainers::{Container, Image};

#[derive(Debug, Default, Clone)]
struct MinioImage;

impl Image for MinioImage {
    fn name(&self) -> &str {
        "minio/minio"
    }

    fn tag(&self) -> &str {
        "RELEASE.2022-02-07T08-17-33Z"
    }

    fn ready_conditions(&self) -> Vec<WaitFor> {
        vec![WaitFor::message_on_stdout("API:")]
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
    container: Container<MinioImage>,
}

impl Minio {
    pub fn start() -> Self {
        use testcontainers::runners::SyncRunner;
        let container = MinioImage::default()
            .start()
            .expect("Failed to start container");

        Self { container }
    }

    pub fn port(&self) -> u16 {
        std::thread::sleep(Duration::from_secs(5));
        self.container
            .get_host_port_ipv6(9000)
            .expect("Failed to get port")
    }
}
