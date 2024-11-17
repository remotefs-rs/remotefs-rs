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
//!     Of course, you also need to have the `FUSE` kernel module installed.
//!     To build `remotefs-fuse` on Linux, you need to have the `libfuse3` development package installed.
//!
//!     In Ubuntu, you can install it with:
//!
//!     ```sh
//!     sudo apt-get install fuse3 libfuse3-dev
//!     ```
//!
//!     In CentOS, you can install it with:
//!
//!     ```sh
//!     sudo yum install fuse-devel
//!     ```
//!
//! - **macOS**: you need to have the `macfuse` service installed on your system.
//!
//!     You can install it with:
//!
//!     ```sh
//!     brew install macfuse
//!     ```
//!
//! - **Windows**: you need to have the `dokany` service installed on your system.
//!    
//!    You can install it from <https://github.com/dokan-dev/dokany?tab=readme-ov-file#installation>
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
//!     umount.umount().expect("Failed to unmount");
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

pub use self::mount::{Mount, MountOption, Umount};
