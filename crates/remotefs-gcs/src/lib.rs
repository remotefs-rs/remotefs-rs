//! # `remotefs-gcs`
//!
//! A synchronous [`remotefs::RemoteFs`] client backed by Google Cloud Storage.
//! It owns the Tokio runtime supplied by the caller and blocks only while
//! crossing into the asynchronous Google Cloud SDK.
//!
//! ## Installation
//!
//! Add the crate to the application that owns the runtime:
//!
//! ```toml
//! [dependencies]
//! remotefs = "0.3"
//! remotefs-gcs = "0.1"
//! tokio = { version = "1", features = ["rt-multi-thread"] }
//! ```
//!
//! The optional `find` feature is enabled by default. `no-log` disables the
//! `log` crate output, while `with-containers` and `with-gcs-ci` enable the
//! corresponding test suites.
//!
//! ## Application Default Credentials
//!
//! [`GoogleCloudStorageFs::new`] uses Application Default Credentials (ADC):
//!
//! ```rust,no_run
//! use std::path::Path;
//! use std::sync::Arc;
//!
//! use remotefs::RemoteFs;
//! use remotefs_gcs::GoogleCloudStorageFs;
//! use tokio::runtime::Runtime;
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let runtime = Arc::new(Runtime::new()?);
//! let mut client = GoogleCloudStorageFs::new("my-bucket", &runtime);
//! client.connect()?;
//! println!("working directory: {}", client.pwd()?.display());
//! let _root = client.list_dir(Path::new("/"))?;
//! client.disconnect()?;
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
//! use std::sync::Arc;
//!
//! use remotefs::RemoteFs;
//! use remotefs_gcs::credentials::Builder;
//! use remotefs_gcs::{GoogleCloudStorageCredentials, GoogleCloudStorageFs};
//! use tokio::runtime::Runtime;
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let credentials = Builder::default().build()?;
//! let runtime = Arc::new(Runtime::new()?);
//! let mut client = GoogleCloudStorageFs::with_credentials(
//!     "my-bucket",
//!     GoogleCloudStorageCredentials::custom(credentials),
//!     &runtime,
//! );
//! client.connect()?;
//! client.disconnect()?;
//! # Ok(())
//! # }
//! ```
//!
//! [`GoogleCloudStorageCredentials::anonymous`] is useful for public buckets
//! and local emulators. Pair it with [`GoogleCloudStorageFs::endpoint`] when
//! the service is not running at Google's default endpoint.
//! Emulators with separate object-data and metadata endpoints can additionally
//! use [`GoogleCloudStorageFs::control_endpoint`] for the latter.
//!
//! ## Filesystem semantics
//!
//! Google Cloud Storage has a flat object namespace. This crate presents
//! trailing-slash marker objects and implicit object prefixes as directories.
//! The root directory `/` always exists, `list_dir` consumes all pages and
//! merges objects with prefixes, and `remove_dir_all` removes every object
//! below its path.
//!
//! Blocking `create_file` and `open_file` are supported. Streaming `create`,
//! `open`, and `append`, plus `setstat`, `symlink`, and `exec`, return
//! [`remotefs::RemoteErrorType::UnsupportedFeature`]. `copy` uses GCS rewrite
//! requests until completion; `mov` copies and then deletes the source.
//!
//! [`GoogleCloudStorageFs::new`]: client::GoogleCloudStorageFs::new
//! [`GoogleCloudStorageFs::endpoint`]: client::GoogleCloudStorageFs::endpoint
//! [`GoogleCloudStorageFs::control_endpoint`]:
//!     client::GoogleCloudStorageFs::control_endpoint
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
mod object;
mod reader;
pub mod retry_policy;
pub mod retry_throttler;
#[doc(inline)]
pub use client::GoogleCloudStorageFs;
#[doc(inline)]
pub use credentials::GoogleCloudStorageCredentials;
mod utils;
