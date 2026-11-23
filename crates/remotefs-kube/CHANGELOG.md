# Changelog

All notable changes to this project are documented in this file.

## remotefs-kube-v1.2.0

Released on 2026-11-23

### Breaking changes

- migrate to remotefs 1 (remotefs-rs/remotefs-rs-kube#3)

> requires remotefs 1; constructors, path handling, and transfer methods changed.

- chore: release 1.0.0

- fix: handle kube exec command completion

- fix: wait for stdin eof on hinted writes

- fix: request output stream for writes

- fix: bound hinted kube uploads

* **remotefs-kube:** preserve symlink targets verbatim (#43)

> KubeMultiPodFs stores absolute targets as supplied. Pass a path inside the container, such as /etc/hosts, without the virtual pod and container prefix. SCP and Kubernetes symlink creation require Perl or ln with -T on the remote host.

### Added

- first commit
- added container name
- multi pod (remotefs-rs/remotefs-rs-kube#1)

> - feat: multi pod client
> - fix: tests and fixes
> - fix: version
> - fix: changelog

- remotefs 0.3
- Breaking: migrate to remotefs 1 (remotefs-rs/remotefs-rs-kube#3)

> - feat!: migrate to remotefs 1
>
> KubeContainerFs and KubeMultiPodFs now implement remotefs::AsyncRemoteFs natively and no longer take a Tokio runtime. Every path must be an absolute POSIX path; pwd and change_dir are gone. open, create, and append return owned streams over the pod exec stream that must be finished explicitly; read_file and write_file replace open_file and create_file, rename replaces mov, set_metadata replaces setstat, and exec returns ExecOutput. Read offsets and lengths are honored natively and capabilities() is advertised. Blocking callers enable the tokio feature and call into_blocking to get a BlockingKubeContainerFs or BlockingKubeMultiPodFs implementing remotefs::RemoteFs.

- allow relative and dangling symlink targets (#41)

> symlink(path, target) no longer requires an absolute, existing target. The target is stored verbatim on every backend, so relative and dangling links can be created; only an empty target is rejected, with InvalidPath. The new remotefs::path::ensure_symlink_target helper implements that check. KubeMultiPodFs writes relative targets inside the link's container and keeps translating absolute targets within the same container.

### Changed

- **ssh:** replace chrono with jiff for file times (#42)

> The ssh and kube backends now use jiff to format the times they send to
> remote hosts and to read the times printed by remote ls. Dates users see
> do not change.
>
> A time past the latest date jiff supports used to panic. It now shows
> that latest date, and a time before the earliest supported date shows
> that date instead.
>
> chrono is still in the dependency graph through russh-util, which russh
> pulls in for wasm32 builds, so its advisory exception stays in deny.toml.

### Fixed

- working on tests
- io
- bump version
- test is sync and send
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
- **deps:** upgrade suppaftp, nix, kube, k8s-openapi and fuser

> Move to suppaftp 13, nix 0.32, kube 5, k8s-openapi 0.29 and fuser 0.19.
>
> k8s-openapi 0.29 drops the v1_32 feature, so remotefs-kube now selects v1_37.
>
> fuser 0.19 adds owner and kill_suid_gid arguments to several Filesystem
> methods and makes the request uid and gid optional. The sync and async
> FUSE drivers follow the new signatures, and a caller without ids is
> treated as an unknown user that only gets the permissions granted to
> everyone.

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
