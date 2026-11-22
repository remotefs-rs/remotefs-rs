#![crate_name = "remotefs_ftp"]
#![crate_type = "lib"]

//! # remotefs-ftp
//!
//! remotefs-ftp provides synchronous and asynchronous client implementations
//! for [remotefs](https://github.com/remotefs-rs/remotefs-rs), with support for
//! the FTP/FTPS protocols.
//!
//! ## Get started
//!
//! First, add **remotefs** and this client to your project dependencies:
//!
//! ```toml
//! remotefs = "1"
//! remotefs-ftp = "1"
//! ```
//!
//! [`FtpFs`] implements [`remotefs::RemoteFs`], and `TokioFtpFs` implements
//! `remotefs::AsyncRemoteFs` when the `tokio` feature is enabled. Every path
//! is absolute; the clients keep no working directory. Operations take `&self`,
//! so a connected client can be shared, but FTP allows a single data connection:
//! while a stream returned by `open`, `create` or `append` is alive, managed
//! control-channel operations return `ProtocolError` until that stream is
//! finished or dropped. The observational `is_connected`, `capabilities` and
//! `welcome_message` methods remain available, while `stream` returns `None`.
//! FTP paths must be UTF-8 POSIX paths rooted by exactly one `/`; ASCII control
//! characters and parent components (`..`) are rejected. Windows drive and UNC
//! forms are not FTP paths and are rejected as invalid.
//!
//! ## Feature flags
//!
//! | name                        | description                                                          | default |
//! | --------------------------- | -------------------------------------------------------------------- | ------- |
//! | `find`                      | Enable the remotefs `find()` and `find_async()` functions.           | ✔       |
//! | `native-tls`                | Enable FTPS for `FtpFs` using native-tls as backend.                 |         |
//! | `native-tls-vendored`       | Statically link native-tls.                                          |         |
//! | `no-log`                    | Disable logging; by default the `log` crate is used.                 |         |
//! | `rustls-aws-lc-rs`          | Enable FTPS for `FtpFs` using rustls with aws-lc-rs backend.         |         |
//! | `rustls-ring`               | Enable FTPS for `FtpFs` using rustls with ring backend.              |         |
//! | `tokio`                     | Enable `TokioFtpFs`, the Tokio client implementing `AsyncRemoteFs`.  |         |
//! | `tokio-native-tls`          | Enable FTPS for `TokioFtpFs` using async-native-tls as backend.      |         |
//! | `tokio-native-tls-vendored` | Statically link native-tls for `TokioFtpFs`.                         |         |
//! | `tokio-rustls-aws-lc-rs`    | Enable FTPS for `TokioFtpFs` using rustls with aws-lc-rs backend.    |         |
//! | `tokio-rustls-ring`         | Enable FTPS for `TokioFtpFs` using rustls with ring backend.         |         |
//!
//! The sync TLS backends are mutually exclusive, and so are the tokio TLS
//! backends. A sync backend and a tokio backend can be enabled together; the
//! sync features never pull Tokio in.
//!
//! ### FTP client
//!
//! ```rust,no_run
//! use std::path::Path;
//!
//! use remotefs::RemoteFs;
//! use remotefs::fs::{ReadOptions, WriteOptions};
//! use remotefs_ftp::FtpFs;
//!
//! # fn run() -> remotefs::RemoteResult<()> {
//! let mut client = FtpFs::new("127.0.0.1", 21)
//!     .username("test")
//!     .password("password");
//! client.connect()?;
//! if let Some(banner) = client.welcome_message() {
//!     println!("{banner}");
//! }
//! let mut source = std::io::Cursor::new(b"hello".to_vec());
//! client.write_file(
//!     Path::new("/upload/hello.txt"),
//!     &WriteOptions::default().size_hint(5),
//!     &mut source,
//! )?;
//! let mut destination = Vec::new();
//! client.read_file(
//!     Path::new("/upload/hello.txt"),
//!     &ReadOptions::default().offset(1).length(3),
//!     &mut destination,
//! )?;
//! assert_eq!(destination, b"ell");
//! client.disconnect()?;
//! # Ok(())
//! # }
//! ```
//!
//! ### Tokio client
//!
//! With the `tokio` feature, `TokioFtpFs` implements
//! `remotefs::AsyncRemoteFs` with the same builder, path rules and
//! capabilities. Operations share the control connection through an async
//! mutex, so concurrent callers wait for each other instead of failing.
//!
//! ```rust,no_run
//! # #[cfg(feature = "tokio")]
//! # async fn run() -> remotefs::RemoteResult<()> {
//! use std::path::Path;
//!
//! use remotefs::AsyncRemoteFs;
//! use remotefs::fs::ReadOptions;
//! use remotefs_ftp::TokioFtpFs;
//!
//! let mut client = TokioFtpFs::new("127.0.0.1", 21)
//!     .username("test")
//!     .password("password");
//! client.connect().await?;
//! let mut destination = Vec::new();
//! client
//!     .read_file(
//!         Path::new("/upload/hello.txt"),
//!         &ReadOptions::default(),
//!         &mut destination,
//!     )
//!     .await?;
//! client.disconnect().await?;
//! # Ok(())
//! # }
//! ```
//!
//! Async streams cannot drain in `Drop`. Call `finish().await` on every
//! stream: dropping a download before EOF marks the control connection
//! unusable until the client reconnects, while dropped uploads and downloads
//! at EOF let the next command consume the pending reply. Operation futures
//! are not cancellation safe; cancelling one after its command was sent also
//! requires a reconnect.
//!
//! ### Transfers
//!
//! `open` returns an owned read stream backed by `RETR`. Read offsets request
//! FTP `REST` before `RETR`; when the server refuses the marker or the offset
//! does not fit the platform's `usize`, the client skips the prefix locally.
//! A read at or beyond EOF still opens `RETR` and becomes exhausted after the
//! native or local skip, while missing and permission errors remain visible. A
//! read length limits the bytes returned to the caller, but explicit `finish`
//! and wrapper drop drain the unread tail before closing the data socket and
//! reading the transfer reply. Draining can block and consume bandwidth for
//! the rest of the file. A returned `426` remains a typed `ConnectionError`; it
//! is not treated as success.
//!
//! `create` and `append` return owned write streams backed by `STOR` and `APPE`.
//! Call `finish` on every stream: it closes the data socket and reads the
//! transfer reply, which is the only way to learn whether the transfer
//! succeeded. Drop cleanup is best-effort; a cleanup failure marks the control
//! connection unusable until the client reconnects. Limited read streams drain
//! their unread tail during either cleanup path, so dropping one can block.
//!
//! `stream` is a raw escape hatch into the FTP control connection and returns
//! `None` during a managed transfer. Commands issued through it bypass managed
//! coordination. Raw transfers must be finished and their completion reply
//! consumed before managed operations resume; leave the connection authenticated
//! and in binary transfer mode.
//!
//! `exec` runs `SITE` subcommands (for example `CHMOD 644 /path`), not shell
//! commands. `copy`, `symlink` and `set_metadata` are not supported by FTP and
//! return `UnsupportedFeature`.
//!

