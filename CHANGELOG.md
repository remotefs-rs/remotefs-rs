# Changelog

All notable changes to this project are documented in this file.

## 1.0.0

Released on 2026-09-09

### Breaking changes

- remove `Welcome` from the connection contract

> both filesystem traits now return `RemoteResult<()>` from `connect`.
> Server banners belong to backend-specific APIs.

- move to edition 2024 and refresh the dependency set

> the crate now requires Rust 1.89.0. The `github-actions` and
> `with-containers` features are removed: nothing in the crate was gated on
> them. `find` now enables `dep:wildmatch`, so the implicit `wildmatch` feature
> no longer exists; enable `find` instead.

- preserve remote error sources and revise kinds

> replace public RemoteError fields and legacy error variants with source-preserving accessors and the remotefs 1.0 error taxonomy.

- add extensible filesystem value types

> make metadata sizes optional, add transfer options and capabilities, and require constructors for non-exhaustive public values.

- replace blocking transfers with owned streams

> replace the mutable blocking trait, legacy transfer methods, and marker stream types with shared-receiver operations and consuming stream finalizers.

- require absolute paths for every filesystem operation

> remove working-directory methods and relative-path resolution. Validate remote
> POSIX, Windows drive, and UNC roots independently of the client platform.

### Added

- Breaking: preserve remote error sources and revise kinds
- Breaking: add extensible filesystem value types
- Breaking: replace blocking transfers with owned streams
- add runtime-neutral async transfer streams
- add the object-safe async filesystem contract
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

### Build

- Breaking: move to edition 2024 and refresh the dependency set

> Rewrite Cargo.toml to the shared conventions: name and version first, the
> remaining package keys alphabetical with description last, bare version
> requirements instead of caret ranges, and features sorted and expressed with
> `dep:`.
>
> Set `edition = "2024"` and declare `rust-version = "1.89.0"` so the MSRV is
> checked rather than implied, and bump `thiserror` from 1 to 2. Also refresh
> the maintainer e-mail to the address used by the rest of the family.

## 0.3.0

Released on 2024-09-30

### Fixed

- update readme to latest version for smb & ssh client libs. (#28)
- cargo.toml
- webdav docs
- logo and docs
- removed chrono
- added Send bound to streams

## 0.1.0

Released on 2021-12-08
