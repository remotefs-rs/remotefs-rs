#![crate_name = "remotefs_kube"]
#![crate_type = "lib"]
#![cfg_attr(docsrs, feature(doc_cfg))]

//! # remotefs-kube
//!
//! remotefs-kube is a client implementation for [remotefs](https://github.com/veeso/remotefs-rs), providing support for the Kube API protocol.
//!
//! ## Get started
//!
//! First of all you need to add **remotefs** and the client to your project dependencies:
//!
//! ```toml
//! remotefs = "^0.2"
//! remotefs-kube = "^0.1"
//! ```
//!
//! these features are supported:
//!
//! - `find`: enable `find()` method for RemoteFs. (*enabled by default*)
//! - `no-log`: disable logging. By default, this library will log via the `log` crate.
//!
//!
//! ### Kube client
//!
//! Here is a basic usage example, with the `Kube` client, which is very similiar to the `Scp` client.
//!
//! ```rust,ignore
//!
//! // import remotefs trait and client
//! use remotefs::RemoteFs;
//! use remotefs_ssh::{SshConfigParseRule, SftpFs, SshOpts};
//! use std::path::Path;
//!
//! let rt = Arc::new(
//!     tokio::runtime::Builder::new_current_thread()
//!     .enable_all()
//!     .build()
//!     .unwrap(),
//! );
//! let mut client: KubeFs = KubeFs::new("my-pod", &rt);
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
#![doc(
    html_favicon_url = "https://raw.githubusercontent.com/veeso/suppaftp/main/assets/images/cargo/suppaftp-128.png"
)]
#![doc(
    html_logo_url = "https://raw.githubusercontent.com/veeso/suppaftp/main/assets/images/cargo/suppaftp-512.png"
)]

// -- common deps
#[macro_use]
extern crate lazy_regex;
#[macro_use]
extern crate log;

mod client;
mod utils;

pub use client::KubeFs;
pub use kube::Config;

// -- test logging
#[cfg(test)]
pub fn log_init() {
    let _ = env_logger::builder().is_test(true).try_init();
}
