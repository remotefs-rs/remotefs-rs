# remotefs

<p align="center">
  <a href="https://veeso.github.io/remotefs/blob/main/CHANGELOG.md" target="_blank">Changelog</a>
  ·
  <a href="https://veeso.github.io/remotefs/#get-started" target="_blank">Get started</a>
  ·
  <a href="https://docs.rs/remotefs" target="_blank">Documentation</a>
</p>

<p align="center">~ The Omni Filetransfer Client Library (and more!) ~</p>

<p align="center">Developed by <a href="https://veeso.github.io/" target="_blank">@veeso</a></p>
<p align="center">Current version: 0.3.0 (04/01/2022)</p>

<p align="center">
  <a href="https://opensource.org/licenses/MIT"
    ><img
      src="https://img.shields.io/badge/License-MIT-teal.svg"
      alt="License-MIT"
  /></a>
  <a href="https://github.com/veeso/remotefs-rs/stargazers"
    ><img
      src="https://img.shields.io/github/stars/veeso/remotefs-rs.svg"
      alt="Repo stars"
  /></a>
  <a href="https://crates.io/crates/remotefs"
    ><img
      src="https://img.shields.io/crates/d/remotefs.svg"
      alt="Downloads counter"
  /></a>
  <a href="https://crates.io/crates/remotefs"
    ><img
      src="https://img.shields.io/crates/v/remotefs.svg"
      alt="Latest version"
  /></a>
  <a href="https://ko-fi.com/veeso">
    <img
      src="https://img.shields.io/badge/donate-ko--fi-red"
      alt="Ko-fi"
  /></a>
</p>
<p align="center">
  <a href="https://github.com/veeso/remotefs-rs/actions"
    ><img
      src="https://github.com/veeso/remotefs-rs/workflows/Linux/badge.svg"
      alt="Linux CI"
  /></a>
  <a href="https://github.com/veeso/remotefs-rs/actions"
    ><img
      src="https://github.com/veeso/remotefs-rs/workflows/MacOS/badge.svg"
      alt="MacOS CI"
  /></a>
  <a href="https://github.com/veeso/remotefs-rs/actions"
    ><img
      src="https://github.com/veeso/remotefs-rs/workflows/Windows/badge.svg"
      alt="Windows CI"
  /></a>
  <a href="https://coveralls.io/github/veeso/remotefs-rs"
    ><img
      src="https://coveralls.io/repos/github/veeso/remotefs-rs/badge.svg"
      alt="Coveralls"
  /></a>
  <a href="https://docs.rs/remotefs"
    ><img
      src="https://docs.rs/remotefs/badge.svg"
      alt="Docs"
  /></a>
</p>

---

## About remotefs ☁️

remotefs is a library that provides a file system structure to work with all the most popular file transfer protocols.
This is achieved through a trait called `RemoteFs` which exposes methods to operate on the remote file system.
Currently the library exposes a client for **Sftp**, **Scp**, **Ftp** and **Aws-s3** as external libraries.

### Why remotefs ❓

