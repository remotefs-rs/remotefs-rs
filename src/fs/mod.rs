//! ## Fs
//!
//! `fs` is the module which provides remote file system entities

mod errors;
mod file;
pub mod stream;
mod sync;
mod welcome;

pub use self::errors::{RemoteError, RemoteErrorType, RemoteResult};
pub use self::file::{File, FileType, Metadata, UnixPex, UnixPexClass};
pub use self::stream::{ReadStream, WriteStream};
pub use self::sync::RemoteFs;
pub use self::welcome::Welcome;
