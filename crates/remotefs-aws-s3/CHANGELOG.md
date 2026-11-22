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

## remotefs-aws-s3-v1.0.1

Released on 2026-10-31

### Build

- bump `aws-smithy-types` `>=1`

## remotefs-aws-s3-v1.0.0

Released on 2026-09-15

### Breaking changes

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

- Breaking: migrate to remotefs 1 (remotefs-rs/remotefs-rs-aws-s3#4)

> - feat!: migrate to remotefs 1

## remotefs-aws-s3-v0.4.4

Released on 2026-08-29

### Build

- update dependencies. (remotefs-rs/remotefs-rs-aws-s3#3)

> Increased MSRV to 1.94.1

## remotefs-aws-s3-v0.4.3

Released on 2025-12-20

### Fixed

- lint

## remotefs-aws-s3-v0.4.2

Released on 2025-03-23

### Fixed

- fixed `remove_file` which didn't removed files

## remotefs-aws-s3-v0.4.1

Released on 2025-03-16

### Fixed

- **upload:** fixed aws-s3 upload. It doesn't support offsets for write, but only multipart

## remotefs-aws-s3-v0.4.0

Released on 2025-03-16

### Breaking changes

- use testcontainers for tests; rust edition 2024

> use testcontainers for tests; rust edition 2024

- **aws:** migrated to aws-sdk

> migrated to aws-sdk

### Added

- Breaking: use testcontainers for tests; rust edition 2024
- Breaking: **aws:** migrated to aws-sdk

> I've finally with like 3 years of delay **migrated** to the **official AWS SDK for Rust**.

### Fixed

- test is sync and send

## remotefs-aws-s3-v0.3.1

Released on 2024-10-07

### Fixed

- removed users dep

## remotefs-aws-s3-v0.3.0

Released on 2024-09-30

### Added

- remotefs 0.3.0

### Fixed

- bump rust-s3
- bump rust-s3 to 0.34
- lint
- windows build
- version

## remotefs-aws-s3-v0.1.0

Released on 2022-01-04
