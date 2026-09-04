#![crate_name = "remotefs_fuse"]
#![crate_type = "lib"]
#![cfg_attr(docsrs, feature(doc_cfg))]

//! # remotefs-fuse
//!
//! **remotefs-fuse** is a library that allows you to mount a remote file system using **FUSE** on Linux and macOS and with
//! **Dokany** on Windows.
//!
//! ## Requirements
//!
//! - **Linux**: you need to have `fuse3` installed on your system.
//!
//!   Of course, you also need to have the `FUSE` kernel module installed.
//!   To build `remotefs-fuse` on Linux with the default `libfuse` feature, you also need the
//!   `libfuse3` development package. Building with `--no-default-features` needs only `fuse3`.
//!
//!   In Ubuntu, you can install it with:
//!
//!   ```sh
//!   sudo apt-get install fuse3 libfuse3-dev
//!   ```
//!
//!   In CentOS, you can install it with:
//!
//!   ```sh
//!   sudo yum install fuse-devel
//!   ```
//!
//! - **macOS**: you need to have the `macfuse` service installed on your system.
//!
//!   You can install it with:
//!
//!   ```sh
//!   brew install macfuse
//!   ```
//!
//! - **Windows**: you need to have the `dokany` service installed on your system.
//!
//!   You can install it from <https://github.com/dokan-dev/dokany?tab=readme-ov-file#installation>
//!
//! ## Get started
//!
//! First of all you need to add **remotefs-fuse** to your project dependencies:
//!
//! ```toml
//! remotefs-fuse = "^0.1.0"
//! ```
//!
//! ## Feature flags
//!
//! | name                | description                                              | default |
//! |----------------------|----------------------------------------------------------|---------|
//! | `libfuse`           | Link against the system `libfuse3` (Unix only). See below. | ✅       |
//! | `no-log`            | Disable logging. By default, this library logs via the `log` crate. |         |
//! | `integration-tests` | Enable tests that mount a real filesystem; only meant for this crate's own test suite. |         |
//!
//! `libfuse` *(enabled by default, Unix only)*: link against the system `libfuse3`.
//!
//! With the feature disabled, `fuser` uses its pure-Rust mount implementation instead:
//! nothing is linked at build time, so `libfuse3-dev` is not needed to compile, and
//! mounting shells out to the `fusermount3` binary from the `fuse3` package. Either way
//! `fusermount3` must be present at runtime for unprivileged mounts, so the practical
//! difference is only in what you need in order to *build*.
//!
//! The feature is inert on macOS and Windows: on macOS `fuser` always links macFUSE, and
//! on Windows the driver is Dokany.
//!
//! ## Example
//!
//! ```rust,no_run,ignore
//! use remotefs_fuse::Mount;
//!
//! let options = vec![
//!     #[cfg(unix)]
//!     remotefs_fuse::MountOption::AllowRoot,
//!     #[cfg(unix)]
//!     remotefs_fuse::MountOption::RW,
//!     #[cfg(unix)]
//!     remotefs_fuse::MountOption::Exec,
//!     #[cfg(unix)]
//!     remotefs_fuse::MountOption::Sync,
//!     #[cfg(unix)]
//!     remotefs_fuse::MountOption::FSName(volume),
//! ];
//!
//! let remote = MyRemoteFileSystem::new();
//! let mount_path = std::path::PathBuf::from("/mnt/remote");
//! let mut mount = Mount::mount(remote, &mount_path, &options).expect("Failed to mount");
//! let mut umount = mount.unmounter();
//!
//! // setup signal handler
//! ctrlc::set_handler(move || {
//!     if let Err(err) = umount.unmount() {
//!         eprintln!("Failed to unmount: {err}");
//!     }
//! })?;
//!
//! mount.run().expect("Failed to run filesystem event loop");
//!
//! ```
//!
//! > To mount on a Windows system **specify a drive letter** (e.g. `Z`) instead of a path.
//!
//! ## Project stability
//!
//! Please consider this is an early-stage project and I haven't heavily tested it, in particular on Windows systems.
//!
//! I suggest you to first test it on test filesystems to see whether the library behaves correctly with your system.
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
mod mount;

pub use self::mount::{Mount, MountOption, Unmount};
