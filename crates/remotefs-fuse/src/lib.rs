#![crate_name = "remotefs_fuse"]
#![crate_type = "lib"]
#![cfg_attr(docsrs, feature(doc_cfg))]

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

mod driver;

use std::{ffi::OsStr, path::Path};

#[cfg(target_family = "unix")]
pub use fuse::spawn_mount;

pub use self::driver::Driver;

/// Mount a remote filesystem to a local directory.
///
/// The `mount` function will take a [`Driver`] instance and mount it to the provided mountpoint.
pub fn mount<P>(driver: Driver, mountpoint: &P, options: &[&OsStr]) -> Result<(), std::io::Error>
where
    P: AsRef<Path>,
{
    fuse::mount(driver, mountpoint, options)
}
