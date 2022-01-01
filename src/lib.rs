#![crate_name = "remotefs"]
#![crate_type = "lib"]

//! # remotefs
//!
//! remotefs is a library that provides a file system structure to work with all the most used file transfer protocols.
//! This is achieved through a trait called `RemoteFs` which exposes methods to operate on the remote file system.
//! Currently the library exposes a client for **Sftp**, **Scp**, **Ftp** and **Aws-s3**.
//!
//! ## Why remotefs
//!
//! You might be wondering what's the reasons behind remotefs.
//! The first reason is to provide an easy way to operate with multiple protocols at the same time.
//! For example, in [termscp](https://github.com/veeso/termscp), this came very handily to me.
//! The second reason is that often, users need to implement just a simple client to operate on a remote file system, and they have to waste a lot of time in understanding how the protocol works just to achieve a single task.
//!
//! With remotefs this is no more a problem: all you need is to configure the options to connect to the remote host and you're ready to deal with the remote file system, as it were mounted on your pc.
//!
//! ## Get started
//!
//! First of all you need to add **remotefs** to your project dependencies:
//!
//! ```toml
//! remotefs = "^0.2.0"
//! ```
//!
//! these features are supported:
//!
//! - `no-log`: disable logging. By default, this library will log via the `log` crate.

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

// -- export
pub use fs::{Directory, Entry, File, RemoteError, RemoteErrorType, RemoteFs, RemoteResult};
// -- modules
pub mod fs;

// -- utils
pub(crate) mod utils;
// -- mock
#[cfg(test)]
pub(crate) mod mock;
