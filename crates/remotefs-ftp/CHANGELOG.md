# Changelog

All notable changes to this project are documented in this file.

## 1.2.0

Released on 2026-11-23

### Breaking changes

- SuppaFTP 7

> Renamed features

- Replaced `rustls` feature with `rustls-aws-lc-rs` and `rustls-ring` features to choose the desired rustls backend

> `rustls` feature has been removed. Use `rustls-aws-lc-rs` or `rustls-ring` instead

- migrate to remotefs 1

> migrate FtpFs to the remotefs 1 blocking contract on top of suppaftp 12 and release version 1.0.0. Every path must be absolute, pwd and change_dir are removed, operations take a shared reference, connect returns unit and the server banner moves to FtpFs::welcome_message, transfers return owned streams that must be finished, and the minimum supported Rust version is 1.89.0.

### Added

- remotefs 0.3
- Added the `passive_stream_builder` option
- implement exec via the FTP SITE command

> exec() was a stub returning UnsupportedFeature. Implement it on top of
> suppaftp's site(), which is the closest FTP primitive to a generic
> exec: it only runs commands the server exposes as SITE subcommands
> (e.g. CHMOD, HELP), not arbitrary shell commands.

- Breaking: migrate to remotefs 1
- add tokio transfer streams
- add tokio ftp client connection lifecycle
- implement async filesystem operations for tokio ftp client
- allow relative and dangling symlink targets (#41)

> symlink(path, target) no longer requires an absolute, existing target. The target is stored verbatim on every backend, so relative and dangling links can be created; only an empty target is rejected, with InvalidPath. The new remotefs::path::ensure_symlink_target helper implements that check. KubeMultiPodFs writes relative targets inside the link's container and keeps translating absolute targets within the same container.

### Changed

- split ftp client into sync module and shared helpers

### Fixed

- bump `suppaftp` to `6.0.0`
- revert webkpi
- tests
- lint
- removed users dep
- rustls 0.23 compatibility
- test is sync and send
- path_slash 0.2 fixup
- gate tokio-only test helpers
- compile path validation on Windows
- **remotefs-ftp:** send REST right before RETR

> suppaftp 12.2 follows RFC 3659 section 5.3: REST must immediately precede
> the transfer command, so it now goes out after PASV and right before RETR.
> The client only stores the offset, and a refused marker surfaces from the
> transfer command. The read path already retries without REST and skips the
> prefix locally, so behavior is unchanged for servers that refuse the marker.
>
> The scripted tests expect PASV, then REST, then RETR, and a new test covers
> the refusal fallback for both the sync and tokio backends.

### Build

- Breaking: SuppaFTP 7

> Edition 2024
> MSRV 1.85.1
> Removed `secure` feature
> Renamed `vendored` feature to `native-tls-vendored`

- Breaking: Replaced `rustls` feature with `rustls-aws-lc-rs` and `rustls-ring` features to choose the desired rustls backend
- msrv 1.88
- bump suppaftp to 11.0.0 (remotefs-rs/remotefs-rs-ftp#2)
- add tokio feature family
- **release:** bump every workspace crate version
- unify crates in a cargo workspace at version 1.2.0

> Every member crate shares version 1.2.0 and inherits its package metadata
> from the workspace root. Every third-party dependency version now lives in
> the root `[workspace.dependencies]` table, and members reference it with
> `workspace = true`, keeping only their own features and optional flags.
>
> The lockfile is committed so the workspace resolves the same versions
> everywhere. Cargo.lock is no longer ignored, and the toolchain is pinned to
> 1.100.0.

- **msrv:** bump msrv to 1.98
- inherit the workspace lints in every crate

> Eight crates lacked a [lints] table, so the workspace lint rules were
> never applied to them and cargo warned about the missing inheritance.
>
> Enabling them surfaced the findings they had been hiding:
>
> - remove imports already provided by the prelude or a glob import
> - implement Debug for the public clients and SSH options; secrets such
>   as passwords and key storage are left out of the output
> - use expect_err in a remotefs-smb test now that its Ok type is Debug
