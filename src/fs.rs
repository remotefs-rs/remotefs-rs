//! The remote file system contract and the types that travel across it.
//!
//! This module is the whole public surface of the crate. It is split in four
//! parts, each of which a client crate re-exports as-is:
//!
//! - [`RemoteFs`], the trait every protocol client implements. It is the only
//!   thing a consumer programs against.
//! - The entry types — [`File`], [`Metadata`], [`FileType`], [`UnixPex`] and
//!   [`UnixPexClass`] — which describe a remote entry the way `std::fs`
//!   describes a local one, minus the parts no protocol can answer.
//! - The transfer types [`ReadStream`] and [`WriteStream`], which hide whether
//!   the underlying protocol stream can seek.
//! - The failure types [`RemoteError`], [`RemoteErrorType`] and
//!   [`RemoteResult`], plus [`Welcome`], the banner a server may greet with.
//!
//! Because every client crate re-exports these types, a change here is a change
//! for the whole remotefs family: treat a new [`RemoteErrorType`] variant or a
//! new [`Metadata`] field as a breaking change.
//!
//! # Examples
//!
//! ```
//! use std::time::SystemTime;
//!
//! use remotefs::fs::{File, FileType, Metadata, UnixPex, UnixPexClass};
//!
//! let file = File {
//!     path: "/home/omar/README.md".into(),
//!     metadata: Metadata::default()
//!         .file_type(FileType::File)
//!         .size(1024)
//!         .modified(SystemTime::UNIX_EPOCH)
//!         .mode(UnixPex::new(
//!             UnixPexClass::new(true, true, false),
//!             UnixPexClass::new(true, false, false),
//!             UnixPexClass::new(true, false, false),
//!         )),
//! };
//!
//! assert_eq!(file.name(), "README.md");
//! assert!(file.is_file());
//! ```

mod errors;
mod file;
pub mod stream;
mod sync;
mod welcome;

#[doc(inline)]
pub use self::errors::{RemoteError, RemoteErrorType, RemoteResult};
#[doc(inline)]
pub use self::file::{File, FileType, Metadata, UnixPex, UnixPexClass};
#[doc(inline)]
pub use self::stream::{ReadStream, WriteStream};
#[doc(inline)]
pub use self::sync::RemoteFs;
#[doc(inline)]
pub use self::welcome::Welcome;
