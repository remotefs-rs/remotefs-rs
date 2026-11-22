# remotefs-memory

<p align="center">
  <img src="https://raw.githubusercontent.com/remotefs-rs/remotefs-rs/main/assets/logo.png" alt="logo" width="256" height="256" />
</p>

<p align="center">~ A remotefs implementation for testing and simulation ~</p>

<p align="center">Developed by <a href="https://veeso.github.io/" target="_blank">@veeso</a></p>
<p align="center">Current version: 1.0.0</p>

<p align="center">
  <a href="https://opensource.org/licenses/MIT"
    ><img
      src="https://img.shields.io/badge/License-MIT-teal.svg"
      alt="License-MIT"
  /></a>
  <a href="https://github.com/remotefs-rs/remotefs-rs-memory/stargazers"
    ><img
      src="https://img.shields.io/github/stars/remotefs-rs/remotefs-rs-memory.svg?style=badge"
      alt="Repo stars"
  /></a>
  <a href="https://crates.io/crates/remotefs-memory"
    ><img
      src="https://img.shields.io/crates/d/remotefs-memory.svg"
      alt="Downloads counter"
  /></a>
  <a href="https://crates.io/crates/remotefs-memory"
    ><img
      src="https://img.shields.io/crates/v/remotefs-memory.svg"
      alt="Latest version"
  /></a>
  <a href="https://ko-fi.com/veeso">
    <img
      src="https://img.shields.io/badge/donate-ko--fi-red"
      alt="Ko-fi"
  /></a>
</p>
<p align="center">
  <a href="https://github.com/remotefs-rs/remotefs-rs-memory/actions"
    ><img
      src="https://github.com/remotefs-rs/remotefs-rs-memory/workflows/CI/badge.svg"
      alt="CI"
  /></a>
  <a href="https://docs.rs/remotefs-memory"
    ><img
      src="https://docs.rs/remotefs-memory/badge.svg"
      alt="Docs"
  /></a>
</p>

---

## Getting Started

Add `remotefs-memory` to your `Cargo.toml`:

```toml
remotefs = "1"
remotefs-memory = "1"
```

`MemoryFs` implements the blocking `remotefs::RemoteFs` trait from remotefs 1.
Every path is absolute and the tree root is `/`; there is no working
directory. Transfers return owned streams that must be finished. Async
consumers can wrap the client in `remotefs::adapters::r#async::Unblock`
(remotefs `tokio` feature).

## Example

```rust
use std::io::Write;
use std::path::{Path, PathBuf};

use remotefs_memory::{Inode, MemoryFs, node, Node, Tree};
use remotefs::RemoteFs;
use remotefs::fs::{ReadOptions, UnixPex, WriteOptions};

let tempdir = PathBuf::from("/tmp");
let tree = Tree::new(node!(
    PathBuf::from("/"),
    Inode::dir(0, 0, UnixPex::from(0o755)),
    node!(tempdir.clone(), Inode::dir(0, 0, UnixPex::from(0o755)))
));
let mut client = MemoryFs::new(tree);
client.connect().unwrap();

let mut stream = client
    .create(Path::new("/tmp/hello.txt"), &WriteOptions::default())
    .unwrap();
stream.write_all(b"hello").unwrap();
stream.finish().unwrap();

let mut output = Vec::new();
client
    .read_file(
        Path::new("/tmp/hello.txt"),
        &ReadOptions::default(),
        &mut output,
    )
    .unwrap();
assert_eq!(output, b"hello");
```

## Contributing 🤝

Contributions, bug reports, new features, and questions are welcome! 😉
If you have any questions or concerns, or you want to suggest a new feature, or you want just want to improve remotefs, feel free to open an issue or a PR.

Please read the [AI policy](AI_POLICY.md) before opening a pull request.

---

## Changelog ⏳

View remotefs` changelog [HERE](CHANGELOG.md)

---

## License 📃

remotefs is licensed under the MIT license.

You can read the entire license [HERE](LICENSE)
