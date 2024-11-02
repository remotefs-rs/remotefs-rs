# remotefs-fuse

<p align="center">
  <img src="https://raw.githubusercontent.com/remotefs-rs/remotefs-rs/main/assets/logo.png" alt="logo" width="256" height="256" />
</p>

<p align="center">~ A FUSE Driver for remotefs-rs ~</p>

<p align="center">Developed by <a href="https://veeso.github.io/" target="_blank">@veeso</a></p>
<p align="center">Current version: 0.1.0</p>

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
  <a href="https://github.com/remotefs-rs/remotefs-rs-fuse/actions/workflows/linux.yml"
    ><img
      src="https://github.com/remotefs-rs/remotefs-rs-fuse/workflows/linux/badge.svg"
      alt="Linux CI"
  /></a>
  <a href="https://github.com/remotefs-rs/remotefs-rs-fuse/actions/workflows/macos.yml"
    ><img
      src="https://github.com/remotefs-rs/remotefs-rs-fuse/workflows/macos/badge.svg"
      alt="MacOS CI"
  /></a>
  <a href="https://github.com/remotefs-rs/remotefs-rs-fuse/actions/workflows/windows.yml"
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

First of all you need to add **remotefs-fuse** to your project dependencies:

```toml
remotefs-fuse = "^0.1.0"
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
    umount.umount().expect("Failed to unmount");
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
remotefs-fuse-cli -o opt1 -o opt2=abc --to /mnt/to --volume <volume-name> <aws-s3|ftp|kube|smb|scp|sftp|webdav> [protocol-options...]
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
- memory: runs a virtual file system in memory
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

Other options are:

- `--uid <uid>`: specify the UID to overwrite when mounting the remote fs. See [UID and GID override](#uid-and-gid-override).
- `--gid <gid>`: specify the GID to overwrite when mounting the remote fs. See [UID and GID override](#uid-and-gid-override).
- `--default-mode <mode>`: set the default file mode to use when the remote fs doesn't support it.

Mount options can be viewed in the docs at <https://docs.rs/remotefs-fuse/latest/remotefs-fuse/enum.MountOption.html>.

## UID and GID override

The possibility to override UID and GID is used because sometimes this scenario can happen:

1. my UID is `1000`
2. I'm mounting for instance a SFTP file system and the remote user I used to sign in has UID `1002`
3. I'm unable to operate on the file system because UID `1000` can't operate to files owned by `1002`

But of course this doesn't make sense: I signed in with user who owns those files, so I should be able to operate on them.
That's why I've added `Uid` and `Gid` into the `MountOption` variant.

Setting the `Uid` option to `1002` you'll be able to operate on the File system as it should.

> ❗ This doesn't apply to Windows.

## Changelog ⏳

View remotefs-fuse`s changelog [HERE](CHANGELOG.md)

---

## License 📃

remotefs-fuse is licensed under the MIT license.

You can read the entire license [HERE](LICENSE)
