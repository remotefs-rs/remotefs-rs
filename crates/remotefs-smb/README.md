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
<p align="center">Current version: 1.0.0 (13/09/2026)</p>

<p align="center">
  <a href="https://opensource.org/licenses/MIT"
    ><img
      src="https://img.shields.io/badge/License-MIT-teal.svg"
      alt="License-MIT"
  /></a>
  <a href="https://github.com/remotefs-rs/remotefs-rs-smb/stargazers"
    ><img
      src="https://img.shields.io/github/stars/remotefs-rs/remotefs-rs-smb.svg"
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
  <a href="https://conventionalcommits.org">
    <img
      src="https://img.shields.io/badge/Conventional%20Commits-1.0.0-%23FE5196?logo=conventionalcommits&logoColor=white"
      alt="Conventional commits"
  /></a>
</p>
<p align="center">
  <a href="https://github.com/remotefs-rs/remotefs-rs-smb/actions/workflows/ci.yml"
    ><img
      src="https://github.com/remotefs-rs/remotefs-rs-smb/actions/workflows/ci.yml/badge.svg"
      alt="CI"
  /></a>
  <a href="https://coveralls.io/github/remotefs-rs/remotefs-rs-smb"
    ><img
      src="https://coveralls.io/repos/github/remotefs-rs/remotefs-rs-smb/badge.svg"
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

remotefs-smb is a client implementation for [remotefs](https://github.com/remotefs-rs/remotefs-rs), providing support for the SMB protocol.

---

## Get started 🚀

First of all, add `remotefs-smb` to your project dependencies:

```toml
remotefs = "1"
remotefs-smb = "1"
```

These features are supported:

- `pavao`: enable the `libsmbclient`-based client on UNIX (_enabled by default_)
- `smb`: enable the Rust-native async client on every platform (enables `remotefs/async` and `remotefs/tokio`)
- `find`: enable `remotefs::find` and `remotefs::find_async` (_enabled by default_)
- `no-log`: disable logging. By default, this library will log via the `log` crate.
- `vendored`: build pavao with **vendored libsmbclient** (implies `pavao`)

Three clients ship in this crate and can be enabled together:

| Client       | Feature | Platforms | Dialects                   | System libraries  |
| ------------ | ------- | --------- | -------------------------- | ----------------- |
| `SmbFs`      | `smb`   | all       | SMB 2.0.2 to 3.1.1 (async) | none              |
| `PavaoSmbFs` | `pavao` | UNIX      | SMB1 to SMB 3.1.1          | `libsmbclient`    |
| `WNetSmbFs`  | always  | Windows   | OS managed                 | `WNet` (built in) |

### Install dependencies (pavao client only)

The `pavao` feature relies on `pavao`, which requires the `libsmbclient` library, which can be installed with the following instructions:

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

#### Rust-native client (all platforms)

Enable the `smb` feature and add Tokio:

```toml
remotefs-smb = { version = "1", features = ["smb"] }
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

```rust
use std::path::Path;

use remotefs::fs::WriteOptions;
use remotefs::AsyncRemoteFs;
use remotefs_smb::{SmbCredentials, SmbFs, SmbOptions};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut client = SmbFs::try_new(
        SmbCredentials::default()
            .server("smb://localhost:3445")
            .share("/temp")
            .username("test")
            .password("test")
            .workgroup("pavao"),
        SmbOptions::default(),
    )?;

    client.connect().await?;
    client.create_dir(Path::new("/cargo"), None).await?;
    let mut source = futures::io::Cursor::new(b"hello".to_vec());
    client
        .write_file(
            Path::new("/cargo/hello.txt"),
            &WriteOptions::default(),
            &mut source,
        )
        .await?;
    client.disconnect().await?;
    Ok(())
}
```

Blocking code wraps the client with `into_blocking`; the result implements
`remotefs::RemoteFs`:

```rust
use std::path::Path;

use remotefs::RemoteFs;
use remotefs_smb::{SmbCredentials, SmbFs, SmbOptions};

