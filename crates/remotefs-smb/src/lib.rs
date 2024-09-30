#![crate_name = "remotefs_smb"]
#![crate_type = "lib"]

//! # remotefs-smb
//!
//! remotefs-smb is a client implementation for [remotefs](https://github.com/remotefs-rs/remotefs-rs), providing support for the SMB protocol.
//!
//! ## Get started
//!
//! First of all you need to add **remotefs** and the client to your project dependencies:
//!
//! ```toml
//! remotefs = "^0.3"
//! remotefs-smb = "^0.3"
//! ```
//!
//! these features are supported:
//!
//! - `find`: enable `find()` method for RemoteFs. (*enabled by default*)
//! - `no-log`: disable logging. By default, this library will log via the `log` crate.
//!
//!
//! ### Smb client (UNIX)
//!
//! Here is a basic usage example, with the `Smb` client.
//!
//! ```rust
//!
//! // import remotefs trait and client
//! use remotefs::{RemoteFs, fs::UnixPex};
//! use remotefs_smb::{SmbFs, SmbOptions, SmbCredentials};
//! use std::path::Path;
//!
//! let mut client = SmbFs::try_new(
//!     SmbCredentials::default()
//!         .server("smb://localhost:3445")
//!         .share("/temp")
//!         .username("test")
//!         .password("test")
//!         .workgroup("pavao"),
//!     SmbOptions::default()
//!         .case_sensitive(true)
//!         .one_share_per_server(true),
//! )
//! .unwrap();
//!
//! // connect
//! assert!(client.connect().is_ok());
//! // get working directory
//! println!("Wrkdir: {}", client.pwd().ok().unwrap().display());
//! // make directory
//! assert!(client.create_dir(Path::new("/cargo"), UnixPex::from(0o755)).is_ok());
//! // change working directory
//! assert!(client.change_dir(Path::new("/cargo")).is_ok());
//! // disconnect
//! assert!(client.disconnect().is_ok());
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
#[macro_use]
extern crate log;

mod client;

#[cfg(target_family = "unix")]
pub use client::{SmbCredentials, SmbEncryptionLevel, SmbFs, SmbOptions, SmbShareMode};
#[cfg(target_family = "windows")]
pub use client::{SmbCredentials, SmbFs};

// -- utils
#[cfg(target_family = "unix")]
pub(crate) mod utils;
// -- mock
#[cfg(test)]
pub(crate) mod mock;
