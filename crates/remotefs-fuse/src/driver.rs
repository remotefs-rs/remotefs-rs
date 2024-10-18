#[cfg(target_family = "unix")]
#[cfg_attr(docsrs, doc(cfg(target_family = "unix")))]
mod unix;

use remotefs::RemoteFs;

/// Remote Filesystem Driver
///
/// This driver takes a instance which implements the [`RemoteFs`] trait and mounts it to a local directory.
///
/// The driver will use the [`fuser`](https://crates.io/crates/fuser) crate to mount the filesystem, on Unix systems, while
/// it will use [dokan](https://crates.io/crates/dokan) on Windows.
pub struct Driver {
    remote: Box<dyn RemoteFs>,
}

impl From<Box<dyn RemoteFs>> for Driver {
    fn from(remote: Box<dyn RemoteFs>) -> Self {
        Self::new(remote)
    }
}

impl Driver {
    /// Create a new instance of the [`Driver`] providing a instance which implements the [`RemoteFs`] trait.
    ///
    /// The [`RemoteFs`] instance must be boxed.
    pub fn new(remote: Box<dyn RemoteFs>) -> Self {
        Self { remote }
    }
}
