# Changelog

All notable changes to this project are documented in this file.

## Unreleased

### Breaking changes

- **remotefs-smb:** remove the rust-native smb client

> the `smb` feature and the `SmbFs`, `BlockingSmbFs`,
> `SmbOptions`, `SmbCredentials`, and `SmbEncryptionLevel` items are removed.
> Use `PavaoSmbFs` on UNIX or `WNetSmbFs` on Windows instead.

### Added

- Breaking: **remotefs-smb:** remove the rust-native smb client

> The smb crate and its sspi dependency pull prerelease RustCrypto crates,
> and they cannot share one lockfile with russh. The rust-native backend is
> removed until a stable smb release exists. The pavao client on UNIX and
> the WNet client on Windows are unchanged.

### Fixed

- **remotefs-smb:** keep filetime for Windows metadata updates

> filetime is deprecated in favor of std::fs::set_times, but std cannot open
> directories or read-only files on Windows, and filetime handles both. The
> deprecated calls keep their behavior under a scoped expect with the reason.

### Build

- **release:** bump every workspace crate version

### Style

- format imported crates with workspace rustfmt

## remotefs-smb-v1.0.1

Released on 2026-11-03

### Fixed

- accept backslash separators in share paths (remotefs-rs/remotefs-rs-smb#15)

> - fix: accept backslash separators in share paths
>
> Paths built with Path::join on Windows contain a backslash, which every client rejected as an invalid SMB name character since the move to remotefs 1. A backslash can never be part of an SMB name, so share paths now treat both / and \ as separators on every platform, while .. and non-share roots are still rejected. The Rust-native client also built listed entry paths with the platform separator, so on Windows it returned entries such as /docs\a.txt; they now always use /.
>
> closes remotefs-rs/remotefs-rs-smb#14
>
> - chore: ignore RUSTSEC-2026-0387 in cargo-deny config
>
> The crypto-bigint advisory has no fix reachable without a major update of the smb crate. smb 0.11 pins sspi exactly, and sspi pins crypto-bigint exactly, so cargo update cannot move it. The ignore keeps just deny passing. It is revisited on every smb or sspi bump.

## remotefs-smb-v1.0.0

Released on 2026-09-15

### Breaking changes

- migrate to remotefs 1 (remotefs-rs/remotefs-rs-smb#13)

> remotefs-smb now depends on remotefs 1. SmbFs is async (AsyncRemoteFs) and no longer takes an Arc<Runtime>; use SmbFs::into_blocking for RemoteFs. pwd and change_dir are gone: every path is absolute and rooted at the share. PavaoSmbFs offers one-shot transfers only (open, create, and append return UnsupportedFeature). Released as 1.0.0.

- chore: release 1.0.0

- fix(windows): satisfy clippy on windows

- chore: release 1.0

### Added

- Breaking: migrate to remotefs 1 (remotefs-rs/remotefs-rs-smb#13)

> - feat!: migrate to remotefs 1
>
> SmbFs implements remotefs::AsyncRemoteFs natively and drops the runtime constructor parameter; SmbFs::into_blocking returns BlockingSmbFs for blocking callers. PavaoSmbFs and WNetSmbFs implement the remotefs 1 blocking contract. Every client takes absolute share-rooted paths, advertises capabilities, returns owned streams finished explicitly, and maps failures onto the remotefs 1 error taxonomy.

## remotefs-smb-v0.5.0

Released on 2026-09-02

### Breaking changes

- add rust-native smb client (remotefs-rs/remotefs-rs-smb#11)

> the pavao client types are now PavaoSmbFs, PavaoSmbCredentials, PavaoSmbOptions, PavaoSmbEncryptionLevel, and PavaoSmbShareMode; the Windows client types are now WNetSmbFs and WNetSmbCredentials. The plain SmbFs, SmbCredentials, SmbOptions, and SmbEncryptionLevel names belong to the new Rust-native client behind the smb feature.

### Added

- Breaking: add rust-native smb client (remotefs-rs/remotefs-rs-smb#11)

> Add SmbFs, a RemoteFs implementation built on the pure-Rust smb crate, gated behind the new smb feature and driven by a caller-supplied Tokio runtime. Gate the libsmbclient client behind the pavao feature (still on by default) so both clients build together, document all clients, and bump to 0.5.0.

## remotefs-smb-v0.4.0

Released on 2026-09-02

### Added

- add smb dialect protocol versions

> Expose a shared dialect API, enforce secure SMB2 and SMB3 negotiation bounds on Unix, and provide cross-platform constructor parity on Windows.

### Fixed

- mark smb client doctest as no_run so it does not need a live server

> The lib.rs example connects to a live SMB server unconditionally,
> unlike the with-containers-gated unit tests, so `just test` failed
> without Docker running. no_run keeps it compiled and type-checked
> without executing it.

- create container test fixtures over SMB instead of the host fs

> The samba container's entrypoint recursively chowns/chmods the
> bind-mounted share root, which on Linux CI runners strips write access
> from the host's shared /tmp for the CI user. init_client/finalize_client
> used to mkdir/rmdir /cargo-test directly on the host, so every
> with-containers test failed with a permission error before ever
> exercising the client.

### Build

- bump pavao to 0.3.0

## remotefs-smb-v0.3.1

Released on 2025-03-20

### Fixed

- test is sync and send
- added `vendored` feature to vendor `libsmbclient`

## remotefs-smb-v0.3.0

Released on 2024-09-30

### Added

- remotefs 0.3

### Fixed

- lint

## remotefs-smb-v0.2.1

Released on 2023-12-15

### Added

- 0.2.1

## remotefs-smb-v0.2.0

Released on 2023-05-13

### Added

- windows client
- windows SmbCredentials
- example
- windows client

### Fixed

- removed utils from windows
- lint
- macos types
- borrow path
- tests
- env! SMB_SHARE/SMB_SERVER

## remotefs-smb-v0.1.0

Released on 2022-05-27
