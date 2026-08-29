# remotefs-gcs

[![Crates.io](https://img.shields.io/crates/v/remotefs-gcs.svg)](https://crates.io/crates/remotefs-gcs)
[![Documentation](https://docs.rs/remotefs-gcs/badge.svg)](https://docs.rs/remotefs-gcs)
[![CI](https://github.com/remotefs-rs/remotefs-rs-gcs/actions/workflows/ci.yml/badge.svg)](https://github.com/remotefs-rs/remotefs-rs-gcs/actions/workflows/ci.yml)
[![Coverage](https://codecov.io/gh/remotefs-rs/remotefs-rs-gcs/branch/main/graph/badge.svg)](https://codecov.io/gh/remotefs-rs/remotefs-rs-gcs)
[![MIT license](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

`remotefs-gcs` is a synchronous [`remotefs`] client for Google Cloud Storage.
It owns the Tokio runtime provided by the caller and uses the Google Cloud
Storage Rust SDK for object bytes, metadata, listing, deletion, and rewrites.

[`remotefs`]: https://github.com/remotefs-rs/remotefs-rs

## Installation

Add the library and a Tokio runtime to `Cargo.toml`:

```toml
[dependencies]
remotefs = "0.3"
remotefs-gcs = "0.1"
tokio = { version = "1", features = ["rt-multi-thread"] }
```

The crate exposes these features:

| Feature           | Default | Purpose                                           |
| ----------------- | :-----: | ------------------------------------------------- |
| `find`            |   yes   | Enables `RemoteFs::find`                          |
| `no-log`          |   no    | Disables logging from this crate                  |
| `with-containers` |   no    | Enables the local GCS testbench integration suite |
| `with-gcs-ci`     |   no    | Enables the optional live GCS smoke test          |

## Application Default Credentials

`GoogleCloudStorageFs::new` uses Application Default Credentials (ADC). The
application must authenticate with the Google Cloud tooling or environment
appropriate to its deployment:

```rust,no_run
use std::path::Path;
use std::sync::Arc;

use remotefs::RemoteFs;
use remotefs_gcs::GoogleCloudStorageFs;
use tokio::runtime::Runtime;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let runtime = Arc::new(Runtime::new()?);
    let mut client = GoogleCloudStorageFs::new("my-bucket", &runtime);

    client.connect()?;
    println!("working directory: {}", client.pwd()?.display());
    let _entries = client.list_dir(Path::new("/"))?;
    client.disconnect()?;
    Ok(())
}
```

## Custom credentials

Pass any `google-cloud-auth` credential provider supported by the SDK. This
example builds the SDK's ADC credentials explicitly:

```rust,no_run
use std::sync::Arc;

use remotefs::RemoteFs;
use remotefs_gcs::credentials::Builder;
use remotefs_gcs::{GoogleCloudStorageCredentials, GoogleCloudStorageFs};
use tokio::runtime::Runtime;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let credentials = Builder::default().build()?;
    let runtime = Arc::new(Runtime::new()?);
    let mut client = GoogleCloudStorageFs::with_credentials(
        "my-bucket",
        GoogleCloudStorageCredentials::custom(credentials),
        &runtime,
    );

    client.connect()?;
    client.disconnect()?;
    Ok(())
}
```

## Anonymous and emulator access

Use `anonymous` for public buckets or an emulator that does not require
authentication. `endpoint` applies to both Google SDK clients:

```rust,no_run
use std::sync::Arc;

use remotefs::RemoteFs;
use remotefs_gcs::{GoogleCloudStorageCredentials, GoogleCloudStorageFs};
use tokio::runtime::Runtime;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let runtime = Arc::new(Runtime::new()?);
    let mut client = GoogleCloudStorageFs::with_credentials(
        "test-bucket",
        GoogleCloudStorageCredentials::anonymous(),
        &runtime,
    )
    .endpoint("http://localhost:4443");

    client.connect()?;
    client.disconnect()?;
    Ok(())
}
```

## Filesystem behavior

The bucket is presented as a rooted filesystem. Google Cloud Storage has a
flat object namespace, so this crate treats both trailing-slash marker objects
and returned object prefixes as directories.

| Operation                    | Support | Notes                                        |
| ---------------------------- | :-----: | -------------------------------------------- |
| `connect`, `disconnect`      |   yes   | Builds both SDK clients                      |
| `pwd`, `change_dir`          |   yes   | Paths are rooted at `/`                      |
| `list_dir`, `stat`, `exists` |   yes   | Listings consume every SDK page              |
| `create_dir`                 |   yes   | Creates a zero-byte trailing-slash marker    |
| `remove_dir`                 |   yes   | Rejects non-empty directories                |
| `remove_dir_all`             |   yes   | Deletes every object below the path          |
| `create_file`, `open_file`   |   yes   | Blocking upload and download                 |
| `copy`, `mov`                |   yes   | Rewrite; move is copy followed by delete     |
| `setstat`, `symlink`, `exec` |   no    | Returns `UnsupportedFeature`                 |
| `create`, `open`, `append`   |   no    | Streaming `remotefs` methods are unsupported |

The root directory always exists and is never represented by an object. An
implicit directory exists when an object has the corresponding prefix, even if
there is no marker object.

## Development and testing

Run the default unit and documentation tests with:

```sh
just test
```

Run the Storage testbench integration suite with Docker available:

```sh
just integration
```

The integration recipe starts a pinned testbench container and runs one test
for every supported or intentionally unsupported `RemoteFs` capability.

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