You might be wondering, "why remotefs?"
The first and foremost reason is to provide a generic interface over multiple protocols.
For example, in [termscp](https://github.com/veeso/termscp) it allows the support of multiple protocols without any protocol-specific code.
The second reason is that often, users just want a simple way to operate on a remote file system, however, they don't have the time to spend researching the ins and outs of each protocol.

Using remotefs, this is no longer a problem: all you need is to configure the options to your liking, then you're ready to connect.

---

## Features 🎁

- 📁 Different communication protocols
  - **SFTP**
  - **SCP**
  - **FTP** and **FTPS**
  - **Aws S3**
- ✔️ Configurable: use only the client that you need
- 🤖 Extensible: adding new protocols is easy
- 🚀 Simple: easy to setup
- 😄 Understandable: no need to understand the underlying protocol

---

## Get started 🚀

First, add `remotefs` to your list of dependencies:

```toml
remotefs = "^0.3.0"
```

select over a variety of features:

- `find`: enable the `find()` method on `RemoteFs` trait (_enabled by default_)
- `no-log`: disable logging. By default, the `log` crate is used.

### Client libraries 🔌

To use an existing client, you must add them to your `Cargo.toml`, along with remotefs:

- [aws-s3](https://github.com/veeso/remotefs-rs-aws-s3)

  ```toml
  remotefs-aws-s3 = "^0.1.0"
  ```

- [ftp](https://github.com/veeso/remotefs-rs-ftp)

  ```toml
  remotefs-ftp = { version = "^0.1.0", features = [ "secure" ] }
  ```

- [ssh](https://github.com/veeso/remotefs-rs-ssh)

  ```toml
  remotefs-ssh = "^0.1.0"
  ```

---

## Remote file system 💾

As mentioned earlier, this library exposes a trait called `RemoteFs`.
This trait exposes several methods to operate on a remote file system via the chosen client.

Let's briefly go over which methods are available:

- **connect**: connect to the remote host.
- **disconnect**: disconnect from the remote host.
- **is_connected**: returns whether the client is connected to the remote host.
- **append_file**: append specified buffer to the specified file.
- **append**: open a file for append and returns a stream to write to it.
- **change_dir**: change the working directory to provided path.
- **copy**: copy a file from the specified source path to the specified destination.
- **create_dir**: create a directory with the specified file mode at the specified path.
- **create_file**: create a file at a specified path with the specified content.
- **create**: create a file and returns a stream to write to it.
- **exec**: executes a shell command.
- **exists**: checks whether file at specified path exists.
- **list_dir**: get entries at the provided path.
- **mov**: move a file from the specified source path to the specified destination.
- **open_file**: open a file for reading and fill the specified buffer with the file content.
- **open**: open a file and returns a stream to read it.
- **pwd**: get working directory.
- **remove_dir_all**: remove file/directory and all of its content.
- **remove_dir**: remove directory at the specified path. It fails if it is not an empty directory.
- **remove_file**: remove file at the specified path. It fails if it is not a file.
- **setstat**: set file metadata for file at the specified path.
- **stat**: get file information of file at the specified path.
- **symlink**: create a symlink at the specified path, pointing to the specified file.

### Client compatibility table ✔️

The following table states the compatibility for each client associated with the remote file system trait method.

Note: `connect()`, `disconnect()` and `is_connected()` **MUST** always be supported, and are so omitted in the table.

| Client/Method  | Aws-S3 | Ftp | Scp | Sftp |
| -------------- | ------ | --- | --- | ---- |
| append_file    | No     | Yes | No  | Yes  |
| append         | No     | Yes | No  | Yes  |
| change_dir     | Yes    | Yes | Yes | Yes  |
| copy           | No     | No  | Yes | Yes  |
| create_dir     | Yes    | Yes | Yes | Yes  |
| create_file    | Yes    | Yes | Yes | Yes  |
| create         | No     | Yes | Yes | Yes  |
| exec           | No     | No  | Yes | Yes  |
| exists         | Yes    | Yes | Yes | Yes  |
| list_dir       | Yes    | Yes | Yes | Yes  |
| mov            | No     | Yes | Yes | Yes  |
| open_file      | Yes    | Yes | Yes | Yes  |
| open           | No     | Yes | Yes | Yes  |
| pwd            | Yes    | Yes | Yes | Yes  |
| remove_dir_all | Yes    | Yes | Yes | Yes  |
| remove_dir     | Yes    | Yes | Yes | Yes  |
| remove_file    | Yes    | Yes | Yes | Yes  |
| setstat        | No     | No  | Yes | Yes  |
| stat           | Yes    | Yes | Yes | Yes  |
| symlink        | No     | No  | Yes | Yes  |

---

## Support the developer ☕

If you like remotefs and you're grateful for the work I've done, please consider a little donation 🥳

You can make a donation on the following platforms:

[![ko-fi](https://img.shields.io/badge/Ko--fi-F16061?style=for-the-badge&logo=ko-fi&logoColor=white)](https://ko-fi.com/veeso)
[![PayPal](https://img.shields.io/badge/PayPal-00457C?style=for-the-badge&logo=paypal&logoColor=white)](https://www.paypal.me/chrisintin)

---

## Apps using remotefs 🚀

- [termscp](https://github.com/veeso/termscp)

---

## Contributing and issues 🤝🏻

Contributions, bug reports, new features, and questions are welcome! 😉
If you have any questions or concerns, or you want to suggest a new feature, or you want just want to improve remotefs, feel free to open an issue or leave a pull request.

Please follow [our contributing guidelines](CONTRIBUTING.md)

---

## Changelog ⏳

View remotefs` changelog [HERE](CHANGELOG.md)

---

## Powered by 💪

remotefs is powered by these awesome projects:

- [rust-s3](https://github.com/durch/rust-s3)
- [ssh2-rs](https://github.com/alexcrichton/ssh2-rs)
- [suppaftp](https://github.com/veeso/suppaftp)
- [whoami](https://github.com/libcala/whoami)
- [wildmatch](https://github.com/becheran/wildmatch)

---

## License 📃

remotefs is licensed under the MIT license.

You can read the entire license [HERE](LICENSE)
