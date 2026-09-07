# remotefs WebDAV

<p align="center">
  <a href="https://github.com/remotefs-rs/remotefs-rs-webdav/blob/main/CHANGELOG.md" target="_blank">Changelog</a>
  ·
  <a href="https://docs.rs/remotefs-webdav#get-started" target="_blank">Get started</a>
  ·
  <a href="https://docs.rs/remotefs-webdav" target="_blank">Documentation</a>
</p>

<p align="center">~ Remotefs WebDAV client ~</p>

<p align="center">Developed by <a href="https://veeso.me/" target="_blank">@veeso</a></p>

<p align="center">
  <a href="https://opensource.org/licenses/MIT"
    ><img
      src="https://img.shields.io/badge/License-MIT-teal.svg"
      alt="License-MIT"
  /></a>
  <a href="https://github.com/remotefs-rs/remotefs-rs-webdav/stargazers"
    ><img
      src="https://img.shields.io/github/stars/remotefs-rs/remotefs-rs-webdav.svg?style=plain"
      alt="Repo stars"
  /></a>
  <a href="https://crates.io/crates/remotefs-webdav"
    ><img
      src="https://img.shields.io/crates/d/remotefs-webdav.svg"
      alt="Downloads counter"
  /></a>
  <a href="https://crates.io/crates/remotefs-webdav"
    ><img
      src="https://img.shields.io/crates/v/remotefs-webdav.svg"
      alt="Latest version"
  /></a>
  <a href="https://ko-fi.com/veeso">
    <img
      src="https://img.shields.io/badge/donate-ko--fi-red"
      alt="Ko-fi"
  /></a>
</p>
<p align="center">
  <a href="https://github.com/remotefs-rs/remotefs-rs-webdav/actions"
    ><img
      src="https://github.com/remotefs-rs/remotefs-rs-webdav/workflows/CI/badge.svg"
      alt="CI"
  /></a>
  <a href="https://docs.rs/remotefs-webdav"
    ><img
      src="https://docs.rs/remotefs-webdav/badge.svg"
      alt="Docs"
  /></a>
</p>

---

## About remotefs-webdav ☁️

remotefs-webdav is a client implementation for [remotefs](https://github.com/remotefs-rs/remotefs-rs), providing support for the WebDAV protocol as specified in [RFC4918](https://www.rfc-editor.org/rfc/rfc4918).

---

## Get started 🚀

First of all, add `remotefs-webdav` to your project dependencies:

```toml
remotefs = "0.3"
remotefs-webdav = "0.2"
```

then connect to the server and use the client as any other remotefs client:

```rust
use std::path::Path;

use remotefs::RemoteFs;
use remotefs_webdav::WebDAVFs;

let mut client = WebDAVFs::new("alice", "secret1234", "http://localhost:3080");

client.connect().expect("connection failed");
client.change_dir(Path::new("/tmp")).expect("cd failed");
client.disconnect().expect("disconnection failed");
```

these features are supported:

- `find`: enable `find()` method on client (_enabled by default_)
- `no-log`: disable logging. By default, this library will log via the `log` crate.

---

### Client compatibility table ✔️

The following table states the compatibility for the client client and the remote file system trait method.

Note: `connect()`, `disconnect()` and `is_connected()` **MUST** always be supported, and are so omitted in the table.

| Client/Method  | webdav |
| -------------- | ------ |
| append_file    | No     |
| append         | No     |
| change_dir     | Yes    |
| copy           | No     |
| create_dir     | Yes    |
| create_file    | Yes    |
| create         | No     |
| exec           | No     |
| exists         | Yes    |
| list_dir       | Yes    |
| mov            | Yes    |
| open_file      | Yes    |
| open           | No     |
| pwd            | Yes    |
| remove_dir_all | Yes    |
| remove_dir     | Yes    |
| remove_file    | Yes    |
| setstat        | No     |
| stat           | Yes    |
| symlink        | No     |

---

## Contributing and issues 🤝🏻

Contributions, bug reports, new features, and questions are welcome! 😉
If you have any questions or concerns, or you want to suggest a new feature, or you want just want to improve remotefs, feel free to open an issue or a PR.

Please follow [our contributing guidelines](CONTRIBUTING.md) and read the [AI policy](AI_POLICY.md) before opening a pull request.

---

## Changelog ⏳

View remotefs' changelog [HERE](CHANGELOG.md)

---

## License 📃

remotefs-webdav is licensed under the MIT license.

You can read the entire license [HERE](LICENSE)
