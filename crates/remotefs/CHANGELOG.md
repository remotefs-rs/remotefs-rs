# Changelog

All notable changes to this project are documented in this file.

## Unreleased

### Changed

- move remotefs crate into crates/remotefs

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

## 1.0.1

Released on 2026-09-15

### Breaking changes

- preserve remote error sources and revise kinds

> replace public RemoteError fields and legacy error variants with source-preserving accessors and the remotefs 1.0 error taxonomy.

- add extensible filesystem value types

> make metadata sizes optional, add transfer options and capabilities, and require constructors for non-exhaustive public values.

- replace blocking transfers with owned streams

> replace the mutable blocking trait, legacy transfer methods, and marker stream types with shared-receiver operations and consuming stream finalizers.

- enforce absolute paths and harden transfer adapters

> remove WorkingDir, AsyncWorkingDir, and the working_dir module; callers must supply absolute remote paths.

- remove welcome from the connection contract

> remove Welcome and change RemoteFs::connect and AsyncRemoteFs::connect to return RemoteResult<()>.

### Added

- Breaking: preserve remote error sources and revise kinds
- Breaking: add extensible filesystem value types
- Breaking: replace blocking transfers with owned streams
- add runtime-neutral async transfer streams
- add the object-safe async filesystem contract
- add working-directory wrappers for both filesystem traits
- support explicit-root sync and async search
- bridge async backends into blocking clients with tokio
- offload blocking filesystem clients and transfers with tokio

### Changed

- adopt the module_name.rs layout

> Replace the legacy `module/mod.rs` files with the `module.rs` form introduced
> by the 2018 edition, so editor tabs name the module instead of showing four
> identical `mod.rs` entries.

- satisfy Clippy on the pinned toolchain

> Clear every lint the 1.98 toolchain reports and drop the pre-2018 idioms the
> crate still carried.
>
> - assert a boolean directly instead of comparing it to a literal
> - pass `Path::new(..)` by value where a reference was immediately dereferenced
> - derive `Default` on `FileType` rather than hand-writing the impl
> - drop `#![crate_name]`/`#![crate_type]` and `#[macro_use] extern crate log`,
>   importing `log::{debug, trace}` where they are used
> - name every format placeholder instead of relying on argument position

- Breaking: remove welcome from the connection contract

> Return unit on successful connection in both filesystem traits and update adapters, mocks, and migration guidance. Keep server banners in backend-specific APIs.

### Fixed

- Breaking: enforce absolute paths and harden transfer adapters

> Validate remote path syntax independently of the client platform and remove working-directory wrappers. Preserve buffered data and transfer errors across cancellation, complete bounded uploads correctly, and reuse workers for blocking I/O.

- expose feature-gated types on docs.rs with correct cfg (#34)

> - fix: expose feature-gated types on docs.rs with correct cfg
>
> Enable nightly doc_cfg behind the docsrs cfg and build every feature
> on docs.rs so feature-gated items like Unblock and BlockOn render
> with their "available on crate feature" badges.
>
> - chore: release 1.0.1

## 0.3.0

Released on 2024-09-30

### Fixed

- logo and docs
- added Send bound to streams

## 0.1.0

Released on 2021-12-08
