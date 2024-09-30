#![crate_name = "remotefs_aws_s3"]
#![crate_type = "lib"]

//! # remotefs-aws-s3
//!
//! remotefs-aws-s3 is a client implementation for [remotefs](https://github.com/remotefs-rs/remotefs-rs), providing support for the Aws S3 protocol.
//!
//! ## Get started
//!
//! First of all you need to add **remotefs** and the client to your project dependencies:
//!
//! ```toml
//! remotefs = "^0.3.0"
//! remotefs-aws-s3 = "^0.3.0"
//! ```
//!
//! these features are supported:
//!
//! - `find`: enable `find()` method for RemoteFs. (*enabled by default*)
//! - `no-log`: disable logging. By default, this library will log via the `log` crate.
//!
//!
//! ### Aws s3 client
//!
//! ```rust,ignore
//! use remotefs::RemoteFs;
//! use remotefs_aws_s3::AwsS3Fs;
//! use std::path::Path;
//!
//! let mut client = AwsS3Fs::new("test-bucket")
//!     .region("eu-west-1")
//!     .profile("default")
//!     .access_key("AKIAxxxxxxxxxxxx")
//!     .secret_access_key("****************");
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
//! ### MinIO client
//!
//! ```rust,ignore
//! use remotefs::RemoteFs;
//! use remotefs_aws_s3::AwsS3Fs;
//! use std::path::Path;
//!
//! let mut client = AwsS3Fs::new("test-bucket")
//!     .endpoint("http://localhost:9000")
//!     .new_path_style(true) // required for MinIO
//!     .access_key("minioadmin")
//!     .secret_access_key("minioadmin");
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
pub use client::AwsS3Fs;

// -- object
pub(crate) mod object;
// -- utils
pub(crate) mod utils;
// -- mock
#[cfg(test)]
pub(crate) mod mock;
