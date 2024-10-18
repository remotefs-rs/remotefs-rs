# remotefs-fuse

<p align="center">
  <img src="https://raw.githubusercontent.com/remotefs-rs/remotefs-rs/main/assets/logo.png" alt="logo" width="256" height="256" />
</p>

<p align="center">~ A FUSE Driver for remotefs-rs ~</p>

<p align="center">Developed by <a href="https://veeso.github.io/" target="_blank">@veeso</a></p>
<p align="center">Current version: WIP</p>

<p align="center">
  <a href="https://opensource.org/licenses/MIT"
    ><img
      src="https://img.shields.io/badge/License-MIT-teal.svg"
      alt="License-MIT"
  /></a>
  <a href="https://github.com/remotefs-rs/remotefs-rs-fuse/stargazers"
    ><img
      src="https://img.shields.io/github/stars/remotefs-rs/remotefs-rs-fuse.svg?style=badge"
      alt="Repo stars"
  /></a>
  <a href="https://crates.io/crates/remotefs-fuse"
    ><img
      src="https://img.shields.io/crates/d/remotefs-fuse.svg"
      alt="Downloads counter"
  /></a>
  <a href="https://crates.io/crates/remotefs-fuse"
    ><img
      src="https://img.shields.io/crates/v/remotefs-fuse.svg"
      alt="Latest version"
  /></a>
  <a href="https://ko-fi.com/veeso">
    <img
      src="https://img.shields.io/badge/donate-ko--fi-red"
      alt="Ko-fi"
  /></a>
</p>
<p align="center">
  <a href="https://github.com/remotefs-rs/remotefs-rs-fuse/actions"
    ><img
      src="https://github.com/remotefs-rs/remotefs-rs-fuse/workflows/linux/badge.svg"
      alt="Linux CI"
  /></a>
  <a href="https://github.com/remotefs-rs/remotefs-rs-fuse/actions"
    ><img
      src="https://github.com/remotefs-rs/remotefs-rs-fuse/workflows/windows/badge.svg"
      alt="Windows CI"
  /></a>
  <a href="https://docs.rs/remotefs-fuse"
    ><img
      src="https://docs.rs/remotefs-fuse/badge.svg"
      alt="Docs"
  /></a>
</p>

---

## Get started

Coming soon...

## CLI Tool

remotefs-fuse comes with a CLI tool **remotefs-fuse-cli** to mount remote file systems with FUSE.

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
remotefs-fuse-cli --to /mnt/to <aws-s3|ftp|kube|smb|scp|sftp|webdav> [protocol-options...]
```

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
- smb
  - `--address <address>`
  - `--port <port>` (default: `139`; Linux/Mac only)
  - `--share <share_name>`
  - `--username <username>` (optional)
  - `--password <password>` (optional)
  - `--workgroup <workgroup>` (optional; Linux/Mac only)
- scp / sftp
  - `--hostname <hostname>`
  - `--port <port>` (default `22`)
  - `--username <username>`
  - `--password <password>`
- webdav
  - `--url <url>`
  - `--username <username>`
  - `--password <password>`

## Changelog ⏳

View remotefs` changelog [HERE](CHANGELOG.md)

---

## License 📃

remotefs is licensed under the MIT license.

You can read the entire license [HERE](LICENSE)
