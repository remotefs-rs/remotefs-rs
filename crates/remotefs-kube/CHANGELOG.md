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

## remotefs-kube-v1.0.0

Released on 2026-09-15

### Breaking changes

- migrate to remotefs 1 (remotefs-rs/remotefs-rs-kube#3)

> requires remotefs 1; constructors, path handling, and transfer methods changed.

- chore: release 1.0.0

- fix: handle kube exec command completion

- fix: wait for stdin eof on hinted writes

- fix: request output stream for writes

- fix: bound hinted kube uploads

### Added

- Breaking: migrate to remotefs 1 (remotefs-rs/remotefs-rs-kube#3)

> - feat!: migrate to remotefs 1
>
> KubeContainerFs and KubeMultiPodFs now implement remotefs::AsyncRemoteFs natively and no longer take a Tokio runtime. Every path must be an absolute POSIX path; pwd and change_dir are gone. open, create, and append return owned streams over the pod exec stream that must be finished explicitly; read_file and write_file replace open_file and create_file, rename replaces mov, set_metadata replaces setstat, and exec returns ExecOutput. Read offsets and lengths are honored natively and capabilities() is advertised. Blocking callers enable the tokio feature and call into_blocking to get a BlockingKubeContainerFs or BlockingKubeMultiPodFs implementing remotefs::RemoteFs.

### Fixed

- test is sync and send

## remotefs-kube-v0.4.0

Released on 2024-09-30

### Added

- remotefs 0.3

### Fixed

- bump version

## remotefs-kube-v0.3.0

Released on 2024-09-29

### Added

- first commit
- added container name
- multi pod (remotefs-rs/remotefs-rs-kube#1)

> - feat: multi pod client
> - fix: tests and fixes
> - fix: version
> - fix: changelog

### Fixed

- working on tests
- io
