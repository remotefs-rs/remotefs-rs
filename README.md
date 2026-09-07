# remotefs

<p align="center">
  <img src="/assets/logo.png" alt="logo" width="256" height="256" />
</p>

<p align="center">
  <a href="https://github.com/remotefs-rs/remotefs-rs/blob/main/CHANGELOG.md" target="_blank">Changelog</a>
  ·
  <a href="https://docs.rs/remotefs#get-started" target="_blank">Get started</a>
  ·
  <a href="https://docs.rs/remotefs" target="_blank">Documentation</a>
</p>

<p align="center">~ The Omni Filetransfer Client Library (and more!) ~</p>

<p align="center">Developed by <a href="https://veeso.github.io/" target="_blank">@veeso</a></p>

<p align="center">
  <a href="https://opensource.org/licenses/MIT"
    ><img
      src="https://img.shields.io/badge/License-MIT-teal.svg"
      alt="License-MIT"
  /></a>
  <a href="https://github.com/remotefs-rs/remotefs-rs/stargazers"
    ><img
      src="https://img.shields.io/github/stars/remotefs-rs/remotefs-rs.svg?style=plain"
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
  <a href="https://conventionalcommits.org">
    <img
      src="https://img.shields.io/badge/Conventional%20Commits-1.0.0-%23FE5196?logo=conventionalcommits&logoColor=white"
      alt="Conventional commits"
  /></a>
</p>
<p align="center">
  <a href="https://github.com/remotefs-rs/remotefs-rs/actions/workflows/ci.yml"
    ><img
      src="https://github.com/remotefs-rs/remotefs-rs/actions/workflows/ci.yml/badge.svg"
      alt="CI"
  /></a>
  <a href="https://coveralls.io/github/remotefs-rs/remotefs-rs"
    ><img
      src="https://coveralls.io/repos/github/remotefs-rs/remotefs-rs/badge.svg"
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

This crate is the root of the family: it defines the trait and the shared types, but ships no client of its own.
The clients live in their own crates and implement `RemoteFs` for **SFTP**, **SCP**, **FTP**, **AWS S3**, **Google Cloud Storage**, **Kube**, **SMB**, **WebDAV** and an in-memory backend.

### Why remotefs ❓

