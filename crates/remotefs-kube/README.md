# remotefs-kube

<p align="center">
  <a href="https://docs.rs/remotefs-kube" target="_blank">Documentation</a>
</p>

<p align="center">~ Remotefs kube client ~</p>

<p align="center">Developed by <a href="https://veeso.github.io/" target="_blank">@veeso</a></p>
<p align="center">Current version: 1.0.0 (12/09/2026)</p>

<p align="center">
  <a href="https://opensource.org/licenses/MIT"
    ><img
      src="https://img.shields.io/badge/License-MIT-teal.svg"
      alt="License-MIT"
  /></a>
  <a href="https://github.com/remotefs-rs/remotefs-rs-kube/stargazers"
    ><img
      src="https://img.shields.io/github/stars/remotefs-rs/remotefs-rs-kube.svg?style=badge"
      alt="Repo stars"
  /></a>
  <a href="https://crates.io/crates/remotefs-kube"
    ><img
      src="https://img.shields.io/crates/d/remotefs-kube.svg"
      alt="Downloads counter"
  /></a>
  <a href="https://crates.io/crates/remotefs-kube"
    ><img
      src="https://img.shields.io/crates/v/remotefs-kube.svg"
      alt="Latest version"
  /></a>
  <a href="https://ko-fi.com/veeso">
    <img
      src="https://img.shields.io/badge/donate-ko--fi-red"
      alt="Ko-fi"
  /></a>
</p>

---

## About remotefs-kube ☁️

remotefs-kube is a client implementation for [remotefs](https://github.com/remotefs-rs/remotefs-rs), giving shell-level access to the containers of a Kubernetes pod.

## Get started

First of all you need to add **remotefs** and **remotefs-kube** to your project dependencies:

```toml
[dependencies]
remotefs = { version = "1", features = ["async"] }
remotefs-kube = "1"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

these features are supported:

- `find`: enable `remotefs::find_async` for the clients (_enabled by default_)
- `no-log`: disable logging. By default, this library logs via the `log` crate.
- `tokio`: enable `into_blocking`, wrapping a client for blocking
  `remotefs::RemoteFs` callers.

The library provides two different clients:

- **KubeMultiPodFs** client
- **KubeContainerFs** client

### Kube multi pod client

The MultiPod client gives access to all the pods with their own containers in a namespace.

This client creates an abstract file system with the following structure

- / (root)
  - pod-a
    - container-a
      - / (container-a root)
        - /bin
        - /home
        - ...
    - container-b
      - / (container-b root)
        - ...
  - pod-b
    - container-c
      - / (container-c root)
        - ...

So paths have the following structure: `/pod-name/container-name/path/to/file`.

```rust
use std::path::Path;

use remotefs::AsyncRemoteFs;
use remotefs_kube::KubeMultiPodFs;

#[tokio::main]
async fn main() -> remotefs::RemoteResult<()> {
    let mut client = KubeMultiPodFs::new();

    // connect, using the default kubeconfig
    client.connect().await?;
    for pod in client.list_dir(Path::new("/")).await? {
        println!("pod: {}", pod.name());
    }
    let _files = client.list_dir(Path::new("/my-pod/alpine/tmp")).await?;
    // disconnect
    client.disconnect().await?;
    Ok(())
}
```

### Kube container client

Here is a basic usage example with the `KubeContainerFs` client, which
connects to and interacts with a single container on a pod.

```rust
use std::path::Path;

use remotefs::AsyncRemoteFs;
use remotefs::fs::{ReadOptions, WriteOptions};
use remotefs_kube::KubeContainerFs;

#[tokio::main]
async fn main() -> remotefs::RemoteResult<()> {
    let mut client = KubeContainerFs::new("my-pod", "container-name");

    // connect, using the default kubeconfig
    client.connect().await?;
    let mut source = futures::io::Cursor::new(b"hello".to_vec());
    client
        .write_file(
            Path::new("/tmp/hello.txt"),
            &WriteOptions::default().size_hint(5),
            &mut source,
        )
        .await?;
    let mut destination = futures::io::Cursor::new(Vec::new());
    client
        .read_file(
            Path::new("/tmp/hello.txt"),
            &ReadOptions::default().offset(1).length(3),
            &mut destination,
        )
        .await?;
    assert_eq!(destination.into_inner(), b"ell");
    // disconnect
    client.disconnect().await?;
    Ok(())
}
```

---

### Blocking usage

Enable the `tokio` feature and call `into_blocking` to get a blocking wrapper
that implements `remotefs::RemoteFs`:

```rust
use remotefs::RemoteFs;
use remotefs_kube::KubeContainerFs;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let runtime = tokio::runtime::Runtime::new()?;
    let mut client: Box<dyn RemoteFs> = Box::new(
        KubeContainerFs::new("my-pod", "container-name").into_blocking(runtime.handle().clone()),
    );
    client.connect()?;
    client.disconnect()?;
    Ok(())
}
```

### Client compatibility table ✔️

The following table states the compatibility for each client and remote file
system trait method.

Note: `connect()`, `disconnect()` and `is_connected()` **MUST** always be
supported, and are omitted from the table.

| Client/Method  | KubeContainerFs | KubeMultiPodFs       |
| -------------- | --------------- | -------------------- |
| append_file    | Yes             | Yes                  |
| append         | Yes             | Yes                  |
| copy           | Yes             | Yes (same container) |
| create_dir     | Yes             | Yes                  |
| create         | Yes             | Yes                  |
| exec           | Yes             | No                   |
| exists         | Yes             | Yes                  |
| list_dir       | Yes             | Yes                  |
| open           | Yes             | Yes                  |
| read_file      | Yes             | Yes                  |
| remove_dir_all | Yes             | Yes                  |
| remove_dir     | Yes             | Yes                  |
| remove_file    | Yes             | Yes                  |
| rename         | Yes             | Yes (same container) |
| set_metadata   | Yes             | Yes                  |
| stat           | Yes             | Yes                  |
| symlink        | Yes             | Yes (same container) |
| write_file     | Yes             | Yes                  |

### Migrating from 0.4

See the [remotefs migration guide](https://github.com/remotefs-rs/remotefs-rs/blob/main/MIGRATION.md)
for upstream API changes and the [1.0.0 changelog](CHANGELOG.md) for this
client's migration details.

## Contributing 🤝

Contributions, bug reports, new features, and questions are welcome! 😉
If you have any questions or concerns, or you want to suggest a new feature, or you want just want to improve remotefs, feel free to open an issue or a PR.

Please read the [AI policy](AI_POLICY.md) before opening a pull request.

---

## Changelog ⏳

View remotefs-kube's changelog [HERE](CHANGELOG.md)

---

## License 📃

remotefs-kube is licensed under the MIT license.

You can read the entire license [HERE](LICENSE)
