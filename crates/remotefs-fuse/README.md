# remotefs-fuse

<p align="center">
  <img src="https://raw.githubusercontent.com/remotefs-rs/remotefs-rs/main/assets/logo.png" alt="logo" width="256" height="256" />
</p>

<p align="center">~ A FUSE Driver for remotefs-rs ~</p>

<p align="center">Developed by <a href="https://veeso.me/" target="_blank">@veeso</a></p>

<p align="center">
  <a href="https://opensource.org/licenses/MIT"
    ><img
      src="https://img.shields.io/crates/l/remotefs-fuse.svg"
      alt="License-MIT"
  /></a>
  <a href="https://github.com/remotefs-rs/remotefs-rs-fuse/stargazers"
    ><img
      src="https://img.shields.io/github/stars/remotefs-rs/remotefs-rs-fuse?style=flat"
      alt="Repo stars"
  /></a>
  <a href="https://crates.io/crates/remotefs-fuse"
    ><img
      src="https://img.shields.io/crates/d/remotefs-fuse.svg?logo=rust"
      alt="Downloads counter"
  /></a>
  <a href="https://crates.io/crates/remotefs-fuse"
    ><img
      src="https://img.shields.io/crates/v/remotefs-fuse.svg?logo=rust"
      alt="Latest version"
  /></a>
  <a href="https://conventionalcommits.org"
    ><img
      src="https://img.shields.io/badge/Conventional%20Commits-1.0.0-%23FE5196?logo=conventionalcommits&logoColor=white"
      alt="Conventional commits"
  /></a>
</p>
<p align="center">
  <a href="https://github.com/remotefs-rs/remotefs-rs-fuse/actions/workflows/ci.yml"
    ><img
      src="https://github.com/remotefs-rs/remotefs-rs-fuse/actions/workflows/ci.yml/badge.svg"
      alt="CI"
  /></a>
  <a href="https://docs.rs/remotefs-fuse"
    ><img
      src="https://img.shields.io/docsrs/remotefs-fuse?logo=rust"
      alt="Docs"
  /></a>
</p>

---

## About remotefs-fuse ☁️

remotefs-fuse mounts any [`remotefs`](https://github.com/remotefs-rs/remotefs-rs) `RemoteFs`
implementation (SFTP/SCP, FTP, AWS S3, Google Cloud Storage, SMB, WebDAV, Kube, in-memory, ...) as
a local filesystem, via FUSE on Linux/macOS and Dokany on Windows. It's the library (`Mount`,
`MountOption`, `Driver`) to embed in your own project.

