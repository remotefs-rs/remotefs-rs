# Changelog

All notable changes to this project are documented in this file.

## remotefs-aws-s3-v1.2.0

Released on 2026-11-23

### Breaking changes

- use testcontainers for tests; rust edition 2024

> use testcontainers for tests; rust edition 2024

- **aws:** migrated to aws-sdk

> migrated to aws-sdk

- migrate to remotefs 1 (remotefs-rs/remotefs-rs-aws-s3#4)

> migrate AwsS3Fs to the remotefs 1 asynchronous API and release version 1.0.0.

- feat: add blocking client behind tokio feature

Add AwsS3Fs::into_blocking and BlockingAwsS3Fs, mirroring the pattern
used in remotefs-webdav, so callers can get a RemoteFs implementation
without wiring remotefs::adapters::blocking::BlockOn themselves.

- build: pin aws-smithy-types below the breaking 1.7.0 release

aws-smithy-types 1.7.0 changed the Document::Object variant's inner
type without a major version bump, which breaks compilation of
aws-smithy-json 0.63.0 (pulled in transitively via aws-config) once
cargo unifies the whole tree to smithy-types 1.7.0. Constrain the
dependency until the ecosystem catches up.

- docs: gate the tokio feature correctly for docs.rs

The blocking client lives behind the non-default tokio feature, so
docs.rs never rendered it. Build docs.rs with that feature enabled
and mark the gated items with doc(cfg(...)) so the rendered docs show
the feature requirement.

- fix: switch MinIO test image to quay.io and watch its stderr banner

Docker Hub removed the minio/minio repository entirely, so the pinned
2022 release tag 404s on pull. Point testcontainers at quay.io/minio/minio
instead, pinned to a current release. That image also moved its

### Added

- remotefs 0.3.0
- Breaking: use testcontainers for tests; rust edition 2024
- Breaking: **aws:** migrated to aws-sdk

> I've finally with like 3 years of delay **migrated** to the **official AWS SDK for Rust**.

- Breaking: migrate to remotefs 1 (remotefs-rs/remotefs-rs-aws-s3#4)

> - feat!: migrate to remotefs 1

### Fixed

- bump rust-s3
- bump rust-s3 to 0.34
- lint
- windows build
- version
- removed users dep
- test is sync and send
- **upload:** fixed aws-s3 upload. It doesn't support offsets for write, but only multipart
- fixed `remove_file` which didn't removed files
- lint

### Build

- update dependencies. (remotefs-rs/remotefs-rs-aws-s3#3)

> Increased MSRV to 1.94.1

- bump `aws-smithy-types` `>=1`
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
