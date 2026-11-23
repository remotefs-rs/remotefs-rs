# Changelog

All notable changes to this project are documented in this file.

## 1.2.0

Released on 2026-11-23

### Breaking changes

- **auth:** accept a generic Auth method in WebDAVFs::new (remotefs-rs/remotefs-rs-webdav#3)

> WebDAVFs::new(url, auth) replaces WebDAVFs::new(username, password, url).

- chore: add `gh_release` recipe

- chore: release 0.3.0

* migrate to remotefs 1 (remotefs-rs/remotefs-rs-webdav#4)

> WebDAVFs implements AsyncRemoteFs, uses absolute paths, returns a RemoteResult from new, drops pwd and change_dir, replaces open_file/create_file/mov/setstat with read_file/write_file/rename/set_metadata, and releases as 1.0.0.

- chore: release 1

### Added

- lib implementation
- remotefs 0.3
- Breaking: **auth:** accept a generic Auth method in WebDAVFs::new (remotefs-rs/remotefs-rs-webdav#3)

> - feat(auth)!: accept a generic Auth method in WebDAVFs::new
>
> Bump dav-xml and dav-xml-client to 0.2 and drop the now-unneeded direct
> ureq dependency. WebDAVFs::new now takes the base url and a dav_xml_client::Auth
> value instead of separate username/password arguments, letting callers pick
> any auth scheme the client supports rather than basic auth only.
>
> Also fix the with-containers test client to authenticate with the container's
> configured credentials instead of the placeholder ones shared with the plain
> unit tests.

- Breaking: migrate to remotefs 1 (remotefs-rs/remotefs-rs-webdav#4)

> - feat!: migrate to remotefs 1
>
> Implement remotefs::AsyncRemoteFs natively over dav-xml-client with owned transfer streams, HTTP range reads, WebDAV COPY support, typed error sources, and a BlockOn-based blocking wrapper behind the tokio feature.

- allow relative and dangling symlink targets (#41)

> symlink(path, target) no longer requires an absolute, existing target. The target is stored verbatim on every backend, so relative and dangling links can be created; only an empty target is rejected, with InvalidPath. The new remotefs::path::ensure_symlink_target helper implements that check. KubeMultiPodFs writes relative targets inside the link's container and keeps translating absolute targets within the same container.

### Changed

- make the crate clippy-clean

> Collapse a nested `if let` into a let chain, build test metadata with
> struct update syntax instead of assigning fields after
> `Default::default()`, take the first list entry with `first()`, and use
> named format placeholders in the parser log line.
>
> Rename the `mod.rs` files of the vendored `webdav_xml` tree to the
> `module_name.rs` layout, and move its `dead_code` allowance to the module
> root: the vendored copy deliberately keeps upstream's full element and
> property set even though the client reads only a subset of it.

- replace rustydav with a reqwest client

> rustydav is GPL-3.0, which is incompatible with this crate's MIT
> license, and its last release pins reqwest 0.11, pulling in the h2
> unbounded DATA frame advisory (RUSTSEC-2025-0037) and the unmaintained
> rustls-pemfile (RUSTSEC-2025-0134). The crate has had no release since
> 0.1.3, so there is nothing to upgrade to.
>
> `src/client.rs` replaces it with `DavClient`, a blocking reqwest wrapper
> exposing only the verbs the filesystem needs: GET, PUT, DELETE, MKCOL,
> MOVE and PROPFIND. rustydav was never part of the public API, so the
> change is internal; behaviour is unchanged and the container-backed
> suite passes against the same server. `cargo deny` is now clean.
>
> reqwest 0.13 defaults to rustls, so the crate no longer needs a system
> OpenSSL to build.

### Fixed

- ci and docs
- unused tempfile
- docs
- temporarly included webdav-xml into project to fix parsing issues
- badge
- test is sync and send

### Build

- **deps:** update dependencies, MSRV and edition

> Move the crate to edition 2024 with a 1.94.1 MSRV, and bump every
> dependency to its current release: thiserror 2, quick-xml 0.42,
> nonempty 0.12 and serial_test 4.
>
> quick-xml 0.42 works on `&str` instead of `&[u8]` and reports entity and
> character references as separate `GeneralRef` events, so the vendored
> webdav-xml reader now resolves those references and concatenates the
> pieces of a split text node. Text containing an escaped `&` used to be
> truncated at the escape; it is now read in full, covered by a new parser
> test.
>
> Drop the unused `github-actions` feature and declare docs.rs metadata.

- migrate webdav client to `dav-xml` and `dav-xml-client` crates. (remotefs-rs/remotefs-rs-webdav#2)

> - build: migrate webdav client to `dav-xml` and `dav-xml-client` crates.
> - fix: ignore self-dir when listing entries

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

- make `seahash` and `futures` unix-only dependencies
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

### Style

- format the tree with dprint