If you're looking for a ready-to-use CLI instead, check out
[`fusibile`](https://github.com/remotefs-rs/remotefs-rs-fuse/tree/main/crates/fusibile), which
wires a chosen `remotefs` backend into `remotefs-fuse` for you.

---

## Install fusibile 📦

`fusibile` is the CLI. Pick whichever suits you:

### Linux and macOS

```sh
curl -sSLf https://remotefs-rs.github.io/remotefs-rs-fuse/install.sh | sh
```

The script uses Homebrew when it is available and falls back to downloading the release binary,
verifying its SHA-256 checksum. Pass `--version X.Y.Z` for a specific release, `--yes` to skip the
prompt, or set `BIN_DIR` to change the install directory (default `/usr/local/bin`).

### Windows

```powershell
irm https://remotefs-rs.github.io/remotefs-rs-fuse/install.ps1 | iex
```

### Homebrew

```sh
brew install remotefs-rs/fusibile/fusibile
```

### Cargo

```sh
cargo install fusibile --locked
```

### Manual download

Grab an archive from the [releases page](https://github.com/remotefs-rs/remotefs-rs-fuse/releases).
Assets are named `fusibile-v<version>-<target>.tar.gz` (`.zip` on Windows), each with a matching
`<target>.sha256`. Prebuilt targets:

| Platform            | Target                                |
| ------------------- | ------------------------------------- |
| Linux x86_64        | `x86_64-unknown-linux-musl` (static)  |
| Linux aarch64       | `aarch64-unknown-linux-musl` (static) |
| macOS Apple Silicon | `aarch64-apple-darwin`                |
| macOS Intel         | `x86_64-apple-darwin`                 |
| Windows x86_64      | `x86_64-pc-windows-msvc`              |

Windows on ARM64 is not available yet: `fusibile` depends on Dokany, whose Rust bindings have no
aarch64 support.

### Runtime requirements

- **Linux**: the `fuse3` package, for its setuid `fusermount3` binary
  (`sudo apt-get install fuse3`)
- **macOS**: [macFUSE](https://osxfuse.github.io/) (`brew install --cask macfuse`)
- **Windows**: [Dokany](https://github.com/dokan-dev/dokany/releases) (`choco install dokany`)

## Get started 🚀

First of all you need to add **remotefs-fuse** to your project dependencies:

```toml
remotefs-fuse = "0.1"
```

these features are supported:

- `libfuse` (enabled by default, Unix only): link against the system `libfuse3`. Disable it to
  use `fuser`'s pure-Rust mount implementation, which needs no `libfuse3-dev` at build time and
  shells out to `fusermount3` instead. Inert on macOS and Windows.
- `no-log`: disable logging. By default, this library will log via the `log` crate.

## Example

```rust,no_run,ignore
use remotefs_fuse::Mount;

let options = vec![
    #[cfg(unix)]
    remotefs_fuse::MountOption::AllowRoot,
    #[cfg(unix)]
    remotefs_fuse::MountOption::RW,
    #[cfg(unix)]
    remotefs_fuse::MountOption::Exec,
    #[cfg(unix)]
    remotefs_fuse::MountOption::Sync,
    #[cfg(unix)]
    remotefs_fuse::MountOption::FSName(volume),
];

let remote = MyRemoteFsImpl::new();
let mount_path = std::path::PathBuf::from("/mnt/remote");
let mut mount = Mount::mount(remote, &mount_path, &options).expect("Failed to mount");
let mut umount = mount.unmounter();

// setup signal handler
ctrlc::set_handler(move || {
    umount.unmount().expect("Failed to unmount");
})?;

mount.run().expect("Failed to run filesystem event loop");
```

## Requirements

- **Linux**: you need to have `fuse3` installed on your system.

  Of course, you also need to have the `FUSE` kernel module installed.
  To build `remotefs-fuse` on Linux, you need to have the `libfuse3` development package installed.

  In Ubuntu, you can install it with:

  ```sh
  sudo apt-get install fuse3 libfuse3-dev
  ```

  In CentOS, you can install it with:

  ```sh
  sudo yum install fuse-devel
  ```

- **macOS**: you need to have the `macfuse` service installed on your system.

  You can install it with:

  ```sh
  brew install macfuse
  ```

- **Windows**: you need to have the `dokany` service installed on your system.

  You can install it from <https://github.com/dokan-dev/dokany?tab=readme-ov-file#installation>

## UID and GID override

> ❗ This doesn't apply to Windows.

The possibility to override UID and GID is used because sometimes this scenario can happen:

1. my UID is `1000`
2. I'm mounting for instance a SFTP file system and the remote user I used to sign in has UID `1002`
3. I'm unable to operate on the file system because UID `1000` can't operate to files owned by `1002`

But of course this doesn't make sense: I signed in with user who owns those files, so I should be able to operate on them.
That's why I've added `Uid` and `Gid` into the `MountOption` variant.

Setting the `Uid` option to `1002` you'll be able to operate on the File system as it should.

## Project stability

Please consider this is an early-stage project and I haven't heavily tested it, in particular the Windows version.

I suggest you to first test it on test filesystems to see whether the library behaves correctly with your system.

---

## Development 🛠️

Every task runs through a [`just`](https://just.systems) recipe. Run `just` to list them all.

```sh
just build                 # cargo build --all-targets
just test                  # cargo test --workspace, then --doc
just fmt                   # dprint fmt (Markdown, Rust, TOML, YAML)
just fmt_check             # dprint check
just lint "-- -D warnings" # clippy with all features
just doc                   # cargo doc --all-features
just deny                  # cargo deny check
just scan_secrets          # trufflehog filesystem
just check                 # the full local quality gate
```

`just check` chains `fmt_check`, Clippy with warnings denied, `doc`, `deny`, and `test`, and is
the required gate before opening a pull request.

Integration tests actually mount a filesystem, so they're gated behind the `integration-tests`
feature and need the platform FUSE/Dokany service installed (see [Requirements](#requirements)):

```sh
just test "--features integration-tests"
```

---

## Contributing and issues 🤝🏻

Contributions, bug reports, new features, and questions are welcome! 😉
If you have any questions or concerns, or you want to suggest a new feature, or you just want to
improve remotefs-fuse, feel free to open an issue or a PR.

Before contributing with AI-assisted tools, please read the [AI Policy](AI_POLICY.md).

---

## Changelog ⏳

View the remotefs-fuse [changelog](CHANGELOG.md).

---

## License 📃

remotefs-fuse is licensed under the MIT license.

You can read the entire [MIT license](LICENSE).
