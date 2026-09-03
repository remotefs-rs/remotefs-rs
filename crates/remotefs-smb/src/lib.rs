#![crate_name = "remotefs_smb"]
#![crate_type = "lib"]
#![cfg_attr(docsrs, feature(doc_cfg))]

//! # remotefs-smb
//!
//! remotefs-smb is a client implementation for
//! [remotefs](https://github.com/remotefs-rs/remotefs-rs), providing support
//! for the SMB protocol.
//!
//! Three clients are available:
//!
//! - **Rust-native client** (`smb` feature, every platform): `SmbFs`, a
//!   pure-Rust implementation built on the [smb](https://crates.io/crates/smb)
//!   crate. Supports SMB 2.0.2 through 3.1.1, needs no system library, and
//!   requires a Tokio runtime supplied by the caller.
//! - **pavao client** (`pavao` feature, UNIX only): `PavaoSmbFs`, a binding
//!   to `libsmbclient` through the [pavao](https://github.com/veeso/pavao)
//!   crate. Supports SMB1 through SMB 3.1.1 and needs `libsmbclient`
//!   installed.
//! - **WNet client** (Windows only, always available): `WNetSmbFs`, built on
//!   the Win32 `WNet` API.
//!
//! `pavao` and `smb` can be enabled together.
//!
//! ## Get started
//!
//! First of all you need to add **remotefs** and the client to your project
//! dependencies:
//!
//! ```toml
//! remotefs = "^0.3"
//! remotefs-smb = "^0.5"
//! ```
//!
//! These features are supported:
//!
//! - `pavao`: enable the `libsmbclient`-based client on UNIX (*enabled by
//!   default*)
//! - `smb`: enable the Rust-native client on every platform
//! - `find`: enable `find()` method for RemoteFs. (*enabled by default*)
//! - `no-log`: disable logging. By default, this library will log via the
//!   `log` crate.
//! - `vendored`: build pavao with a vendored `libsmbclient` (implies `pavao`)
//!
//! ### Rust-native client (all platforms)
//!
//! `SmbFs` takes the same credentials shape as the pavao client plus an
//! `Arc<tokio::runtime::Runtime>` that it uses to drive the asynchronous `smb`
//! crate. Add `tokio = { version = "1", features = ["rt-multi-thread"] }`
//! to your dependencies and enable the `smb` feature.
//! `SmbFs::try_new` automatically negotiates only SMB2 through SMB3.1.1.
//!
//! ```rust,no_run
//! # #[cfg(feature = "smb")]
//! # {
//! use std::path::Path;
//! use std::sync::Arc;
//!
//! use remotefs::{fs::UnixPex, RemoteFs};
//! use remotefs_smb::{SmbCredentials, SmbFs, SmbOptions};
//! use tokio::runtime::Runtime;
//!
//! let runtime = Arc::new(Runtime::new().unwrap());
//! let mut client = SmbFs::try_new(
//!     SmbCredentials::default()
//!         .server("smb://localhost:3445")
//!         .share("/temp")
//!         .username("test")
//!         .password("test")
//!         .workgroup("pavao"),
//!     SmbOptions::default(),
//!     &runtime,
//! )
//! .unwrap();
//!
//! assert!(client.connect().is_ok());
//! println!("Wrkdir: {}", client.pwd().unwrap().display());
//! assert!(client
//!     .create_dir(Path::new("/cargo"), UnixPex::from(0o755))
//!     .is_ok());
//! assert!(client.change_dir(Path::new("/cargo")).is_ok());
//! assert!(client.disconnect().is_ok());
//! # }
//! ```
//!
//! To select a narrower inclusive range explicitly, use
//! `SmbFs::try_new_with_dialect`:
//!
//! ```rust,no_run
//! # #[cfg(feature = "smb")]
//! # {
//! use std::sync::Arc;
//!
//! use remotefs_smb::{SmbCredentials, SmbDialect, SmbFs, SmbOptions};
//! use tokio::runtime::Runtime;
//!
//! let runtime = Arc::new(Runtime::new().unwrap());
//! let _client = SmbFs::try_new_with_dialect(
//!     SmbCredentials::default()
//!         .server("smb://server.example")
//!         .share("/documents"),
//!     SmbOptions::default(),
//!     SmbDialect::Smb300,
//!     SmbDialect::Smb311,
//!     &runtime,
//! )
//! .unwrap();
//! # }
//! ```
//!
//! ### Pavao client (UNIX)
//!
//! `PavaoSmbFs::try_new` automatically negotiates only SMB2 through SMB3.1.1;
//! `PavaoSmbFs::try_new_with_dialect` accepts `SmbDialect::Nt1` for legacy
//! devices.
//!
//! ```rust,no_run
//! # #[cfg(all(target_family = "unix", feature = "pavao"))]
//! # {
//! use std::path::Path;
//!
//! use remotefs::{fs::UnixPex, RemoteFs};
//! use remotefs_smb::{PavaoSmbCredentials, PavaoSmbFs, PavaoSmbOptions};
//!
//! let mut client = PavaoSmbFs::try_new(
//!     PavaoSmbCredentials::default()
//!         .server("smb://localhost:3445")
//!         .share("/temp")
//!         .username("test")
//!         .password("test")
//!         .workgroup("pavao"),
//!     PavaoSmbOptions::default()
//!         .case_sensitive(true)
//!         .one_share_per_server(true),
//! )
//! .unwrap();
//!
//! assert!(client.connect().is_ok());
//! println!("Wrkdir: {}", client.pwd().unwrap().display());
//! assert!(client
//!     .create_dir(Path::new("/cargo"), UnixPex::from(0o755))
//!     .is_ok());
//! assert!(client.change_dir(Path::new("/cargo")).is_ok());
//! assert!(client.disconnect().is_ok());
//! # }
//! ```
//!
//! ### WNet client (Windows)
//!
//! ```rust,no_run
//! # #[cfg(target_family = "windows")]
//! # {
//! use std::path::Path;
//!
//! use remotefs::{fs::UnixPex, RemoteFs};
//! use remotefs_smb::{WNetSmbCredentials, WNetSmbFs};
//!
//! let mut client = WNetSmbFs::new(
//!     WNetSmbCredentials::new("localhost:3445", "temp")
//!         .username("test")
//!         .password("test"),
//! );
//! assert!(client.connect().is_ok());
//! assert!(client
//!     .create_dir(Path::new("\\cargo"), UnixPex::from(0o755))
//!     .is_ok());
//! assert!(client.disconnect().is_ok());
//! # }
//! ```
//!

