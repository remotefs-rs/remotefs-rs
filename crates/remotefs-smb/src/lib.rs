#![crate_name = "remotefs_smb"]
#![crate_type = "lib"]

//! # remotefs-smb
//!
//! remotefs-smb is a client implementation for [remotefs](https://github.com/veeso/remotefs-rs), providing support for the SMB protocol.
//!
//! ## Get started
//!
//! First of all you need to add **remotefs** and the client to your project dependencies:
//!
//! ```toml
//! remotefs = "^0.2.0"
//! remotefs-smb = "^0.1.0"
//! ```
//!
//! these features are supported:
//!
//! - `find`: enable `find()` method for RemoteFs. (*enabled by default*)
//! - `no-log`: disable logging. By default, this library will log via the `log` crate.
//!
//!
//! ### Smb client
//!
//! Here is a basic usage example, with the `Smb` client.
//!
//! ```rust
//!
//! // import remotefs trait and client
//! use remotefs::RemoteFs;
//! use remotefs_smb::SmbFs;
//! use std::path::Path;
//!
//! let mut client: SftpFs = SshOpts::new("127.0.0.1")
//!     .port(22)
//!     .username("test")
//!     .password("password")
//!     .config_file(Path::new("/home/cvisintin/.ssh/config"))
//!     .into();
//!
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

// -- crates
#[macro_use]
extern crate lazy_static;
#[macro_use]
extern crate log;

mod client;
pub use client::SmbFs;

// -- utils
pub(crate) mod utils;
// -- mock
#[cfg(test)]
pub(crate) mod mock;
