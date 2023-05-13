# remotefs SMB

<p align="center">
  <a href="https://veeso.github.io/remotefs-smb/blob/main/CHANGELOG.md" target="_blank">Changelog</a>
  ·
  <a href="#get-started">Get started</a>
  ·
  <a href="https://docs.rs/remotefs-smb" target="_blank">Documentation</a>
</p>

<p align="center">~ Remotefs SMB client ~</p>

<p align="center">Developed by <a href="https://veeso.github.io/" target="_blank">@veeso</a></p>
<p align="center">Current version: 0.2.0 (13/05/2023)</p>

<p align="center">
  <a href="https://opensource.org/licenses/MIT"
    ><img
      src="https://img.shields.io/badge/License-MIT-teal.svg"
      alt="License-MIT"
  /></a>
  <a href="https://github.com/veeso/remotefs-rs-smb/stargazers"
    ><img
      src="https://img.shields.io/github/stars/veeso/remotefs-rs-smb.svg"
      alt="Repo stars"
  /></a>
  <a href="https://crates.io/crates/remotefs-smb"
    ><img
      src="https://img.shields.io/crates/d/remotefs-smb.svg"
      alt="Downloads counter"
  /></a>
  <a href="https://crates.io/crates/remotefs-smb"
    ><img
      src="https://img.shields.io/crates/v/remotefs-smb.svg"
      alt="Latest version"
  /></a>
  <a href="https://ko-fi.com/veeso">
    <img
      src="https://img.shields.io/badge/donate-ko--fi-red"
      alt="Ko-fi"
  /></a>
</p>
<p align="center">
  <a href="https://github.com/veeso/remotefs-rs-smb/actions"
    ><img
      src="https://github.com/veeso/remotefs-rs-smb/workflows/Linux/badge.svg"
      alt="Linux CI"
  /></a>
  <a href="https://github.com/veeso/remotefs-rs-smb/actions"
    ><img
      src="https://github.com/veeso/remotefs-rs-smb/workflows/MacOS/badge.svg"
      alt="MacOS CI"
  /></a>
  <a href="https://github.com/veeso/remotefs-rs-smb/actions"
    ><img
      src="https://github.com/veeso/remotefs-rs-smb/workflows/Windows/badge.svg"
      alt="Windows CI"
  /></a>
  <a href="https://coveralls.io/github/veeso/remotefs-rs-smb"
    ><img
      src="https://coveralls.io/repos/github/veeso/remotefs-rs-smb/badge.svg"
      alt="Coveralls"
  /></a>
  <a href="https://docs.rs/remotefs-smb"
    ><img
      src="https://docs.rs/remotefs-smb/badge.svg"
      alt="Docs"
  /></a>
</p>

---

## About remotefs-smb ☁️

remotefs-smb is a client implementation for [remotefs](https://github.com/veeso/remotefs-rs), providing support for the SMB protocol.

---

## Get started 🚀

First of all, add `remotefs-smb` to your project dependencies:

```toml
remotefs-smb = "^0.2"
```

these features are supported:

- `find`: enable `find()` method on client (*enabled by default*)
- `no-log`: disable logging. By default, this library will log via the `log` crate.

### Install dependencies (UNIX based only)

remotefs-smb relies on `pavao`, which requires the `libsmbclient` library, which can be installed with the following instructions:

#### MacOS 🍎

Install samba with brew:

```sh
brew install samba
```

#### Debian based systems 🐧

Install libsmbclient with apt:

```sh
apt install -y libsmbclient-dev libsmbclient
```

⚠️ `libsmbclient-dev` is required only on the machine where you build the application

#### RedHat based systems 🐧

Install libsmbclient with dnf:

```sh
dnf install libsmbclient-devel libsmbclient
```

⚠️ `libsmbclient-devel` is required only on the machine where you build the application

#### Build from sources 📁

Install libsmbclient building from sources:

```sh
wget -O samba.tar.gz https://github.com/samba-team/samba/archive/refs/tags/samba-4.16.1.tar.gz
mkdir -p samba/
tar  xzvf samba.tar.gz -C samba/ --strip-components=1
rm samba.tar.gz
cd samba/
./configure
make
make install
cd ..
rm -rf samba/
```

### Client implementation

#### UNIX client

```rust
// import remotefs trait and client
use remotefs::{RemoteFs, fs::UnixPex};
use remotefs_smb::{SmbFs, SmbOptions, SmbCredentials};
use std::path::Path;
let mut client = SmbFs::try_new(
    SmbCredentials::default()
        .server("smb://localhost:3445")
        .share("/temp")
        .username("test")
        .password("test")
        .workgroup("pavao"),
    SmbOptions::default()
        .case_sensitive(true)
        .one_share_per_server(true),
)
.unwrap();
// connect
assert!(client.connect().is_ok());
// get working directory
println!("Wrkdir: {}", client.pwd().ok().unwrap().display());
// make directory
assert!(client.create_dir(Path::new("/cargo"), UnixPex::from(0o755)).is_ok());
// change working directory
assert!(client.change_dir(Path::new("/cargo")).is_ok());
// disconnect
assert!(client.disconnect().is_ok());
```

#### Windows client

```rust
// import remotefs trait and client
use remotefs::{RemoteFs, fs::UnixPex};
use remotefs_smb::{SmbFs, SmbCredentials};
use std::path::Path;
let mut client = SmbFs::new(
    SmbCredentials::new("localhost:3445", "temp")
        .username("test")
        .password("test")
);
// connect
assert!(client.connect().is_ok());
// get working directory
println!("Wrkdir: {}", client.pwd().ok().unwrap().display());
// make directory
assert!(client.create_dir(Path::new("\\cargo"), UnixPex::from(0o755)).is_ok());
// change working directory
assert!(client.change_dir(Path::new("\\cargo")).is_ok());
// disconnect
assert!(client.disconnect().is_ok());
```

---

### Client compatibility table ✔️

The following table states the compatibility for the client client and the remote file system trait method.

Note: `connect()`, `disconnect()` and `is_connected()` **MUST** always be supported, and are so omitted in the table.

| Client/Method  | Support (UNIX) | Support (Win ) |
|----------------|----------------|----------------|
| append_file    | Yes            | Yes            |
| append         | No             | Yes            |
| change_dir     | Yes            | Yes            |
| copy           | No             | Yes            |
| create_dir     | Yes            | Yes            |
| create_file    | Yes            | Yes            |
| create         | No             | Yes            |
| exec           | No             | No             |
| exists         | Yes            | Yes            |
| list_dir       | Yes            | Yes            |
| mov            | Yes            | Yes            |
| open_file      | Yes            | Yes            |
| open           | No             | Yes            |
| pwd            | Yes            | Yes            |
| remove_dir_all | Yes            | Yes            |
| remove_dir     | Yes            | Yes            |
| remove_file    | Yes            | Yes            |
| setstat        | No             | Yes            |
| stat           | Yes            | Yes            |
| symlink        | Yes            | Yes            |

---

## Support the developer ☕

If you like remotefs-smb and you're grateful for the work I've done, please consider a little donation 🥳

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

remotefs-smb is powered by these aweseome projects:

- [pavao](https://github.com/veeso/pavao)

---

## License 📃

remotefs-smb is licensed under the MIT license.

You can read the entire license [HERE](LICENSE)
