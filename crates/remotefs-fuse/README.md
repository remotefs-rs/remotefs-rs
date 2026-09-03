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
  <a href="https://ko-fi.com/veeso">
    <img
      src="https://img.shields.io/badge/donate-ko--fi-red"
      alt="Ko-fi"
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
implementation (SFTP/SCP, FTP, AWS S3, SMB, WebDAV, Kube, in-memory, ...) as a local filesystem,
via FUSE on Linux/macOS and Dokany on Windows. It ships as two crates:

- **remotefs-fuse**: the library (`Mount`, `MountOption`, `Driver`), to embed in your own project.
- **remotefs-fuse-cli**: a ready-to-use CLI that wires a chosen `remotefs` backend into
  `remotefs-fuse` for you.

---

## Get started 🚀

First of all you need to add **remotefs-fuse** to your project dependencies:

```toml
remotefs-fuse = "0.1"
```

these features are supported:

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

## CLI Tool

remotefs-fuse comes with a CLI tool **remotefs-fuse-cli** to mount remote file systems with FUSE or Dokany.

```sh
cargo install remotefs-fuse-cli
```

### Features

remotefs-fuse-cli can be built with the features below; each feature enables a different file transfer protocol

- `aws-s3`
- `ftp`
- `kube`
- `smb`: requires `libsmbclient` on MacOS and GNU/Linux systems
- `ssh` (enables **both sftp and scp**); requires `libssh2` on MacOS and GNU/Linux systems
- `webdav`

All the features are enabled by default; so if you want to build it with only certain features, pass the `--no-default-features` option.

### Usage

```sh
remotefs-fuse-cli -o opt1 -o opt2=abc --to /mnt/to --volume <volume-name> <aws-s3|ftp|kube|smb|scp|sftp|webdav> [protocol-options...]
```

On Windows the mountpoint can be specified simply using the drive letter `--to M` will mount the FS to `M:\`

where protocol options are

- aws-s3
  - `--bucket <name>`
  - `--region <region>` (optional)
  - `--endpoint <endpoint_url>` (optional)
  - `--profile <profile_name>` (optional)
  - `--access-key <access_key>` (optional)
  - `--security-token <security_access_token>` (optional)
  - `--new-path-style` use new path style
- ftp
  - `--hostname <host>`
  - `--port <port>` (default 21)
  - `--username <username>` (default: `anonymous`)
  - `--password <password>` (optional)
  - `--secure` specify it if you want to use FTPS
  - `--active` specify it if you want to use ACTIVE mode
- kube
  - `--namespace <namespace>` (default: `default`)
  - `--cluster-url <url>`
- memory: runs a virtual file system in memory
- smb
  - `--address <address>`
  - `--port <port>` (default: `139`; Linux/Mac only)
  - `--share <share_name>`
  - `--username <username>` (optional)
  - `--password <password>` (optional)
  - `--workgroup <workgroup>` (optional; Linux/Mac only)
  - `--dialect <dialect>` (optional; Linux/Mac only; possible values: `Auto`, `Nt1`, `Smb2`, `Smb3`; default: `Auto`)
- scp / sftp
  - `--hostname <hostname>`
  - `--port <port>` (default `22`)
  - `--username <username>`
  - `--password <password>`
  - `--config-file <path>` (optional; default: `~/.ssh/config`)
- webdav
  - `--url <url>`
  - `--username <username>`
  - `--password <password>`

Other options are:

- `--uid <uid>`: specify the UID to overwrite when mounting the remote fs. See [UID and GID override](#uid-and-gid-override).
- `--gid <gid>`: specify the GID to overwrite when mounting the remote fs. See [UID and GID override](#uid-and-gid-override).
- `--default-mode <mode>`: set the default file mode to use when the remote fs doesn't support it.

Mount options can be viewed in the docs at <https://docs.rs/remotefs-fuse/latest/remotefs-fuse/enum.MountOption.html>.

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

## Support the developer ☕

If you like remotefs-fuse and you're grateful for the work I've done, please consider a little donation 🥳

You can make a donation with one of these platforms:

[![ko-fi](https://img.shields.io/badge/Ko--fi-F16061?style=for-the-badge&logo=ko-fi&logoColor=white)](https://ko-fi.com/veeso)
[![PayPal](https://img.shields.io/badge/PayPal-00457C?style=for-the-badge&logo=paypal&logoColor=white)](https://www.paypal.me/chrisintin)
[![bitcoin](https://img.shields.io/badge/Bitcoin-ff9416?style=for-the-badge&logo=bitcoin&logoColor=white)](https://btc.com/bc1qvlmykjn7htz0vuprmjrlkwtv9m9pan6kylsr8w)

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
