# remotefs WebDAV

<p align="center">
  <a href="https://veeso.github.io/remotefs-webdav/blob/main/CHANGELOG.md" target="_blank">Changelog</a>
  ·
  <a href="https://veeso.github.io/remotefs-webdav/#get-started" target="_blank">Get started</a>
  ·
  <a href="https://docs.rs/remotefs-webdav" target="_blank">Documentation</a>
</p>

<p align="center">~ Remotefs WebDAV client ~</p>

<p align="center">Developed by <a href="https://veeso.github.io/" target="_blank">@veeso</a></p>
<p align="center">Current version: 0.1.0 (02/03/2024)</p>

<p align="center">
  <a href="https://opensource.org/licenses/MIT"
    ><img
      src="https://img.shields.io/badge/License-MIT-teal.svg"
      alt="License-MIT"
  /></a>
  <a href="https://github.com/veeso/remotefs-rs-webdav/stargazers"
    ><img
      src="https://img.shields.io/github/stars/veeso/remotefs-rs-webdav.svg"
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
  <a href="https://github.com/veeso/remotefs-rs-webdav/actions"
    ><img
      src="https://github.com/veeso/remotefs-rs-webdav/workflows/build-test/badge.svg"
      alt="Linux CI"
  /></a>
  <a href="https://docs.rs/remotefs-webdav"
    ><img
      src="https://docs.rs/remotefs-webdav/badge.svg"
      alt="Docs"
  /></a>
</p>

---

## About remotefs-webdav ☁️

remotefs-webdav is a client implementation for [remotefs](https://github.com/veeso/remotefs-rs), providing support for the Aws S3 protocol.

---

## Get started 🚀

First of all, add `remotefs-webdav` to your project dependencies:

```toml
remotefs-webdav = "^0.2.0"
```

these features are supported:

- `find`: enable `find()` method on client (*enabled by default*)
- `no-log`: disable logging. By default, this library will log via the `log` crate.

---

### Client compatibility table ✔️

The following table states the compatibility for the client client and the remote file system trait method.

Note: `connect()`, `disconnect()` and `is_connected()` **MUST** always be supported, and are so omitted in the table.

| Client/Method  | webdav |
|----------------|--------|
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

## Support the developer ☕

If you like remotefs-webdav and you're grateful for the work I've done, please consider a little donation 🥳

You can make a donation with one of these platforms:

[![ko-fi](https://img.shields.io/badge/Ko--fi-F16061?style=for-the-badge&logo=ko-fi&logoColor=white)](https://ko-fi.com/veeso)
[![PayPal](https://img.shields.io/badge/PayPal-00457C?style=for-the-badge&logo=paypal&logoColor=white)](https://www.paypal.me/chrisintin)

---

## Contributing and issues 🤝🏻

Contributions, bug reports, new features, and questions are welcome! 😉
If you have any questions or concerns, or you want to suggest a new feature, or you want just want to improve remotefs, feel free to open an issue or a PR.

Please follow [our contributing guidelines](CONTRIBUTING.md)

---

## Changelog ⏳

View remotefs' changelog [HERE](CHANGELOG.md)

---

## License 📃

remotefs-webdav is licensed under the MIT license.

You can read the entire license [HERE](LICENSE)
