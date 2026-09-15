//! FTP clients implementing the remotefs contracts.
//!
//! [`FtpFs`] is the blocking client. With the `tokio` feature, `TokioFtpFs` is
//! the asynchronous client. Both share the LIST parser, path validation and
//! error mapping in this module.

mod error;
mod guard;
mod list;
mod path;
#[cfg(test)]
mod scripted;
mod sync;
#[cfg(feature = "tokio")]
mod tokio;

pub use self::sync::{FtpFs, FtpStream, PassiveStreamBuilder};
// TODO(task 4): re-export async client types when the implementation lands.
