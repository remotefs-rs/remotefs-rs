# remotefs FTP

<p align="center">
  <a href="https://veeso.github.io/remotefs-ftp/blob/main/CHANGELOG.md" target="_blank">Changelog</a>
  ·
  <a href="https://veeso.github.io/remotefs-ftp/#get-started" target="_blank">Get started</a>
  ·
  <a href="https://docs.rs/remotefs-ftp" target="_blank">Documentation</a>
</p>

<p align="center">~ Remotefs FTP client ~</p>

<p align="center">Developed by <a href="https://veeso.me/" target="_blank">@veeso_dev</a></p>
<p align="center">Current version: 0.4.0 (18/01/2026)</p>

<p align="center">
  <a href="https://opensource.org/licenses/MIT"
    ><img
      src="https://img.shields.io/badge/License-MIT-teal.svg"
      alt="License-MIT"
  /></a>
  <a href="https://github.com/remotefs-rs/remotefs-rs-ftp/stargazers"
    ><img
      src="https://img.shields.io/github/stars/remotefs-rs/remotefs-rs-ftp.svg?style=badge"
      alt="Repo stars"
  /></a>
  <a href="https://crates.io/crates/remotefs-ftp"
    ><img
      src="https://img.shields.io/crates/d/remotefs-ftp.svg"
      alt="Downloads counter"
  /></a>
  <a href="https://crates.io/crates/remotefs-ftp"
    ><img
      src="https://img.shields.io/crates/v/remotefs-ftp.svg"
      alt="Latest version"
  /></a>
  <a href="https://ko-fi.com/veeso">
    <img
      src="https://img.shields.io/badge/donate-ko--fi-red"
      alt="Ko-fi"
  /></a>
</p>
<p align="center">
  <a href="https://github.com/remotefs-rs/remotefs-rs-ftp/actions"
    ><img
      src="https://github.com/remotefs-rs/remotefs-rs-ftp/workflows/Linux/badge.svg"
      alt="Linux CI"
  /></a>
  <a href="https://coveralls.io/github/remotefs-rs/remotefs-rs-ftp"
    ><img
      src="https://coveralls.io/repos/github/remotefs-rs/remotefs-rs-ftp/badge.svg"
      alt="Coveralls"
  /></a>
  <a href="https://docs.rs/remotefs-ftp"
    ><img
      src="https://docs.rs/remotefs-ftp/badge.svg"
      alt="Docs"
  /></a>
</p>

---

## About remotefs-ftp ☁️

remotefs-ftp is a client implementation for [remotefs](https://github.com/remotefs-rs/remotefs-rs), providing support
for the FTP/FTPS protocols.

---

## Get started 🚀

First of all, add `remotefs-ftp` to your project dependencies:

```toml
remotefs = "0.3"
remotefs-ftp = "0.4"
```

## Features

these features are supported:

- `find`: enable `find()` method on client (_enabled by default_)
- `native-tls`: enable FTPS support using native-tls as backend
- `native-tls-vendored`: enable static link for native-tls
- `no-log`: disable logging. By default, this library will log via the `log` crate.
- `rustls-aws-lc-rs`: enable FTPS support using rustls with aws-lc-rs as backend
- `rustls-ring`: enable FTPS support using rustls with ring as backend

---

### Client compatibility table ✔️

The following table states the compatibility for the client client and the remote file system trait method.

Note: `connect()`, `disconnect()` and `is_connected()` **MUST** always be supported, and are so omitted in the table.

| Client/Method  | Ftp |
| -------------- | --- |
| append_file    | Yes |
| append         | Yes |
| change_dir     | Yes |
| copy           | No  |
| create_dir     | Yes |
| create_file    | Yes |
| create         | Yes |
| exec           | Yes |
| exists         | Yes |
| list_dir       | Yes |
| mov            | Yes |
| open_file      | Yes |
| open           | Yes |
| pwd            | Yes |
| remove_dir_all | Yes |
| remove_dir     | Yes |
| remove_file    | Yes |
| setstat        | No  |
| stat           | Yes |
| symlink        | No  |

> `exec` is implemented via the FTP `SITE` command, so it only runs commands the server exposes as `SITE` subcommands (e.g. `CHMOD`, `HELP`), not arbitrary shell/console commands.

---

## Support the developer ☕

If you like remotefs-ftp and you're grateful for the work I've done, please consider a little donation 🥳

You can make a donation with one of these platforms:

[![ko-fi](https://img.shields.io/badge/Ko--fi-F16061?style=for-the-badge&logo=ko-fi&logoColor=white)](https://ko-fi.com/veeso)
[![PayPal](https://img.shields.io/badge/PayPal-00457C?style=for-the-badge&logo=paypal&logoColor=white)](https://www.paypal.me/chrisintin)

---

## Contributing and issues 🤝🏻

Contributions, bug reports, new features, and questions are welcome! 😉
If you have any questions or concerns, or you want to suggest a new feature, or you want just want to improve remotefs,
feel free to open an issue or a PR.

Please follow [our contributing guidelines](CONTRIBUTING.md)

---

## Changelog ⏳

View remotefs' changelog [HERE](CHANGELOG.md)

---

## Powered by 💪

remotefs-ftp is powered by these aweseome projects:

- [suppaftp](https://github.com/veeso/suppaftp)

---

## License 📃

remotefs-ftp is licensed under the MIT license.

You can read the entire license [HERE](LICENSE)
