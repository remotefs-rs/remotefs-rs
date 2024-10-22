# remotefs-memory

<p align="center">
  <img src="https://raw.githubusercontent.com/remotefs-rs/remotefs-rs/main/assets/logo.png" alt="logo" width="256" height="256" />
</p>

<p align="center">~ A remotefs implementation for testing and simulation ~</p>

<p align="center">Developed by <a href="https://veeso.github.io/" target="_blank">@veeso</a></p>
<p align="center">Current version: 0.1.1</p>

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
      src="https://github.com/remotefs-rs/remotefs-rs-memory/workflows/linux/badge.svg"
      alt="Linux CI"
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
remotefs = "0.3"
remotefs-memory = "0.1"
```

## Example

```rust
use std::path::PathBuf;

use remotefs_memory::{Inode, MemoryFs, node, Node, Tree};
use remotefs::RemoteFs;
use remotefs::fs::{UnixPex, Metadata};

let tempdir = PathBuf::from("/tmp");
let tree = Tree::new(node!(
    PathBuf::from("/"),
    Inode::dir(0, 0, UnixPex::from(0o755)),
    node!(tempdir.clone(), Inode::dir(0, 0, UnixPex::from(0o755)))
));
let mut client = MemoryFs::new(tree);
assert!(client.connect().is_ok());
// Change directory
assert!(client.change_dir(tempdir.as_path()).is_ok());
```

## Changelog ⏳

View remotefs` changelog [HERE](CHANGELOG.md)

---

## License 📃

remotefs is licensed under the MIT license.

You can read the entire license [HERE](LICENSE)
