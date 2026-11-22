# Changelog

All notable changes to this project are documented in this file.

## Unreleased

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

## remotefs-memory-v1.0.0

Released on 2026-09-15

### Breaking changes

- migrate to remotefs 1 (remotefs-rs/remotefs-rs-memory#2)

> migrate MemoryFs to the remotefs 1 blocking contract and release version 1.0.0. Every path must be absolute, pwd and change_dir are removed, operations take a shared reference over a mutex-guarded tree, connect returns unit, transfers return owned streams that commit on finish, rename and copy move whole subtrees, and the minimum supported Rust version is 1.89.

- chore: release 1.0]

- chore: add release script

### Added

- Breaking: migrate to remotefs 1 (remotefs-rs/remotefs-rs-memory#2)

> - feat!: migrate to remotefs 1

### Fixed

- send + sync
- bump
- Return WriteStream as WriteAndSeek

## remotefs-memory-v0.1.4

Released on 2024-10-25

### Fixed

- symlink issues

## remotefs-memory-v0.1.2

Released on 2024-10-23

### Fixed

- ver
- log

## remotefs-memory-v0.1.1

Released on 2024-10-22

### Fixed

- `with_get_uid` and `with_get_gid` constructors

## remotefs-memory-v0.1.0

Released on 2024-10-22

### Added

- remotefs memory
