use std::borrow::Cow;
use std::time::Duration;

use testcontainers::core::{ContainerPort, WaitFor};
use testcontainers::{Container, Image};

#[derive(Debug, Clone)]
struct OpensshServerImage {
    public_key: String,
}

impl Default for OpensshServerImage {
    fn default() -> Self {
        Self {
            public_key: "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABAQDErJhQxEI0+VvhlXVUyh+vMCm7aXfCA/g633AG8ezD/5EylwchtAr2JCoBWnxn4zV8nI9dMqOgm0jO4IsXpKOjQojv+0VOH7I+cDlBg0tk4hFlvyyS6YviDAfDDln3jYUM+5QNDfQLaZlH2WvcJ3mkDxLVlI9MBX1BAeSmChLxwAvxALp2ncImNQLzDO9eHcig3dtMrEKkzXQowRW5Y7eUzg2+vvVq4H2DOjWwUndvB5sJkhEfTUVE7ID8ZdGJo60kUb/02dZYj+IbkAnMCsqktk0cg/4XFX82hEfRYFeb1arkysFisPU1DOb6QielL/axeTebVplaouYcXY0pFdJt root@8c50fd4c345a".to_string(),
        }
    }
}

impl Image for OpensshServerImage {
    fn name(&self) -> &str {
        "ghcr.io/linuxserver/openssh-server"
    }

    fn tag(&self) -> &str {
        // Use a modern OpenSSH (>= 8.8) which rejects the legacy ssh-rsa (SHA-1)
        // signature algorithm by default. This ensures the test suite exercises
        // rsa-sha2-256/512 negotiation, matching what real users run.
        "10.2_p1-r0-ls226"
    }

    fn ready_conditions(&self) -> Vec<WaitFor> {
        vec![WaitFor::message_on_stdout("done.")]
    }

    fn expose_ports(&self) -> &[ContainerPort] {
        &[ContainerPort::Tcp(2222)]
    }

    fn env_vars(
        &self,
    ) -> impl IntoIterator<Item = (impl Into<Cow<'_, str>>, impl Into<Cow<'_, str>>)> {
        vec![
            ("PUID", "1000"),
            ("PGID", "1000"),
            ("TZ", "Europe/London"),
            ("SUDO_ACCESS", "false"),
            ("PASSWORD_ACCESS", "true"),
            ("PUBLIC_KEY", self.public_key.as_str()),
            ("USER_PASSWORD", "password"),
            ("USER_NAME", "sftp"),
        ]
    }
}

pub struct OpensshServer {
    container: Container<OpensshServerImage>,
}

impl OpensshServer {
    pub fn start() -> Self {
        Self::start_with_image(OpensshServerImage::default())
    }

    pub fn start_with_public_key(public_key: &str) -> Self {
        Self::start_with_image(OpensshServerImage {
            public_key: public_key.to_string(),
        })
    }

    fn start_with_image(image: OpensshServerImage) -> Self {
        use testcontainers::runners::SyncRunner;
        let container = image.start().expect("Failed to start container");

        Self { container }
    }

    pub fn port(&self) -> u16 {
        std::thread::sleep(Duration::from_secs(5));
        self.container
            .get_host_port_ipv4(2222)
            .expect("Failed to get port")
    }
}
