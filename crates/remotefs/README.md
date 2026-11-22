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
  <a href="https://opensource.org/licenses/MIT"><img src="https://img.shields.io/badge/License-MIT-teal.svg" alt="License-MIT" /></a>
  <a href="https://github.com/remotefs-rs/remotefs-rs/stargazers"><img src="https://img.shields.io/github/stars/remotefs-rs/remotefs-rs.svg?style=plain" alt="Repo stars" /></a>
  <a href="https://crates.io/crates/remotefs"><img src="https://img.shields.io/crates/d/remotefs.svg" alt="Downloads counter" /></a>
  <a href="https://crates.io/crates/remotefs"><img src="https://img.shields.io/crates/v/remotefs.svg" alt="Latest version" /></a>
  <a href="https://ko-fi.com/veeso"><img src="https://img.shields.io/badge/donate-ko--fi-red" alt="Ko-fi" /></a>
</p>

<p align="center">
  <a href="https://github.com/remotefs-rs/remotefs-rs/actions/workflows/ci.yml"><img src="https://github.com/remotefs-rs/remotefs-rs/actions/workflows/ci.yml/badge.svg" alt="CI" /></a>
  <a href="https://coveralls.io/github/remotefs-rs/remotefs-rs"><img src="https://coveralls.io/repos/github/remotefs-rs/remotefs-rs/badge.svg" alt="Coveralls" /></a>
  <a href="https://docs.rs/remotefs"><img src="https://docs.rs/remotefs/badge.svg" alt="Docs" /></a>
</p>

## About remotefs ☁️

remotefs is the protocol-agnostic contract for remote file systems. Version 1
provides blocking and runtime-neutral asynchronous traits, shared metadata and
error types, range-aware transfer options, and owned streams with explicit
completion.

This crate is the root of the family. It defines the contract and shared types,
but ships no client of its own. Backend crates provide implementations for
SFTP, SCP, FTP, AWS S3, Google Cloud Storage, Kubernetes, SMB, WebDAV, and
in-memory testing.

### Why remotefs ❓

remotefs lets applications support several protocols without protocol-specific
branches. Configure a backend, connect it, and use the same file-system API for
file managers, backup jobs, and FUSE mounts.

## Features 🎁

- 📁 One contract for popular remote file-transfer protocols
- 🔄 Blocking and runtime-neutral asynchronous APIs
- 📏 Absolute paths, explicit ranges, and capability reporting
- ✅ Owned streams with an explicit `finish` lifecycle
- 🤖 Tokio adapters for bridging blocking and async clients
- 🚀 Extensible backend interface with typed error causes

## Get started 🚀

Add remotefs to your dependencies:

```toml
remotefs = "1"
```

The supported features are:

| Feature  | Purpose                                                            | Default |
| -------- | ------------------------------------------------------------------ | ------- |
| `async`  | Enable `AsyncRemoteFs` and futures I/O transfer types.             | No      |
| `find`   | Enable the `find` and `find_async` explicit-root search functions. | Yes     |
| `tokio`  | Enable `BlockOn` and `Unblock` runtime adapters.                   | No      |
| `no-log` | Compile out logging through `log/max_level_off`.                   | No      |

Depend on this crate directly when writing a backend or generic consumer.
Otherwise, the backend crate re-exports the shared types you need.

### Client libraries 🔌

Backend crates are maintained separately. Choose a release compatible with
remotefs 1 from their project documentation:

- [aws-s3](https://github.com/remotefs-rs/remotefs-rs-aws-s3)
- [ftp](https://github.com/remotefs-rs/remotefs-rs-ftp)
- [gcs](https://github.com/remotefs-rs/remotefs-rs-gcs)
- [kube](https://github.com/remotefs-rs/remotefs-rs-kube)
- [memory](https://github.com/remotefs-rs/remotefs-rs-memory)
- [smb](https://github.com/remotefs-rs/remotefs-rs-smb)
- [ssh](https://github.com/remotefs-rs/remotefs-rs-ssh)
- [webdav](https://github.com/remotefs-rs/remotefs-rs-webdav)

To mount a backend as a local file system, see
[remotefs-fuse](https://github.com/remotefs-rs/remotefs-rs-fuse).

## Remote file system 💾

`RemoteFs` accepts absolute paths and exposes lifecycle, metadata, directory,
rename, copy, link, stream, one-shot transfer, and command operations. A
backend that cannot provide an operation returns
`RemoteErrorType::UnsupportedFeature`; treat that as a capability answer and
fall back accordingly.

All filesystem paths must be absolute. `path::ensure_absolute` accepts POSIX
roots, fully qualified Windows drive paths, and UNC server/share paths on every
client platform. Use `find` or `find_async` with an explicit absolute root for
recursive search.

Streams returned by `open`, `create`, and `append` own their protocol state.
They implement standard I/O (or futures I/O for async clients), may support
seeking, and must be consumed with `finish` exactly once. `ReadOptions` and
`WriteOptions` carry ranges, size hints, modes, and timestamps.

See [MIGRATION.md](MIGRATION.md) for the complete 0.3-to-1.0 mapping and
backend checklist.

## Development 🛠️

Every task runs through a [`just`](https://just.systems) recipe. Run `just` to
list them all.

```sh
just build                 # cargo build --all-targets
just test                  # cargo test --lib, then --doc
just test_features         # every supported feature combination
just dependency_tree       # inspect normal dependency edges
just coverage              # cargo llvm-cov, writes lcov.info
just fmt                   # dprint fmt (Markdown, Rust, TOML, YAML)
just fmt_check             # dprint check
just lint "-- -D warnings" # clippy
just doc                   # cargo doc --no-deps
just deny                  # cargo deny check
just zizmor                # audit GitHub Actions
just package_list          # inspect package contents without Cargo.lock
just scan_secrets          # trufflehog filesystem
just check                 # the full local quality gate
```

`just check` chains `fmt_check`, Clippy with warnings denied, `doc`, `deny`,
and `test`, and is the required gate before declaring work complete. CI also
tests async-only, Tokio, no-default, default, and all-feature builds.

See [AGENTS.md](AGENTS.md) for the repository contract.

## Support the developer ☕

If remotefs is useful to you, please consider supporting its developer through
[Ko-fi](https://ko-fi.com/veeso) or [PayPal](https://www.paypal.me/chrisintin).

## Contributing and issues 🤝🏻

Contributions, bug reports, new features, and questions are welcome. Please
follow [the contributing guidelines](CONTRIBUTING.md) and read [the AI policy](AI_POLICY.md)
before opening a pull request.

## Apps using remotefs 🚀

- [termscp](https://github.com/veeso/termscp)
- [remotefs-fuse](https://github.com/remotefs-rs/remotefs-rs-fuse)

## Changelog ⏳

See the [remotefs changelog](CHANGELOG.md).

## License 📃

remotefs is licensed under the MIT license. Read the full text in [LICENSE](LICENSE).
