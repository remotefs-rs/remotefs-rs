# remotefs Aws S3

<p align="center">
  <a href="https://veeso.github.io/remotefs-aws-s3/blob/main/CHANGELOG.md" target="_blank">Changelog</a>
  ·
  <a href="https://veeso.github.io/remotefs-aws-s3/#get-started" target="_blank">Get started</a>
  ·
  <a href="https://docs.rs/remotefs-aws-s3" target="_blank">Documentation</a>
</p>

<p align="center">~ Remotefs Aws S3 client ~</p>

<p align="center">Developed by <a href="https://veeso.github.io/" target="_blank">@veeso</a></p>
<p align="center">Current version: 0.1.0 (29/12/2021)</p>

<p align="center">
  <a href="https://opensource.org/licenses/MIT"
    ><img
      src="https://img.shields.io/badge/License-MIT-teal.svg"
      alt="License-MIT"
  /></a>
  <a href="https://github.com/veeso/remotefs-rs-aws-s3/stargazers"
    ><img
      src="https://img.shields.io/github/stars/veeso/remotefs-rs-aws-s3.svg"
      alt="Repo stars"
  /></a>
  <a href="https://crates.io/crates/remotefs-aws-s3"
    ><img
      src="https://img.shields.io/crates/d/remotefs-aws-s3.svg"
      alt="Downloads counter"
  /></a>
  <a href="https://crates.io/crates/remotefs-aws-s3"
    ><img
      src="https://img.shields.io/crates/v/remotefs-aws-s3.svg"
      alt="Latest version"
  /></a>
  <a href="https://ko-fi.com/veeso">
    <img
      src="https://img.shields.io/badge/donate-ko--fi-red"
      alt="Ko-fi"
  /></a>
</p>
<p align="center">
  <a href="https://github.com/veeso/remotefs-rs-aws-s3/actions"
    ><img
      src="https://github.com/veeso/remotefs-rs-aws-s3/workflows/Linux/badge.svg"
      alt="Linux CI"
  /></a>
  <a href="https://github.com/veeso/remotefs-rs-aws-s3/actions"
    ><img
      src="https://github.com/veeso/remotefs-rs-aws-s3/workflows/MacOS/badge.svg"
      alt="MacOS CI"
  /></a>
  <a href="https://github.com/veeso/remotefs-rs-aws-s3/actions"
    ><img
      src="https://github.com/veeso/remotefs-rs-aws-s3/workflows/Windows/badge.svg"
      alt="Windows CI"
  /></a>
  <a href="https://coveralls.io/github/veeso/remotefs-rs-aws-s3"
    ><img
      src="https://coveralls.io/repos/github/veeso/remotefs-rs-aws-s3/badge.svg"
      alt="Coveralls"
  /></a>
  <a href="https://docs.rs/remotefs-aws-s3"
    ><img
      src="https://docs.rs/remotefs-aws-s3/badge.svg"
      alt="Docs"
  /></a>
</p>

---

## About remotefs-aws-s3 ☁️

remotefs-aws-s3 is a client implementation for [remotefs](https://github.com/veeso/remotefs-rs), providing support for the Aws S3 protocol.

---

## Get started 🚀

First of all, add `remotefs-aws-s3` to your project dependencies:

```toml
remotefs-aws-s3 = "^0.1.0"
```

these features are supported:

- `no-log`: disable logging. By default, this library will log via the `log` crate.

---

### Client compatibility table ✔️

The following table states the compatibility for the client client and the remote file system trait method.

Note: `connect()`, `disconnect()` and `is_connected()` **MUST** always be supported, and are so omitted in the table.

| Client/Method  | Aws-S3 |
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
| mov            | No     |
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

If you like remotefs-aws-s3 and you're grateful for the work I've done, please consider a little donation 🥳

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

## Powered by 💪

remotefs-aws-s3 is powered by these aweseome projects:

- [rust-s3](https://github.com/durch/rust-s3)

---

## License 📃

remotefs-aws-s3 is licensed under the MIT license.

You can read the entire license [HERE](LICENSE)
