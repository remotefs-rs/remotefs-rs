# remotefs-kube

<p align="center">
  <a href="https://docs.rs/remotefs-kube" target="_blank">Documentation</a>
</p>

<p align="center">~ Remotefs kube client ~</p>

<p align="center">Developed by <a href="https://veeso.github.io/" target="_blank">@veeso</a></p>
<p align="center">Current version: 0.4.0 (29/09/2024)</p>

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
remotefs = "0.3"
remotefs-kube = "0.4"
```

these features are supported:

- `find`: enable `find()` method for RemoteFs. (_enabled by default_)
- `no-log`: disable logging. By default, this library will log via the `log` crate.

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
use std::sync::Arc;

use remotefs::RemoteFs;
use remotefs_kube::KubeMultiPodFs;

let runtime = Arc::new(
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("failed to build the Tokio runtime"),
);
let mut client = KubeMultiPodFs::new(&runtime);

// connect, using the default kubeconfig
client.connect().expect("connection failed");
// print the working directory
println!("wrkdir: {wrkdir}", wrkdir = client.pwd().expect("pwd failed").display());
// change the working directory to a container's `/tmp`
client
    .change_dir(Path::new("/my-pod/alpine/tmp"))
    .expect("cd failed");
// disconnect
client.disconnect().expect("disconnection failed");
```

### Kube container client

Here is a basic usage example, with the `KubeContainerFs` client, which is used to connect and interact with a single container on a certain pod. This client gives the entire access to the container file system.

```rust
use std::path::Path;
use std::sync::Arc;

use remotefs::RemoteFs;
use remotefs_kube::KubeContainerFs;

let runtime = Arc::new(
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("failed to build the Tokio runtime"),
);
let mut client = KubeContainerFs::new("my-pod", "container-name", &runtime);

// connect, using the default kubeconfig
client.connect().expect("connection failed");
// print the working directory
println!("wrkdir: {wrkdir}", wrkdir = client.pwd().expect("pwd failed").display());
// change the working directory
client.change_dir(Path::new("/tmp")).expect("cd failed");
// disconnect
client.disconnect().expect("disconnection failed");
```

---

### Client compatibility table ✔️

The following table states the compatibility for the client client and the remote file system trait method.

Note: `connect()`, `disconnect()` and `is_connected()` **MUST** always be supported, and are so omitted in the table.

| Client/Method  | Kube |
| -------------- | ---- |
| append_file    | No   |
| append         | No   |
| change_dir     | Yes  |
| copy           | Yes  |
| create_dir     | Yes  |
| create_file    | Yes  |
| create         | No   |
| exec           | Yes  |
| exists         | Yes  |
| list_dir       | Yes  |
| mov            | Yes  |
| open_file      | Yes  |
| open           | No   |
| pwd            | Yes  |
| remove_dir_all | Yes  |
| remove_dir     | Yes  |
| remove_file    | Yes  |
| setstat        | Yes  |
| stat           | Yes  |
| symlink        | Yes  |

---

---

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