#![doc(html_playground_url = "https://play.rust-lang.org")]
#![doc(
    html_favicon_url = "https://raw.githubusercontent.com/remotefs-rs/remotefs-rs/main/assets/logo-128.png"
)]
#![doc(
    html_logo_url = "https://raw.githubusercontent.com/remotefs-rs/remotefs-rs/main/assets/logo.png"
)]

// -- crates
#[cfg(any(
    all(target_family = "unix", feature = "pavao"),
    feature = "smb",
    target_family = "windows",
))]
#[macro_use]
extern crate log;

mod client;

pub use client::SmbDialect;
#[cfg(all(target_family = "unix", feature = "pavao"))]
#[cfg_attr(docsrs, doc(cfg(all(target_family = "unix", feature = "pavao"))))]
pub use client::{
    PavaoSmbCredentials, PavaoSmbEncryptionLevel, PavaoSmbFs, PavaoSmbOptions, PavaoSmbShareMode,
};
#[cfg(feature = "smb")]
#[cfg_attr(docsrs, doc(cfg(feature = "smb")))]
pub use client::{SmbCredentials, SmbEncryptionLevel, SmbFs, SmbOptions};
#[cfg(target_family = "windows")]
#[cfg_attr(docsrs, doc(cfg(target_family = "windows")))]
pub use client::{WNetSmbCredentials, WNetSmbFs};

// -- utils
#[cfg(any(all(target_family = "unix", feature = "pavao"), feature = "smb"))]
pub(crate) mod utils;
// -- mock
#[cfg(test)]
pub(crate) mod mock;
