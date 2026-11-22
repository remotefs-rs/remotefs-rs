#![crate_name = "remotefs_kube"]
#![crate_type = "lib"]
#![cfg_attr(docsrs, feature(doc_cfg))]

//! # remotefs-kube
//!
//! remotefs-kube is a [remotefs](https://github.com/remotefs-rs/remotefs-rs)
//! client implementation for Kubernetes, giving shell-level access to the
//! containers of a pod.
//!
//! It exposes two async client types implementing
//! [`remotefs::AsyncRemoteFs`]: [`KubeContainerFs`], which talks to a single
//! container, and [`KubeMultiPodFs`], which exposes every pod and container
//! in a namespace as one abstract file system. Every path must be an absolute
//! POSIX path; there is no working directory.
//!
//! ## Get started
//!
//! First of all you need to add **remotefs** and **remotefs-kube** to your
//! project dependencies:
//!
//! ```toml
//! [dependencies]
//! remotefs = { version = "1", features = ["async"] }
//! remotefs-kube = "1"
//! tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
//! ```
//!
//! ## Feature flags
//!
//! | name     | description                                                                   | default |
//! | -------- | ----------------------------------------------------------------------------- | ------- |
//! | `find`   | Enable `remotefs::find_async` for the clients.                                | ✔       |
//! | `no-log` | Disable logging. By default this library logs via the `log` crate.            |         |
//! | `tokio`  | Enable `into_blocking`, wrapping a client for blocking `remotefs::RemoteFs` callers. |         |
//!
//! ## `KubeContainerFs`
//!
//! The container client connects to and interacts with a single container on
//! a given pod, giving full access to that container's file system.
//!
//! ```rust,no_run
//! use std::path::Path;
//!
//! use remotefs::AsyncRemoteFs;
//! use remotefs::fs::{ReadOptions, WriteOptions};
//! use remotefs_kube::KubeContainerFs;
//!
//! # async fn run() -> remotefs::RemoteResult<()> {
//! let mut client = KubeContainerFs::new("my-pod", "container-name");
//! client.connect().await?;
//! let mut source = futures::io::Cursor::new(b"hello".to_vec());
//! client
//!     .write_file(
//!         Path::new("/tmp/hello.txt"),
//!         &WriteOptions::default().size_hint(5),
//!         &mut source,
//!     )
//!     .await?;
//! let mut destination = futures::io::Cursor::new(Vec::new());
//! client
//!     .read_file(
//!         Path::new("/tmp/hello.txt"),
//!         &ReadOptions::default().offset(1).length(3),
//!         &mut destination,
//!     )
//!     .await?;
//! assert_eq!(destination.into_inner(), b"ell");
//! client.disconnect().await?;
//! # Ok(())
//! # }
//! ```
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
//! The root, pod, and container levels are virtual directories: they can be
//! listed and stat-ed but not modified, and `exec` is unsupported.
//!
//! ```rust,no_run
//! use std::path::Path;
//!
//! use remotefs::AsyncRemoteFs;
//! use remotefs_kube::KubeMultiPodFs;
//!
//! # async fn run() -> remotefs::RemoteResult<()> {
//! let mut client = KubeMultiPodFs::new();
//! client.connect().await?;
//! for pod in client.list_dir(Path::new("/")).await? {
//!     println!("pod: {}", pod.name());
//! }
//! let files = client.list_dir(Path::new("/my-pod/alpine/tmp")).await?;
//! client.disconnect().await?;
//! # Ok(())
//! # }
//! ```
//!
//! ## Blocking usage
//!
//! Enable the `tokio` feature and call `into_blocking` to get a
//! `BlockingKubeContainerFs` or `BlockingKubeMultiPodFs`, which implement
//! [`remotefs::RemoteFs`]. The handle must belong to a multi-thread runtime
//! and the wrapper must not be used from inside an async context.
//!
//! ```rust,no_run
//! use remotefs::RemoteFs;
//! use remotefs_kube::KubeContainerFs;
//!
//! # #[cfg(feature = "tokio")]
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let runtime = tokio::runtime::Runtime::new()?;
//! let mut client: Box<dyn RemoteFs> = Box::new(
//!     KubeContainerFs::new("my-pod", "container-name")
//!         .into_blocking(runtime.handle().clone()),
//! );
//! client.connect()?;
//! client.disconnect()?;
//! # Ok(())
//! # }
//! # #[cfg(not(feature = "tokio"))]
//! # fn main() {}
//! ```
//!
//! ## Transfers
//!
//! `open` streams `cat` (or `tail -c`/`head -c` for ranges), `create` streams
//! into `cat > path`, and `append` into `cat >> path`. A stream is complete
//! only when `finish` succeeds; dropping it aborts the remote command. Read
//! offsets and lengths are honored natively (`Capabilities::RANGE_READ`);
//! seeking is not supported. `WriteOptions::size_hint` is optional; when set,
//! `write_file` reads at most that many bytes and the transfer fails unless
//! exactly that many were written.

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
#[cfg(feature = "tokio")]
pub use kube_container_fs::BlockingKubeContainerFs;
pub use kube_container_fs::KubeContainerFs;
#[cfg(feature = "tokio")]
pub use kube_multipod_fs::BlockingKubeMultiPodFs;
pub use kube_multipod_fs::KubeMultiPodFs;

// -- test logging
#[cfg(test)]
pub fn log_init() {
    let _ = env_logger::builder()
        .is_test(true)
        .filter_module("remotefs_kube", log::LevelFilter::Debug)
        .try_init();
}
