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

## remotefs-gcs-v1.0.0

Released on 2026-09-15

### Breaking changes

- migrate to remotefs 1 (remotefs-rs/remotefs-rs-gcs#2)

> GoogleCloudStorageFs now uses absolute paths, exposes AsyncRemoteFs, removes the runtime constructor parameter, and releases as 1.0.0.

- chore: update release date

- chore: windows lint

### Added

- Breaking: migrate to remotefs 1 (remotefs-rs/remotefs-rs-gcs#2)

> - feat!: migrate to remotefs 1
>
> Migrate Google Cloud Storage to the native AsyncRemoteFs contract with owned transfer streams and blocking adapter support.

## remotefs-gcs-v0.1.0

Released on 2026-08-29

### Added

- add Google Cloud Storage client

### Changed

- remove useless mod visibility
