#![crate_name = "remotefs_ssh"]
#![crate_type = "lib"]
#![cfg_attr(docsrs, feature(doc_cfg))]

//! # remotefs-ssh
//!
//! remotefs-ssh is a client implementation for [remotefs](https://github.com/remotefs-rs/remotefs-rs),
//! providing support for the SCP/SFTP protocols on top of remotefs 1.
//!
//! ## Get started
//!
//! ```toml
//! remotefs = "1"
//! remotefs-ssh = "1"
//! ```
//!
//! > The library supports multiple ssh backends: `libssh2` (default), `libssh`, and `russh`.
//!
//! ### Available backends
//!
//! - `libssh2`: the default backend, using the `libssh2` C library. Blocking [`SftpFs`] / [`ScpFs`].
//! - `libssh`: alternative backend using the `libssh` C library. Blocking [`SftpFs`] / [`ScpFs`].
//! - `russh`: pure-Rust backend. Native asynchronous [`RusshSftpFs`] / [`RusshScpFs`]
//!   implementing [`remotefs::AsyncRemoteFs`], plus blocking wrappers via `into_blocking`.
//!
//! Each C backend can be built with the vendored version of the library:
//! `libssh-vendored` and `libssh2-vendored`. Without them the corresponding
//! system libraries must be installed.
//!
//! Every client is `Send + Sync`; operations take `&self` and every path must be absolute.
//!
//! ### Other features
//!
//! - `find`: enable [`remotefs::find`] / [`remotefs::find_async`] (*enabled by default*)
//! - `no-log`: disable logging.
//!
//! ## Examples
//!
//! ### libssh2 / libssh (blocking)
//!
//! ```rust,no_run
//! # #[cfg(feature = "libssh2")]
//! use std::io::Cursor;
//! # #[cfg(feature = "libssh2")]
//! use std::path::Path;
//!
//! # #[cfg(feature = "libssh2")]
//! use remotefs::RemoteFs;
//! # #[cfg(feature = "libssh2")]
//! use remotefs::fs::{ReadOptions, WriteOptions};
//! # #[cfg(feature = "libssh2")]
//! use remotefs_ssh::{SftpFs, SshConfigParseRule, SshOpts};
//!
//! # #[cfg(feature = "libssh2")]
//! # fn main() -> remotefs::RemoteResult<()> {
//! let opts = SshOpts::new("127.0.0.1")
//!     .port(22)
//!     .username("test")
//!     .password("password")
//!     .config_file(Path::new("/home/cvisintin/.ssh/config"), SshConfigParseRule::STRICT);
//!
//! let mut client = SftpFs::libssh2(opts);
//! client.connect()?;
//! let data = b"hello";
//! client.write_file(
//!     Path::new("/tmp/hello.txt"),
//!     &WriteOptions::default().size_hint(data.len() as u64),
//!     &mut Cursor::new(data),
//! )?;
//! let mut output = Vec::new();
//! client.read_file(Path::new("/tmp/hello.txt"), &ReadOptions::default(), &mut output)?;
//! assert_eq!(output, data);
//! client.disconnect()
//! # }
//! # #[cfg(not(feature = "libssh2"))]
//! # fn main() {}
//! ```
//!
//! ### russh (async)
//!
//! The `russh` backend needs a Tokio runtime driving its futures and a type
//! implementing `russh::client::Handler` for server key verification.
//! [`NoCheckServerKey`] accepts every host key.
//!
//! ```rust,no_run
//! # #[cfg(feature = "russh")]
//! use std::path::Path;
//!
//! # #[cfg(feature = "russh")]
//! use remotefs::AsyncRemoteFs;
//! # #[cfg(feature = "russh")]
//! use remotefs_ssh::{NoCheckServerKey, RusshSftpFs, SshOpts};
//!
//! # #[cfg(feature = "russh")]
//! # async fn example() -> remotefs::RemoteResult<()> {
//! let mut client: RusshSftpFs<NoCheckServerKey> =
//!     RusshSftpFs::new(SshOpts::new("127.0.0.1").port(22).username("test").password("password"));
//! client.connect().await?;
//! for entry in client.list_dir(Path::new("/tmp")).await? {
//!     println!("{name}", name = entry.name());
//! }
//! client.disconnect().await
//! # }
//! ```
//!
//! ### russh (blocking wrapper)
//!
//! ```rust,no_run
//! # #[cfg(feature = "russh")]
//! use remotefs::RemoteFs;
//! # #[cfg(feature = "russh")]
//! use remotefs_ssh::{NoCheckServerKey, RusshSftpFs, SshOpts};
//!
//! # #[cfg(feature = "russh")]
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let runtime = tokio::runtime::Runtime::new()?;
//! let mut client: Box<dyn RemoteFs> = Box::new(
//!     RusshSftpFs::<NoCheckServerKey>::new(SshOpts::new("127.0.0.1"))
//!         .into_blocking(runtime.handle().clone()),
//! );
//! client.connect()?;
//! client.disconnect()?;
//! # Ok(())
//! # }
//! # #[cfg(not(feature = "russh"))]
//! # fn main() {}
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
    KeyMethod, MethodType, ParseRule as SshConfigParseRule, SCP_CAPABILITIES, SFTP_CAPABILITIES,
    ScpFs, SftpFs, SshAgentIdentity, SshKeyStorage, SshOpts, SshSession,
};

#[cfg(feature = "libssh2")]
#[cfg_attr(docsrs, doc(cfg(feature = "libssh2")))]
pub use self::ssh::LibSsh2Session;
#[cfg(feature = "libssh")]
#[cfg_attr(docsrs, doc(cfg(feature = "libssh")))]
pub use self::ssh::LibSshSession;
#[cfg(feature = "russh")]
#[cfg_attr(docsrs, doc(cfg(feature = "russh")))]
pub use self::ssh::{
    BlockingRusshScpFs, BlockingRusshSftpFs, NoCheckServerKey, RusshScpFs, RusshSession,
    RusshSftpFs,
};

// -- utils
pub(crate) mod utils;
// -- mock
#[cfg(test)]
pub(crate) mod mock;
