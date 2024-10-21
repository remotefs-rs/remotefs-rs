mod error;
#[cfg(target_family = "unix")]
#[cfg_attr(docsrs, doc(cfg(target_family = "unix")))]
mod unix;

use remotefs::RemoteFs;

pub use self::error::{DriverError, DriverResult};

/// Remote Filesystem Driver
///
/// This driver takes a instance which implements the [`RemoteFs`] trait and mounts it to a local directory.
///
/// The driver will use the [`fuser`](https://crates.io/crates/fuser) crate to mount the filesystem, on Unix systems, while
/// it will use [dokan](https://crates.io/crates/dokan) on Windows.
pub struct Driver {
    /// Inode database
    #[cfg(target_family = "unix")]
    database: unix::InodeDb,
    /// File handle database
    #[cfg(target_family = "unix")]
    file_handlers: unix::FileHandleDb,
    /// [`RemoteFs`] instance
    remote: Box<dyn RemoteFs>,
}

impl Driver {
    /// Create a new instance of the [`Driver`] providing a instance which implements the [`RemoteFs`] trait.
    ///
    /// The [`RemoteFs`] instance must be boxed.
    ///
    /// # Arguments
    ///
    /// * `remote` - The instance which implements the [`RemoteFs`] trait.
    pub fn new(remote: Box<dyn RemoteFs>) -> Self {
        Self {
            #[cfg(target_family = "unix")]
            database: unix::InodeDb::load(),
            #[cfg(target_family = "unix")]
            file_handlers: unix::FileHandleDb::default(),
            remote,
        }
    }
}
