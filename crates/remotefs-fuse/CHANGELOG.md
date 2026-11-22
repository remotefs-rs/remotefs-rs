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

## fusibile-v1.0.1

Released on 2026-09-22

### Fixed

- apply uid and gid overrides to file attributes (remotefs-rs/remotefs-rs-fuse#13)

> - test: correct uid and gid fixture callbacks
> - fix: apply uid and gid overrides to file attributes
> - chore: release 1.0.1

## fusibile-v1.0.0

Released on 2026-09-16

### Breaking changes

- migrate to remotefs 1

> Mount and Driver now require remotefs 1.x APIs. Add AsyncMount and AsyncUnmount behind the tokio feature and remove the previous Mount::mount_async API. fusibile now requires async-capable backend implementations and no longer exposes the old synchronous RemoteFsWrapper.

### Added

- Breaking: migrate to remotefs 1

> Migrate remotefs-fuse's synchronous drivers to remotefs 1 and extract shared transfer and platform state modules used by native async Unix and Windows drivers.
>
> Add tokio-gated AsyncMount and AsyncUnmount. Unix FUSE callbacks spawn runtime tasks, while Windows Dokany callbacks await through the runtime handle at the callback boundary.
>
> Make fusibile a Tokio application using AsyncRemoteFs backends, Unblock for memory and SMB, and signal-driven unmount coordination.
>
> Add futures-util and async test support, remove ctrlc and the blocking backend wrapper, and update backend dependencies to remotefs 1.

- make `password`, `username`, and `port` arguments for ssh optionals

> if these arguments are missing, they are inferred from the ssh config matching the host. Port fallbacks to `22` if unresolved

### Fixed

- show better error for macfuse issues

## fusibile-v0.2.0

Released on 2026-09-04

### Breaking changes

- **fusibile:** rename `remotefs-fuse-cli` to `fusibile` (remotefs-rs/remotefs-rs-fuse#6)

> the CLI package has changed from `remotefs-fuse-cli` to `fusibile`

### Added

- add Google Cloud Storage client (remotefs-rs/remotefs-rs-fuse#5)
- Breaking: **fusibile:** rename `remotefs-fuse-cli` to `fusibile` (remotefs-rs/remotefs-rs-fuse#6)
- **remotefs-fuse:** add optional libfuse feature and ship LICENSE in published crates

### Fixed

- fix data-loss, collision, and credential-leak bugs found in review (remotefs-rs/remotefs-rs-fuse#8)

> - chore: update fundings
> - fix(remotefs-fuse): fix data-loss, collision, and unbounded-allocation bugs in the drivers
>
> * write() on Unix and write_file() on Windows called remote.create()
>   (create-or-truncate) on every single write, so any file written via
>   more than one write call ended up containing only the last chunk.
>   Writes are now staged per open handle and only persisted once, on
>   flush/release (Unix) or flush_file_buffers/cleanup (Windows).
> * Unix file handle numbers could collide: a freed handle was reused via
>   handles.len(), which could still equal a currently open handle's
>   number after further opens/closes, silently aliasing two open files
>   under one fh.
> * Unix inodes were derived from a path hash with no collision handling,
>   letting two unrelated paths alias onto the same inode. Inode
>   allocation is now a real bidirectional path<->inode table.
> * Reads at a caller-supplied offset (Unix and Windows) allocated a
>   zero-filled buffer sized directly from that offset, letting a
>   misbehaving remote trigger a multi-terabyte allocation; offsets are
>   now skipped by discarding bytes through a fixed-size copy instead.
>   The same issue in readlink's buffer, sized from the remote-reported
>   symlink length, is now capped to PATH_MAX.
>   the target's declared length is now capped to PATH_MAX.
> * Windows alternate-stream reads underflowed (and panicked) when the
>   read offset was past the stream's current length; this is now a
>   normal short/empty read like a regular file.
> * Windows delete_file() panicked on a poisoned stat lock instead of
>   returning an error like every other call site in the file.
> * EntryNameRef's unsafe pointer cast relied on layout equivalence with
>   U16Str without `#[repr(transparent)]`; the struct is now marked as
>   such and the cast documents its safety requirement.
>
> - fix(fusibile): stop leaking credentials via Debug and panicking on unmount failure
>
> * Passwords, access keys, and secret tokens taken as CLI args were
>   exposed by the derived Debug impl on each backend's Args struct,
>   which risks leaking them into logs. Debug is now hand-written to
>   redact those fields.
> * The Ctrl-C handler called umount.unmount().expect(...), so a
>   recoverable unmount failure (e.g. EBUSY) crashed the whole process
>   instead of just logging the error.
>
> - style(remotefs-fuse): enable core compiler lints and clean up doc/lint attributes
>
> * Add a workspace [lints.rust] table with the compiler lints from the
>   Microsoft Rust guidelines' M-STATIC-VERIFICATION (missing Debug
>   impls, redundant imports/lifetimes, unsafe fn hygiene, etc.), and
>   wire both crates into it.
> * Mount and Unmount now derive Debug (Driver already needed it to make
>   that possible), per M-PUBLIC-DEBUG.
> * Replace #[allow] lint overrides with #[expect(..., reason = "...")],
>   scoped to the specific items that actually need them instead of a
>   blanket allow across the whole DriverInner impl block.
> * Fix MountOption::Gid's doc comment, copy-pasted from Uid: it said the
>   option treats files as owned by a given user, but it sets the group.
> * Document remotefs-fuse's feature flags in a table, and stop teaching
>   the panicking unmount pattern in the crate's own doc example.
>
> - fix: fix CI failures from the previous commits
>
> * windows.rs: skip_bytes referenced the bare Read trait name, which
>   only compiles with an unaliased `use std::io::Read`; the crate
>   imports it as `Read as _` (methods only), so the Windows build
>   failed outright. Use the fully qualified `std::io::Read` instead.
> * Drop trivial_numeric_casts from the workspace lints: several mode_t
>   casts in the Unix driver are only trivial on Linux (mode_t is a
>   narrower type on macOS/BSD), so the lint can't be satisfied on every
>   target without per-cast, per-platform attributes.
> * fusibile: the #[expect(clippy::large_enum_variant)] on RemoteFsWrapper
>   was unfulfilled when built with zero or one backend feature (no size
>   disparity to flag with a single variant), which is exactly the
>   --no-default-features CI configuration. Use #[allow] instead, since
>   whether this lint fires depends on which backend features are enabled.

- **release:** drop smb-vendored from the intel macOS build

> pavao-src's vendored Samba build fails to link on macos-15-intel runners
> with undefined libintl symbols building tdb. Confirmed by a real dry run
> of the release workflow. Ship that target without SMB instead.

- **driver:** log per-syscall FUSE/Dokany handlers at debug instead of info

> These fire on every filesystem operation (read, write, getattr,
> lookup, readdir, ...), flooding logs at the default info level.
> Lifecycle events (mount, unmount, connect, disconnect) stay at info.

### Build

- bump fuser to 0.18 and nix to 0.31
- bump remotefs-smb to 0.5
- bump remotefs-ssh to 0.9
- add remotefs-smb windows compatibility

## fusibile-v0.1.0

Released on 2024-12-19

### Added

- CLI tool
- wip
- wip
- file handlers and read
- file handlers by pid
- implemented fuse
- memory fs in cli tool
- unix is kinda working
- new mount
- gid and uid mount options
- default mode option
- windows base; use Generic instead of Boxes
- working on windows
- working on windows driver
- dokany impl

### Fixed

- initial commit
- readme
- workspace with -cli
- wip
- macos
- fmt
- test mount
- macos ci
- tests
- test
- setup tests
- setup fuse
- test check access
- use nix
- macos
- test
- min rust version
- integration tests
- windows build
- windows build
- set mount option on mount
- don't expose Driver
- generic remoteFs
- import
- linux
- docs
- test
- linux
- windows recursion issue
- should list only direct children
- fuser 0.15
- integration tests for windows
- tests
- cargo toml
- gran coglioni
- umount
- lint
