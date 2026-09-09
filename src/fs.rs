//! The remote file system contract and the types that travel across it.
//!
//! This module is the whole public surface of the crate. It is split in four
//! parts, each of which a client crate re-exports as-is:
//!
//! - [`RemoteFs`] and `AsyncRemoteFs`, the blocking and runtime-neutral async
//!   contracts every protocol client can implement.
//! - The entry types — [`File`], [`Metadata`], [`FileType`], [`UnixPex`] and
//!   [`UnixPexClass`] — which describe a remote entry the way `std::fs`
//!   describes a local one, minus the parts no protocol can answer.
//! - The transfer types [`ReadStream`] and [`WriteStream`], which hide whether
//!   the underlying protocol stream can seek.
//! - The failure types [`RemoteError`], [`RemoteErrorType`] and
//!   [`RemoteResult`], plus [`Welcome`], the banner a server may greet with.
//!
//! Because every client crate re-exports these types, additions to the public
//! contract are coordinated across the remotefs family. Paths passed to these
//! contracts are absolute. [`crate::path::ensure_absolute`] validates remote
//! roots independently of the client platform.
//!
//! # Examples
//!
//! ```
//! use std::time::SystemTime;
//!
//! use remotefs::fs::{File, FileType, Metadata, UnixPex, UnixPexClass};
//!
//! let file = File::new(
//!     "/home/omar/README.md",
//!     Metadata::default()
//!         .file_type(FileType::File)
//!         .size(1024)
//!         .modified(SystemTime::UNIX_EPOCH)
//!         .mode(UnixPex::new(
//!             UnixPexClass::new(true, true, false),
//!             UnixPexClass::new(true, false, false),
//!             UnixPexClass::new(true, false, false),
//!         )),
//! );
//!
//! assert_eq!(file.name(), "README.md");
//! assert!(file.is_file());
//! ```

#[cfg(feature = "async")]
mod r#async;
mod capabilities;
mod errors;
mod file;
mod forward;
mod options;
pub mod stream;
mod sync;
mod welcome;

#[cfg(feature = "async")]
#[doc(inline)]
pub use self::r#async::AsyncRemoteFs;
#[doc(inline)]
pub use self::capabilities::Capabilities;
#[doc(inline)]
pub use self::errors::{RemoteError, RemoteErrorType, RemoteResult};
#[doc(inline)]
pub use self::file::{File, FileType, Metadata, SetMetadata, UnixPex, UnixPexClass};
#[doc(inline)]
pub use self::options::{ExecOutput, ReadOptions, WriteOptions};
#[cfg(feature = "async")]
#[doc(inline)]
pub use self::stream::r#async::{
    AsyncReadStream, AsyncRemoteRead, AsyncRemoteWrite, AsyncWriteStream,
};
#[doc(inline)]
pub use self::stream::{ReadStream, RemoteRead, RemoteWrite, WriteStream};
#[doc(inline)]
pub use self::sync::RemoteFs;
#[doc(inline)]
pub use self::welcome::Welcome;
