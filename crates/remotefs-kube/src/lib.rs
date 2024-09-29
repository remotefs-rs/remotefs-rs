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
//! remotefs-kube = "^0.3"
//! ```
//!
//! these features are supported:
//!
//! - `find`: enable `find()` method for RemoteFs. (*enabled by default*)
//! - `no-log`: disable logging. By default, this library will log via the `log` crate.
//!
//!
//! ### Kube multi pod client
//!
//! The MultiPod client gives access to all the pods with their own containers in a namespace.
//!
//! This client creates an abstract file system with the following structure
//!
//! - / (root)
//!   - pod-a
//!     - container-a
//!       - / (container-a root)
//!         - /bin
//!         - /home
//!         - ...
//!     - container-b
//!       - / (container-b root)
//!         - ...
//!   - pod-b
//!     - container-c
//!       - / (container-c root)
//!         - ...
//!
//! So paths have the following structure: `/pod-name/container-name/path/to/file`.
//!
//! ```rust,ignore
//!
//! // import remotefs trait and client
//! use remotefs::RemoteFs;
//! use remotefs_kube::KubeMultiPodFs;
//! use std::path::Path;
//!
//! let rt = Arc::new(
//!     tokio::runtime::Builder::new_current_thread()
//!     .enable_all()
//!     .build()
//!     .unwrap(),
//! );
//! let mut client: KubeMultiPodFs = KubeMultiPodFs::new(&rt);
//!
//! // connect
//! assert!(client.connect().is_ok());
//! // get working directory
//! println!("Wrkdir: {}", client.pwd().ok().unwrap().display());
//! // change working directory
//! assert!(client.change_dir(Path::new("/my-pod/alpine/tmp")).is_ok());
//! // disconnect
//! assert!(client.disconnect().is_ok());
//! ```
//!
//! ### Kube container client
//!
//! Here is a basic usage example, with the `KubeContainerFs` client, which is used to connect and interact with a single container on a certain pod. This client gives the entire access to the container file system.
//!
//! ```rust,ignore
//!
//! // import remotefs trait and client
//! use remotefs::RemoteFs;
//! use remotefs_kube::KubeContainerFs;
//! use std::path::Path;
//!
//! let rt = Arc::new(
//!     tokio::runtime::Builder::new_current_thread()
//!     .enable_all()
//!     .build()
//!     .unwrap(),
//! );
//! let mut client: KubeContainerFs = KubeContainerFs::new("my-pod", "container-name", &rt);
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

mod kube_container_fs;
mod kube_multipod_fs;
mod utils;

pub use kube::Config;
pub use kube_container_fs::KubeContainerFs;
pub use kube_multipod_fs::KubeMultiPodFs;

// -- test logging
#[cfg(test)]
pub fn log_init() {
    let _ = env_logger::builder()
        .is_test(true)
        .filter_module("remotefs_kube", log::LevelFilter::Debug)
        .try_init();
}
