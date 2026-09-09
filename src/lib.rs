#![doc(html_playground_url = "https://play.rust-lang.org")]
#![doc(
    html_favicon_url = "https://raw.githubusercontent.com/remotefs-rs/remotefs-rs/main/assets/logo-128.png"
)]
#![doc(
    html_logo_url = "https://raw.githubusercontent.com/remotefs-rs/remotefs-rs/main/assets/logo.png"
)]
#![deny(missing_docs)]

//! # remotefs
//!
//! A protocol-agnostic view of a remote host as a file system.
//!
//! remotefs describes a remote host as if it were a directory tree mounted on the
//! local machine. A protocol implements [`RemoteFs`] or, with the `async`
//! feature, `AsyncRemoteFs`. Every consumer — a file manager, a FUSE
//! mount, a backup job — can then work against every compatible protocol without
//! a protocol-specific branch.
//!
//! This crate carries the contract and the types that travel across it
//! ([`File`], [`fs::Metadata`], [`fs::UnixPex`], [`fs::ReadStream`],
//! [`fs::WriteStream`], [`RemoteError`]). It ships no client of its own; the
//! clients are separate crates that depend on this one:
//!
//! | crate                                                                | protocol           |
//! | -------------------------------------------------------------------- | ------------------ |
//! | [remotefs-aws-s3](https://github.com/remotefs-rs/remotefs-rs-aws-s3) | AWS S3             |
//! | [remotefs-ftp](https://github.com/remotefs-rs/remotefs-rs-ftp)       | FTP and FTPS       |
//! | [remotefs-kube](https://github.com/remotefs-rs/remotefs-rs-kube)     | Kubernetes         |
//! | [remotefs-smb](https://github.com/remotefs-rs/remotefs-rs-smb)       | SMB                |
//! | [remotefs-ssh](https://github.com/remotefs-rs/remotefs-rs-ssh)       | SFTP and SCP       |
//! | [remotefs-webdav](https://github.com/remotefs-rs/remotefs-rs-webdav) | WebDAV             |
//!
//! ## Get started
//!
//! Add remotefs and the client you need to your dependencies:
//!
//! ```toml
//! remotefs = "1"
//! ```
//!
//! Depend on this crate directly only when you write a client of your own or when
//! your code is generic over the protocol; otherwise the client crate re-exports
//! everything you need.
//!
//! ## Feature flags
//!
//! | name     | description                                                                     | default |
//! | -------- | ------------------------------------------------------------------------------- | ------- |
//! | `async`  | Enable the runtime-neutral asynchronous filesystem contract and transfer types. |         |
//! | `find`   | Enable the `find` and `find_async` explicit-root search functions.                | ✔       |
//! | `tokio`  | Enable Tokio adapters for bridging blocking and asynchronous clients.             |         |
//! | `no-log` | Compile out every log statement by forcing `log/max_level_off`.                  |         |
//!
//! ## Examples
//!
//! One-shot transfers borrow the caller's I/O object and return the byte count:
//!
//! ```
//! use std::io::Cursor;
//! use std::path::Path;
//!
//! use remotefs::fs::WriteOptions;
//! use remotefs::{RemoteFs, RemoteResult};
//!
//! fn upload(fs: &dyn RemoteFs, path: &Path, bytes: &[u8]) -> RemoteResult<u64> {
//!     let opts = WriteOptions::default().size_hint(bytes.len() as u64);
//!     let mut input = Cursor::new(bytes);
//!     fs.write_file(path, &opts, &mut input)
//! }
//! ```
//!
//! Backends receive absolute paths. [`path::ensure_absolute`] validates remote
//! POSIX, drive, and UNC roots independently of the client platform. Use `find`
//! or `find_async` with an explicit absolute root for recursive search.
//! Streams are owned and must be consumed by calling `finish`; dropping one
//! abandons the transfer.
//!
//! With `tokio`, `adapters::blocking::BlockOn` exposes an async client to a
//! blocking consumer and `adapters::r#async::Unblock` offloads a blocking
//! client. Native async clients should be preferred when the protocol provides
//! them.

// -- export
#[cfg(feature = "async")]
#[doc(no_inline)]
pub use async_trait::async_trait;
#[cfg(feature = "async")]
#[doc(inline)]
pub use fs::AsyncRemoteFs;
#[doc(inline)]
pub use fs::{File, RemoteError, RemoteErrorType, RemoteFs, RemoteResult};
// -- modules
#[cfg(feature = "tokio")]
pub mod adapters;
#[cfg(feature = "tokio")]
#[doc(inline)]
pub use adapters::r#async;
#[cfg(feature = "tokio")]
#[doc(inline)]
pub use adapters::blocking;
#[cfg(feature = "find")]
mod find;
pub mod fs;
#[cfg(feature = "async")]
mod io;
pub mod path;
#[cfg(feature = "find")]
#[doc(inline)]
pub use find::find;
#[cfg(all(feature = "async", feature = "find"))]
#[doc(inline)]
pub use find::find_async;

// -- mock
#[cfg(test)]
pub(crate) mod mock;
