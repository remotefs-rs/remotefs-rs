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
//! remotefs = "^0.2.0"
//! remotefs-ftp = "^0.1.0"
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

/**
 * MIT License
 *
 * remotefs - Copyright (c) 2021 Christian Visintin
 *
 * Permission is hereby granted, free of charge, to any person obtaining a copy
 * of this software and associated documentation files (the "Software"), to deal
 * in the Software without restriction, including without limitation the rights
 * to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
 * copies of the Software, and to permit persons to whom the Software is
 * furnished to do so, subject to the following conditions:
 *
 * The above copyright notice and this permission notice shall be included in all
 * copies or substantial portions of the Software.
 *
 * THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
 * IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
 * FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
 * AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
 * LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
 * OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
 * SOFTWARE.
 */
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
