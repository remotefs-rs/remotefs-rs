# fusibile

<p align="center">
  <img src="https://raw.githubusercontent.com/remotefs-rs/remotefs-rs/main/assets/logo.png" alt="logo" width="256" height="256" />
</p>

<p align="center">~ A CLI to mount remote file systems locally via FUSE or Dokany ~</p>

<p align="center">Developed by <a href="https://veeso.me/" target="_blank">@veeso</a></p>

<p align="center">
  <a href="https://opensource.org/licenses/MIT"
    ><img
      src="https://img.shields.io/crates/l/fusibile.svg"
      alt="License-MIT"
  /></a>
  <a href="https://crates.io/crates/fusibile"
    ><img
      src="https://img.shields.io/crates/d/fusibile.svg?logo=rust"
      alt="Downloads counter"
  /></a>
  <a href="https://crates.io/crates/fusibile"
    ><img
      src="https://img.shields.io/crates/v/fusibile.svg?logo=rust"
      alt="Latest version"
  /></a>
</p>
<p align="center">
  <a href="https://github.com/remotefs-rs/remotefs-rs-fuse/actions/workflows/ci.yml"
    ><img
      src="https://github.com/remotefs-rs/remotefs-rs-fuse/actions/workflows/ci.yml/badge.svg"
      alt="CI"
  /></a>
</p>

---

## About fusibile ☁️

`fusibile` is a ready-to-use CLI built on top of
[`remotefs-fuse`](https://crates.io/crates/remotefs-fuse) that wires a chosen
[`remotefs`](https://github.com/remotefs-rs/remotefs-rs) backend (SFTP/SCP, FTP, AWS S3, Google
Cloud Storage, SMB, WebDAV, Kube, in-memory, ...) straight into a local mount, via FUSE on
Linux/macOS and Dokany on Windows.

## Requirements

- **Linux**: you need to have `fuse3` installed on your system.

  Of course, you also need to have the `FUSE` kernel module installed.
  To build `fusibile` on Linux, you need to have the `libfuse3` development package installed.

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

## Installation

```sh
cargo install fusibile
```

### Features

`fusibile` can be built with the features below; each feature enables a different file transfer
protocol

- `aws-s3`
- `ftp`: FTP and FTPS through the Tokio `remotefs-ftp` client (FTPS uses native-tls, vendored on
  Linux and macOS)
- `gcs`
- `kube`
- `libfuse`: link against the system `libfuse3` on Unix; see the
  [`remotefs-fuse` docs](https://docs.rs/remotefs-fuse) for what disabling it changes
- `smb`: requires `libsmbclient` on MacOS and GNU/Linux systems
- `smb-vendored` (UNIX only): build Samba from source instead of linking the system
  `libsmbclient`, so the resulting binary carries no `libsmbclient` dependency. **Not enabled by
  default** — it adds well over an hour to a build. The released Linux and macOS binaries are
  built with it. It has no effect on Windows, where the SMB client is `WNetSmbFs` on the Win32
  WNet API and needs no external library at all.
- `ssh` (enables **both sftp and scp**); requires `libssh2` on MacOS and GNU/Linux systems
- `webdav`

All the features except `smb-vendored` are enabled by default; so if you want to build it with
only certain features, pass the `--no-default-features` option.

`fusibile` is a Tokio application: every backend implements `remotefs`'s `AsyncRemoteFs` contract
and is mounted with `remotefs-fuse`'s `AsyncMount`. The in-memory and SMB clients are adapted with
`remotefs::adapters::r#async::Unblock`. Ctrl+C, and SIGTERM on Linux and macOS, unmounts and exits
cleanly.

## Usage

```sh
fusibile -o opt1 -o opt2=abc --to /mnt/to --volume <volume-name> <aws-s3|ftp|gcs|kube|smb|scp|sftp|webdav> [protocol-options...]
```

On Windows the mountpoint can be specified simply using the drive letter `--to M` will mount the
FS to `M:\`

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
- gcs
  - `--bucket <name>`
  - `--endpoint <endpoint_url>` (optional; default: `https://storage.googleapis.com`)
  - `--service-account-key <path>` path to a service-account JSON file (optional; defaults to
    application-default credentials)
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
  - `--dialect <dialect>` (optional; Linux/Mac only; possible values: `Auto`, `Nt1`, `Smb2`,
    `Smb3`; default: `Auto`)
- scp / sftp
  - `--hostname <hostname>`
  - `--port <port>` (optional; falls back to `22` if not resolved from configuration)
  - `--username <username>` (optional)
  - `--password <password>` (optional)
  - `--ssh-config <path>` (optional; default: `~/.ssh/config`)
- webdav
  - `--url <url>`
  - `--username <username>` (optional; required `--password` if specified; conflicts with `--bearer-token`)
  - `--password <password>` (optional; required `--username` if specified; conflicts with `--bearer-token`)
  - `--bearer-token <token>` (optional; conflicts with `--username` and `--password`)

Other options are:

- `--uid <uid>`: specify the UID to overwrite when mounting the remote fs. See
  [UID and GID override](#uid-and-gid-override).
- `--gid <gid>`: specify the GID to overwrite when mounting the remote fs. See
  [UID and GID override](#uid-and-gid-override).
- `--default-mode <mode>`: set the default file mode to use when the remote fs doesn't support it.

Mount options can be viewed in the docs at
<https://docs.rs/remotefs-fuse/latest/remotefs-fuse/enum.MountOption.html>.

## UID and GID override

> ❗ This doesn't apply to Windows.

The UID and GID overrides make remote entries appear to be owned by local
identities. They are useful when, for example:

1. your local UID is `1000`;
2. the SFTP account owns remote files as UID `1002`; and
3. local requests from UID `1000` would otherwise be checked against UID
   `1002`.

Pass the local IDs that should own the mounted entries:

```sh
--uid "$(id -u)" --gid "$(id -g)"
```

In this example, `--uid` resolves to `1000`, not the remote UID `1002`. The
overrides affect ownership presented by the mount and its permission checks;
they do not change ownership on the remote filesystem.

## Project stability

Please consider this is an early-stage project and I haven't heavily tested it, in particular the
Windows version.

I suggest you to first test it on test filesystems to see whether it behaves correctly with your
system.

---

## Contributing and issues 🤝🏻

Contributions, bug reports, new features, and questions are welcome! 😉
If you have any questions or concerns, or you want to suggest a new feature, or you just want to
improve `fusibile`, feel free to open an issue or a PR on the
[remotefs-rs-fuse](https://github.com/remotefs-rs/remotefs-rs-fuse) repository.

Before contributing with AI-assisted tools, please read the
[AI Policy](https://github.com/remotefs-rs/remotefs-rs-fuse/blob/main/AI_POLICY.md).

---

## Changelog ⏳

View the [changelog](https://github.com/remotefs-rs/remotefs-rs-fuse/blob/main/CHANGELOG.md).

---

## License 📃

`fusibile` is licensed under the MIT license.

You can read the entire
[MIT license](https://github.com/remotefs-rs/remotefs-rs-fuse/blob/main/LICENSE).
