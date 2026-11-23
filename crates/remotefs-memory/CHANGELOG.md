# Changelog

All notable changes to this project are documented in this file.

## 1.2.0

Released on 2026-11-23

### Breaking changes

- migrate to remotefs 1 (remotefs-rs/remotefs-rs-memory#2)

> migrate MemoryFs to the remotefs 1 blocking contract and release version 1.0.0. Every path must be absolute, pwd and change_dir are removed, operations take a shared reference over a mutex-guarded tree, connect returns unit, transfers return owned streams that commit on finish, rename and copy move whole subtrees, and the minimum supported Rust version is 1.89.

- chore: release 1.0]

- chore: add release script

* **remotefs-kube:** preserve symlink targets verbatim (#43)

> KubeMultiPodFs stores absolute targets as supplied. Pass a path inside the container, such as /etc/hosts, without the virtual pod and container prefix. SCP and Kubernetes symlink creation require Perl or ln with -T on the remote host.

### Added

- remotefs memory
- Breaking: migrate to remotefs 1 (remotefs-rs/remotefs-rs-memory#2)

> - feat!: migrate to remotefs 1

- allow relative and dangling symlink targets (#41)

> symlink(path, target) no longer requires an absolute, existing target. The target is stored verbatim on every backend, so relative and dangling links can be created; only an empty target is rejected, with InvalidPath. The new remotefs::path::ensure_symlink_target helper implements that check. KubeMultiPodFs writes relative targets inside the link's container and keeps translating absolute targets within the same container.

### Fixed

- `with_get_uid` and `with_get_gid` constructors
- ver
- log
- symlink issues
- send + sync
- bump
- Return WriteStream as WriteAndSeek
- Breaking: **remotefs-kube:** preserve symlink targets verbatim (#43)

> Apply the reviewed fixes for relative, absolute, and dangling targets across the symlink-capable clients.

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