let runtime = tokio::runtime::Runtime::new().unwrap();
let mut client = SmbFs::try_new(
    SmbCredentials::default()
        .server("smb://localhost:3445")
        .share("/temp"),
    SmbOptions::default(),
)
.unwrap()
.into_blocking(runtime.handle().clone());
client.connect().unwrap();
for entry in client.list_dir(Path::new("/")).unwrap() {
    println!("{}", entry.name());
}
client.disconnect().unwrap();
```

`SmbFs` is asynchronous and needs a Tokio runtime. `into_blocking` requires
a multi-thread runtime and must not be called from inside an async task.

#### Pavao client (UNIX)

```rust
use std::io::Cursor;
use std::path::Path;

use remotefs::RemoteFs;
use remotefs::fs::WriteOptions;
use remotefs_smb::{PavaoSmbCredentials, PavaoSmbFs, PavaoSmbOptions};

let mut client = PavaoSmbFs::try_new(
    PavaoSmbCredentials::default()
        .server("smb://localhost:3445")
        .share("/temp")
        .username("test")
        .password("test")
        .workgroup("pavao"),
    PavaoSmbOptions::default()
        .case_sensitive(true)
        .one_share_per_server(true),
)
.unwrap();
client.connect().unwrap();
client.create_dir(Path::new("/cargo"), None).unwrap();
let mut source = Cursor::new(b"hello".to_vec());
client
    .write_file(
        Path::new("/cargo/hello.txt"),
        &WriteOptions::default(),
        &mut source,
    )
    .unwrap();
client.disconnect().unwrap();
```

The pavao client offers one-shot transfers only; `open`, `create`, and
`append` return `UnsupportedFeature`.

#### WNet client (Windows)

```rust
use std::path::Path;

use remotefs::RemoteFs;
use remotefs_smb::{WNetSmbCredentials, WNetSmbFs};

let mut client = WNetSmbFs::new(
    WNetSmbCredentials::new("localhost:3445", "temp")
        .username("test")
        .password("test"),
);
client.connect().unwrap();
client.create_dir(Path::new("/cargo"), None).unwrap();
client.disconnect().unwrap();
```

---

### Client capabilities ✔️

Every client supports the lifecycle (`connect`, `disconnect`,
`is_connected`), `list_dir`, `stat`, `exists`, `create_dir`,
`remove_file`, `remove_dir`, `remove_dir_all`, `rename`, and the one-shot
transfers `read_file`, `write_file`, and `append_file`. Paths are absolute
and rooted at the share. The table lists the `Capabilities` flags each client
advertises; `exec` and `symlink` are unsupported everywhere.

| Capability     | Rust-native (`SmbFs`) | pavao (`PavaoSmbFs`) | WNet (`WNetSmbFs`) |
| -------------- | --------------------- | -------------------- | ------------------ |
| `STREAM_READ`  | Yes                   | No                   | Yes                |
| `STREAM_WRITE` | Yes                   | No                   | Yes                |
| `APPEND`       | Yes                   | Yes                  | Yes                |
| `RANGE_READ`   | Yes                   | Yes                  | Yes                |
| `SEEK_READ`    | Yes                   | No                   | Yes                |
| `SEEK_WRITE`   | Yes                   | No                   | Yes                |
| `COPY`         | Yes                   | No                   | Yes                |
| `SYMLINK`      | No                    | No                   | No                 |
| `SET_METADATA` | Yes (times)           | Yes (mode)           | Yes (times)        |
| `POSIX_MODE`   | No                    | Yes                  | No                 |
| `EXEC`         | No                    | No                   | No                 |

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
and `test`, and is the required gate before opening a pull request. Most
tests need the Samba container from `tests/docker-compose.yml` running
locally (`docker compose -f tests/docker-compose.yml up -d --build`) before
`just test "--no-default-features --features find,pavao,with-containers"` will pass.
Pass `--features smb` (or `find,pavao,smb,with-containers`) to cover the
Rust-native client; it shares the same container.

See [AGENTS.md](AGENTS.md) for the full contract.

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
- [smb-rs](https://github.com/afiffon/smb-rs)

---

## License 📃

remotefs-smb is licensed under the MIT license.

You can read the entire license [HERE](LICENSE)
