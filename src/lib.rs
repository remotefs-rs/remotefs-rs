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
//! local machine. The entire contract is the [`RemoteFs`] trait: a protocol
//! implements it once, and every consumer — a file manager, a FUSE mount, a backup
//! job — works against every protocol without a single protocol-specific branch.
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
//! remotefs = "0.3"
//! remotefs-ssh = "0.4"
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
//! | `find`   | Enable `RemoteFs::find`, a recursive search matching names against a wildcard.   | ✔       |
//! | `no-log` | Compile out every log statement by forcing `log/max_level_off`.                  |         |
//!
//! ## Examples
//!
//! Code written against the trait works with any client:
//!
//! ```
//! use remotefs::{RemoteFs, RemoteResult};
//!
//! /// Collect the names of every entry in the remote working directory.
//! fn list_working_dir<T>(client: &mut T) -> RemoteResult<Vec<String>>
//! where
//!     T: RemoteFs,
//! {
//!     let wrkdir = client.pwd()?;
//!
//!     Ok(client
//!         .list_dir(&wrkdir)?
//!         .into_iter()
//!         .map(|file| file.name())
//!         .collect())
//! }
//! ```

// -- export
#[doc(inline)]
pub use fs::{File, RemoteError, RemoteErrorType, RemoteFs, RemoteResult};
// -- modules
pub mod fs;

// -- utils
pub(crate) mod utils;
// -- mock
#[cfg(test)]
pub(crate) mod mock;
