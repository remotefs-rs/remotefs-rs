#![crate_name = "remotefs"]
#![crate_type = "lib"]

//! # remotefs
//!
//! remotefs TODO:
//!
//! ## Get started
//!
//! First of you need to add **remotefs** to your project dependencies:
//!
//! ```toml
//! remotefs = "0.7.0"
//! ```
//!
//! TODO: features and protocols
//!
//! ## Usage
//!
//! Here is a basic usage example:
//!
//! ```rust
//! ```
//!

#![doc(html_playground_url = "https://play.rust-lang.org")]
#![doc(
    html_favicon_url = "https://raw.githubusercontent.com/veeso/remotefs/main/assets/images/cargo/remotefs-128.png"
)]
#![doc(
    html_logo_url = "https://raw.githubusercontent.com/veeso/remotefs/main/assets/images/cargo/remotefs-512.png"
)]

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
extern crate chrono;
#[macro_use]
extern crate log;
#[cfg(target_family = "windows")]
extern crate path_slash;
extern crate thiserror;
#[cfg(target_family = "unix")]
extern crate users;
extern crate wildmatch;

// -- export
pub use fs::{Directory, Entry, File};
// -- modules
pub mod fs;
// mod utils; TODO: add when available
