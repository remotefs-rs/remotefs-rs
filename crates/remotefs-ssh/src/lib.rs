#![crate_name = "remotefs_ssh"]
#![crate_type = "lib"]
#![cfg_attr(docsrs, feature(doc_cfg))]

//! # remotefs-ssh
//!
//! remotefs-ssh is a client implementation for [remotefs](https://github.com/remotefs-rs/remotefs-rs), providing support for the SCP/SFTP protocols.
//!
//! ## Get started
//!
//! First of all you need to add **remotefs** and the client to your project dependencies:
//!
//! ```toml
//! remotefs = "^0.3"
//! remotefs-ssh = "^0.8"
//! ```
//!
//! > The library supports multiple ssh backends.
//! > Currently `libssh2`, `libssh`, and `russh` are supported.
//! >
//! > By default the library is using `libssh2`.
//!
//! ### Available backends
//!
//! Each backend can be set as a feature in your `Cargo.toml`. Multiple backends can be enabled at the same time.
//!
//! - `libssh2`: The default backend, using the `libssh2` library for SSH connections.
//! - `libssh`: An alternative backend, using the `libssh` library for SSH connections.
//! - `russh`: A pure-Rust backend, using the `russh` library for SSH connections. Does not require any system C libraries.
//!
//! Each C backend can be built with the vendored version, using the vendored feature instead:
//!
//! - `libssh-vendored`: Build the `libssh` backend with the vendored version of the library.
//! - `libssh2-vendored`: Build the `libssh2` backend with the vendored version of the library.
//!
//! If the vendored feature is **NOT** provided, you will need to have the corresponding system libraries installed on your machine.
//!
//! > If you need SftpFs to be `Sync` YOU MUST use `libssh2` or `russh`. The `libssh` backend does not support `Sync`.
//!
//! ### Other features
//!
//! these features are supported:
//!
//! - `find`: enable `find()` method on client (*enabled by default*)
//! - `no-log`: disable logging. By default, this library will log via the `log` crate.
//!
//! ## Examples
//!
//! Here is a basic usage example, with the `Sftp` client, which is very similiar to the `Scp` client.
//!
//! The [`SftpFs`] and [`ScpFs`] constructors vary depending on the enabled backend:
//! [`SftpFs::libssh2`], [`SftpFs::libssh`], or [`SftpFs::russh`] (and likewise for [`ScpFs`]).
//!
//! ### libssh2 / libssh
//!
//! ```rust,ignore
//! use remotefs::RemoteFs;
//! use remotefs_ssh::{SshConfigParseRule, SftpFs, SshOpts};
//! use std::path::Path;
//!
//! let opts = SshOpts::new("127.0.0.1")
//!     .port(22)
//!     .username("test")
//!     .password("password")
//!     .config_file(Path::new("/home/cvisintin/.ssh/config"), ParseRule::STRICT);
//!
//! let mut client = SftpFs::libssh2(opts);
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
//! ### russh
//!
//! The `russh` backend requires a Tokio runtime and a type implementing `russh::client::Handler`
//! for server key verification. [`NoCheckServerKey`] is provided as a convenience handler that
//! accepts all host keys.
//!
//! ```rust,ignore
//! use remotefs::RemoteFs;
//! use remotefs_ssh::{NoCheckServerKey, SftpFs, SshOpts};
//! use std::path::Path;
//! use std::sync::Arc;
//!
//! let runtime = Arc::new(
//!     tokio::runtime::Builder::new_current_thread()
//!         .enable_all()
//!         .build()
//!         .unwrap(),
//! );
//!
//! let opts = SshOpts::new("127.0.0.1")
//!     .port(22)
//!     .username("test")
//!     .password("password");
//!
//! let mut client: SftpFs<remotefs_ssh::RusshSession<NoCheckServerKey>> =
//!     SftpFs::russh(opts, runtime);
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
    html_favicon_url = "https://raw.githubusercontent.com/remotefs-rs/remotefs-rs/main/assets/logo-128.png"
)]
#![doc(
    html_logo_url = "https://raw.githubusercontent.com/remotefs-rs/remotefs-rs/main/assets/logo.png"
)]

// -- crates
#[macro_use]
extern crate lazy_regex;
#[macro_use]
extern crate log;

// compile error if no backend is chosen
#[cfg(not(any(feature = "libssh2", feature = "libssh", feature = "russh")))]
compile_error!(
    "No SSH backend chosen. Please enable either `libssh2`, `libssh`, or `russh` feature."
);

mod ssh;
pub use ssh::{
    KeyMethod, MethodType, ParseRule as SshConfigParseRule, ScpFs, SftpFs, SshAgentIdentity,
    SshKeyStorage, SshOpts, SshSession,
};

#[cfg(feature = "libssh2")]
#[cfg_attr(docsrs, doc(cfg(feature = "libssh2")))]
pub use self::ssh::LibSsh2Session;
#[cfg(feature = "libssh")]
#[cfg_attr(docsrs, doc(cfg(feature = "libssh")))]
pub use self::ssh::LibSshSession;
#[cfg(feature = "russh")]
#[cfg_attr(docsrs, doc(cfg(feature = "russh")))]
pub use self::ssh::{NoCheckServerKey, RusshSession};

// -- utils
pub(crate) mod utils;
// -- mock
#[cfg(test)]
pub(crate) mod mock;
