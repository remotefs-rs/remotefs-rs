# remotefs SSH

[![license-mit](https://img.shields.io/crates/l/remotefs-ssh.svg)](https://opensource.org/licenses/MIT)
[![repo-stars](https://img.shields.io/github/stars/remotefs-rs/remotefs-rs-ssh?style=flat)](https://github.com/remotefs-rs/remotefs-rs-ssh/stargazers)
[![downloads](https://img.shields.io/crates/d/remotefs-ssh.svg)](https://crates.io/crates/remotefs-ssh)
[![latest-version](https://img.shields.io/crates/v/remotefs-ssh.svg)](https://crates.io/crates/remotefs-ssh)
[![ko-fi](https://img.shields.io/badge/donate-ko--fi-red)](https://ko-fi.com/veeso)
[![conventional-commits](https://img.shields.io/badge/Conventional%20Commits-1.0.0-%23FE5196?logo=conventionalcommits&logoColor=white)](https://conventionalcommits.org)

[![Build](https://github.com/remotefs-rs/remotefs-rs-ssh/actions/workflows/ci.yml/badge.svg)](https://github.com/veeso/remotefs-rs-ssh/actions/workflows/ci.yml)
[![coveralls](https://coveralls.io/repos/github/remotefs-rs/remotefs-rs-ssh/badge.svg)](https://coveralls.io/github/veeso/remotefs-rs-ssh)
[![docs](https://docs.rs/remotefs-ssh/badge.svg)](https://docs.rs/remotefs-ssh)

---

## About remotefs-ssh ☁️

remotefs-ssh is a client implementation for [remotefs](https://github.com/remotefs-rs/remotefs-rs), providing support
for the SFTP/SCP protocol.

---

## Get started 🚀

First of all, add `remotefs-ssh` to your project dependencies:

```toml
remotefs = "1"
remotefs-ssh = "1"
```

> [!NOTE]
> The library supports multiple ssh backends.
> Currently `libssh2`, `libssh`, and `russh` are supported.
>
> By default the library is using `libssh2`.

### Available backends

Each backend can be set as a feature in your `Cargo.toml`. Multiple backends can be enabled at the same time.

- `libssh2`: The default backend, using the `libssh2` library for SSH connections.
- `libssh`: An alternative backend, using the `libssh` library for SSH connections.
- `russh`: A pure-Rust backend, using the `russh` library for SSH connections. Does not require any system C libraries.

Each C backend can be built with the vendored version, using the vendored feature instead:

- `libssh2-vendored`: Build the `libssh2` backend with the vendored version of the library.
- `libssh-vendored`: Build the `libssh` backend with the vendored version of the library.

If the vendored feature is **NOT** provided, you will need to have the corresponding system libraries installed on your
machine.

### Other features

these features are supported:

- `find`: enable `find()` method on client (_enabled by default_)
- `no-log`: disable logging. By default, this library will log via the `log` crate.

## Ssh client

The blocking `SftpFs` and `ScpFs` clients use the `libssh2` or `libssh` backends.
The `russh` backend provides native asynchronous `RusshSftpFs` and `RusshScpFs`
clients, plus blocking wrappers for applications that need the `RemoteFs` trait.
Every path passed to a client must be absolute.

### libssh2 / libssh example

```rust,ignore
use std::io::Cursor;
use std::path::Path;

use remotefs::RemoteFs;
use remotefs::fs::{ReadOptions, WriteOptions};
use remotefs_ssh::{SftpFs, SshConfigParseRule, SshOpts};

let opts = SshOpts::new("127.0.0.1")
    .port(22)
    .username("test")
    .password("password")
    .config_file(Path::new("/home/cvisintin/.ssh/config"), SshConfigParseRule::STRICT);

let mut client = SftpFs::libssh2(opts);
client.connect()?;
let data = b"hello";
client.write_file(
    Path::new("/tmp/hello.txt"),
    &WriteOptions::default().size_hint(data.len() as u64),
    &mut Cursor::new(data),
)?;
let mut output = Vec::new();
client.read_file(Path::new("/tmp/hello.txt"), &ReadOptions::default(), &mut output)?;
assert_eq!(output, data);
client.disconnect()?;
# Ok::<(), remotefs::RemoteError>(())
```

### russh (async)

The `russh` backend runs on the Tokio runtime that drives its futures. A type
implementing `russh::client::Handler` controls server key verification;
`NoCheckServerKey` accepts every host key.

```rust,ignore
use std::path::Path;

use remotefs::AsyncRemoteFs;
use remotefs_ssh::{NoCheckServerKey, RusshSftpFs, SshOpts};

#[tokio::main]
async fn main() -> remotefs::RemoteResult<()> {
    let mut client: RusshSftpFs<NoCheckServerKey> = RusshSftpFs::new(
        SshOpts::new("127.0.0.1")
            .username("test")
            .password("password"),
    );
    client.connect().await?;
    for entry in client.list_dir(Path::new("/tmp")).await? {
        println!("{name}", name = entry.name());
    }
    client.disconnect().await
}
```

### russh (blocking wrapper)

```rust,ignore
use remotefs::RemoteFs;
use remotefs_ssh::{NoCheckServerKey, RusshSftpFs, SshOpts};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let runtime = tokio::runtime::Runtime::new()?;
    let mut client: Box<dyn RemoteFs> = Box::new(
        RusshSftpFs::<NoCheckServerKey>::new(SshOpts::new("127.0.0.1"))
            .into_blocking(runtime.handle().clone()),
    );
    client.connect()?;
    client.disconnect()?;
    Ok(())
}
```

---

### Client compatibility table

The following table states the compatibility for each client and the remote
file system trait method. `connect()`, `disconnect()`, and `is_connected()` are
always supported and are omitted.

| Client/Method  | Scp                    | Sftp |
| -------------- | ---------------------- | ---- |
| append_file    | No                     | Yes  |
| append         | No                     | Yes  |
| copy           | Yes                    | Yes  |
| create_dir     | Yes                    | Yes  |
| create         | Yes (size hint needed) | Yes  |
| exec           | Yes                    | Yes  |
| exists         | Yes                    | Yes  |
| list_dir       | Yes                    | Yes  |
| open           | Yes (no seek/range)    | Yes  |
| read_file      | Yes                    | Yes  |
| remove_dir_all | Yes                    | Yes  |
| remove_dir     | Yes                    | Yes  |
| remove_file    | Yes                    | Yes  |
| rename         | Yes                    | Yes  |
| set_metadata   | Yes                    | Yes  |
| stat           | Yes                    | Yes  |
| symlink        | Yes                    | Yes  |
| write_file     | Yes                    | Yes  |

`capabilities()` reports the same information at runtime
(`SFTP_CAPABILITIES`, `SCP_CAPABILITIES`).

## Migrating from 0.9

- Paths are absolute only.
- `pwd` and `change_dir` were removed.
- `connect` returns `()`; the server banner is available through `SftpFs::banner` and `ScpFs::banner` for the `libssh2` and `libssh` backends.
- `open_file` and `create_file` became `read_file` and `write_file`, using `ReadOptions` and `WriteOptions`.
- SCP `create` requires `WriteOptions::size_hint`.
- Streams must be explicitly finished with `finish`.
- `mov` became `rename`.
- `setstat` became `set_metadata`.
- `SftpFs::russh(opts, runtime)` became `RusshSftpFs::new(opts)` for async use, or `RusshSftpFs::new(opts).into_blocking(handle)` for blocking use.
- See the [canonical migration guide](https://github.com/remotefs-rs/remotefs-rs/blob/main/MIGRATION.md).

---

## Development 🛠️

Every task runs through a [`just`](https://just.systems) recipe. Run `just`
to list them all.

```sh
just build                 # cargo build --all-targets
just test                  # cargo test --all-targets, then --doc
just coverage              # cargo llvm-cov, writes lcov.info
just fmt                   # dprint fmt (Markdown, Rust, TOML, YAML)
just fmt_check             # dprint check
just lint "-- -D warnings" # clippy with all features
just doc                   # cargo doc --all-features
just deny                  # cargo deny check
just scan_secrets          # trufflehog filesystem
just check                 # the full local quality gate
```

None of the SSH backend features are enabled by default beyond `find` and
`libssh2`. Backend-gated code and its examples/benches only build with the
matching feature passed explicitly:

```sh
just build "--features russh"
just test "--features libssh,russh"
```

`just check` chains `fmt_check`, Clippy with warnings denied, `doc`, `deny`,
and `test`, and is the required gate before opening a pull request. Some
tests start real SSH/SFTP containers via `testcontainers`, so Docker must be
available locally.

See [AGENTS.md](AGENTS.md) for the full contract.

---

## Support the developer ☕

If you like remotefs-ssh and you're grateful for the work I've done, please consider a little donation 🥳

You can make a donation with one of these platforms:

[![ko-fi](https://img.shields.io/badge/Ko--fi-F16061?style=for-the-badge&logo=ko-fi&logoColor=white)](https://ko-fi.com/veeso)
[![PayPal](https://img.shields.io/badge/PayPal-00457C?style=for-the-badge&logo=paypal&logoColor=white)](https://www.paypal.me/chrisintin)
[![bitcoin](https://img.shields.io/badge/Bitcoin-ff9416?style=for-the-badge&logo=bitcoin&logoColor=white)](https://btc.com/bc1qvlmykjn7htz0vuprmjrlkwtv9m9pan6kylsr8w)

---

## Contributing and issues 🤝🏻

Contributions, bug reports, new features, and questions are welcome! 😉
If you have any questions or concerns, or you want to suggest a new feature, or you want just want to improve remotefs,
feel free to open an issue or a PR.

Please follow [our contributing guidelines](CONTRIBUTING.md)

---

## Changelog ⏳

View remotefs-ssh changelog [HERE](CHANGELOG.md)

---

## Powered by 💪

remotefs-ssh is powered by these awesome projects:

- [ssh2-config](https://github.com/veeso/ssh2-config)
- [ssh2-rs](https://github.com/alexcrichton/ssh2-rs)
- [russh](https://github.com/warp-tech/russh)

---

## License 📃

remotefs-ssh is licensed under the MIT license.

You can read the entire license [HERE](LICENSE)