You might be wondering, "why remotefs?"
The first and foremost reason is to provide a generic interface over multiple protocols.
For example, in [termscp](https://github.com/veeso/termscp) it allows the support of multiple protocols without any protocol-specific code.
The second reason is that often, users just want a simple way to operate on a remote file system, however, they don't have the time to spend researching the ins and outs of each protocol.

Using remotefs, this is no longer a problem: all you need is to configure the options to your liking, then you're ready to connect.

---

## Features 🎁

- 📁 Different communication protocols
  - **AWS S3**
  - **FTP** and **FTPS**
  - **Google Cloud Storage**
  - **Kube**
  - **SCP**
  - **SFTP**
  - **SMB**
  - **WebDAV**
- ✔️ Configurable: use only the client that you need
- 🤖 Extensible: adding new protocols is easy
- 🚀 Simple: easy to setup
- 😄 Understandable: no need to understand the underlying protocol

---

## Get started 🚀

First, add `remotefs` to your list of dependencies:

```toml
remotefs = "0.3"
```

these features are supported:

- `find`: enable the `find()` method on the `RemoteFs` trait (_enabled by default_)
- `no-log`: disable logging. By default, this library will log via the `log` crate.

Depend on this crate directly only when you write a client of your own, or when your code is generic over the protocol.
Otherwise the client crate re-exports everything you need.

### Client libraries 🔌

To use an existing client, add it to your `Cargo.toml` along with remotefs:

- [aws-s3](https://github.com/remotefs-rs/remotefs-rs-aws-s3)

  ```toml
  remotefs-aws-s3 = "0.4"
  ```

- [ftp](https://github.com/remotefs-rs/remotefs-rs-ftp)

  ```toml
  remotefs-ftp = { version = "0.4", features = ["secure"] }
  ```

- [gcs](https://github.com/remotefs-rs/remotefs-rs-gcs)

  ```toml
  remotefs-gcs = "0.1"
  ```

- [kube](https://github.com/remotefs-rs/remotefs-rs-kube)

  ```toml
  remotefs-kube = "0.4"
  ```

- [memory](https://github.com/remotefs-rs/remotefs-rs-memory)

  ```toml
  remotefs-memory = "0.1"
  ```

- [smb](https://github.com/remotefs-rs/remotefs-rs-smb)

  ```toml
  remotefs-smb = "0.5"
  ```

- [ssh](https://github.com/remotefs-rs/remotefs-rs-ssh)

  ```toml
  remotefs-ssh = "0.9"
  ```

- [webdav](https://github.com/remotefs-rs/remotefs-rs-webdav)

  ```toml
  remotefs-webdav = "0.2"
  ```

To mount any of these as a local filesystem, see [remotefs-fuse](https://github.com/remotefs-rs/remotefs-rs-fuse).

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

A client with no equivalent for a method returns a `RemoteErrorType::UnsupportedFeature` error rather than failing.
Treat that as an answer, not a bug, and fall back accordingly.

### Client compatibility table ✔️

The following table states the compatibility for each client associated with the remote file system trait method.

Note: `connect()`, `disconnect()` and `is_connected()` **MUST** always be supported, and are so omitted in the table.

Note: the `Smb` column describes the Rust-native and pavao clients. On Windows the `WNet` client additionally supports `append`, `copy`, `create`, `open`, `setstat` and `symlink`; see the [remotefs-smb](https://github.com/remotefs-rs/remotefs-rs-smb) table.

| Client/Method  | Aws-S3 | Ftp | Gcs | Kube | Memory | Scp | Sftp | Smb | WebDAV |
| -------------- | ------ | --- | --- | ---- | ------ | --- | ---- | --- | ------ |
| append_file    | No     | Yes | No  | No   | Yes    | No  | Yes  | Yes | No     |
| append         | No     | Yes | No  | No   | Yes    | No  | Yes  | No  | No     |
| change_dir     | Yes    | Yes | Yes | Yes  | Yes    | Yes | Yes  | Yes | Yes    |
| copy           | No     | No  | Yes | Yes  | Yes    | Yes | Yes  | No  | No     |
| create_dir     | Yes    | Yes | Yes | Yes  | Yes    | Yes | Yes  | Yes | Yes    |
| create_file    | Yes    | Yes | Yes | Yes  | Yes    | Yes | Yes  | Yes | Yes    |
| create         | No     | Yes | No  | No   | Yes    | Yes | Yes  | No  | No     |
| exec           | No     | No  | No  | Yes  | No     | Yes | Yes  | No  | No     |
| exists         | Yes    | Yes | Yes | Yes  | Yes    | Yes | Yes  | Yes | Yes    |
| list_dir       | Yes    | Yes | Yes | Yes  | Yes    | Yes | Yes  | Yes | Yes    |
| mov            | No     | Yes | Yes | Yes  | Yes    | Yes | Yes  | Yes | Yes    |
| open_file      | Yes    | Yes | Yes | Yes  | Yes    | Yes | Yes  | Yes | Yes    |
| open           | No     | Yes | No  | No   | Yes    | Yes | Yes  | No  | No     |
| pwd            | Yes    | Yes | Yes | Yes  | Yes    | Yes | Yes  | Yes | Yes    |
| remove_dir_all | Yes    | Yes | Yes | Yes  | Yes    | Yes | Yes  | Yes | Yes    |
| remove_dir     | Yes    | Yes | Yes | Yes  | Yes    | Yes | Yes  | Yes | Yes    |
| remove_file    | Yes    | Yes | Yes | Yes  | Yes    | Yes | Yes  | Yes | Yes    |
| setstat        | No     | No  | No  | Yes  | Yes    | Yes | Yes  | No  | No     |
| stat           | Yes    | Yes | Yes | Yes  | Yes    | Yes | Yes  | Yes | Yes    |
| symlink        | No     | No  | No  | Yes  | Yes    | Yes | Yes  | No  | No     |

---

## Development 🛠️

Every task runs through a [`just`](https://just.systems) recipe. Run `just`
to list them all.

```sh
just build                 # cargo build --all-targets
just test                  # cargo test --lib, then --doc
just coverage              # cargo llvm-cov, writes lcov.info
just fmt                   # dprint fmt (Markdown, Rust, TOML, YAML)
just fmt_check             # dprint check
just lint "-- -D warnings" # clippy
just doc                   # cargo doc --no-deps
just deny                  # cargo deny check
just scan_secrets          # trufflehog filesystem
just check                 # the full local quality gate
```

`just check` chains `fmt_check`, Clippy with warnings denied, `doc`, `deny`,
and `test`, and is the required gate before opening a pull request. The suite
is plain unit tests plus doctests, so it needs no container, network access or
platform setup. CI additionally lints with `--no-default-features` and with
`--all-features`, because `no-log` compiles out every `log` macro.

See [AGENTS.md](AGENTS.md) for the full contract.

---

## Support the developer ☕

If you like remotefs and you're grateful for the work I've done, please consider a little donation 🥳

You can make a donation with one of these platforms:

[![ko-fi](https://img.shields.io/badge/Ko--fi-F16061?style=for-the-badge&logo=ko-fi&logoColor=white)](https://ko-fi.com/veeso)
[![PayPal](https://img.shields.io/badge/PayPal-00457C?style=for-the-badge&logo=paypal&logoColor=white)](https://www.paypal.me/chrisintin)

---

## Contributing and issues 🤝🏻

Contributions, bug reports, new features, and questions are welcome! 😉
If you have any questions or concerns, or you want to suggest a new feature, or you want just want to improve remotefs, feel free to open an issue or a PR.

Please follow [our contributing guidelines](CONTRIBUTING.md) and read the [AI policy](AI_POLICY.md) before opening a pull request.

---

## Apps using remotefs 🚀

- [termscp](https://github.com/veeso/termscp)
- [remotefs-fuse](https://github.com/remotefs-rs/remotefs-rs-fuse)

---

## Changelog ⏳

View remotefs' changelog [HERE](CHANGELOG.md)

---

## License 📃

remotefs is licensed under the MIT license.

You can read the entire license [HERE](LICENSE)
