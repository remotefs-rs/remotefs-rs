use std::borrow::Cow;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use testcontainers::core::{CmdWaitFor, ContainerPort, ExecCommand, WaitFor};
use testcontainers::{Container, Image, ImageExt as _};

static PROXY_JUMP_NETWORK_ID: AtomicUsize = AtomicUsize::new(0);

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

    pub fn start_with_tcp_forwarding() -> Self {
        let server = Self::start();
        server.enable_tcp_forwarding();
        server
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

    fn start_in_network(network: &str, container_name: Option<&str>) -> Self {
        use testcontainers::runners::SyncRunner;

        let request = OpensshServerImage::default().with_network(network);
        let container = match container_name {
            Some(container_name) => request.with_container_name(container_name).start(),
            None => request.start(),
        }
        .expect("Failed to start container in proxy jump network");

        Self { container }
    }

    pub fn port(&self) -> u16 {
        std::thread::sleep(Duration::from_secs(5));
        self.container
            .get_host_port_ipv4(2222)
            .expect("Failed to get port")
    }

    fn enable_tcp_forwarding(&self) {
        let command = ExecCommand::new([
            "sh",
            "-c",
            "sed -i 's/^AllowTcpForwarding no$/AllowTcpForwarding yes/' /config/sshd/sshd_config && kill -HUP $(pgrep -o sshd)",
        ])
        .with_cmd_ready_condition(CmdWaitFor::exit_code(0));
        let result = self
            .container
            .exec(command)
            .expect("Failed to enable TCP forwarding on jump server");
        assert_eq!(
            result.exit_code().expect("Failed to read setup exit code"),
            Some(0),
            "Could not enable TCP forwarding on jump server"
        );
    }
}

pub struct ProxyJumpServers {
    pub _target: OpensshServer,
    pub _second_jump: OpensshServer,
    pub first_jump: OpensshServer,
    pub second_jump_host: String,
    pub target_host: String,
}

impl ProxyJumpServers {
    pub fn start() -> Self {
        let id = PROXY_JUMP_NETWORK_ID.fetch_add(1, Ordering::Relaxed);
        let network = format!("remotefs-proxy-{}-{id}", std::process::id());
        let target_host = format!("remotefs-target-{}-{id}", std::process::id());
        let second_jump_host = format!("remotefs-jump-{}-{id}", std::process::id());
        let target = OpensshServer::start_in_network(&network, Some(&target_host));
        let second_jump = OpensshServer::start_in_network(&network, Some(&second_jump_host));
        second_jump.enable_tcp_forwarding();
        let first_jump = OpensshServer::start_in_network(&network, None);
        first_jump.enable_tcp_forwarding();

        Self {
            _target: target,
            _second_jump: second_jump,
            first_jump,
            second_jump_host,
            target_host,
        }
    }
}
