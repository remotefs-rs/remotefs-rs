#![crate_name = "remotefs_ftp"]
#![crate_type = "lib"]

//! # remotefs-ftp
//!
//! remotefs-ftp is a client implementation for [remotefs](https://github.com/veeso/remotefs-rs), providing support for the FTP/FTPS protocols.
//!
//! ## Get started
//!
//! First of all you need to add **remotefs** and the client to your project dependencies:
//!
//! ```toml
//! remotefs = "^0.3"
//! remotefs-ftp = "^0.2"
//! ```
//!
//! these features are supported:
//!
//! - `find`: enable `find()` method for RemoteFs. (*enabled by default*)
//! - `secure`: enable FTPS
//! - `no-log`: disable logging. By default, this library will log via the `log` crate.
//!
//!
//! ### FTP client
//!
//! ```rust,ignore
//! use remotefs::RemoteFs;
//! use remotefs::client::ftp::FtpFs;
//! use std::path::Path;
//!
//! let mut client = FtpFs::new("127.0.0.1", 21)
//!     .username("test")
//!     .password("password");
//! // connect
//! assert!(client.connect().is_ok());
//! // get working directory
//! println!("Wrkdir: {}", client.pwd().ok().unwrap().display());
//! // change working directory
//! assert!(client.change_dir(Path::new("/tmp")).is_ok());
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

pub mod client;
pub use client::FtpFs;

// -- utils
pub(crate) mod utils;
// -- mock
#[cfg(test)]
pub(crate) mod mock;
