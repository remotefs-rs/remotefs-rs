#![crate_name = "remotefs_fuse"]
#![crate_type = "lib"]

//! # remotefs-fuse
//!
//! TODO
//!
//! ## Get started
//!
//! First of all you need to add **remotefs-fuse** to your project dependencies:
//!
//! ```toml
//! remotefs-fuse = "^0.1.0"
//! ```
//!
//! these features are supported:
//!
//! - `no-log`: disable logging. By default, this library will log via the `log` crate.
//!

#![doc(html_playground_url = "https://play.rust-lang.org")]
#![doc(
    html_favicon_url = "https://raw.githubusercontent.com/remotefs-rs/remotefs-rs/main/assets/logo-128.png"
)]
#![doc(
    html_logo_url = "https://raw.githubusercontent.com/remotefs-rs/remotefs-rs/main/assets/logo.png"
)]

#[macro_use]
extern crate log;