#![doc(html_playground_url = "https://play.rust-lang.org")]
#![doc(
    html_favicon_url = "https://raw.githubusercontent.com/remotefs-rs/remotefs-rs/main/assets/logo-128.png"
)]
#![doc(
    html_logo_url = "https://raw.githubusercontent.com/remotefs-rs/remotefs-rs/main/assets/logo.png"
)]
#![cfg_attr(docsrs, feature(doc_cfg))]

#[cfg(any(
    all(feature = "native-tls", feature = "rustls-aws-lc-rs"),
    all(feature = "native-tls", feature = "rustls-ring"),
    all(feature = "rustls-aws-lc-rs", feature = "rustls-ring"),
))]
compile_error!(
    "`native-tls`, `rustls-aws-lc-rs` and `rustls-ring` are mutually exclusive; enable one"
);

#[cfg(any(
    all(feature = "tokio-native-tls", feature = "tokio-rustls-aws-lc-rs"),
    all(feature = "tokio-native-tls", feature = "tokio-rustls-ring"),
    all(feature = "tokio-rustls-aws-lc-rs", feature = "tokio-rustls-ring"),
))]
compile_error!(
    "`tokio-native-tls`, `tokio-rustls-aws-lc-rs` and `tokio-rustls-ring` are mutually exclusive; enable one"
);

// -- crates
#[macro_use]
extern crate log;

pub mod client;
#[doc(inline)]
pub use client::FtpFs;
#[cfg(feature = "tokio")]
#[cfg_attr(docsrs, doc(cfg(feature = "tokio")))]
pub use client::TokioFtpFs;
// TODO(task 4): re-export TokioFtpFs when its implementation lands.

// -- utils
pub(crate) mod utils;

// test containers
#[cfg(test)]
mod test_container;

#[cfg(test)]
pub fn log_init() {
    use std::sync::Once;

    static INIT: Once = Once::new();

    INIT.call_once(|| {
        let _ = env_logger::builder()
            .filter_level(log::LevelFilter::Trace)
            .is_test(true)
            .try_init();
    });
}
