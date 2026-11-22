# remotefs-gcs

[![Crates.io](https://img.shields.io/crates/v/remotefs-gcs.svg)](https://crates.io/crates/remotefs-gcs)
[![Documentation](https://docs.rs/remotefs-gcs/badge.svg)](https://docs.rs/remotefs-gcs)
[![CI](https://github.com/remotefs-rs/remotefs-rs-gcs/actions/workflows/ci.yml/badge.svg)](https://github.com/remotefs-rs/remotefs-rs-gcs/actions/workflows/ci.yml)
[![Coverage](https://codecov.io/gh/remotefs-rs/remotefs-rs-gcs/branch/main/graph/badge.svg)](https://codecov.io/gh/remotefs-rs/remotefs-rs-gcs)
[![MIT license](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

`remotefs-gcs` is an asynchronous [`remotefs`] client backed by Google Cloud
Storage. It uses the Google Cloud Storage Rust SDK for object bytes, metadata,
listing, deletion, and rewrites. Every remote path is absolute and rooted at
the bucket.

[`remotefs`]: https://github.com/remotefs-rs/remotefs-rs

## Installation

Add the library to `Cargo.toml`:

```toml
[dependencies]
remotefs = "1"
remotefs-gcs = "1"
futures = "0.3"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

The crate exposes these features:

| Feature           | Default | Purpose                                                     |
| ----------------- | :-----: | ----------------------------------------------------------- |
| `find`            |   yes   | Enables `remotefs::find_async`                              |
| `no-log`          |   no    | Disables logging from this crate                            |
| `tokio`           |   no    | Enables `BlockingGoogleCloudStorageFs` for blocking callers |
| `with-containers` |   no    | Enables the local GCS testbench integration suite           |
| `with-gcs-ci`     |   no    | Enables the optional live GCS smoke test                    |

## Application Default Credentials

`GoogleCloudStorageFs::new` uses Application Default Credentials (ADC). The
application must authenticate with the Google Cloud tooling or environment
appropriate to its deployment:

```rust,no_run
use std::path::Path;

use remotefs::AsyncRemoteFs;
use remotefs::fs::{ReadOptions, WriteOptions};
use remotefs_gcs::GoogleCloudStorageFs;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut client = GoogleCloudStorageFs::new("my-bucket");
    client.connect().await?;

    let mut source = futures::io::Cursor::new(b"hello".to_vec());
    client
        .write_file(
            Path::new("/docs/hello.txt"),
            &WriteOptions::default().size_hint(5),
            &mut source,
        )
        .await?;

    let mut destination = futures::io::Cursor::new(Vec::new());
    client
        .read_file(
            Path::new("/docs/hello.txt"),
            &ReadOptions::default().offset(1).length(3),
            &mut destination,
        )
        .await?;
    assert_eq!(destination.into_inner(), b"ell");

    client.disconnect().await?;
    Ok(())
}
```

## Custom credentials

Pass any `google-cloud-auth` credential provider supported by the SDK. This
example builds the SDK's ADC credentials explicitly:

```rust,no_run
use remotefs::AsyncRemoteFs;
use remotefs_gcs::credentials::Builder;
use remotefs_gcs::{GoogleCloudStorageCredentials, GoogleCloudStorageFs};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let credentials = Builder::default().build()?;
    let mut client = GoogleCloudStorageFs::with_credentials(
        "my-bucket",
        GoogleCloudStorageCredentials::custom(credentials),
    );

    client.connect().await?;
    client.disconnect().await?;
    Ok(())
}
```

## Anonymous and emulator access

Use `anonymous` for public buckets or an emulator that does not require
authentication. `endpoint` applies to both SDK clients, while
`control_endpoint` can override the metadata client with a separate endpoint:

```rust,no_run
use remotefs::AsyncRemoteFs;
use remotefs_gcs::{GoogleCloudStorageCredentials, GoogleCloudStorageFs};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut client = GoogleCloudStorageFs::with_credentials(
        "test-bucket",
        GoogleCloudStorageCredentials::anonymous(),
    )
    .endpoint("http://localhost:4443");

    client.connect().await?;
    client.disconnect().await?;
    Ok(())
}
```

## Blocking usage

Enable the `tokio` feature when a blocking `remotefs::RemoteFs` trait object is
needed. Create a multi-thread runtime, pass its handle to `into_blocking`, and
use the resulting `BlockingGoogleCloudStorageFs` from synchronous code:

```rust,no_run
use remotefs::RemoteFs;
use remotefs_gcs::GoogleCloudStorageFs;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let runtime = tokio::runtime::Runtime::new()?;
    let mut client: Box<dyn RemoteFs> =
        Box::new(GoogleCloudStorageFs::new("my-bucket").into_blocking(runtime.handle().clone()));

    client.connect()?;
    client.disconnect()?;
    Ok(())
}
```

Do not call the blocking adapter from inside an async context.

## Filesystem behavior

Google Cloud Storage has a flat object namespace, so this crate treats both
trailing-slash marker objects and returned object prefixes as directories. The
root directory `/` always exists and is never represented by an object.

| Operation                         | Support | Notes                                                        |
| --------------------------------- | :-----: | ------------------------------------------------------------ |
| `connect`, `disconnect`           |   yes   | Builds both SDK clients                                      |
| `list_dir`, `stat`, `exists`      |   yes   | Absolute paths rooted at `/`; listings consume every page    |
| `create_dir`                      |   yes   | Creates a zero-byte trailing-slash marker; mode is ignored   |
| `remove_dir`                      |   yes   | Rejects non-empty directories                                |
| `remove_dir_all`                  |   yes   | Deletes every object below the path                          |
| `open`, `read_file`               |   yes   | Owned ranged read stream; offsets and lengths are native     |
| `create`, `write_file`            |   yes   | Owned resumable-upload stream committed by `finish`          |
| `copy`, `rename`                  |   yes   | Rewrite; rename is copy followed by delete and is not atomic |
| `append`, `append_file`           |   no    | Returns `UnsupportedFeature`                                 |
| `set_metadata`, `symlink`, `exec` |   no    | Returns `UnsupportedFeature`                                 |

`open` and `create` return owned streams. A writer must be completed with
`finish` for the object to be committed; dropping an unfinished writer creates
nothing. `copy` uses GCS rewrite requests until completion. `rename` copies and
then deletes the source, so it is not atomic.

## Development and testing

Run the default unit and documentation tests with:

```sh
just test
```

Run the Storage testbench integration suite with Docker available:

```sh
just integration
```

This runs the asynchronous testbench suite and the blocking adapter suite with
`--features with-containers,tokio`.

The live smoke test is opt-in. Set `GCS_TEST_BUCKET` and provide ADC before
running it:

```sh
GCS_TEST_BUCKET=my-test-bucket just test "--features with-gcs-ci"
```

The complete local quality gate is:

```sh
just check
```

The minimum supported Rust version is 1.98.0. See the [Rust toolchain file]
for the pinned compiler and the [contribution guide] for project conventions.

[Rust toolchain file]: rust-toolchain.toml
[contribution guide]: https://github.com/remotefs-rs/remotefs-rs-gcs/blob/main/AGENTS.md

## License

Licensed under the [MIT License](LICENSE).
