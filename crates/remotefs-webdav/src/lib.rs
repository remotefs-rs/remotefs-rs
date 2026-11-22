#![crate_name = "remotefs_webdav"]
#![crate_type = "lib"]

//! # remotefs-webdav
//!
//! remotefs-webdav is a [remotefs](https://github.com/remotefs-rs/remotefs-rs)
//! client implementation for the WebDAV protocol, as specified in
//! [RFC 4918](https://www.rfc-editor.org/rfc/rfc4918).
//!
//! It exposes [`WebDAVFs`], which implements [`remotefs::AsyncRemoteFs`] and
//! can therefore be used interchangeably with any other remotefs client. With
//! the `tokio` feature, `WebDAVFs::into_blocking` returns a
//! `BlockingWebDAVFs` implementing the blocking [`remotefs::RemoteFs`].
//!
//! ## Get started
//!
//! Add **remotefs** and **remotefs-webdav** to your project dependencies:
//!
//! ```toml
//! [dependencies]
//! futures = "0.3"
//! remotefs = "1"
//! remotefs-webdav = "1"
//! tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
//! ```
//!
//! Then connect to the server and use the client with absolute paths:
//!
//! ```rust,no_run
//! use std::path::Path;
//!
//! use remotefs::AsyncRemoteFs;
//! use remotefs::fs::{ReadOptions, WriteOptions};
//! use remotefs_webdav::{Auth, WebDAVFs};
//!
//! # async fn run() -> remotefs::RemoteResult<()> {
//! let mut client = WebDAVFs::new("http://localhost:3080", Auth::basic("alice", "secret1234"))?;
//! client.connect().await?;
//!
//! let mut source = futures::io::Cursor::new(b"hello".to_vec());
//! client
//!     .write_file(
//!         Path::new("/docs/hello.txt"),
//!         &WriteOptions::default().size_hint(5),
//!         &mut source,
//!     )
//!     .await?;
//!
//! let mut destination = futures::io::Cursor::new(Vec::new());
//! client
//!     .read_file(
//!         Path::new("/docs/hello.txt"),
//!         &ReadOptions::default().offset(1).length(3),
//!         &mut destination,
//!     )
//!     .await?;
//! assert_eq!(destination.into_inner(), b"ell");
//!
//! client.disconnect().await?;
//! # Ok(())
//! # }
//! ```
//!
//! ## Blocking usage
//!
//! Enable the `tokio` feature and call `WebDAVFs::into_blocking` to get a
//! `BlockingWebDAVFs`, which implements [`remotefs::RemoteFs`] and can be
//! stored as `Box<dyn RemoteFs>`. It must not be called from inside an async
//! context, and the handle must belong to a multi-thread Tokio runtime.
//!
//! ```rust,no_run
//! use remotefs::RemoteFs;
//! use remotefs_webdav::{Auth, WebDAVFs};
//!
//! # #[cfg(feature = "tokio")]
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let runtime = tokio::runtime::Runtime::new()?;
//! let mut client: Box<dyn RemoteFs> = Box::new(
//!     WebDAVFs::new("http://localhost:3080", Auth::basic("alice", "secret1234"))?
//!         .into_blocking(runtime.handle().clone()),
//! );
//! client.connect()?;
//! client.disconnect()?;
//! # Ok(())
//! # }
//! # #[cfg(not(feature = "tokio"))]
//! # fn main() {}
//! ```
//!
//! ## Filesystem semantics
//!
//! Every path is absolute and resolved against the base URL given to
//! [`WebDAVFs::new`]. `open` downloads the requested range with an HTTP
//! `Range` header and returns a seekable in-memory stream; `create` buffers
//! writes and uploads them with one `PUT` when the stream is finished, so a
//! dropped stream creates nothing. `rename` and `copy` use `MOVE` and `COPY`
//! with overwrite enabled. `remove_dir` refuses non-empty collections and
//! `remove_dir_all` issues a single recursive `DELETE`. `append`,
//! `set_metadata`, `symlink`, and `exec` return
//! [`remotefs::RemoteErrorType::UnsupportedFeature`].
//!
//! ## Feature flags
//!
//! | name              | description                                                        | default |
//! | ----------------- | ------------------------------------------------------------------ | ------- |
//! | `find`            | Enable `remotefs::find_async` and `remotefs::find`.                | ✔       |
//! | `no-log`          | Disable logging. By default this library logs via the `log` crate. |         |
//! | `tokio`           | Enable `BlockingWebDAVFs` and `WebDAVFs::into_blocking`.           |         |
//! | `with-containers` | Enable the tests which need the WebDAV container. Internal only.   |         |

#![doc(html_playground_url = "https://play.rust-lang.org")]
#![doc(
    html_favicon_url = "https://raw.githubusercontent.com/remotefs-rs/remotefs-rs/main/assets/logo-128.png"
)]
#![doc(
    html_logo_url = "https://raw.githubusercontent.com/remotefs-rs/remotefs-rs/main/assets/logo.png"
)]

#[macro_use]
extern crate log;

mod client;
mod error;
#[cfg(test)]
mod mock;
mod resource;
mod stream;
mod url;

#[cfg(feature = "tokio")]
#[doc(inline)]
pub use client::BlockingWebDAVFs;
#[doc(inline)]
pub use client::WebDAVFs;
#[doc(no_inline)]
pub use dav_xml_client;
pub use dav_xml_client::Auth;
