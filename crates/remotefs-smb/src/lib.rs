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
//!   implements `remotefs::AsyncRemoteFs` natively on a Tokio runtime.
//!   `SmbFs::into_blocking` returns `BlockingSmbFs`, which implements
//!   `remotefs::RemoteFs` for blocking callers.
//! - **pavao client** (`pavao` feature, UNIX only): `PavaoSmbFs`, a blocking
//!   `remotefs::RemoteFs` binding to `libsmbclient` through the
//!   [pavao](https://github.com/veeso/pavao) crate. Supports SMB1 through
//!   SMB 3.1.1 and needs `libsmbclient` installed.
//! - **WNet client** (Windows only, always available): `WNetSmbFs`, a
//!   blocking `remotefs::RemoteFs` built on the Win32 `WNet` API.
//!
//! `pavao` and `smb` can be enabled together. Every client takes absolute
//! paths rooted at the share (`/docs/report.txt`); there is no working
//! directory. Check `capabilities()` before relying on streams, ranges,
//! copy, or metadata changes.
//!
//! ## Get started
//!
//! First of all you need to add **remotefs** and the client to your project
//! dependencies:
//!
//! ```toml
//! remotefs = "1"
//! remotefs-smb = "1"
//! ```
//!
//! These features are supported:
//!
//! - `pavao`: enable the `libsmbclient`-based client on UNIX (*enabled by
//!   default*)
//! - `smb`: enable the Rust-native client on every platform (enables
//!   `remotefs/async` and `remotefs/tokio`)
//! - `find`: enable `remotefs::find` and `remotefs::find_async`. (*enabled
//!   by default*)
//! - `no-log`: disable logging. By default, this library will log via the
//!   `log` crate.
//! - `vendored`: build pavao with a vendored `libsmbclient` (implies `pavao`)
//!
//! ### Rust-native client (all platforms)
//!
//! `SmbFs` is asynchronous. Add `tokio` to your dependencies and enable the
//! `smb` feature. `SmbFs::try_new` automatically negotiates only SMB2 through
//! SMB3.1.1.
//!
//! ```rust,no_run
//! # #[cfg(feature = "smb")]
//! # {
//! use std::path::Path;
//!
//! use remotefs::AsyncRemoteFs;
//! use remotefs::fs::WriteOptions;
//! use remotefs_smb::{SmbCredentials, SmbFs, SmbOptions};
//!
//! # tokio::runtime::Runtime::new().unwrap().block_on(async {
//! let mut client = SmbFs::try_new(
//!     SmbCredentials::default()
//!         .server("smb://localhost:3445")
//!         .share("/temp")
//!         .username("test")
//!         .password("test")
//!         .workgroup("pavao"),
//!     SmbOptions::default(),
//! )
//! .unwrap();
//!
//! client.connect().await.unwrap();
//! client.create_dir(Path::new("/cargo"), None).await.unwrap();
//! let mut source = futures::io::Cursor::new(b"hello".to_vec());
//! client
//!     .write_file(Path::new("/cargo/hello.txt"), &WriteOptions::default(), &mut source)
//!     .await
//!     .unwrap();
//! client.disconnect().await.unwrap();
//! # });
//! # }
//! ```
//!
//! Blocking code wraps the client with `into_blocking`; the result implements
//! `remotefs::RemoteFs` and can be boxed as `Box<dyn RemoteFs>`:
//!
//! ```rust,no_run
//! # #[cfg(feature = "smb")]
//! # {
//! use std::path::Path;
//!
//! use remotefs::RemoteFs;
//! use remotefs_smb::{BlockingSmbFs, SmbCredentials, SmbFs, SmbOptions};
//!
//! let runtime = tokio::runtime::Runtime::new().unwrap();
//! let mut client: BlockingSmbFs = SmbFs::try_new(
//!     SmbCredentials::default()
//!         .server("smb://localhost:3445")
//!         .share("/temp"),
//!     SmbOptions::default(),
//! )
//! .unwrap()
//! .into_blocking(runtime.handle().clone());
//! client.connect().unwrap();
//! for entry in client.list_dir(Path::new("/")).unwrap() {
//!     println!("{}", entry.name());
//! }
//! client.disconnect().unwrap();
//! # }
//! ```
//!
//! To select a narrower inclusive range explicitly, use
//! `SmbFs::try_new_with_dialect`:
//!
//! ```rust,no_run
//! # #[cfg(feature = "smb")]
//! # {
//! use remotefs_smb::{SmbCredentials, SmbDialect, SmbFs, SmbOptions};
//!
//! let _client = SmbFs::try_new_with_dialect(
//!     SmbCredentials::default()
//!         .server("smb://server.example")
//!         .share("/documents"),
//!     SmbOptions::default(),
//!     SmbDialect::Smb300,
//!     SmbDialect::Smb311,
//! )
//! .unwrap();
//! # }
//! ```
//!
//! ### Pavao client (UNIX)
//!
//! `PavaoSmbFs::try_new` automatically negotiates only SMB2 through SMB3.1.1;
//! `PavaoSmbFs::try_new_with_dialect` accepts `SmbDialect::Nt1` for legacy
//! devices. The pavao client offers one-shot transfers only (`read_file`,
//! `write_file`, `append_file`); `open`, `create`, and `append` return
//! `UnsupportedFeature`.
//!
//! ```rust,no_run
//! # #[cfg(all(target_family = "unix", feature = "pavao"))]
//! # {
//! use std::io::Cursor;
//! use std::path::Path;
//!
//! use remotefs::RemoteFs;
//! use remotefs::fs::WriteOptions;
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
//! client.connect().unwrap();
//! client.create_dir(Path::new("/cargo"), None).unwrap();
//! let mut source = Cursor::new(b"hello".to_vec());
//! client
//!     .write_file(Path::new("/cargo/hello.txt"), &WriteOptions::default(), &mut source)
//!     .unwrap();
//! client.disconnect().unwrap();
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
//! use remotefs::RemoteFs;
//! use remotefs_smb::{WNetSmbCredentials, WNetSmbFs};
//!
//! let mut client = WNetSmbFs::new(
//!     WNetSmbCredentials::new("localhost:3445", "temp")
//!         .username("test")
//!         .password("test"),
//! );
//! client.connect().unwrap();
//! client.create_dir(Path::new("/cargo"), None).unwrap();
//! client.disconnect().unwrap();
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
#[cfg(feature = "smb")]
#[cfg_attr(docsrs, doc(cfg(feature = "smb")))]
pub use client::{BlockingSmbFs, SmbCredentials, SmbEncryptionLevel, SmbFs, SmbOptions};
#[cfg(all(target_family = "unix", feature = "pavao"))]
#[cfg_attr(docsrs, doc(cfg(all(target_family = "unix", feature = "pavao"))))]
pub use client::{
    PavaoSmbCredentials, PavaoSmbEncryptionLevel, PavaoSmbFs, PavaoSmbOptions, PavaoSmbShareMode,
};
#[cfg(target_family = "windows")]
#[cfg_attr(docsrs, doc(cfg(target_family = "windows")))]
pub use client::{WNetSmbCredentials, WNetSmbFs};

// -- utils
#[cfg(any(
    all(target_family = "unix", feature = "pavao"),
    feature = "smb",
    target_family = "windows",
))]
pub(crate) mod utils;
// -- mock
#[cfg(test)]
pub(crate) mod mock;
