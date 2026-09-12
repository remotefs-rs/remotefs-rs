#![crate_name = "remotefs_kube"]
#![crate_type = "lib"]
#![cfg_attr(docsrs, feature(doc_cfg))]

//! # remotefs-kube
//!
//! remotefs-kube is a [remotefs](https://github.com/remotefs-rs/remotefs-rs)
//! client implementation for Kubernetes, giving shell-level access to the
//! containers of a pod.
//!
//! It exposes two client types: [`KubeContainerFs`], which talks to a single
//! container, and [`KubeMultiPodFs`], which exposes every pod and container
//! in a namespace as one abstract file system.
//!
//! ## Get started
//!
//! First of all you need to add **remotefs** and **remotefs-kube** to your
//! project dependencies:
//!
//! ```toml
//! [dependencies]
//! remotefs = "0.3"
//! remotefs-kube = "0.4"
//! ```
//!
//! ## Feature flags
//!
//! | name      | description                                                          | default |
//! | --------- | --------------------------------------------------------------------- | ------- |
//! | `find`    | Enable the `find()` method on the client.                             | ✔       |
//! | `no-log`  | Disable logging. By default this library logs via the `log` crate.    |         |
//!
//! ## `KubeMultiPodFs`
//!
//! The MultiPod client gives access to every pod and container in a
//! namespace at once, laid out as one abstract file system:
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
//! ```rust,no_run
//! use std::path::Path;
//! use std::sync::Arc;
//!
//! use remotefs::RemoteFs;
//! use remotefs_kube::KubeMultiPodFs;
//!
//! let runtime = Arc::new(
//!     tokio::runtime::Builder::new_current_thread()
//!         .enable_all()
//!         .build()
//!         .expect("failed to build the Tokio runtime"),
//! );
//! let mut client = KubeMultiPodFs::new(&runtime);
//!
//! // connect, using the default kubeconfig
//! client.connect().expect("connection failed");
//! // print the working directory
//! println!("wrkdir: {wrkdir}", wrkdir = client.pwd().expect("pwd failed").display());
//! // change the working directory to a container's `/tmp`
//! client
//!     .change_dir(Path::new("/my-pod/alpine/tmp"))
//!     .expect("cd failed");
//! // disconnect
//! client.disconnect().expect("disconnection failed");
//! ```
//!
//! ## `KubeContainerFs`
//!
//! The container client connects to and interacts with a single container on
//! a given pod, giving full access to that container's file system.
//!
//! ```rust,no_run
//! use std::path::Path;
//! use std::sync::Arc;
//!
//! use remotefs::RemoteFs;
//! use remotefs_kube::KubeContainerFs;
//!
//! let runtime = Arc::new(
//!     tokio::runtime::Builder::new_current_thread()
//!         .enable_all()
//!         .build()
//!         .expect("failed to build the Tokio runtime"),
//! );
//! let mut client = KubeContainerFs::new("my-pod", "container-name", &runtime);
//!
//! // connect, using the default kubeconfig
//! client.connect().expect("connection failed");
//! // print the working directory
//! println!("wrkdir: {wrkdir}", wrkdir = client.pwd().expect("pwd failed").display());
//! // change the working directory
//! client.change_dir(Path::new("/tmp")).expect("cd failed");
//! // disconnect
//! client.disconnect().expect("disconnection failed");
//! ```
//!

#![doc(html_playground_url = "https://play.rust-lang.org")]
#![doc(
    html_favicon_url = "https://raw.githubusercontent.com/remotefs-rs/remotefs-rs/main/assets/logo-128.png"
)]
#![doc(
    html_logo_url = "https://raw.githubusercontent.com/remotefs-rs/remotefs-rs/main/assets/logo.png"
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
