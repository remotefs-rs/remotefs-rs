# Changelog

All notable changes to this project are documented in this file.

## 1.2.0

Released on 2026-11-23

### Breaking changes

- migrate to remotefs 1 (remotefs-rs/remotefs-rs-gcs#2)

> GoogleCloudStorageFs now uses absolute paths, exposes AsyncRemoteFs, removes the runtime constructor parameter, and releases as 1.0.0.

- chore: update release date

- chore: windows lint

### Added

- add Google Cloud Storage client
- Breaking: migrate to remotefs 1 (remotefs-rs/remotefs-rs-gcs#2)

> - feat!: migrate to remotefs 1
>
> Migrate Google Cloud Storage to the native AsyncRemoteFs contract with owned transfer streams and blocking adapter support.

- allow relative and dangling symlink targets (#41)

> symlink(path, target) no longer requires an absolute, existing target. The target is stored verbatim on every backend, so relative and dangling links can be created; only an empty target is rejected, with InvalidPath. The new remotefs::path::ensure_symlink_target helper implements that check. KubeMultiPodFs writes relative targets inside the link's container and keeps translating absolute targets within the same container.

### Changed

- remove useless mod visibility

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

- **msrv:** bump msrv to 1.98
