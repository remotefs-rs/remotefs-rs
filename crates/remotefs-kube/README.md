# remotefs-kube

<p align="center">
  <a href="https://docs.rs/remotefs-kube" target="_blank">Documentation</a>
</p>

<p align="center">~ Remotefs kube client ~</p>

<p align="center">Developed by <a href="https://veeso.github.io/" target="_blank">@veeso</a></p>
<p align="center">Current version: 0.1.0 (16/07/2024)</p>

<p align="center">
  <a href="https://opensource.org/licenses/MIT"
    ><img
      src="https://img.shields.io/badge/License-MIT-teal.svg"
      alt="License-MIT"
  /></a>
  <a href="https://github.com/veeso/remotefs-rs-kube/stargazers"
    ><img
      src="https://img.shields.io/github/stars/veeso/remotefs-rs-kube.svg?style=badge"
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

remotefs client implementation for Kube.

## Get started

First of all you need to add **remotefs** and the client to your project dependencies:

```toml
remotefs = "^0.2"
remotefs-kube = "^0.1"
```

these features are supported:

- `find`: enable `find()` method for RemoteFs. (*enabled by default*)
- `no-log`: disable logging. By default, this library will log via the `log` crate.

### Kube client

Here is a basic usage example, with the `Kube` client, which is very similiar to the `Scp` client.

```rust,ignore

// import remotefs trait and client
use remotefs::RemoteFs;
use remotefs_ssh::{SshConfigParseRule, SftpFs, SshOpts};
use std::path::Path;

let rt = Arc::new(
    tokio::runtime::Builder::new_current_thread()
    .enable_all()
    .build()
    .unwrap(),
);
let mut client: KubeFs = KubeFs::new("my-pod", &rt);

// connect
assert!(client.connect().is_ok());
// get working directory
println!("Wrkdir: {}", client.pwd().ok().unwrap().display());
// change working directory
assert!(client.change_dir(Path::new("/tmp")).is_ok());
// disconnect
assert!(client.disconnect().is_ok());
```

---

### Client compatibility table ✔️

The following table states the compatibility for the client client and the remote file system trait method.

Note: `connect()`, `disconnect()` and `is_connected()` **MUST** always be supported, and are so omitted in the table.

| Client/Method  | Kube |
|----------------|------|
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

## Changelog ⏳

View remotefs-kube's changelog [HERE](CHANGELOG.md)

---

## License 📃

remotefs-kube is licensed under the MIT license.

You can read the entire license [HERE](LICENSE)
