//! # `remotefs-gcs`
//!
//! An asynchronous [`remotefs::AsyncRemoteFs`] client backed by Google Cloud
//! Storage. Every path is absolute and rooted at the bucket (`/` is the
//! bucket root); trailing-slash marker objects and implicit object prefixes
//! are presented as directories.
//!
//! ## Installation
//!
//! ```toml
//! [dependencies]
//! remotefs = "1"
//! remotefs-gcs = "1"
//! futures = "0.3"
//! tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
//! ```
//!
//! | Feature           | Default | Purpose                                                     |
//! | ----------------- | :-----: | ----------------------------------------------------------- |
//! | `find`            |   yes   | Enables `remotefs::find_async`                              |
//! | `no-log`          |   no    | Disables logging from this crate                            |
//! | `tokio`            |   no    | Enables `BlockingGoogleCloudStorageFs` for blocking callers |
//! | `with-containers` |   no    | Enables the local testbench integration suite               |
//! | `with-gcs-ci`     |   no    | Enables the optional live smoke test                        |
//!
//! ## Application Default Credentials
//!
//! [`GoogleCloudStorageFs::new`] uses Application Default Credentials (ADC):
//!
//! ```rust,no_run
//! use std::path::Path;
//!
//! use remotefs::AsyncRemoteFs;
//! use remotefs::fs::{ReadOptions, WriteOptions};
//! use remotefs_gcs::GoogleCloudStorageFs;
//!
//! # async fn run() -> remotefs::RemoteResult<()> {
//! let mut client = GoogleCloudStorageFs::new("my-bucket");
//! client.connect().await?;
//! let mut source = futures::io::Cursor::new(b"hello".to_vec());
//! client
//!     .write_file(
//!         Path::new("/docs/hello.txt"),
//!         &WriteOptions::default().size_hint(5),
//!         &mut source,
//!     )
//!     .await?;
//! let mut destination = futures::io::Cursor::new(Vec::new());
//! client
//!     .read_file(
//!         Path::new("/docs/hello.txt"),
//!         &ReadOptions::default().offset(1).length(3),
//!         &mut destination,
//!     )
//!     .await?;
//! assert_eq!(destination.into_inner(), b"ell");
//! client.disconnect().await?;
//! # Ok(())
//! # }
//! ```
//!
//! ## Explicit credentials
//!
//! Use [`GoogleCloudStorageCredentials::custom`] to pass any credentials
//! provider supported by `google-cloud-auth`:
//!
//! ```rust,no_run
//! use remotefs::AsyncRemoteFs;
//! use remotefs_gcs::credentials::Builder;
//! use remotefs_gcs::{GoogleCloudStorageCredentials, GoogleCloudStorageFs};
//!
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! let credentials = Builder::default().build()?;
//! let mut client = GoogleCloudStorageFs::with_credentials(
//!     "my-bucket",
//!     GoogleCloudStorageCredentials::custom(credentials),
//! );
//! client.connect().await?;
//! client.disconnect().await?;
//! # Ok(())
//! # }
//! ```
//!
//! [`GoogleCloudStorageCredentials::anonymous`] is useful for public buckets
//! and local emulators. Pair it with [`GoogleCloudStorageFs::endpoint`] when
//! the service is not running at Google's default endpoint, and
//! [`GoogleCloudStorageFs::control_endpoint`] for emulators with separate
//! object-data and metadata endpoints.
//!
//! ## Blocking usage
//!
//! Enable the `tokio` feature and call [`GoogleCloudStorageFs::into_blocking`]
//! to get a [`BlockingGoogleCloudStorageFs`], which implements
//! [`remotefs::RemoteFs`] and can be stored as `Box<dyn RemoteFs>`. It must
//! not be called from inside an async context. The supplied handle must belong
//! to a multi-thread Tokio runtime.
//!
//! ```rust,no_run
//! use remotefs::RemoteFs;
//! use remotefs_gcs::GoogleCloudStorageFs;
//!
//! # #[cfg(feature = "tokio")]
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let runtime = tokio::runtime::Runtime::new()?;
//! let mut client: Box<dyn RemoteFs> = Box::new(
//!     GoogleCloudStorageFs::new("my-bucket").into_blocking(runtime.handle().clone()),
//! );
//! client.connect()?;
//! client.disconnect()?;
//! # Ok(())
//! # }
//! # #[cfg(not(feature = "tokio"))]
//! # fn main() {}
//! ```
//!
//! ## Filesystem semantics
//!
//! The root directory `/` always exists and is never an object. `list_dir`
//! consumes every page and merges objects with prefixes. `remove_dir_all`
//! removes every object below its path with one flat listing.
//!
//! `open` returns an owned read stream over a ranged `ReadObject` request;
//! offsets and lengths are honored natively (`Capabilities::RANGE_READ`).
//! `create` returns an owned write stream that feeds a resumable upload; the
//! object is committed only when `finish` succeeds, and a dropped stream
//! creates nothing. `copy` uses rewrite requests until completion; `rename`
//! copies and then deletes the source and is not atomic. `append`,
//! `set_metadata`, `symlink`, and `exec` return
//! [`remotefs::RemoteErrorType::UnsupportedFeature`].
//!
//! [`GoogleCloudStorageFs::new`]: client::GoogleCloudStorageFs::new
//! [`GoogleCloudStorageFs::endpoint`]: client::GoogleCloudStorageFs::endpoint
//! [`GoogleCloudStorageFs::control_endpoint`]:
//!     client::GoogleCloudStorageFs::control_endpoint
//! [`GoogleCloudStorageFs::into_blocking`]:
//!     client::GoogleCloudStorageFs::into_blocking
//! [`BlockingGoogleCloudStorageFs`]: client::BlockingGoogleCloudStorageFs
//! [`GoogleCloudStorageCredentials::anonymous`]:
//!     credentials::GoogleCloudStorageCredentials::anonymous
//! [`GoogleCloudStorageCredentials::custom`]:
//!     credentials::GoogleCloudStorageCredentials::custom

#![doc(html_playground_url = "https://play.rust-lang.org")]
#![doc(
    html_favicon_url = "https://raw.githubusercontent.com/remotefs-rs/remotefs-rs/main/assets/logo-128.png"
)]
#![doc(
    html_logo_url = "https://raw.githubusercontent.com/remotefs-rs/remotefs-rs/main/assets/logo.png"
)]

#[macro_use]
extern crate log;

pub mod backoff_policy;
mod client;
pub mod credentials;
mod error;
mod key;
mod object;
pub mod retry_policy;
pub mod retry_throttler;
mod stream;
#[cfg(feature = "tokio")]
#[doc(inline)]
pub use client::BlockingGoogleCloudStorageFs;
#[doc(inline)]
pub use client::GoogleCloudStorageFs;
#[doc(inline)]
pub use credentials::GoogleCloudStorageCredentials;
