# Changelog

All notable changes to this project are documented in this file.

## Unreleased

### Fixed

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

## remotefs-ftp-v1.0.0

Released on 2026-09-15

### Breaking changes

- migrate to remotefs 1

> migrate FtpFs to the remotefs 1 blocking contract on top of suppaftp 12 and release version 1.0.0. Every path must be absolute, pwd and change_dir are removed, operations take a shared reference, connect returns unit and the server banner moves to FtpFs::welcome_message, transfers return owned streams that must be finished, and the minimum supported Rust version is 1.89.0.

### Added

- Breaking: migrate to remotefs 1
- add tokio transfer streams
- add tokio ftp client connection lifecycle
- implement async filesystem operations for tokio ftp client

### Changed

- split ftp client into sync module and shared helpers

### Fixed

- gate tokio-only test helpers
- compile path validation on Windows

### Build

- add tokio feature family

## remotefs-ftp-v0.5.0

Released on 2026-09-08

### Added

- implement exec via the FTP SITE command

> exec() was a stub returning UnsupportedFeature. Implement it on top of
> suppaftp's site(), which is the closest FTP primitive to a generic
> exec: it only runs commands the server exposes as SITE subcommands
> (e.g. CHMOD, HELP), not arbitrary shell commands.

## remotefs-ftp-v0.4.1

Released on 2026-08-31

### Build

- bump suppaftp to 11.0.0 (remotefs-rs/remotefs-rs-ftp#2)

## remotefs-ftp-v0.4.0

Released on 2026-01-18

### Breaking changes

- Replaced `rustls` feature with `rustls-aws-lc-rs` and `rustls-ring` features to choose the desired rustls backend

> `rustls` feature has been removed. Use `rustls-aws-lc-rs` or `rustls-ring` instead

### Build

- Breaking: Replaced `rustls` feature with `rustls-aws-lc-rs` and `rustls-ring` features to choose the desired rustls backend
- msrv 1.88

## remotefs-ftp-v0.3.0

Released on 2025-08-31

### Breaking changes

- SuppaFTP 7

> Renamed features

### Added

- Added the `passive_stream_builder` option

### Fixed

- test is sync and send
- path_slash 0.2 fixup

### Build

- Breaking: SuppaFTP 7

> Edition 2024
> MSRV 1.85.1
> Removed `secure` feature
> Renamed `vendored` feature to `native-tls-vendored`

## remotefs-ftp-v0.2.2

Released on 2024-10-18

### Fixed

- rustls 0.23 compatibility

## remotefs-ftp-v0.2.1

Released on 2024-10-07

### Fixed

- removed users dep

## remotefs-ftp-v0.2.0

Released on 2024-09-30

### Added

- remotefs 0.3

### Fixed

- bump `suppaftp` to `6.0.0`
- revert webkpi
- tests
- lint

## remotefs-ftp-v0.1.1

Released on 2022-01-04
